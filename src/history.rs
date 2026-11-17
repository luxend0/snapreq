//! Sent requests, newest first. Kept in the user's data folder, not in the project, and stored
//! as typed in the editor: {{variables}} stay placeholders, so secrets from environments are not copied here.
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::http::Request;

const KEEP: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    /// File and request name it was sent from, to find it again in the sidebar.
    pub file: String,
    pub req: Request,
    /// Status code, or 0 if the request failed.
    pub status: u16,
    pub time: String,
}

fn path() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("APPDATA")?
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support")
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")))?
    };
    Some(base.join("snapreq").join("history.jsonl"))
}

fn to_line(s: &Sent) -> String {
    json!({ "file": s.file, "name": s.req.name, "method": s.req.method, "url": s.req.url, "headers": s.req.headers, "body": s.req.body, "status": s.status, "time": s.time }).to_string()
}

fn from_line(l: &str) -> Option<Sent> {
    let v: Value = serde_json::from_str(l).ok()?;
    let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Some(Sent {
        file: text("file"),
        req: Request { name: text("name"), method: text("method"), url: text("url"), headers: serde_json::from_value(v["headers"].clone()).ok()?, body: text("body"), ..Default::default() },
        status: v["status"].as_u64().unwrap_or(0) as u16,
        time: text("time"),
    })
}

pub fn load() -> Vec<Sent> {
    let text = path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    text.lines().rev().filter_map(from_line).take(KEEP).collect()
}

pub fn add(list: &mut Vec<Sent>, s: Sent) {
    list.insert(0, s);
    list.truncate(KEEP);
    // ponytail: rewrites the whole file (at most 200 lines) per request; append + occasional trim if that ever shows up.
    if let Some(p) = path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
        let _ = std::fs::write(p, list.iter().rev().map(|s| to_line(s) + "\n").collect::<String>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_roundtrip() {
        let s = Sent { file: "/a/b.http".into(), req: Request { name: "Login".into(), method: "POST".into(), url: "{{host}}/x".into(), headers: vec![("A".into(), "b\"c".into())], body: "line1\nline2".into(), ..Default::default() }, status: 201, time: "now".into() };
        let line = to_line(&s);
        assert!(!line.contains('\n'));
        assert_eq!(from_line(&line), Some(s));
        assert_eq!(from_line("not json"), None);
    }
}
