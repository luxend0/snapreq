mod http;
mod run;

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

slint::include_modules!();

const HELP: &str = "snapreq - fast API client, requests are plain .http files

USAGE:
  snapreq [FOLDER]                       open the window for a folder (default: current folder)
  snapreq run FILE.http [-e ENV] [-n NAME]
                                         send the requests of a file in the terminal;
                                         exit code 1 if one fails or returns status >= 400
OPTIONS:
  -e, --env ENV     environment from http-client.env.json
  -n, --name NAME   only the request with this name
  -h, --help        this text
  -V, --version     version";

fn de() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty()).is_some_and(|v| v.starts_with("de"))
}

fn t(en: &'static str, de_text: &'static str) -> &'static str {
    if de() { de_text } else { en }
}

fn vars_for(file: &http::File, env: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let mut vars = env.cloned().unwrap_or_default();
    vars.extend(file.vars.iter().cloned());
    vars
}

fn cli_run(args: &[String]) -> i32 {
    let mut file = None;
    let (mut env, mut name) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-e" | "--env" => env = it.next().cloned(),
            "-n" | "--name" => name = it.next().cloned(),
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let Some(path) = file else {
        eprintln!("{HELP}");
        return 2;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return 2;
        }
    };
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let envs = run::environments(dir);
    let env_vars = match &env {
        Some(n) => match envs.iter().find(|(k, _)| k == n) {
            Some((_, v)) => Some(v),
            None => {
                eprintln!("environment \"{n}\" not found in {}/http-client.env.json", dir.display());
                return 2;
            }
        },
        None => None,
    };
    let parsed = http::parse(&text);
    let vars = vars_for(&parsed, env_vars);
    let todo: Vec<&http::Request> = parsed.requests.iter().filter(|r| name.as_ref().is_none_or(|n| r.name == *n)).collect();
    if todo.is_empty() {
        eprintln!("no matching request in {}", path.display());
        return 2;
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let client = run::client();
    let mut failed = false;
    for r in todo {
        let (req, missing) = http::resolve(r, &vars);
        for m in missing {
            eprintln!("warning: variable {{{{{m}}}}} is not defined");
        }
        match rt.block_on(run::send(&client, &req)) {
            Ok(resp) => {
                eprintln!("{} {}  ->  {} {}  ({} ms, {})", req.method, req.url, resp.status, resp.reason, resp.ms, run::size(resp.body.len()));
                println!("{}", run::display(&resp));
                failed |= resp.status >= 400;
            }
            Err(e) => {
                eprintln!("{} {}  ->  {e}", req.method, req.url);
                failed = true;
            }
        }
    }
    failed as i32
}

/// One row of the sidebar: either a file or one request of that file.
struct Item {
    file: PathBuf,
    req: Option<http::Request>,
}

struct State {
    dir: PathBuf,
    items: Vec<Item>,
    /// Sidebar rows currently shown (indices into `items`), after the search filter.
    shown: Vec<usize>,
    filter: String,
    envs: Vec<(String, HashMap<String, String>)>,
}

fn http_files(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            if depth < 6 {
                http_files(&p, depth + 1, out);
            }
        } else if matches!(p.extension().and_then(|x| x.to_str()), Some("http" | "rest")) {
            out.push(p);
        }
    }
}

fn scan(dir: &Path) -> Vec<Item> {
    let mut files = Vec::new();
    http_files(dir, 0, &mut files);
    files.sort();
    let mut items = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        items.push(Item { file: f.clone(), req: None });
        items.extend(http::parse(&text).requests.into_iter().map(|r| Item { file: f.clone(), req: Some(r) }));
    }
    items
}

fn kv_model(v: &[(String, String)]) -> ModelRc<KV> {
    ModelRc::new(VecModel::from(v.iter().map(|(k, v)| KV { key: k.as_str().into(), value: v.as_str().into() }).collect::<Vec<_>>()))
}

