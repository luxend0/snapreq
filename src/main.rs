mod http;
mod run;

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

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

/// One row of the sidebar: either a file name or one request of that file.
struct Item {
    file: PathBuf,
    req: Option<http::Request>,
}

struct State {
    dir: PathBuf,
    items: Vec<Item>,
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

fn refresh(app: &App, st: &State) {
    let rows: Vec<Entry> = st
        .items
        .iter()
        .map(|i| match &i.req {
            Some(r) => Entry { label: r.name.as_str().into(), method: r.method.as_str().into(), header: false },
            None => Entry { label: i.file.strip_prefix(&st.dir).unwrap_or(&i.file).to_string_lossy().as_ref().into(), method: SharedString::new(), header: true },
        })
        .collect();
    app.set_entries(ModelRc::new(VecModel::from(rows)));
}

fn show(app: &App, r: &http::Request) {
    app.set_method(r.method.as_str().into());
    app.set_url(r.url.as_str().into());
    app.set_headers(http::headers_to_text(&r.headers).into());
    app.set_body(r.body.as_str().into());
    app.set_dirty(false);
}

fn edited(app: &App, base: Option<&http::Request>) -> http::Request {
    http::Request {
        method: app.get_method().to_string(),
        url: app.get_url().trim().to_string(),
        headers: http::headers_from_text(&app.get_headers()),
        body: app.get_body().trim_end().to_string(),
        ..base.cloned().unwrap_or_default()
    }
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
    let st = Rc::new(RefCell::new(State { items: scan(&dir), dir, envs }));
    refresh(&app, &st.borrow());
    if let Some(i) = st.borrow().items.iter().position(|i| i.req.is_some()) {
        app.set_current(i as i32);
        show(&app, st.borrow().items[i].req.as_ref().unwrap());
    }

    // Requests run on one background thread; the window thread never blocks on the network.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let handle = rt.handle().clone();
    std::thread::Builder::new().name("net".into()).spawn(move || rt.block_on(std::future::pending::<()>()))?;
    let client = run::client();

    let (w, s) = (app.as_weak(), st.clone());
    app.on_select(move |i| {
        let (Some(app), st) = (w.upgrade(), s.borrow()) else { return };
        if let Some(r) = st.items.get(i as usize).and_then(|x| x.req.as_ref()) {
            app.set_current(i);
            show(&app, r);
        }
    });

    let (w, s) = (app.as_weak(), st.clone());
    let save = move || {
        let Some(app) = w.upgrade() else { return };
        let cur = app.get_current() as usize;
        let mut st = s.borrow_mut();
        let Some(Item { file, req: Some(old) }) = st.items.get(cur) else { return };
        let (file, old) = (file.clone(), old.clone());
        let new = edited(&app, Some(&old));
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        match std::fs::write(&file, http::replace(&text, &old, &new)) {
            Ok(()) => {
                // line numbers of later requests in the file changed: rescan, keep the selection
                let nth = st.items[..cur].iter().filter(|i| i.file == file && i.req.is_some()).count();
                st.items = scan(&st.dir);
                let again = st.items.iter().enumerate().filter(|(_, i)| i.file == file && i.req.is_some()).nth(nth).map(|(i, _)| i);
                refresh(&app, &st);
                app.set_current(again.map(|i| i as i32).unwrap_or(-1));
                app.set_dirty(false);
                app.set_note(t("Saved", "Gespeichert").into());
            }
            Err(e) => app.set_note(format!("{}: {e}", file.display()).into()),
        }
    };
    app.on_save(save);

    let (w, s) = (app.as_weak(), st.clone());
    app.on_new_request(move || {
        let Some(app) = w.upgrade() else { return };
        let mut st = s.borrow_mut();
        let cur = app.get_current();
        let file = st.items.get(cur.max(0) as usize).map(|i| i.file.clone()).unwrap_or_else(|| st.dir.join("requests.http"));
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
        refresh(&app, &st);
        if let Some(i) = st.items.iter().rposition(|i| i.file == file && i.req.is_some()) {
            app.set_current(i as i32);
            show(&app, st.items[i].req.as_ref().unwrap());
        }
    });

    let (w, s) = (app.as_weak(), st.clone());
    app.on_send(move || {
        let Some(app) = w.upgrade() else { return };
        if app.get_busy() || app.get_url().trim().is_empty() {
            return;
        }
        let st = s.borrow();
        let item = st.items.get(app.get_current().max(0) as usize);
        let file_vars = item.map(|i| http::parse(&std::fs::read_to_string(&i.file).unwrap_or_default())).unwrap_or(http::File { vars: vec![], requests: vec![] });
        let env = app.get_env();
        let vars = vars_for(&file_vars, st.envs.iter().find(|(n, _)| *n == env.as_str()).map(|(_, v)| v));
        let (req, missing) = http::resolve(&edited(&app, None), &vars);
        app.set_busy(true);
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
                (r.status, format!("{} {}", r.status, r.reason), format!("{} ms · {}", r.ms, run::size(r.body.len())), body, http::headers_to_text(&r.headers), cut)
            });
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = w.upgrade() else { return };
                app.set_busy(false);
                match out {
                    Ok((code, status, meta, body, headers, cut)) => {
                        app.set_status(status.into());
                        app.set_status_class(if code >= 400 { 4 } else if code >= 300 { 3 } else { 2 });
                        app.set_meta(meta.into());
                        app.set_resp_body(body.into());
                        app.set_resp_headers(headers.into());
                        if cut {
                            app.set_note(t("Showing the first 1 MB", "Zeigt das erste MB").into());
                        }
                    }
                    Err(e) => {
                        app.set_status(t("Failed", "Fehlgeschlagen").into());
                        app.set_status_class(4);
                        app.set_meta(SharedString::new());
                        app.set_resp_body(e.into());
                        app.set_resp_headers(SharedString::new());
                    }
                }
            });
        });
    });

    app.show()?;
    // SNAPREQ_AUTOSEND=1: send the selected request right away (for screenshots and checks).
    if std::env::var_os("SNAPREQ_AUTOSEND").is_some() {
        app.invoke_send();
    }
    // SNAPREQ_PERF=1: print startup time and memory, then quit (for scripts/perf.sh).
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
