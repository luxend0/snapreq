//! The .http file format (as used by VS Code REST Client and JetBrains): parse, substitute, write back.
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// Line range of the request text (request line to end of body) inside the file.
    pub first: usize,
    pub end: usize,
}

pub struct File {
    pub vars: Vec<(String, String)>,
    pub requests: Vec<Request>,
}

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"];

pub fn parse(text: &str) -> File {
    let lines: Vec<&str> = text.lines().collect();
    let mut vars = Vec::new();
    let mut requests = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        // one block: up to the next ### separator
        let mut name = String::new();
        if let Some(rest) = lines[i].strip_prefix("###") {
            name = rest.trim().to_string();
            i += 1;
        }
        let block_end = (i..lines.len()).find(|&j| lines[j].starts_with("###")).unwrap_or(lines.len());
        // preamble: blank lines, comments, @variables
        while i < block_end {
            let l = lines[i].trim();
            if let Some(v) = l.strip_prefix('@') {
                if let Some((k, val)) = v.split_once('=') {
                    vars.push((k.trim().to_string(), val.trim().to_string()));
                }
            } else if let Some(c) = l.strip_prefix('#').or_else(|| l.strip_prefix("//")) {
                if let Some(n) = c.trim().strip_prefix("@name") {
                    name = n.trim().to_string();
                }
            } else if !l.is_empty() {
                break;
            }
            i += 1;
        }
        if i < block_end {
            let first = i;
            let mut parts = lines[i].split_whitespace();
            let a = parts.next().unwrap_or("");
            let (method, url) = if METHODS.contains(&a.to_ascii_uppercase().as_str()) {
                (a.to_ascii_uppercase(), parts.next().unwrap_or("").to_string())
            } else {
                ("GET".to_string(), a.to_string())
            };
            i += 1;
            let mut headers = Vec::new();
            while i < block_end && !lines[i].trim().is_empty() {
                if let Some((k, v)) = lines[i].split_once(':') {
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                }
                i += 1;
            }
            let mut end = block_end;
            while end > i && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
            let body = if i < end { lines[i + 1..end].join("\n") } else { String::new() };
            if name.is_empty() {
                name = format!("{method} {url}");
            }
            requests.push(Request { name, method, url, headers, body, first, end: end.max(i) });
        }
        i = block_end;
    }
    File { vars, requests }
}

pub fn serialize(r: &Request) -> String {
    let mut s = format!("{} {}\n", r.method, r.url);
    for (k, v) in &r.headers {
        s += &format!("{k}: {v}\n");
    }
    if !r.body.is_empty() {
        s += "\n";
        s += &r.body;
        s += "\n";
    }
    s
}

/// Replaces the text of request `old` inside `text` with `new`, leaving everything else untouched.
pub fn replace(text: &str, old: &Request, new: &Request) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines[..old.first.min(lines.len())].join("\n");
    if !out.is_empty() {
        out += "\n";
    }
    out += &serialize(new);
    let rest = &lines[old.end.min(lines.len())..];
    if !rest.is_empty() {
        out += &rest.join("\n");
        out += "\n";
    }
    out
}

/// Fills in {{name}} placeholders. Unknown names stay as they are and are returned for reporting.
pub fn substitute(s: &str, vars: &HashMap<String, String>, missing: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    // ponytail: values are resolved one level deep per pass, 4 passes cover nested variables.
    while let Some(a) = rest.find("{{") {
        let Some(b) = rest[a..].find("}}") else { break };
        let key = rest[a + 2..a + b].trim();
        out += &rest[..a];
        match key {
            "$timestamp" => out += &std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).to_string(),
            _ => match vars.get(key) {
                Some(v) => out += v,
                None => {
                    out += &rest[a..a + b + 2];
                    if !missing.iter().any(|m| m == key) {
                        missing.push(key.to_string());
                    }
                }
            },
        }
        rest = &rest[a + b + 2..];
    }
    out + rest
}

pub fn resolve(r: &Request, vars: &HashMap<String, String>) -> (Request, Vec<String>) {
    let mut vars = vars.clone();
    for _ in 0..4 {
        let snapshot = vars.clone();
        for v in vars.values_mut() {
            *v = substitute(v, &snapshot, &mut Vec::new());
        }
    }
    let mut missing = Vec::new();
    let mut sub = |s: &str| substitute(s, &vars, &mut missing);
    let out = Request {
        url: sub(&r.url),
        headers: r.headers.iter().map(|(k, v)| (sub(k), sub(v))).collect(),
        body: sub(&r.body),
        ..r.clone()
    };
    (out, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "@host = https://x.test\n\n### Login\n# a comment\nPOST {{host}}/login\nContent-Type: application/json\n\n{\"u\": \"{{user}}\"}\n\n###\nhttps://x.test/ping\n";

    #[test]
    fn parse_resolve_replace() {
        let f = parse(SAMPLE);
        assert_eq!(f.vars, vec![("host".to_string(), "https://x.test".to_string())]);
        assert_eq!(f.requests.len(), 2);
        let r = &f.requests[0];
        assert_eq!((r.name.as_str(), r.method.as_str(), r.url.as_str()), ("Login", "POST", "{{host}}/login"));
        assert_eq!(r.headers, vec![("Content-Type".to_string(), "application/json".to_string())]);
        assert_eq!(r.body, "{\"u\": \"{{user}}\"}");
        assert_eq!((f.requests[1].method.as_str(), f.requests[1].url.as_str()), ("GET", "https://x.test/ping"));

        let vars: HashMap<String, String> = f.vars.iter().cloned().collect();
        let (res, missing) = resolve(r, &vars);
        assert_eq!(res.url, "https://x.test/login");
        assert_eq!(missing, vec!["user".to_string()]);

        // editing one request must keep variables, comments and the other request intact
        let mut edited = r.clone();
        edited.method = "PUT".into();
        edited.body = String::new();
        let text = replace(SAMPLE, r, &edited);
        let g = parse(&text);
        assert_eq!(g.vars, f.vars);
        assert_eq!(g.requests.len(), 2);
        assert_eq!((g.requests[0].name.as_str(), g.requests[0].method.as_str(), g.requests[0].body.as_str()), ("Login", "PUT", ""));
        assert_eq!(g.requests[1].url, "https://x.test/ping");
        assert!(text.contains("# a comment"));
    }
}
