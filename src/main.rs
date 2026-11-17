mod history;
mod http;
mod run;
mod snippet;

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

fn snippets(r: &http::Request) -> slint::ModelRc<SharedString> {
    slint::ModelRc::new(slint::VecModel::from(snippet::all(r).map(SharedString::from).to_vec()))
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

/// Raw view: the body exactly as received, one row per line, no colouring.
fn plain(text: &str) -> Vec<Line> {
    text.lines().map(|l| Line { key: SharedString::new(), value: l.into(), kind: 0 }).collect()
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
        fill(app, st, item, r);
        app.set_dirty(false);
    }
}

/// Puts a request into the editor.
fn fill(app: &App, st: &State, item: Option<usize>, r: &http::Request) {
    app.set_method(r.method.as_str().into());
    app.set_url(r.url.as_str().into());
    app.set_params(kv_model(&query_of(&r.url)));
    app.set_headers(kv_model(&r.headers));
    app.set_body(r.body.as_str().into());
    app.set_snippets(snippets(&resolved(app, st, item).0));
}

thread_local! {
    /// Only touched on the window thread.
    static HISTORY: RefCell<Vec<history::Sent>> = const { RefCell::new(Vec::new()) };
}

fn show_history(app: &App) {
    let rows: Vec<Entry> = HISTORY.with_borrow(|h| {
        h.iter()
            .map(|s| Entry {
                label: s.req.url.split_once("://").map(|(_, u)| u).unwrap_or(&s.req.url).into(),
                method: s.req.method.as_str().into(),
                sub: if s.status == 0 { "ERR".into() } else { s.status.to_string().into() },
                header: false,
            })
            .collect()
    });
    app.set_history(ModelRc::new(VecModel::from(rows)));
}

/// What the response panel shows.
#[derive(Clone, Default)]
struct Resp {
    status: String,
    class: i32,
    duration: String,
    size: String,
    sent: String,
    sent_url: String,
    text: String,
    raw: String,
    headers: Vec<(String, String)>,
}

/// One open tab: the editor contents and the last response. The active tab lives in the window
/// and is copied back here whenever tabs are switched or listed.
#[derive(Clone)]
struct Tab {
    id: u32,
    /// The request it belongs to: file and position among that file's requests.
    at: Option<(PathBuf, usize)>,
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
    dirty: bool,
    busy: bool,
    resp: Resp,
}

struct Tabs {
    list: Vec<Tab>,
    active: usize,
    next: u32,
}

impl Tabs {
    fn fresh(&mut self) -> Tab {
        self.next += 1;
        Tab { id: self.next, at: None, method: "GET".into(), url: String::new(), headers: vec![], body: String::new(), dirty: false, busy: false, resp: Resp::default() }
    }
}

thread_local! {
    /// Only touched on the window thread.
    static TABS: RefCell<Tabs> = RefCell::new({
        let mut t = Tabs { list: vec![], active: 0, next: 0 };
        let first = t.fresh();
        t.list.push(first);
        t
    });
}

fn ident(st: &State, item: Option<usize>) -> Option<(PathBuf, usize)> {
    let n = item?;
    st.items[n].req.as_ref()?;
    let file = &st.items[n].file;
    Some((file.clone(), st.items[..n].iter().filter(|i| i.file == *file && i.req.is_some()).count()))
}

fn locate(st: &State, at: &Option<(PathBuf, usize)>) -> Option<usize> {
    let (file, nth) = at.as_ref()?;
    st.items.iter().enumerate().filter(|(_, i)| i.file == *file && i.req.is_some()).nth(*nth).map(|(i, _)| i)
}

fn grab(app: &App, st: &State, old: &Tab) -> Tab {
    Tab {
        id: old.id,
        // a search filter can hide the selected row; the tab still belongs to its request then
        at: ident(st, selected(app, st)).or_else(|| old.at.clone()),
        method: app.get_method().into(),
        url: app.get_url().into(),
        headers: kv_read(&app.get_headers()),
        body: app.get_body().into(),
        dirty: app.get_dirty(),
        busy: app.get_busy(),
        resp: Resp {
            status: app.get_status().into(),
            class: app.get_status_class(),
            duration: app.get_duration().into(),
            size: app.get_size().into(),
            sent: app.get_sent().into(),
            sent_url: app.get_sent_url().into(),
            text: app.get_resp_text().into(),
            raw: app.get_resp_raw().into(),
            headers: kv_read(&app.get_resp_headers()),
        },
    }
}

