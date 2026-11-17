//! Plugins: JavaScript files in `.snapreq/plugins/` of the project folder. Each may define
//! `onRequest(req)` and `onResponse(req, res)`. The engine (QuickJS) only starts when such a file
//! exists, and runs sandboxed: no file or network access, limited time and memory.
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rquickjs::{CatchResultExt, Context, Runtime};
use serde_json::{json, Map, Value};

use crate::{http::Request, run::Response};

const LIMIT: Duration = Duration::from_secs(2);

pub fn files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir.join(".snapreq/plugins")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "js")).collect();
    v.sort();
    v
}

/// Runs all plugins for one phase: before sending (`res` is None) or with the response.
/// Returns what they have to say (logs, notes, errors), each prefixed with the plugin name.
pub fn run(files: &[PathBuf], req: &mut Request, res: Option<&Response>) -> Vec<String> {
    let mut said = Vec::new();
    for f in files {
        let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let out = std::fs::read_to_string(f).map_err(|e| e.to_string()).and_then(|src| run_one(&src, req, res, LIMIT));
        match out {
            Ok(lines) => said.extend(lines.into_iter().map(|l| format!("{name}: {l}"))),
            Err(e) => said.push(format!("{name}: {e}")),
        }
    }
    said
}

// ponytail: headers cross into JavaScript as an object, so repeated header names collapse to the last one.
fn headers(h: &[(String, String)]) -> Value {
    Value::Object(h.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<Map<_, _>>())
}

fn run_one(src: &str, req: &mut Request, res: Option<&Response>, limit: Duration) -> Result<Vec<String>, String> {
    let req_js = json!({ "name": req.name, "method": req.method, "url": req.url, "headers": headers(&req.headers), "body": req.body });
    let res_js = res.map(|r| json!({ "status": r.status, "reason": r.reason, "headers": headers(&r.headers), "body": String::from_utf8_lossy(&r.body), "ms": r.ms as u64 }));
    // The plugin source starts on the first line so line numbers in its errors are right.
    let script = format!(
        "(function(){{var __said=[];var snapreq={{log:function(){{__said.push(Array.prototype.join.call(arguments,' '))}}}};var console=snapreq;var req={req_js};var res={};{src}\n\
         ;if(!res&&typeof onRequest==='function'){{var r=onRequest(req);if(r&&typeof r==='object')req=r}}\
         if(res&&typeof onResponse==='function'){{var n=onResponse(req,res);if(n!=null)__said.push(String(n))}}\
         return JSON.stringify({{req:req,said:__said}})}})()",
        res_js.unwrap_or(Value::Null)
    );
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(32 << 20);
    let deadline = Instant::now() + limit;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
    let out: String = ctx.with(|ctx| ctx.eval::<String, _>(script).catch(&ctx).map_err(|e| e.to_string()))?;
    let v: Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    if res.is_none() {
        let text = |k: &str, old: &str| v["req"][k].as_str().unwrap_or(old).to_string();
        req.method = text("method", &req.method).to_ascii_uppercase();
        req.url = text("url", &req.url);
        req.body = text("body", &req.body);
        if let Some(h) = v["req"]["headers"].as_object() {
            req.headers = h.iter().map(|(k, v)| (k.clone(), v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))).collect();
        }
    }
    Ok(v["said"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_and_limits() {
        let src = "function onRequest(req){ req.headers['X-A'] = '1'; req.url += '?x=1'; snapreq.log('hi', req.method) }\nfunction onResponse(req, res){ return res.status + ' ' + res.body.length }";
        let mut req = Request { method: "GET".into(), url: "http://x.test".into(), headers: vec![("Accept".into(), "*/*".into())], ..Default::default() };
        assert_eq!(run_one(src, &mut req, None, LIMIT), Ok(vec!["hi GET".to_string()]));
        assert_eq!(req.url, "http://x.test?x=1");
        assert_eq!(req.headers, vec![("Accept".to_string(), "*/*".to_string()), ("X-A".to_string(), "1".to_string())]);

        let res = Response { status: 201, reason: "Created".into(), headers: vec![], body: b"abc".to_vec(), ms: 5 };
        let before = req.clone();
        assert_eq!(run_one(src, &mut req, Some(&res), LIMIT), Ok(vec!["201 3".to_string()]));
        assert_eq!(req, before); // the response phase never changes the request

        // a broken or endless plugin is an error message, not a hang or a crash
        assert!(run_one("syntax error here", &mut req, None, LIMIT).is_err());
        assert!(run_one("while(true){}", &mut req, None, Duration::from_millis(50)).is_err());
        // no way out of the sandbox
        assert!(run_one("require('fs')", &mut req, None, LIMIT).is_err());
        assert_eq!(run_one("snapreq.log(typeof fetch, typeof process, typeof std, typeof os)", &mut req, None, LIMIT), Ok(vec!["undefined undefined undefined undefined".to_string()]));
    }
}
