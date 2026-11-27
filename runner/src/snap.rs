//! The builder's eyes, as a mode of the Runner (`Runner --snap`), so it works
//! wherever the Runner does:
//!   Runner --snap APP_DIR OUT.png [WIDTH HEIGHT] [SCRIPT_FILE] [dark|light]
//! Opens the app folder in a borderless window placed far off screen and
//! never focused, with a simulated desktop instead of the real `window.app`
//! (snap/desk.js) and a pointer for tests (snap/ui.js). Lets it settle, runs
//! the test script, saves a screenshot, and prints what happened as JSON
//! (`errors`, `result`, `moves`, `desktop`), also written to OUT.png.json.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::serve::{not_found, policy, respond, serve_dir, url};

const DESK: &str = include_str!("snap/desk.js");
const UI: &str = include_str!("snap/ui.js");

#[derive(Default)]
struct Seen {
    errors: Vec<String>,
    done: Option<Value>,
    loaded: bool,
}

/// What the app's code reveals about asking for the internet: then the
/// person in a check says yes (as in the real runtime after Allow).
fn asks_for_internet(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
            if !matches!(ext, "js" | "html" | "mjs") {
                continue;
            }
            if let Ok(t) = std::fs::read_to_string(&p) {
                if t.contains("permissions.request")
                    && (t.contains("'internet'")
                        || t.contains("\"internet\"")
                        || t.contains("`internet`"))
                {
                    return true;
                }
            }
        }
    }
    false
}

