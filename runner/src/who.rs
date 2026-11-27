//! Who a webview speaks for.
//!
//! An installed app's process is that one app, whichever of its webviews
//! calls. OpenAs's own window also shows apps while they're made: there the
//! home page (`main`) speaks for OpenAs, and the draft's page (`app`) and its
//! conversation (`talk`) speak for the draft, with a draft's limits, never
//! OpenAs's. Every command that acts for someone asks here first; nothing
//! reads the process's own identity directly (a test below holds that).

use crate::*;

/// An app being made, as OpenAs's window shows it.
#[derive(Clone)]
pub(crate) struct Draft {
    pub(crate) app: Arc<App>,
    pub(crate) store: Arc<Store>,
}

pub(crate) struct Who {
    pub(crate) home: Arc<App>,
    home_store: Arc<Store>,
    shown: Mutex<Option<Draft>>,
    /// The draft's code is open in the panel, in place of the app.
    code_open: std::sync::atomic::AtomicBool,
    /// The app fills the window, with the conversation put away.
    full: std::sync::atomic::AtomicBool,
}

impl Who {
    pub(crate) fn new(home: App, store: Store) -> Self {
        Who {
            home: Arc::new(home),
            home_store: Arc::new(store),
            shown: Mutex::new(None),
            code_open: Default::default(),
            full: Default::default(),
        }
    }

