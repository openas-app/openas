//! The internet, as a permission. An app starts offline; it asks
//! (`app.permissions.request(['internet'])`) when it needs live data, or the
//! person turns it on from the app's menu. Like every power it belongs to one
//! version: after a change, an app that was online asks again, by itself,
//! when its page loads.
//!
//! Kept in `<app>/permissions.json` as `{"network": bool, "version": key}`,
//! next to the code, never inside it. No file is offline; a yes for another
//! version (or from before versions) asks again. A change being tried keeps
//! its own answer in `.pending/permissions.json`: Discard drops it with the
//! change, Save makes it the saved version's. The load check reads the
//! stage's `network` as written by the Helper; a draft being tried in OpenAs
//! is online once it asks (`.preview-internet`), and the installed app asks
//! the person again at its first launch.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogResult};

use super::{version_key, Grant};
use crate::App;

fn read(folder: &Path) -> Value {
    std::fs::read_to_string(folder.join("permissions.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// The change being tried, when there is one (its code is what runs).
fn trying(folder: &Path) -> Option<PathBuf> {
    let p = folder.join(".pending");
    p.join("app/index.html").exists().then_some(p)
}

/// Whether the person allowed the internet for the code that runs now.
pub fn net_state(folder: &Path) -> Grant {
    let saved = read(folder);
    let saved_on = saved.get("network").and_then(Value::as_bool) == Some(true);
    if let Some(p) = trying(folder) {
        return match read(&p).get("network").and_then(Value::as_bool) {
            Some(true) => Grant::Yes,
            Some(false) => Grant::No,
            None if saved_on => Grant::Stale,
            None => Grant::No,
        };
    }
    if !saved_on {
        Grant::No
    } else if saved.get("version").and_then(Value::as_str) == Some(version_key(folder).as_str()) {
        Grant::Yes
    } else {
        Grant::Stale
    }
}

/// Records the person's answer for the code that runs now: a change being
/// tried keeps its own, so the saved version's answer stands if it's dropped.
pub fn set_net(folder: &Path, on: bool) -> Result<(), String> {
    let (dir, body) = match trying(folder) {
        Some(p) => (p, json!({ "network": on })),
        None => (
            folder.to_path_buf(),
            json!({ "network": on, "version": version_key(folder) }),
        ),
    };
    let tmp = dir.join("permissions.json.tmp");
    std::fs::write(&tmp, body.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join("permissions.json")).map_err(|e| e.to_string())
}

/// What only an app with the internet has, besides the content policy:
/// peer connections (their STUN step) and link preconnects (a lookup of any
/// host name). Sets them on `prefs`; true if anything changed.
#[cfg(target_os = "macos")]
unsafe fn web_extras(prefs: &objc2::runtime::AnyObject, on: bool) -> bool {
    use objc2::{class, msg_send, runtime::AnyObject, sel};
    let mut changed = false;
    let can: bool = msg_send![prefs, respondsToSelector: sel!(_setPeerConnectionEnabled:)];
    if can {
        let was: bool = msg_send![prefs, _peerConnectionEnabled];
        if was != on {
            let _: () = msg_send![prefs, _setPeerConnectionEnabled: on];
            changed = true;
        }
    }
    let cls = class!(WKPreferences);
    let listed: bool = msg_send![cls, respondsToSelector: sel!(_features)];
    let settable: bool = msg_send![prefs, respondsToSelector: sel!(_setEnabled:forFeature:)];
    if listed && settable {
        let features: *mut AnyObject = msg_send![cls, _features];
        let n: usize = if features.is_null() {
            0
        } else {
            msg_send![features, count]
        };
        for i in 0..n {
            let f: *mut AnyObject = msg_send![features, objectAtIndex: i];
            let key: *mut AnyObject = msg_send![f, key];
            if key.is_null() {
                continue;
            }
            let utf8: *const std::ffi::c_char = msg_send![key, UTF8String];
            if utf8.is_null() || std::ffi::CStr::from_ptr(utf8).to_bytes() != b"LinkPreconnect" {
                continue;
            }
            let was: bool = msg_send![prefs, _isEnabledForFeature: f];
            if was != on {
                let _: () = msg_send![prefs, _setEnabled: on, forFeature: f];
                changed = true;
            }
        }
    }
    changed
}

/// A configuration whose first page starts with the extras set for the
/// app's answer (`web_extras`), so it loads once. None off the main thread
/// (the page's start then sets them, reloading once).
#[cfg(target_os = "macos")]
pub(crate) fn no_peers_config(
    incognito: bool,
    on: bool,
) -> Option<objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration>> {
    let mtm = objc2::MainThreadMarker::new()?;
    unsafe {
        let cfg = objc2_web_kit::WKWebViewConfiguration::new(mtm);
        let prefs = cfg.preferences();
        web_extras(&prefs, on);
        if incognito {
            cfg.setWebsiteDataStore(&objc2_web_kit::WKWebsiteDataStore::nonPersistentDataStore(
                mtm,
            ));
        }
        Some(cfg)
    }
}

/// A draft tried in OpenAs that asked for the internet.
pub(crate) fn preview_mark(folder: &Path) -> PathBuf {
    folder.join(".preview-internet")
}

/// What this window's page may reach now.
pub(crate) fn online(me: &App) -> bool {
    if me.check.is_some() {
        // The stage's switch, as the Helper copied it from the app.
        return read(&me.folder).get("network").and_then(Value::as_bool) == Some(true);
    }
    if me.making {
        return preview_mark(&me.folder).exists();
    }
    net_state(&me.folder) == Grant::Yes
}

/// Called as each page starts: an app without the internet has no peer
/// connections or preconnects (`web_extras`). A page that started under the
/// other setting is loaded again (once; next time it matches).
pub(crate) fn peer_connections(w: &impl PageView, on: bool) {
    let w = w.view();
    #[cfg(target_os = "macos")]
    let _ = w.with_webview(move |p| unsafe {
        use objc2::{msg_send, runtime::AnyObject};
        let view = p.inner().cast::<AnyObject>();
        if view.is_null() {
            return;
        }
        let cfg: *mut AnyObject = msg_send![view, configuration];
        let prefs: *mut AnyObject = if cfg.is_null() {
            std::ptr::null_mut()
        } else {
            msg_send![cfg, preferences]
        };
        if let Some(prefs) = prefs.as_ref() {
            if web_extras(prefs, on) {
                let _: () = msg_send![view, reload];
            }
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = (w, on);
}

/// A fresh page under `on`: the switches first, so the new page starts with
/// them, then the reload. Every reload of an app's page goes through here.
pub(crate) fn reload(w: &impl PageView, on: bool) {
    let w = w.view();
    #[cfg(target_os = "macos")]
    {
        let _ = w.with_webview(move |p| unsafe {
            use objc2::{msg_send, runtime::AnyObject};
            let view = p.inner().cast::<AnyObject>();
            if view.is_null() {
                return;
            }
            let cfg: *mut AnyObject = msg_send![view, configuration];
            let prefs: *mut AnyObject = if cfg.is_null() {
                std::ptr::null_mut()
            } else {
                msg_send![cfg, preferences]
            };
            if let Some(prefs) = prefs.as_ref() {
                web_extras(prefs, on);
            }
            let _: () = msg_send![view, reload];
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (on, w.eval("location.reload()"));
    }
}

/// A page's webview, from a window that is one or from the webview itself.
pub(crate) trait PageView {
    fn view(&self) -> &tauri::Webview;
}
impl PageView for tauri::Webview {
    fn view(&self) -> &tauri::Webview {
        self
    }
}
impl PageView for tauri::WebviewWindow {
    fn view(&self) -> &tauri::Webview {
        self.as_ref()
    }
}

/// The app's page webview (beside the conversation column, or the window's).
pub(crate) fn page(app: &AppHandle) -> Option<tauri::Webview> {
    app.get_webview("app").or_else(|| app.get_webview("main"))
}

/// The menu's tick follows what the page that just loaded may reach.
pub(crate) fn sync_tick(app: &AppHandle, on: bool) {
    if let Some(t) = app.try_state::<crate::NetToggle>() {
        let _ = t.item.set_checked(on);
    }
}

/// After the answer changed: menu tick, peer connections, then a fresh page
/// under the new content policy.
pub(crate) fn apply(app: &AppHandle, on: bool) {
    if let Some(t) = app.try_state::<crate::NetToggle>() {
        let _ = t.item.set_checked(on);
    }
    if let Some(w) = page(app) {
        reload(&w, on);
    }
}

static ASKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// An app that was online and has changed since asks again as its page
/// loads, whether or not its code asks: its new code may use the internet
/// differently. Only in an installed app's own window.
pub(crate) fn ask_if_changed(app: &AppHandle) {
    let me = app.state::<App>().inner().clone();
    if me.manager || me.making || me.check.is_some() || net_state(&me.folder) != Grant::Stale {
        return;
    }
    if ASKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let yes = ask(&app, &me.name, true);
        // Asked about the code that runs now; a change saved meanwhile asks
        // again on its own page load.
        if net_state(&me.folder) == Grant::Stale {
            let _ = set_net(&me.folder, yes);
            if yes {
                apply(&app, true);
            } else if let Some(t) = app.try_state::<crate::NetToggle>() {
                let _ = t.item.set_checked(false);
            }
        }
        ASKING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// The native ask, on its own (from a background thread: it blocks).
pub(crate) fn ask(app: &AppHandle, name: &str, changed: bool) -> bool {
    let mut text = format!("• {}", LINE);
    if changed {
        text.push_str("\n\nYou allowed this before. The app has changed since, so it asks again.");
    }
    let mut d = app
        .dialog()
        .message(text)
        .title(format!("{name} would like to:"))
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Allow".into(),
            "Don't Allow".into(),
        ));
    if let Some(w) = app.get_window("main") {
        d = d.parent(&w);
    }
    match d.blocking_show_with_result() {
        MessageDialogResult::Ok | MessageDialogResult::Yes => true,
        MessageDialogResult::Custom(s) => s == "Allow",
        _ => false,
    }
}

/// How the ask names it.
pub(crate) const LINE: &str = "Connect to the internet";

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-net-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("version.json"), r#"{"t":1}"#).unwrap();
        d
    }

    #[test]
    fn offline_unless_allowed_for_this_version() {
        let f = tmp("versions");
        // No file: offline, and nothing to ask again about.
        assert_eq!(net_state(&f), Grant::No);
        // A file from before versions: asks once.
        std::fs::write(f.join("permissions.json"), r#"{"network":true}"#).unwrap();
        assert_eq!(net_state(&f), Grant::Stale);
        set_net(&f, true).unwrap();
        assert_eq!(net_state(&f), Grant::Yes);
        // A change being tried asks again, and keeps its own answer...
        std::fs::create_dir_all(f.join(".pending/app")).unwrap();
        std::fs::write(f.join(".pending/app/index.html"), "x").unwrap();
        assert_eq!(net_state(&f), Grant::Stale);
        set_net(&f, false).unwrap();
        assert_eq!(net_state(&f), Grant::No);
        // ...so dropping it leaves the saved version's yes as it was.
        std::fs::remove_dir_all(f.join(".pending")).unwrap();
        assert_eq!(net_state(&f), Grant::Yes);
        // Saved as a new version: asks again.
        std::fs::write(f.join("version.json"), r#"{"t":2}"#).unwrap();
        assert_eq!(net_state(&f), Grant::Stale);
        set_net(&f, false).unwrap();
        assert_eq!(net_state(&f), Grant::No);
    }

    fn me(folder: &Path, making: bool, check: bool) -> App {
        App {
            folder: folder.to_path_buf(),
            name: "x".into(),
            id: None,
            check: check.then(|| folder.join("report.json")),
            helper: None,
            manager: false,
            making,
            env: vec![],
        }
    }

    #[test]
    fn what_each_window_may_reach() {
        let f = tmp("windows");
        assert!(!online(&me(&f, false, false)));
        // A draft tried in OpenAs: online once it asked.
        assert!(!online(&me(&f, true, false)));
        std::fs::write(preview_mark(&f), "").unwrap();
        assert!(online(&me(&f, true, false)));
        // ...which says nothing about the installed app.
        assert!(!online(&me(&f, false, false)));
        // The load check follows the stage's switch as written.
        std::fs::write(f.join("permissions.json"), r#"{"network":true}"#).unwrap();
        assert!(online(&me(&f, false, true)));
        assert!(!online(&me(&f, false, false)));
    }
}