pub(crate) fn run(args: &[String], mut context: tauri::Context) -> ! {
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    let dir = PathBuf::from(arg(0));
    let out = PathBuf::from(arg(1));
    let w: f64 = arg(2).parse().unwrap_or(1200.0);
    let h: f64 = arg(3).parse().unwrap_or(800.0);
    let script = Some(arg(4))
        .filter(|s| !s.is_empty())
        .and_then(|f| std::fs::read_to_string(f).ok());
    let dark = arg(5) == "dark";

    let parent = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    // Saved data, if any, seeds the stub store so the app shows real state.
    let seed = std::fs::read_to_string(parent.join("data/store.json"))
        .ok()
        .filter(|t| serde_json::from_str::<Value>(t).is_ok())
        .unwrap_or_else(|| "{}".into());
    // Offline, like a new app, unless the app being changed was allowed the
    // internet, or its code asks for it.
    let switched_on = std::fs::read_to_string(parent.join("permissions.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("network").and_then(Value::as_bool))
        .unwrap_or(false);
    let network = switched_on || asks_for_internet(&dir);
    let csp = policy(network);
    let desk = DESK.replace("__OA_SEED__", &seed).replace(
        "__OA_GRANTS__",
        if network { "{ internet: true }" } else { "{}" },
    );
    // Errors reach the Runner through its own scheme (no IPC needed); the
    // test pointer is set up once the page's elements exist.
    let init = format!(
        "(() => {{ const __post = (p, b) => {{ try {{ fetch(p, {{ method: 'POST', body: b, keepalive: true }}).catch(() => {{}}); }} catch (e) {{}} }};\n\
         window.__oaLog = m => __post('/__snap/log', String(m)); window.__oaPost = __post; }})();\n{desk}\n\
         (() => {{ const go = () => {{ {UI} }}; if (document.readyState === 'loading') addEventListener('DOMContentLoaded', go); else go(); }})();"
    );

    let seen: Arc<Mutex<Seen>> = Arc::default();
    let seen_p = seen.clone();
    let dir_p = dir.clone();
    context.config_mut().identifier = "app.openas.snap".into();
    #[allow(unused_mut)]
    let mut app = tauri::Builder::default()
        .register_uri_scheme_protocol("app", move |_ctx, req| {
            let rel = req.uri().path().trim_start_matches('/').to_string();
            match rel.as_str() {
                "__snap/log" => {
                    let mut s = seen_p
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if s.errors.len() < 30 {
                        s.errors
                            .push(String::from_utf8_lossy(req.body()).into_owned());
                    }
                    respond(vec![], "text/plain", &csp)
                }
                "__snap/done" => {
                    let v = serde_json::from_slice(req.body()).unwrap_or(Value::Null);
                    seen_p
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .done = Some(v);
                    respond(vec![], "text/plain", &csp)
                }
                r if r.starts_with("__") => not_found(),
                r => serve_dir(&dir_p, r, &csp),
            }
        })
        .build(context)
        .expect("snap failed to start");

    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    let seen_l = seen.clone();
    let win = tauri::WebviewWindowBuilder::new(&app, "main", url("/index.html"))
        .title("snap")
        .inner_size(w, h)
        .decorations(false)
        .position(-30000.0, -30000.0)
        .focused(false)
        .skip_taskbar(true)
        .theme(Some(if dark {
            tauri::Theme::Dark
        } else {
            tauri::Theme::Light
        }))
        .initialization_script(&init)
        .on_navigation(|u| {
            u.scheme() == "app"
                || u.host_str() == Some("app.localhost")
                || matches!(u.scheme(), "about" | "data" | "blob")
        })
        .on_page_load(move |_w, p| {
            if matches!(p.event(), tauri::webview::PageLoadEvent::Finished) {
                seen_l
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .loaded = true;
            }
        })
        .build()
        .expect("snap window");

    let handle = app.handle().clone();
    std::thread::spawn(move || drive(handle, win, seen, script, out));
    app.run(|_, _| {});
    std::process::exit(0);
}

/// Waits for the page, runs the script, then takes the picture and reports.
fn drive(
    app: tauri::AppHandle,
    win: tauri::WebviewWindow,
    seen: Arc<Mutex<Seen>>,
    script: Option<String>,
    out: PathBuf,
) {
    let start = Instant::now();
    let limit = Duration::from_secs(40);
    let lock = || {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    while !lock().loaded && start.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(1500));
    let mut timed_out = start.elapsed() >= limit;
    let mut result = String::new();
    let (mut moves, mut desktop) = (json!([]), Value::Null);
    if let (Some(script), false) = (script, timed_out) {
        // The script as a string, so a syntax error is reported like any
        // other error instead of the run never finishing.
        let body = serde_json::to_string(&script).unwrap_or_default();
        let js = format!(
            "(async () => {{ let o;\n\
             try {{ const F = Object.getPrototypeOf(async function () {{}}).constructor;\n\
               const r = await new F({body} + '\\n')();\n\
               let s; try {{ s = typeof r === 'string' ? r : JSON.stringify(r); }} catch (e) {{ s = String(r); }}\n\
               const d = window.desk ? {{ grants: Object.keys(desk.grants), menu_bar: desk.menuBar, notifications: desk.notes, file_calls: desk.log.slice(-30), window: desk.window, media_asks: desk.media, shortcuts: desk.shortcuts, programs_run: desk.runs.slice(-30) }} : null;\n\
               o = {{ r: s === undefined ? '' : s, m: window.__moves || [], d }};\n\
             }} catch (e) {{ o = {{ r: 'script error: ' + String(e), m: window.__moves || [], d: null }}; }}\n\
             window.__oaPost('/__snap/done', JSON.stringify(o)); }})();"
        );
        let _ = win.eval(js);
        loop {
            if let Some(v) = lock().done.take() {
                result = v.get("r").and_then(Value::as_str).unwrap_or("").to_string();
                moves = v.get("m").cloned().unwrap_or(json!([]));
                desktop = v.get("d").cloned().unwrap_or(Value::Null);
                break;
            }
            if start.elapsed() >= limit {
                timed_out = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        std::thread::sleep(Duration::from_millis(600));
    }
    if timed_out {
        lock().errors.push("timed out".into());
    }
    let errors = lock().errors.clone();
    let report = json!({
        "errors": errors,
        "result": result.chars().take(4000).collect::<String>(),
        "moves": moves,
        "desktop": desktop,
    });
    let (tx, rx) = std::sync::mpsc::channel::<Option<Vec<u8>>>();
    screenshot(&win, tx);
    if let Ok(Some(png)) = rx.recv_timeout(Duration::from_secs(10)) {
        let _ = std::fs::write(&out, png);
    }
    let text = report.to_string();
    let _ = std::fs::write(out.with_extension("png.json"), &text);
    println!("{text}");
    app.exit(0);
    std::thread::sleep(Duration::from_millis(500));
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
fn screenshot(win: &tauri::WebviewWindow, tx: std::sync::mpsc::Sender<Option<Vec<u8>>>) {
    let _ = win.with_webview(move |p| unsafe {
        use block2::RcBlock;
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        let wk = p.inner() as *mut AnyObject;
        let tx = std::cell::Cell::new(Some(tx));
        let done = RcBlock::new(move |img: *mut AnyObject, _err: *mut AnyObject| {
            let mut png = None;
            if !img.is_null() {
                let tiff: *mut AnyObject = msg_send![img, TIFFRepresentation];
                if !tiff.is_null() {
                    let rep: *mut AnyObject =
                        msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
                    if !rep.is_null() {
                        let props: *mut AnyObject = msg_send![class!(NSDictionary), dictionary];
                        // NSBitmapImageFileTypePNG
                        let data: *mut AnyObject =
                            msg_send![rep, representationUsingType: 4usize, properties: props];
                        if !data.is_null() {
                            let len: usize = msg_send![data, length];
                            let bytes: *const u8 = msg_send![data, bytes];
                            png = Some(std::slice::from_raw_parts(bytes, len).to_vec());
                        }
                    }
                }
            }
            if let Some(tx) = tx.take() {
                let _ = tx.send(png);
            }
        });
        let none: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![wk, takeSnapshotWithConfiguration: none, completionHandler: &*done];
    });
}

#[cfg(windows)]
fn screenshot(win: &tauri::WebviewWindow, tx: std::sync::mpsc::Sender<Option<Vec<u8>>>) {
    let _ = win.with_webview(move |p| {
        // SAFETY: the controller belongs to this live web view, on its thread.
        let core = unsafe { p.controller().CoreWebView2() };
        match core {
            Ok(core) => openas_platform::capture::png(
                &core,
                Box::new(move |png| {
                    let _ = tx.send(png);
                }),
            ),
            Err(_) => {
                let _ = tx.send(None);
            }
        }
    });
}

#[cfg(not(any(target_os = "macos", windows)))]
fn screenshot(_win: &tauri::WebviewWindow, tx: std::sync::mpsc::Sender<Option<Vec<u8>>>) {
    let _ = tx.send(None);
}