fn show_resp(app: &App, r: &Resp) {
    app.set_status(r.status.as_str().into());
    app.set_status_class(r.class);
    app.set_duration(r.duration.as_str().into());
    app.set_size(r.size.as_str().into());
    app.set_sent(r.sent.as_str().into());
    app.set_sent_url(r.sent_url.as_str().into());
    app.set_resp_lines(ModelRc::new(VecModel::from(if app.get_resp_view() == 1 { plain(&r.raw) } else { lines(&r.text) })));
    app.set_resp_text(r.text.as_str().into());
    app.set_resp_raw(r.raw.as_str().into());
    app.set_resp_headers(kv_model(&r.headers));
}

fn put(app: &App, st: &State, t: &Tab) {
    let item = locate(st, &t.at);
    app.set_current(item.and_then(|n| st.shown.iter().position(|&x| x == n)).map(|r| r as i32).unwrap_or(-1));
    fill(app, st, item, &http::Request { method: t.method.clone(), url: t.url.clone(), headers: t.headers.clone(), body: t.body.clone(), ..Default::default() });
    app.set_dirty(t.dirty);
    app.set_busy(t.busy);
    app.set_note(SharedString::new());
    show_resp(app, &t.resp);
}

/// Copies the window into the active tab and refreshes the tab strip.
fn show_tabs(app: &App, st: &State) {
    TABS.with_borrow_mut(|tabs| {
        let a = tabs.active;
        tabs.list[a] = grab(app, st, &tabs.list[a]);
        let rows: Vec<Entry> = tabs
            .list
            .iter()
            .map(|tab| {
                let name = locate(st, &tab.at).and_then(|n| st.items[n].req.as_ref()).map(|r| r.name.clone());
                let url = tab.url.split_once("://").map(|(_, u)| u).unwrap_or(&tab.url);
                let label = name.unwrap_or_else(|| if url.is_empty() { t("New tab", "Neuer Reiter").to_string() } else { url.to_string() });
                Entry { label: label.into(), method: tab.method.as_str().into(), sub: SharedString::new(), header: false }
            })
            .collect();
        app.set_tabs(ModelRc::new(VecModel::from(rows)));
        app.set_tab(a as i32);
    });
}

fn switch_tab(app: &App, st: &State, to: usize) {
    let tab = TABS.with_borrow_mut(|t| {
        let a = t.active;
        t.list[a] = grab(app, st, &t.list[a]);
        t.active = to.min(t.list.len() - 1);
        t.list[t.active].clone()
    });
    put(app, st, &tab);
    show_tabs(app, st);
}