fn kv_read(m: &ModelRc<KV>) -> Vec<(String, String)> {
    m.iter().map(|r| (r.key.trim().to_string(), r.value.trim().to_string())).filter(|(k, _)| !k.is_empty()).collect()
}

fn kv_push(m: &ModelRc<KV>) {
    if let Some(v) = m.as_any().downcast_ref::<VecModel<KV>>() {
        v.push(KV::default());
    }
}

fn kv_remove(m: &ModelRc<KV>, i: i32) {
    if let Some(v) = m.as_any().downcast_ref::<VecModel<KV>>() {
        if (i as usize) < v.row_count() {
            v.remove(i as usize);
        }
    }
}

/// Query parameters as written in the URL (no decoding: what you see is what is sent).
fn query_of(url: &str) -> Vec<(String, String)> {
    let q = url.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or("")).unwrap_or("");
    q.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).unwrap_or((p.to_string(), String::new()))).collect()
}

fn with_query(url: &str, params: &[(String, String)]) -> String {
    let (base, frag) = match url.split_once('#') {
        Some((b, f)) => (b, format!("#{f}")),
        None => (url, String::new()),
    };
    let base = base.split('?').next().unwrap_or("");
    let q = params.iter().map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{k}={v}") }).collect::<Vec<_>>().join("&");
    if q.is_empty() { format!("{base}{frag}") } else { format!("{base}?{q}{frag}") }
}