    pub(crate) fn shown(&self) -> Option<Draft> {
        self.shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_shown(&self, d: Option<Draft>) {
        self.code_open
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.full.store(false, std::sync::atomic::Ordering::Relaxed);
        *self
            .shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = d;
    }

    /// The app a webview with this label acts for.
    pub(crate) fn me_by_label(&self, label: &str) -> Result<Arc<App>, String> {
        if !self.home.manager {
            return Ok(self.home.clone());
        }
        match label {
            "main" => Ok(self.home.clone()),
            "app" | "talk" => self
                .shown()
                .map(|d| d.app)
                .ok_or_else(|| "not allowed: no app is being made here".into()),
            _ => Err("not allowed".into()),
        }
    }

    pub(crate) fn me(&self, w: &tauri::Webview) -> Result<Arc<App>, String> {
        self.me_by_label(w.label())
    }

    /// The app's own saved data (`app.store`), for the same webview.
    pub(crate) fn store(&self, w: &tauri::Webview) -> Result<Arc<Store>, String> {
        if !self.home.manager || w.label() == "main" {
            return Ok(self.home_store.clone());
        }
        match w.label() {
            "app" => self
                .shown()
                .map(|d| d.store)
                .ok_or_else(|| "not allowed: no app is being made here".into()),
            _ => Err("not allowed".into()),
        }
    }
}

// ---- OpenAs's window: home, and the app being made ---------------------------

/// The drafts folder of this setup.
fn apps_dir(home: &App) -> PathBuf {
    root(home).join("apps")
}

/// A draft's id as Home sends it: `.draft-<digits>`, nothing else.
fn draft_id_ok(id: &str) -> bool {
    id.strip_prefix(".draft-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The lock a running build holds in its draft folder (core's draft.rs
/// takes it under the same name).
const BUILD_LOCK: &str = "build.lock";

/// A draft whose making.json says it's working while its build process is
/// gone (crashed, killed, the Mac restarted) is marked stopped, so it can be
/// continued or discarded instead of showing "Being made" forever.
pub(crate) fn settle(ws: &Path) {
    let p = ws.join("making.json");
    let mut m = read_json(&p);
    // A Save or Discard nobody picked up (its process was gone) would keep
    // both disabled; after a moment with no build alive, it's forgotten.
    if m["phase"] == "done" && !build_alive(ws, &m) {
        for ext in ["keep", "discard"] {
            let f = ws.with_extension(ext);
            let old = std::fs::metadata(&f)
                .and_then(|x| x.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|d| d.as_secs() > 10);
            if old {
                let _ = std::fs::remove_file(f);
            }
        }
    }
    if !matches!(
        m["phase"].as_str(),
        Some("planning" | "building" | "checking")
    ) || build_alive(ws, &m)
    {
        return;
    }
    m["phase"] = json!("stopped");
    let tmp = ws.join(".settle.tmp");
    if std::fs::write(&tmp, m.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
}

pub(crate) fn build_alive(ws: &Path, m: &Value) -> bool {
    match std::fs::OpenOptions::new()
        .write(true)
        .open(ws.join(BUILD_LOCK))
    {
        // Taking it would only work if nobody holds it; dropped at once.
        Ok(f) => matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        // No lock yet: the build is still starting, or it's from before the
        // lock existed. Trust a recent heartbeat only.
        Err(_) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let last = m["now"].as_u64().or(m["started"].as_u64()).unwrap_or(0);
            now.saturating_sub(last) < 60
        }
    }
}

/// The apps being made, newest first, for Home's shelf.
pub(crate) fn drafts(home: &App) -> Vec<Value> {
    let mut out = vec![];
    for e in std::fs::read_dir(apps_dir(home))
        .into_iter()
        .flatten()
        .flatten()
    {
        let id = e.file_name().to_string_lossy().into_owned();
        let p = e.path();
        if !draft_id_ok(&id) || !p.is_dir() || p.with_extension("moved").exists() {
            continue;
        }
        settle(&p);
        let m = read_json(&p.join("making.json"));
        let request = std::fs::read_to_string(p.join("prompt.txt")).unwrap_or_default();
        out.push(json!({
            "id": id,
            "request": request.trim(),
            "name": m["name"],
            "phase": m["phase"],
            "started": m["started"],
            "part": m["part"],
            "parts": m["plan"]["parts"].as_array().map_or(0, Vec::len),
        }));
    }
    out.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    out
}

/// The draft's page: its own origin, so it shares nothing with OpenAs's pages.
fn draft_url() -> WebviewUrl {
    #[cfg(windows)]
    return url("/index.html");
    #[cfg(not(windows))]
    WebviewUrl::CustomProtocol("app://draft/index.html".parse().unwrap())
}

/// Shows an app being made in OpenAs's window, in place of Home.
pub(crate) fn show(app: &AppHandle, id: &str) -> Result<(), String> {
    let who = app.state::<Who>();
    if !who.home.manager {
        return Err("not allowed".into());
    }
    if !draft_id_ok(id) {
        return Err("not a draft".into());
    }
    let folder = apps_dir(&who.home).join(id);
    if !folder.is_dir() {
        return Err("that app is no longer being made".into());
    }
    let name = read_json(&folder.join("making.json"))["name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .unwrap_or("New app")
        .to_string();
    let draft = App {
        folder: folder.clone(),
        name: name.clone(),
        id: None,
        check: None,
        helper: who.home.helper.clone(),
        manager: false,
        making: true,
        env: who.home.env.clone(),
    };
    // OpenAs is here to choose Save as App or Discard for it; while OpenAs
    // runs, the build waits for that choice.
    let _ = std::fs::write(
        folder.with_extension("open"),
        std::process::id().to_string(),
    );
    who.set_shown(Some(Draft {
        app: Arc::new(draft),
        store: Arc::new(load_store(&folder, true)),
    }));
    let win = app.get_window("main").ok_or("no window")?;
    // The conversation spans the window and draws the bar and the panel; the
    // app's page sits on top of it, in the panel. So the conversation comes
    // first, and the page is added after it.
    match app.get_webview("talk") {
        Some(t) => {
            let _ = t.eval("location.reload()");
        }
        None => {
            let tb = tauri::webview::WebviewBuilder::new("talk", url("/__oa/talk.html"))
                .initialization_script(oa_bridge("OpenAs"))
                .on_navigation(|u| u.scheme() == "app" || u.host_str() == Some("app.localhost"))
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
            win.add_child(
                tb,
                tauri::LogicalPosition::new(0.0, 0.0),
                tauri::LogicalSize::new(1.0, 1.0),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    // A new page for each draft: nothing carries over from the last one.
    if let Some(old) = app.get_webview("app") {
        let _ = old.close();
    }
    let ab = tauri::webview::WebviewBuilder::new("app", draft_url());
    // With no peer connections from the first page (a configuration of our
    // own, so it keeps nothing between drafts itself).
    #[cfg(target_os = "macos")]
    let ab =
        match crate::powers::no_peers_config(true, crate::powers::preview_mark(&folder).exists()) {
            Some(c) => ab.with_webview_configuration(c),
            None => ab,
        };
    let ab = ab
        .incognito(true)
        .initialization_script(bridge())
        .on_navigation(|u| {
            let own = u.scheme() == "app" || u.host_str() == Some("app.localhost");
            own || matches!(u.scheme(), "about" | "data" | "blob")
        })
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_download(|_, _| false)
        // No peer connections until the draft asked for the internet.
        .on_page_load(|w, p| {
            if matches!(p.event(), tauri::webview::PageLoadEvent::Started) {
                let on = w
                    .app_handle()
                    .try_state::<Who>()
                    .and_then(|who| who.shown())
                    .is_some_and(|d| crate::powers::online(&d.app));
                crate::powers::peer_connections(&w, on);
            }
        });
    let page = win
        .add_child(
            ab,
            tauri::LogicalPosition::new(0.0, 0.0),
            tauri::LogicalSize::new(1.0, 1.0),
        )
        .map_err(|e| e.to_string())?;
    round_corners(&page, PANEL_R);
    make_room(&win);
    let _ = win.set_title(&name);
    layout(app);
    if let Some(t) = app.get_webview("talk") {
        let _ = t.set_focus();
    }
    Ok(())
}

/// Back to Home. The build carries on; its tile on the shelf opens it again.
/// `saved`: the app that was just saved, for Home to say so.
pub(crate) fn go_home(app: &AppHandle, saved: Option<&str>) {
    let who = app.state::<Who>();
    who.set_shown(None);
    if let Some(a) = app.get_webview("app") {
        let _ = a.close();
    }
    if let Some(w) = app.get_window("main") {
        let _ = w.set_title("OpenAs");
    }
    layout(app);
    if let Some(m) = app.get_webview("main") {
        let note = json!({ "saved": saved });
        let _ = m.eval(format!("window.__home && window.__home({note})"));
        let _ = m.set_focus();
    }
}

/// The app and its conversation need more width than Home: grow the window
/// to fit both, keeping it on screen.
fn make_room(win: &tauri::Window) {
    let (Ok(size), Ok(scale), Ok(pos)) =
        (win.inner_size(), win.scale_factor(), win.outer_position())
    else {
        return;
    };
    let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
    let want = 900.0 + TALK_W;
    if w >= want - 40.0 {
        return;
    }
    let (mut x, y) = (pos.x as f64 / scale, pos.y as f64 / scale);
    let mut w2 = want;
    if let Ok(Some(mon)) = win.current_monitor() {
        let wa = mon.work_area();
        let left = wa.position.x as f64 / scale;
        let right = left + wa.size.width as f64 / scale;
        w2 = w2.min(right - left);
        if x + w2 > right {
            x = (right - w2).max(left);
        }
    }
    let _ = win.set_size(tauri::LogicalSize::new(w2, h));
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
}

/// The bar across the top of OpenAs's window, and the panel the app being
/// made sits in: the conversation page draws both to these numbers.
pub(crate) const BAR_H: f64 = 52.0;
const PANEL_INSET: f64 = 8.0;
const PANEL_R: f64 = 10.0;

/// Rounds a webview's corners and draws a thin outline round it, so the app
/// reads as a panel in the window whatever colour the app itself is.
fn round_corners(w: &tauri::Webview, r: f64) {
    #[cfg(target_os = "macos")]
    let _ = w.with_webview(move |p| unsafe {
        use objc2::encode::{Encoding, RefEncode};
        use objc2::{class, msg_send, runtime::AnyObject};
        #[repr(C)]
        struct CGColor([u8; 0]);
        unsafe impl RefEncode for CGColor {
            const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
        }
        let view = p.inner().cast::<AnyObject>();
        if view.is_null() {
            return;
        }
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: r];
            // Mid grey at low strength: visible on a dark app and a light one.
            let c: *mut AnyObject =
                msg_send![class!(NSColor), colorWithWhite: 0.5f64, alpha: 0.35f64];
            let cg: *const CGColor = msg_send![c, CGColor];
            let _: () = msg_send![layer, setBorderColor: cg];
            let _: () = msg_send![layer, setBorderWidth: 1.0f64];
            let _: () = msg_send![layer, setMasksToBounds: true];
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = (w, r);
}

/// Home fills the window. An app being made: its conversation fills it
/// (the bar, the column on the left, the panel's ground on the right) and
/// the app's own page sits in the panel.
pub(crate) fn layout(app: &AppHandle) {
    let Some(win) = app.get_window("main") else {
        return;
    };
    let (Ok(size), Ok(scale)) = (win.inner_size(), win.scale_factor()) else {
        return;
    };
    let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
    let rect = |x: f64, y: f64, w: f64, h: f64| tauri::Rect {
        position: tauri::LogicalPosition::new(x, y).into(),
        size: tauri::LogicalSize::new(w.max(1.0), h.max(1.0)).into(),
    };
    let who = app.state::<Who>();
    let making = who.shown().is_some();
    let code = who.code_open.load(std::sync::atomic::Ordering::Relaxed);
    let tw = if who.full.load(std::sync::atomic::Ordering::Relaxed) {
        0.0
    } else {
        TALK_W.min(w * 0.5)
    };
    if let Some(m) = app.get_webview("main") {
        let _ = m.set_bounds(rect(0.0, 0.0, w, h));
        let _ = if making { m.hide() } else { m.show() };
    }
    if let Some(a) = app.get_webview("app") {
        let px = tw + PANEL_INSET;
        let _ = a.set_bounds(rect(
            px,
            BAR_H,
            w - px - PANEL_INSET,
            h - BAR_H - PANEL_INSET,
        ));
        let _ = if making && !code { a.show() } else { a.hide() };
    }
    if let Some(t) = app.get_webview("talk") {
        let _ = t.set_bounds(rect(0.0, 0.0, w, h));
        let _ = if making { t.show() } else { t.hide() };
    }
}

/// Keeps the shown draft's page and menu in step with its build: reloads as
/// files are written, replays the test pointer, enables Save as App and
/// Discard once it's done, and goes Home once it's saved or discarded.
pub(crate) fn watch(app: AppHandle, save: MenuItem<tauri::Wry>, discard: MenuItem<tauri::Wry>) {
    std::thread::spawn(move || {
        let mut current: Option<PathBuf> = None;
        let (mut files_seq, mut ghost_seq, mut ghost_cleared) = (0u64, 0u64, false);
        let mut title = String::new();
        let mut reload_at: Option<std::time::Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let Some(d) = app.state::<Who>().shown() else {
                current = None;
                let _ = save.set_enabled(false);
                let _ = discard.set_enabled(false);
                continue;
            };
            let folder = d.app.folder.clone();
            if current.as_ref() != Some(&folder) {
                current = Some(folder.clone());
                (files_seq, ghost_seq, ghost_cleared, reload_at) = (0, 0, false, None);
                title.clear();
            }
            let moved = folder.with_extension("moved");
            if moved.exists() {
                let _ = save.set_enabled(false);
                let _ = discard.set_enabled(false);
                // Saved: the app opens on its own. Discarded: long enough to
                // read so. Either way, back to Home.
                let info = read_json(&moved);
                let name = info["app"].as_str().and_then(|p| {
                    Path::new(p)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                });
                let discarded = info["discarded"] == true;
                std::thread::sleep(Duration::from_millis(if discarded { 1800 } else { 900 }));
                if app
                    .state::<Who>()
                    .shown()
                    .is_some_and(|s| s.app.folder == folder)
                {
                    go_home(&app, if discarded { None } else { name.as_deref() });
                }
                continue;
            }
            let m = read_json(&folder.join("making.json"));
            // The window carries the app's name as soon as the build gives it one.
            let name = m["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .unwrap_or("New app");
            if name != title {
                title = name.to_string();
                if let Some(w) = app.get_window("main") {
                    let _ = w.set_title(&title);
                }
            }
            let ready = m["phase"] == "done"
                && !folder.with_extension("keep").exists()
                && !folder.with_extension("discard").exists();
            let _ = save.set_enabled(ready);
            let _ = discard.set_enabled(ready);
            let view = app.get_webview("app");
            let fs = m["files_seq"].as_u64().unwrap_or(0);
            if fs != files_seq {
                files_seq = fs;
                // Reload once the writing pauses, not on every edit.
                reload_at = Some(std::time::Instant::now());
            }
            if m["phase"] == "done" && !ghost_cleared {
                ghost_cleared = true;
                if let Some(a) = &view {
                    let _ = a.eval("document.getElementById('__oa_ghost')?.remove()");
                }
            }
            let gs = m["ghost"]["seq"].as_u64().unwrap_or(0);
            if gs != ghost_seq && m["phase"] != "done" {
                ghost_seq = gs;
                if let Some(a) = &view {
                    let _ = a.eval(ghost_js(&m["ghost"]));
                }
            }
            if reload_at.is_some_and(|t| t.elapsed() >= Duration::from_millis(900)) {
                reload_at = None;
                if let Some(a) = &view {
                    let on = a
                        .app_handle()
                        .try_state::<Who>()
                        .and_then(|who| who.shown())
                        .is_some_and(|d| crate::powers::online(&d.app));
                    crate::powers::reload(a, on);
                }
            }
        }
    });
}

// ---- Commands for OpenAs's window --------------------------------------------

/// Home asks for the apps being made, for its shelf.
#[tauri::command]
pub fn drafts_list(webview: tauri::Webview, who: State<Who>) -> Result<Value, String> {
    if webview.label() != "main" || !who.home.manager {
        return Err("not allowed".into());
    }
    Ok(json!(drafts(&who.home)))
}

/// Home opens an app being made (just started, or from its tile).
#[tauri::command]
pub fn draft_show(id: String, webview: tauri::Webview, app: AppHandle) -> Result<(), String> {
    let who = app.state::<Who>();
    if webview.label() != "main" || !who.home.manager {
        return Err("not allowed".into());
    }
    show(&app, &id)
}

/// Show code: the panel shows the draft's code instead of the app (the
/// conversation page draws it there), and the app again when it's closed.
#[tauri::command]
pub fn draft_code_view(open: bool, webview: tauri::Webview, app: AppHandle) -> Result<(), String> {
    let who = app.state::<Who>();
    if webview.label() != "talk" || !who.home.manager {
        return Err("not allowed".into());
    }
    who.code_open
        .store(open, std::sync::atomic::Ordering::Relaxed);
    layout(&app);
    Ok(())
}

/// The bar's fill-the-window button: the app takes the whole window, and the
/// conversation comes back when it's pressed again.
#[tauri::command]
pub fn draft_full(open: bool, webview: tauri::Webview, app: AppHandle) -> Result<(), String> {
    let who = app.state::<Who>();
    if webview.label() != "talk" || !who.home.manager {
        return Err("not allowed".into());
    }
    who.full.store(open, std::sync::atomic::Ordering::Relaxed);
    layout(&app);
    Ok(())
}

/// The conversation's way back to Home.
#[tauri::command]
pub fn draft_home(webview: tauri::Webview, app: AppHandle) -> Result<(), String> {
    let who = app.state::<Who>();
    if webview.label() != "talk" || !who.home.manager {
        return Err("not allowed".into());
    }
    go_home(&app, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(manager: bool, making: bool, folder: &str) -> App {
        App {
            folder: PathBuf::from(folder),
            name: "x".into(),
            id: None,
            check: None,
            helper: None,
            manager,
            making,
            env: vec![],
        }
    }

    fn store() -> Store {
        Store {
            file: PathBuf::from("/nonexistent/store.json"),
            map: Mutex::new(Map::new()),
            persist: false,
        }
    }

    #[test]
    fn an_apps_own_process_is_that_app_for_every_webview() {
        let who = Who::new(app(false, false, "/a"), store());
        for l in ["main", "app", "talk", "change", "anything"] {
            assert_eq!(who.me_by_label(l).unwrap().folder, PathBuf::from("/a"));
        }
    }

    #[test]
    fn in_openas_only_home_speaks_for_openas() {
        let who = Who::new(app(true, false, "/home"), store());
        assert!(who.me_by_label("main").unwrap().manager);
        // No draft shown: the draft's labels speak for nobody.
        for l in ["app", "talk", "change", "versions", "becoming", ""] {
            assert!(who.me_by_label(l).is_err(), "{l}");
        }
        who.set_shown(Some(Draft {
            app: Arc::new(app(false, true, "/d")),
            store: Arc::new(store()),
        }));
        for l in ["app", "talk"] {
            let me = who.me_by_label(l).unwrap();
            assert!(!me.manager && me.making, "{l} must be the draft");
            assert_eq!(me.folder, PathBuf::from("/d"));
        }
        assert!(who.me_by_label("change").is_err());
        assert!(who.me_by_label("main").unwrap().manager);
    }

    #[test]
    fn draft_ids_are_only_draft_folders() {
        assert!(draft_id_ok(".draft-1795726158357196000"));
        for bad in [
            "",
            ".draft-",
            "../x",
            ".draft-1/../../etc",
            "Darkroom",
            ".draft-12a",
            ".draft-1 ",
        ] {
            assert!(!draft_id_ok(bad), "{bad}");
        }
    }

    /// Every command reaches its app through `Who`, so a draft's page in
    /// OpenAs's window can never act as OpenAs. A command that takes the
    /// process's own `App` or `Store` straight from Tauri state fails this.
    #[test]
    fn no_command_reads_the_process_identity_directly() {
        let sources = [
            ("api.rs", include_str!("api.rs")),
            ("helper.rs", include_str!("helper.rs")),
            ("talk.rs", include_str!("talk.rs")),
            ("powers/ask.rs", include_str!("powers/ask.rs")),
            ("powers/extras.rs", include_str!("powers/extras.rs")),
            ("powers/files.rs", include_str!("powers/files.rs")),
            ("powers/mod.rs", include_str!("powers/mod.rs")),
        ];
        for (name, src) in sources {
            for bad in [
                "State<App>",
                "State<Store>",
                "state::<App>()",
                "state::<Store>()",
            ] {
                assert!(!src.contains(bad), "{name} uses {bad}: go through Who");
            }
        }
    }
}
