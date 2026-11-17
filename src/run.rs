//! Sending requests and loading environments. Used by both the window and the command line.
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{atomic::{AtomicUsize, Ordering}, Arc},
    time::{Duration, Instant},
};

use crate::http::Request;

pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub ms: u128,
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder().user_agent(concat!("snapreq/", env!("CARGO_PKG_VERSION"))).timeout(Duration::from_secs(60)).build().expect("http client")
}

pub async fn send(c: &reqwest::Client, r: &Request) -> Result<Response, String> {
    let method = reqwest::Method::from_bytes(r.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut rb = c.request(method, &r.url);
    for (k, v) in &r.headers {
        rb = rb.header(k, v);
    }
    if !r.body.is_empty() {
        rb = rb.body(r.body.clone());
    }
    let t = Instant::now();
    let resp = rb.send().await.map_err(|e| {
        // reqwest's top-level message hides the cause (DNS, TLS, refused); show the whole chain
        let mut s = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(x) = src {
            s += &format!(": {x}");
            src = x.source();
        }
        s
    })?;
    let status = resp.status();
    let headers = resp.headers().iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok(Response { status: status.as_u16(), reason: status.canonical_reason().unwrap_or("").to_string(), headers, body, ms: t.elapsed().as_millis() })
}

/// Environments from http-client.env.json and http-client.private.env.json (the JetBrains convention):
/// `{ "dev": { "host": "http://localhost:3000" }, "prod": { ... } }`. Private values win.
pub fn environments(dir: &Path) -> Vec<(String, HashMap<String, String>)> {
    let mut envs: Vec<(String, HashMap<String, String>)> = Vec::new();
    for f in ["http-client.env.json", "http-client.private.env.json"] {
        let Ok(text) = std::fs::read_to_string(dir.join(f)) else { continue };
        let Ok(serde_json::Value::Object(root)) = serde_json::from_str(&text) else { continue };
        for (name, vals) in root {
            let Some(vals) = vals.as_object() else { continue };
            let i = envs.iter().position(|(n, _)| *n == name).unwrap_or_else(|| {
                envs.push((name.clone(), HashMap::new()));
                envs.len() - 1
            });
            for (k, v) in vals {
                envs[i].1.insert(k.clone(), v.as_str().map(String::from).unwrap_or_else(|| v.to_string()));
            }
        }
    }
    envs
}

pub fn size(n: usize) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} kB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// Body as text for display: JSON is indented, anything binary is summarised.
pub fn display(r: &Response) -> String {
    let ctype = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.as_str()).unwrap_or("");
    let Ok(text) = std::str::from_utf8(&r.body) else {
        return format!("[binary, {}, {ctype}]", size(r.body.len()));
    };
    if ctype.contains("json") || text.trim_start().starts_with(['{', '[']) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            return serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string());
        }
    }
    text.to_string()
}

/// Result of a load test. Latencies are in milliseconds, sorted.
#[derive(Default)]
pub struct Bench {
    pub parallel: usize,
    pub secs: f64,
    pub lat: Vec<u128>,
    pub failed: usize,
    pub codes: BTreeMap<u16, usize>,
    pub first_error: String,
}

/// Sends the request `total` times, `parallel` at once.
pub async fn bench(c: &reqwest::Client, r: &Request, total: usize, parallel: usize) -> Bench {
    let parallel = parallel.clamp(1, total.max(1));
    let left = Arc::new(AtomicUsize::new(total));
    let t = Instant::now();
    let workers: Vec<_> = (0..parallel)
        .map(|_| {
            let (c, r, left) = (c.clone(), r.clone(), left.clone());
            tokio::spawn(async move {
                let mut out = Vec::new();
                // take one from the shared budget per request
                while left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_ok() {
                    out.push(send(&c, &r).await.map(|x| (x.status, x.ms)));
                }
                out
            })
        })
        .collect();
    let mut b = Bench { parallel, ..Default::default() };
    for w in workers {
        for res in w.await.unwrap_or_default() {
            match res {
                Ok((code, ms)) => {
                    *b.codes.entry(code).or_default() += 1;
                    b.lat.push(ms);
                }
                Err(e) => {
                    b.failed += 1;
                    if b.first_error.is_empty() {
                        b.first_error = e;
                    }
                }
            }
        }
    }
    b.secs = t.elapsed().as_secs_f64();
    b.lat.sort_unstable();
    b
}

/// Nearest-rank percentile of a sorted list.
fn pct(sorted: &[u128], p: usize) -> u128 {
    if sorted.is_empty() { 0 } else { sorted[((sorted.len() * p).div_ceil(100)).clamp(1, sorted.len()) - 1] }
}

pub fn bench_report(b: &Bench) -> String {
    let n = b.lat.len() + b.failed;
    let mut s = format!("{n} requests, {} parallel, {:.2} s\n{:.1} requests/s, {} failed\n", b.parallel, b.secs, if b.secs > 0.0 { n as f64 / b.secs } else { 0.0 }, b.failed);
    if !b.lat.is_empty() {
        let avg = b.lat.iter().sum::<u128>() / b.lat.len() as u128;
        s += &format!("\nLatency (ms)\n  min     {}\n  average {avg}\n  median  {}\n  p95     {}\n  p99     {}\n  max     {}\n", b.lat[0], pct(&b.lat, 50), pct(&b.lat, 95), pct(&b.lat, 99), b.lat[b.lat.len() - 1]);
        s += "\nStatus\n";
        for (code, count) in &b.codes {
            s += &format!("  {code}     {count}\n");
        }
    }
    if !b.first_error.is_empty() {
        s += &format!("\nFirst error: {}\n", b.first_error);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_and_report() {
        let lat: Vec<u128> = (1..=100).collect();
        assert_eq!((pct(&lat, 50), pct(&lat, 95), pct(&lat, 99), pct(&lat, 100)), (50, 95, 99, 100));
        assert_eq!((pct(&[7], 50), pct(&[7], 99), pct(&[], 50)), (7, 7, 0));
        let b = Bench { parallel: 4, secs: 2.0, lat, failed: 100, codes: BTreeMap::from([(200, 90), (500, 10)]), first_error: "refused".into() };
        let r = bench_report(&b);
        assert!(r.contains("200 requests, 4 parallel") && r.contains("100.0 requests/s, 100 failed") && r.contains("p95     95") && r.contains("500     10") && r.contains("First error: refused"));
    }
}