fn curl(r: &http::Request) -> String {
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

/// Splits display text into lines with a colour class. Works on the indented JSON that
/// `run::display` produces; anything else is shown plain.
fn lines(text: &str) -> Vec<Line> {
    let kind = |v: &str| {
        let v = v.trim_end_matches(',');
        match v.as_bytes().first() {
            Some(b'"') => 1,
            Some(b'-' | b'0'..=b'9') => 2,
            Some(b't' | b'f' | b'n') if matches!(v, "true" | "false" | "null") => 3,
            Some(b'{' | b'}' | b'[' | b']') => 4,
            _ => 0,
        }
    };
    text.lines()
        .map(|l| {
            let body = l.trim_start();
            let indent = &l[..l.len() - body.len()];
            // `"key": value` as written by serde_json's pretty printer
            if let Some(end) = body.strip_prefix('"').and_then(|r| r.find("\": ")) {
                let (k, v) = body.split_at(end + 3);
                return Line { key: format!("{indent}{k}").into(), value: v.into(), kind: kind(v) };
            }
            Line { key: SharedString::new(), value: l.into(), kind: kind(body) }
        })
        .collect()
}

fn refresh(app: &App, st: &mut State) {
    let f = st.filter.to_lowercase();
    let hit = |i: &Item| f.is_empty() || i.req.as_ref().is_some_and(|r| r.name.to_lowercase().contains(&f) || r.url.to_lowercase().contains(&f) || r.method.to_lowercase().contains(&f));
    st.shown.clear();
    let mut rows = Vec::new();
    for (n, i) in st.items.iter().enumerate() {
        match &i.req {
            Some(r) if hit(i) => {
                st.shown.push(n);
                rows.push(Entry { label: r.name.as_str().into(), method: r.method.as_str().into(), sub: SharedString::new(), header: false });
            }
            None => {
                let count = st.items.iter().filter(|x| x.file == i.file && x.req.is_some() && hit(x)).count();
                if count > 0 || f.is_empty() {
                    st.shown.push(n);
                    rows.push(Entry {
                        label: i.file.strip_prefix(&st.dir).unwrap_or(&i.file).to_string_lossy().as_ref().into(),
                        method: SharedString::new(),
                        sub: format!("{count} {}", if count == 1 { t("request", "Anfrage") } else { t("requests", "Anfragen") }).into(),
                        header: true,
                    });
                }
            }
            _ => {}
        }
    }
    app.set_entries(ModelRc::new(VecModel::from(rows)));
}

/// The item behind the selected sidebar row.
fn selected(app: &App, st: &State) -> Option<usize> {
    st.shown.get(usize::try_from(app.get_current()).ok()?).copied()
}

fn select_item(app: &App, st: &State, item: Option<usize>) {
    let row = item.and_then(|n| st.shown.iter().position(|&x| x == n));
    app.set_current(row.map(|r| r as i32).unwrap_or(-1));
    if let Some(r) = item.and_then(|n| st.items[n].req.as_ref()) {
        app.set_method(r.method.as_str().into());
        app.set_url(r.url.as_str().into());
        app.set_params(kv_model(&query_of(&r.url)));
        app.set_headers(kv_model(&r.headers));
        app.set_body(r.body.as_str().into());
        app.set_dirty(false);
        app.set_curl(curl(&resolved(app, st, item).0).into());
    }
}

fn edited(app: &App, base: Option<&http::Request>) -> http::Request {
    http::Request {
        method: app.get_method().to_string(),
        url: app.get_url().trim().to_string(),
        headers: kv_read(&app.get_headers()),
        body: app.get_body().trim_end().to_string(),
        ..base.cloned().unwrap_or_default()
    }
}

/// What is in the editor right now, with variables of the file and the chosen environment filled in.
fn resolved(app: &App, st: &State, item: Option<usize>) -> (http::Request, Vec<String>) {
    let file = item.map(|n| http::parse(&std::fs::read_to_string(&st.items[n].file).unwrap_or_default())).unwrap_or(http::File { vars: vec![], requests: vec![] });
    let env = app.get_env();
    let vars = vars_for(&file, st.envs.iter().find(|(n, _)| *n == env.as_str()).map(|(_, v)| v));
    http::resolve(&edited(app, None), &vars)
}

fn now_text() -> String {
    // ponytail: local time via date(1); avoids a time zone dependency for one label.
    std::process::Command::new("date").arg("+%d.%m.%Y, %H:%M:%S").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn gui(dir: PathBuf, t0: Instant) -> Result<(), Box<dyn std::error::Error>> {
    if de() {
        let _ = slint::select_bundled_translation("de");
    }
    let app = App::new()?;
    // Slint does not resolve the generic "monospace" alias; ask fontconfig for the real family.
    if let Ok(o) = std::process::Command::new("fc-match").args(["monospace", "family"]).output() {
        if let Some(f) = String::from_utf8_lossy(&o.stdout).split(',').next().map(str::trim).filter(|f| !f.is_empty()) {
            app.set_mono(f.into());
        }
    }
    let envs = run::environments(&dir);
    app.set_project(dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string()).into());
    app.set_envs(ModelRc::new(VecModel::from(envs.iter().map(|(n, _)| SharedString::from(n.as_str())).collect::<Vec<_>>())));
    if let Some((n, _)) = envs.first() {
        app.set_env(n.as_str().into());
    }
    let st = Rc::new(RefCell::new(State { items: scan(&dir), shown: vec![], filter: String::new(), dir, envs }));
    {
        let mut s = st.borrow_mut();
        refresh(&app, &mut s);
        let first = s.items.iter().position(|i| i.req.is_some());
        select_item(&app, &s, first);
    }

    // Requests run on one background thread; the window thread never blocks on the network.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let handle = rt.handle().clone();
    std::thread::Builder::new().name("net".into()).spawn(move || rt.block_on(std::future::pending::<()>()))?;
    let client = run::client();

    let (w, s) = (app.as_weak(), st.clone());
    app.on_select(move |row| {
        let (Some(app), st) = (w.upgrade(), s.borrow()) else { return };
        let item = st.shown.get(row as usize).copied().filter(|&n| st.items[n].req.is_some());
        if item.is_some() {
            select_item(&app, &st, item);
        }
    });

    let (w, s) = (app.as_weak(), st.clone());
    app.on_search(move |q| {
        let Some(app) = w.upgrade() else { return };
        let mut st = s.borrow_mut();
        let keep = selected(&app, &st);
        st.filter = q.to_string();
        refresh(&app, &mut st);
        app.set_current(keep.and_then(|n| st.shown.iter().position(|&x| x == n)).map(|r| r as i32).unwrap_or(-1));
    });

    let w = app.as_weak();
    app.on_url_edited(move || {
        if let Some(app) = w.upgrade() {
            app.set_params(kv_model(&query_of(&app.get_url())));
        }
    });
    let w = app.as_weak();
    app.on_params_changed(move || {
        if let Some(app) = w.upgrade() {
            app.set_url(with_query(&app.get_url(), &kv_read(&app.get_params())).into());
        }
    });
    let w = app.as_weak();
    app.on_add_param(move || {
        if let Some(app) = w.upgrade() {
            kv_push(&app.get_params());
        }
    });
    let w = app.as_weak();
    app.on_remove_param(move |i| {
        if let Some(app) = w.upgrade() {
            kv_remove(&app.get_params(), i);
            app.set_url(with_query(&app.get_url(), &kv_read(&app.get_params())).into());
            app.set_dirty(true);
        }
    });
    let w = app.as_weak();
    app.on_add_header(move || {
        if let Some(app) = w.upgrade() {
            kv_push(&app.get_headers());
        }
    });
    let w = app.as_weak();
    app.on_remove_header(move |i| {
        if let Some(app) = w.upgrade() {
            kv_remove(&app.get_headers(), i);
            app.set_dirty(true);
        }
    });

    let (w, s) = (app.as_weak(), st.clone());
    app.on_save(move || {
        let Some(app) = w.upgrade() else { return };
        let mut st = s.borrow_mut();
        let Some(cur) = selected(&app, &st) else { return };
        let Item { file, req: Some(old) } = &st.items[cur] else { return };
        let (file, old) = (file.clone(), old.clone());
        let new = edited(&app, Some(&old));
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        match std::fs::write(&file, http::replace(&text, &old, &new)) {
            Ok(()) => {
                // line numbers of later requests in the file changed: rescan, keep the selection
                let nth = st.items[..cur].iter().filter(|i| i.file == file && i.req.is_some()).count();
                st.items = scan(&st.dir);
                let again = st.items.iter().enumerate().filter(|(_, i)| i.file == file && i.req.is_some()).nth(nth).map(|(i, _)| i);
                refresh(&app, &mut st);
                app.set_current(again.and_then(|n| st.shown.iter().position(|&x| x == n)).map(|r| r as i32).unwrap_or(-1));
                app.set_dirty(false);
                app.set_note(t("Saved", "Gespeichert").into());
            }
            Err(e) => app.set_note(format!("{}: {e}", file.display()).into()),
        }
    });

    let (w, s) = (app.as_weak(), st.clone());
    app.on_new_request(move || {
        let Some(app) = w.upgrade() else { return };
        let mut st = s.borrow_mut();
        let file = selected(&app, &st).map(|n| st.items[n].file.clone()).or_else(|| st.items.first().map(|i| i.file.clone())).unwrap_or_else(|| st.dir.join("requests.http"));
        let mut text = std::fs::read_to_string(&file).unwrap_or_default();
        if !text.is_empty() {
            text = text.trim_end().to_string() + "\n\n";
        }
        text += "### New request\nGET https://\n";
        if let Err(e) = std::fs::write(&file, text) {
            app.set_note(format!("{}: {e}", file.display()).into());
            return;
        }
        st.items = scan(&st.dir);
        st.filter.clear();
        refresh(&app, &mut st);
        let new = st.items.iter().rposition(|i| i.file == file && i.req.is_some());
        select_item(&app, &st, new);
    });

    let (w, s) = (app.as_weak(), st.clone());
    app.on_send(move || {
        let Some(app) = w.upgrade() else { return };
        if app.get_busy() || app.get_url().trim().is_empty() {
            return;
        }
        let st = s.borrow();
        let (req, missing) = resolved(&app, &st, selected(&app, &st));
        app.set_busy(true);
        app.set_curl(curl(&req).into());
        app.set_sent_url(req.url.as_str().into());
        app.set_sent(now_text().into());
        app.set_note(if missing.is_empty() { SharedString::new() } else { format!("{}: {}", t("Undefined variables", "Nicht definierte Variablen"), missing.join(", ")).into() });
        let (w, client) = (w.clone(), client.clone());
        handle.spawn(async move {
            let res = run::send(&client, &req).await;
            // Prepare the text off the window thread. ponytail: display is capped at 1 MB, larger bodies need a streaming viewer.
            let out = res.map(|r| {
                let mut body = run::display(&r);
                let cut = body.len() > 1_000_000;
                if cut {
                    let end = (0..=1_000_000).rev().find(|&i| body.is_char_boundary(i)).unwrap_or(0);
                    body.truncate(end);
                }
                let mut raw = String::from_utf8_lossy(&r.body).into_owned();
                if raw.len() > 1_000_000 {
                    let end = (0..=1_000_000).rev().find(|&i| raw.is_char_boundary(i)).unwrap_or(0);
                    raw.truncate(end);
                }
                (r.status, format!("{} {}", r.status, r.reason), format!("{} ms", r.ms), run::size(r.body.len()), body, raw, r.headers, cut)
            });
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = w.upgrade() else { return };
                app.set_busy(false);
                match out {
                    Ok((code, status, ms, size, body, raw, headers, cut)) => {
                        app.set_status(status.into());
                        app.set_status_class(if code >= 400 { 4 } else if code >= 300 { 3 } else { 2 });
                        app.set_duration(ms.into());
                        app.set_size(size.into());
                        app.set_resp_lines(ModelRc::new(VecModel::from(lines(if app.get_resp_view() == 1 { &raw } else { &body }))));
                        app.set_resp_text(body.into());
                        app.set_resp_raw(raw.into());
                        app.set_resp_headers(kv_model(&headers));
                        if cut {
                            app.set_note(t("Showing the first 1 MB", "Zeigt das erste MB").into());
                        }
                    }
                    Err(e) => {
                        app.set_status(t("Failed", "Fehlgeschlagen").into());
                        app.set_status_class(4);
                        app.set_duration(SharedString::new());
                        app.set_size(SharedString::new());
                        app.set_resp_lines(ModelRc::new(VecModel::from(lines(&e))));
                        app.set_resp_raw(e.as_str().into());
                        app.set_resp_text(e.into());
                        app.set_resp_headers(kv_model(&[]));
                    }
                }
            });
        });
    });

    let w = app.as_weak();
    app.on_view_changed(move || {
        if let Some(app) = w.upgrade() {
            let text = if app.get_resp_view() == 1 { app.get_resp_raw() } else { app.get_resp_text() };
            app.set_resp_lines(ModelRc::new(VecModel::from(lines(&text))));
        }
    });

    app.show()?;
    // SNAPREQ_AUTOSEND=1: send the selected request right away (for screenshots and checks).
    if std::env::var_os("SNAPREQ_AUTOSEND").is_some() {
        app.invoke_send();
    }
    // SNAPREQ_PERF=1: print startup time and memory, then quit.
    if std::env::var_os("SNAPREQ_PERF").is_some() {
        println!("show_ms {:.1}", t0.elapsed().as_secs_f64() * 1000.0);
        slint::Timer::single_shot(Duration::from_secs(4), || {
            let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
            s.lines().filter(|l| l.starts_with("VmRSS") || l.starts_with("VmHWM")).for_each(|l| println!("{l}"));
            let _ = slint::quit_event_loop();
        });
    }
    slint::run_event_loop()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let t0 = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => println!("{HELP}"),
        Some("-V" | "--version") => println!("snapreq {}", env!("CARGO_PKG_VERSION")),
        Some("run") => std::process::exit(cli_run(&args[1..])),
        dir => {
            let dir = PathBuf::from(dir.unwrap_or("."));
            let dir = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
            gui(dir, t0)?;
        }
    }
    Ok(())
}
