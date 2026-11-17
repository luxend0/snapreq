//! The current request as code in other languages, for the details panel.
use crate::http::Request;

pub const LANGS: [&str; 5] = ["cURL", "Python", "JS", "Go", "Rust"];

/// JSON string escaping is also a valid string literal in Python, JavaScript and Go.
fn j(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

pub fn all(r: &Request) -> [String; 5] {
    [curl(r), python(r), js(r), go(r), rust(r)]
}

fn curl(r: &Request) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let mut s = format!("curl -X {} {}", r.method, q(&r.url));
    for (k, v) in &r.headers {
        s += &format!(" \\\n  -H {}", q(&format!("{k}: {v}")));
    }
    if !r.body.is_empty() {
        s += &format!(" \\\n  -d {}", q(&r.body));
    }
    s
}

fn python(r: &Request) -> String {
    let mut s = format!("import requests\n\nr = requests.request(\n    {},\n    {},\n", j(&r.method), j(&r.url));
    if !r.headers.is_empty() {
        s += "    headers={\n";
        for (k, v) in &r.headers {
            s += &format!("        {}: {},\n", j(k), j(v));
        }
        s += "    },\n";
    }
    if !r.body.is_empty() {
        s += &format!("    data={},\n", j(&r.body));
    }
    s + ")\nprint(r.status_code, r.text)"
}

fn js(r: &Request) -> String {
    let mut s = format!("const res = await fetch({}, {{\n  method: {},\n", j(&r.url), j(&r.method));
    if !r.headers.is_empty() {
        s += "  headers: {\n";
        for (k, v) in &r.headers {
            s += &format!("    {}: {},\n", j(k), j(v));
        }
        s += "  },\n";
    }
    if !r.body.is_empty() {
        s += &format!("  body: {},\n", j(&r.body));
    }
    s + "});\nconsole.log(res.status, await res.text());"
}

fn go(r: &Request) -> String {
    let body = if r.body.is_empty() { "nil".to_string() } else { format!("strings.NewReader({})", j(&r.body)) };
    let mut s = format!("req, err := http.NewRequest({}, {}, {body})\nif err != nil {{\n\tpanic(err)\n}}\n", j(&r.method), j(&r.url));
    for (k, v) in &r.headers {
        s += &format!("req.Header.Set({}, {})\n", j(k), j(v));
    }
    s + "res, err := http.DefaultClient.Do(req)\nif err != nil {\n\tpanic(err)\n}\ndefer res.Body.Close()\ndata, _ := io.ReadAll(res.Body)\nfmt.Println(res.Status, string(data))"
}

fn rust(r: &Request) -> String {
    let mut s = format!("let res = reqwest::blocking::Client::new()\n    .request(reqwest::Method::{}, {:?})\n", r.method, r.url);
    for (k, v) in &r.headers {
        s += &format!("    .header({k:?}, {v:?})\n");
    }
    if !r.body.is_empty() {
        s += &format!("    .body({:?})\n", r.body);
    }
    s + "    .send()?;\nprintln!(\"{} {}\", res.status(), res.text()?);"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        let r = Request { method: "POST".into(), url: "https://x.test/a?b=1".into(), headers: vec![("X-A".into(), "it's \"q\"".into())], body: "{\"a\": \"b\\n\"}".into(), ..Default::default() };
        let [c, p, j, g, ru] = all(&r);
        assert!(c.contains(r#"-H 'X-A: it'\''s "q"'"#));
        assert!(p.contains(r#""X-A": "it's \"q\"","#) && p.contains(r#"data="{\"a\": \"b\\n\"}","#));
        assert!(j.contains(r#"body: "{\"a\": \"b\\n\"}","#));
        assert!(g.contains(r#"strings.NewReader("{\"a\": \"b\\n\"}")"#));
        assert!(ru.contains(r#".header("X-A", "it's \"q\"")"#) && ru.contains("Method::POST"));
        // no body: Go passes nil, the others leave the argument out
        let [_, p, _, g, _] = all(&Request { method: "GET".into(), url: "u".into(), ..Default::default() });
        assert!(g.contains(", nil)") && !p.contains("data="));
    }
}