fn new_tab(app: &App, st: &State) {
    let n = TABS.with_borrow_mut(|t| {
        let tab = t.fresh();
        t.list.push(tab);
        t.list.len() - 1
    });
    switch_tab(app, st, n);
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
    // ponytail: local time from the system's own tools; avoids a time zone dependency for one label.
    #[cfg(unix)]
    let out = std::process::Command::new("date").arg("+%d.%m.%Y, %H:%M:%S").output();
    #[cfg(windows)]
    let out = std::process::Command::new("cmd").args(["/C", "echo %date% %time:~0,8%"]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// A monospace family that exists on this system.
fn mono_font() -> String {
    // Slint does not resolve the generic "monospace" alias; on Linux ask fontconfig for the real family.
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Ok(o) = std::process::Command::new("fc-match").args(["monospace", "family"]).output() {
        if let Some(f) = String::from_utf8_lossy(&o.stdout).split(',').next().map(str::trim).filter(|f| !f.is_empty()) {
            return f.to_string();
        }
    }
    if cfg!(target_os = "macos") { "Menlo" } else if cfg!(windows) { "Consolas" } else { "DejaVu Sans Mono" }.to_string()
}

fn gui(dir: PathBuf, t0: Instant) -> Result<(), Box<dyn std::error::Error>> {
    if de() {
        let _ = slint::select_bundled_translation("de");
    }
    let app = App::new()?;
    app.set_mono(mono_font().into());
    app.set_snippet_langs(slint::ModelRc::new(slint::VecModel::from(snippet::LANGS.map(SharedString::from).to_vec())));
    app.set_animated(std::env::var_os("SNAPREQ_STATIC").is_none());
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
        show_tabs(&app, &s);
    }

    HISTORY.set(history::load());
    show_history(&app);

    // Requests run on one background thread; the window thread never blocks on the network.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let handle = rt.handle().clone();
    std::thread::Builder::new().name("net".into()).spawn(move || rt.block_on(std::future::pending::<()>()))?;
    let client = run::client();

    let (w, s) = (app.as_weak(), st.clone());
    app.on_select(move |row| {
        let (Some(app), st) = (w.upgrade(), s.borrow()) else { return };
        let item = st.shown.get(row as usize).copied().filter(|&n| st.items[n].req.is_some());
        if item.is_none() {
            return;
        }
        show_tabs(&app, &st);
        let at = ident(&st, item);
        let (open, active) = TABS.with_borrow(|t| (t.list.iter().position(|x| x.at == at), t.active));
        match open {
            Some(n) if n != active => switch_tab(&app, &st, n),
            // unsaved edits or a running request stay where they are; the request opens next to them
            None if app.get_dirty() || app.get_busy() => {
                new_tab(&app, &st);
                select_item(&app, &st, item);
            }
            _ => select_item(&app, &st, item),
        }
        show_tabs(&app, &st);
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
                show_tabs(&app, &st);
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
        if app.get_dirty() || app.get_busy() {
            new_tab(&app, &st);
        }
        select_item(&app, &st, new);
        show_tabs(&app, &st);
    });

    let (w, s) = (app.as_weak(), st.clone());
    app.on_send(move || {
        let Some(app) = w.upgrade() else { return };
        if app.get_busy() || app.get_url().trim().is_empty() {
            return;
        }
        let st = s.borrow();
        let cur = selected(&app, &st);
        let (req, missing) = resolved(&app, &st, cur);
        let mut sent = history::Sent {
            file: cur.map(|n| st.items[n].file.to_string_lossy().into_owned()).unwrap_or_default(),
            req: edited(&app, cur.and_then(|n| st.items[n].req.as_ref())),
            status: 0,
            time: now_text(),
        };
        app.set_busy(true);
        app.set_snippets(snippets(&req));
        app.set_sent_url(req.url.as_str().into());
        app.set_sent(sent.time.as_str().into());
        app.set_note(if missing.is_empty() { SharedString::new() } else { format!("{}: {}", t("Undefined variables", "Nicht definierte Variablen"), missing.join(", ")).into() });
        let id = TABS.with_borrow(|t| t.list[t.active].id);
        let (w, client) = (w.clone(), client.clone());
        handle.spawn(async move {
            let res = run::send(&client, &req).await;
            // Prepare the text off the window thread. ponytail: display is capped at 1 MB, larger bodies need a streaming viewer.
            let cap = |mut s: String| {
                let cut = s.len() > 1_000_000;
                if cut {
                    let end = (0..=1_000_000).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
                    s.truncate(end);
                }
                (s, cut)
            };
            let base = Resp { sent: sent.time.clone(), sent_url: req.url.clone(), ..Default::default() };
            let (resp, cut) = match res {
                Ok(r) => {
                    sent.status = r.status;
                    let (text, cut) = cap(run::display(&r));
                    let raw = cap(String::from_utf8_lossy(&r.body).into_owned()).0;
                    let class = if r.status >= 400 { 4 } else if r.status >= 300 { 3 } else { 2 };
                    (Resp { status: format!("{} {}", r.status, r.reason), class, duration: format!("{} ms", r.ms), size: run::size(r.body.len()), text, raw, headers: r.headers, ..base }, cut)
                }
                Err(e) => (Resp { status: t("Failed", "Fehlgeschlagen").into(), class: 4, text: e.clone(), raw: e, ..base }, false),
            };
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = w.upgrade() else { return };
                HISTORY.with_borrow_mut(|h| history::add(h, sent));
                show_history(&app);
                // the response belongs to the tab that sent it, even if another one is in front by now
                let front = TABS.with_borrow_mut(|t| {
                    let front = t.list[t.active].id == id;
                    if let Some(tab) = t.list.iter_mut().find(|x| x.id == id) {
                        tab.busy = false;
                        tab.resp = resp.clone();
                    }
                    front
                });
                if front {
                    app.set_busy(false);
                    show_resp(&app, &resp);
                    if cut {
                        app.set_note(t("Showing the first 1 MB", "Zeigt das erste MB").into());
                    }
                }
            });
        });
    });

    let (w, s) = (app.as_weak(), st.clone());
    app.on_tab_select(move |i| {
        if let Some(app) = w.upgrade() {
            switch_tab(&app, &s.borrow(), i.max(0) as usize);
        }
    });
    let (w, s) = (app.as_weak(), st.clone());
    app.on_tab_new(move || {
        if let Some(app) = w.upgrade() {
            new_tab(&app, &s.borrow());
        }
    });
    let (w, s) = (app.as_weak(), st.clone());
    app.on_tab_close(move |i| {
        let (Some(app), st) = (w.upgrade(), s.borrow()) else { return };
        // -1: the tab in front
        let reload = TABS.with_borrow_mut(|t| {
            let i = if i < 0 { t.active } else { i as usize };
            if i >= t.list.len() {
                return None;
            }
            if t.list.len() == 1 {
                t.list[0] = t.fresh();
            } else {
                t.list.remove(i);
                if i < t.active {
                    t.active -= 1;
                    return None;
                } else if i > t.active {
                    return None;
                }
                t.active = t.active.min(t.list.len() - 1);
            }
            Some(t.list[t.active].clone())
        });
        if let Some(tab) = reload {
            put(&app, &st, &tab);
        }
        show_tabs(&app, &st);
    });

    let (w, s) = (app.as_weak(), st.clone());
    app.on_history_select(move |row| {
        let (Some(app), st) = (w.upgrade(), s.borrow()) else { return };
        let Some(h) = HISTORY.with_borrow(|h| h.get(row as usize).cloned()) else { return };
        // back to the request it came from if that still exists, so its file variables and Save apply
        let item = st.items.iter().position(|i| i.file.to_string_lossy() == h.file.as_str() && i.req.as_ref().is_some_and(|r| r.name == h.req.name));
        app.set_current(item.and_then(|n| st.shown.iter().position(|&x| x == n)).map(|r| r as i32).unwrap_or(-1));
        if item.is_none() {
            TABS.with_borrow_mut(|t| {
                let a = t.active;
                t.list[a].at = None;
            });
        }
        fill(&app, &st, item, &h.req);
        let same = |r: &http::Request| (&r.method, &r.url, &r.headers, &r.body) == (&h.req.method, &h.req.url, &h.req.headers, &h.req.body);
        app.set_dirty(item.and_then(|n| st.items[n].req.as_ref()).is_some_and(|r| !same(r)));
        show_tabs(&app, &st);
    });

    let w = app.as_weak();
    app.on_view_changed(move || {
        if let Some(app) = w.upgrade() {
            let rows = if app.get_resp_view() == 1 { plain(&app.get_resp_raw()) } else { lines(&app.get_resp_text()) };
            app.set_resp_lines(ModelRc::new(VecModel::from(rows)));
        }
    });

    app.show()?;
    // SNAPREQ_AUTOSEND=1: send the selected request right away (for screenshots and checks); =history also opens the history tab.
    if std::env::var_os("SNAPREQ_AUTOSEND").is_some() {
        app.invoke_send();
        if std::env::var_os("SNAPREQ_AUTOSEND").is_some_and(|v| v == "history") {
            app.set_side_tab(1);
        }
        // =tabs: open a second tab while the first is still waiting, come back after 2 s
        if std::env::var_os("SNAPREQ_AUTOSEND").is_some_and(|v| v == "tabs") {
            app.invoke_tab_new();
            app.invoke_select(2);
            let w = app.as_weak();
            slint::Timer::single_shot(Duration::from_secs(2), move || {
                if let Some(app) = w.upgrade() {
                    app.invoke_tab_select(0);
                }
            });
        }
    }
    // SNAPREQ_SHOT=file.ppm: write a picture of the window after 3 s, then quit (checks without touching the desktop).
    if let Some(path) = std::env::var_os("SNAPREQ_SHOT") {
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_secs(3), move || {
            if let Some(Ok(img)) = w.upgrade().map(|a| a.window().take_snapshot()) {
                let mut out = format!("P6 {} {} 255\n", img.width(), img.height()).into_bytes();
                out.extend(img.as_slice().iter().flat_map(|p| [p.r, p.g, p.b]));
                let _ = std::fs::write(&path, out);
            }
            let _ = slint::quit_event_loop();
        });
    }
    // SNAPREQ_PERF=1: print startup time and memory, then quit.
    if std::env::var_os("SNAPREQ_PERF").is_some() {
        println!("show_ms {:.1}", t0.elapsed().as_secs_f64() * 1000.0);
        slint::Timer::single_shot(Duration::from_secs(4), || {
            // Linux only; elsewhere this prints nothing
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
