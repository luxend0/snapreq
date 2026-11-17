//! Sending requests and loading environments. Used by both the window and the command line.
use std::{collections::HashMap, path::Path, time::{Duration, Instant}};

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
