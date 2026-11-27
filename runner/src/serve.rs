use crate::*;

#[tauri::command]
pub fn close_sheet(window: Window) {
    if matches!(window.label(), "change" | "versions") {
        let _ = window.close();
    }
}

pub(crate) const BRIDGE_HEAD: &str = r#"
(() => {
  // Failures arrive as Error objects whose message starts with a code ("no-folder: …").
  const call = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args || {}).catch(e => { throw new Error(typeof e === "string" ? e : (e && e.message) || String(e)); });
"#;
const BRIDGE_TAIL: &str = r#"
  window.app = Object.freeze(Object.assign({
    store: Object.freeze({ get: key => call("store_get", { key }), set: (key, value) => call("store_set", { key, value }) }),
    notify: text => call("notify", { text: String(text) }),
  }, powers));
  // A fresh page (after a change, too) starts with no global shortcuts; it sets its own.
  call("shortcuts_clear").catch(() => {});
})();
"#;

pub(crate) fn bridge() -> String {
    format!("{BRIDGE_HEAD}{}{BRIDGE_TAIL}", powers::BRIDGE_JS)
}

/// For the change sheet and the manager window (both OpenAs' own pages).
pub(crate) fn oa_bridge(name: &str) -> String {
    format!(
        r#"(() => {{
  const call = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args || {{}});
  window.oa = Object.freeze({{ appName: {name},
    start: req => call("helper_start", {{ req }}), poll: (job, seen) => call("helper_poll", {{ job, seen }}),
    stop: job => call("helper_stop", {{ job }}), close: () => call("close_sheet"),
    state: () => call("talk_state"), note: text => call("talk_note", {{ text }}), stopMaking: () => call("talk_stop"),
    code: () => call("talk_code"), hide: () => call("talk_close"), choose: (save, name) => call("talk_choose", {{ save, name }}), carryOn: text => call("talk_continue", {{ text }}), codeView: open => call("draft_code_view", {{ open }}), full: open => call("draft_full", {{ open }}), ask: save => call("talk_ask", {{ save }}), shared: () => call("shared_pending"),
    drafts: () => call("drafts_list"), showDraft: id => call("draft_show", {{ id }}), home: () => call("draft_home") }});
}})();"#,
        name = serde_json::to_string(name).unwrap()
    )
}

/// The load check: record errors and dialog calls, then after the page has
/// loaded and settled, measure what it shows and report.
pub(crate) const CHECK: &str = r#"
(() => {
  const R = { errors: [], dialogs: [] };
  const add = s => { if (R.errors.length < 20) R.errors.push(String(s).slice(0, 300)); };
  const ce = console.error;
  console.error = (...a) => { add(a.map(x => x && x.message ? x.message : x).join(" ")); ce.apply(console, a); };
  addEventListener("error", e => {
    if (e.target && e.target !== window) add("Failed to load " + (e.target.src || e.target.href || e.target.tagName));
    else add(e.message + " (line " + e.lineno + ")");
  }, true);
  addEventListener("unhandledrejection", e => add("Unhandled: " + (e.reason && e.reason.message || e.reason)));
  addEventListener("securitypolicyviolation", e => add("It tried to reach " + (e.blockedURI || "something") + ", which isn't allowed: an app loads files from its own folder, and uses the internet (https and wss only) after app.permissions.request(['internet']) says yes."));
  for (const k of ["alert", "confirm", "prompt"]) window[k] = () => { R.dialogs.push(k); return k === "confirm" ? false : null; };
  const done = () => {
    const b = document.body;
    R.text_chars = b ? (b.innerText || "").trim().length : 0;
    R.elements = document.querySelectorAll("body *").length;
    R.controls = document.querySelectorAll("button,input,select,textarea,a[href],[role=button]").length;
    R.canvases = document.querySelectorAll("canvas,svg").length;
    window.__TAURI_INTERNALS__.invoke("check_report", { report: R });
  };
  addEventListener("load", () => setTimeout(done, 2500));
})();
"#;

/// What a page may load. An app reaches the internet only once the person
/// allowed it (`powers::net`), never because the model wrote so; OpenAs' own
/// pages never do.
/// `ipc:` and `ipc.localhost` are how the page talks to the Runner.
pub(crate) fn policy(network: bool) -> String {
    let ipc = "ipc: http://ipc.localhost";
    if network {
        format!("default-src 'self' app: data: blob: https: {ipc}; script-src 'self' app: 'unsafe-inline' 'unsafe-eval'; frame-src 'self' app: data: blob:; \
                 style-src 'self' app: 'unsafe-inline' https:; img-src 'self' app: data: blob: https:; font-src 'self' app: data: https:; \
                 connect-src 'self' {ipc} https: wss:; media-src 'self' app: data: blob: https:; base-uri 'none'")
    } else {
        format!("default-src 'self' app: data: blob: {ipc}; script-src 'self' app: 'unsafe-inline' 'unsafe-eval'; \
                 style-src 'self' app: 'unsafe-inline'; img-src 'self' app: data: blob:; font-src 'self' app: data:; \
                 connect-src 'self' {ipc}; media-src 'self' app: data: blob:; form-action 'none'; base-uri 'none'")
    }
}

pub(crate) fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "wasm" => "application/wasm",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

pub(crate) fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

pub(crate) fn has_history(folder: &Path) -> bool {
    std::fs::read_dir(folder.join("history")).is_ok_and(|mut d| d.next().is_some())
}

pub(crate) fn url(path: &str) -> WebviewUrl {
    #[cfg(windows)]
    let base = "http://app.localhost";
    #[cfg(not(windows))]
    let base = "app://localhost";
    WebviewUrl::CustomProtocol(format!("{base}{path}").parse().unwrap())
}

/// Where the window was, kept beside the app (not in its data).
pub(crate) fn restore_frame(w: &tauri::Window, folder: &Path) {
    let Ok(t) = std::fs::read_to_string(folder.join(".window.json")) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<Value>(&t) else {
        return;
    };
    let n = |k: &str| v.get(k).and_then(Value::as_i64);
    if let (Some(x), Some(y), Some(wd), Some(ht)) = (n("x"), n("y"), n("w"), n("h")) {
        if wd > 200 && ht > 150 {
            let _ = w.set_size(PhysicalSize::new(wd as u32, ht as u32));
            let _ = w.set_position(PhysicalPosition::new(x as i32, y as i32));
        }
    }
}

pub(crate) type Reply = http::Response<Cow<'static, [u8]>>;

pub(crate) fn respond(bytes: Vec<u8>, mime: &str, csp: &str) -> Reply {
    http::Response::builder()
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .header("Content-Security-Policy", csp)
        .body(Cow::Owned(bytes))
        .unwrap()
}

pub(crate) fn not_found() -> Reply {
    http::Response::builder()
        .status(404)
        .body(Cow::Borrowed(&b""[..]))
        .unwrap()
}

/// A file from an app's folder: only files inside `dir`, no climbing out
/// with "..". Before the app's first page exists, the waiting page.
pub(crate) fn serve_dir(dir: &Path, rel: &str, csp: &str) -> Reply {
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let path = dir.join(rel);
    if rel == "index.html" && !path.exists() {
        return respond(
            WAITING_PAGE.as_bytes().to_vec(),
            "text/html",
            &policy(false),
        );
    }
    let inside = path
        .canonicalize()
        .ok()
        .zip(dir.canonicalize().ok())
        .is_some_and(|(p, r)| p.starts_with(r));
    match std::fs::read(&path) {
        Ok(bytes) if inside => respond(bytes, mime(&path), csp),
        _ => not_found(),
    }
}
