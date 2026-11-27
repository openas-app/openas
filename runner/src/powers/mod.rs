//! Desktop powers for an app's page: notifications, the menu bar, staying on
//! top, global shortcuts and one folder the person picked (`ask`, `files`,
//! `extras`). Running programs and reaching any file needs full access, which
//! has its own warning and is never given to a shared app.
//!
//! Every power is asked for in a native dialog and kept in `<app>/grants.json`,
//! next to the app's code (never inside it, so the model can't write it). A
//! grant belongs to one version: after a change the app asks again, because
//! the person hasn't seen what the new code does with it.
//!
//! File calls are confined to the granted folder here, in Rust: paths are
//! relative, `..` is refused, and every path is resolved through symlinks
//! and must still be inside the folder.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogResult};

use crate::{App, Who};

/// The powers a page may ask for, with how the ask names them.
pub const KNOWN: &[(&str, &str)] = &[
    ("notifications", "Send you notifications"),
    ("menu-bar", "Show itself in the menu bar"),
    ("overlay", "Stay on top of other windows"),
    ("folder", "Use a folder you choose"),
    (
        "shortcuts",
        "Hear its keyboard shortcuts while you're in other apps",
    ),
    ("full", "Full access to your Mac"),
    ("internet", net::LINE),
];

pub(crate) const MAX_READ: u64 = 8 << 20;
pub(crate) const MAX_DATA_URL: u64 = 25 << 20;
pub(crate) const MAX_WRITE: usize = 8 << 20;
pub(crate) const MAX_LIST: usize = 5000;

// ---- Grants ------------------------------------------------------------------

/// The version a grant is tied to: when the app's code was last put in place.
/// While a change waits to be saved, its code is what runs, so it is a version
/// of its own and asks again; a grant given to it never carries over.
pub(crate) fn version_key(folder: &Path) -> String {
    let pending = folder.join(".pending");
    if pending.join("app/index.html").exists() {
        let made = std::fs::metadata(&pending)
            .and_then(|m| m.created().or_else(|_| m.modified()))
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        return format!("pending-{made}");
    }
    std::fs::read_to_string(folder.join("version.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("t").map(|t| t.to_string()))
        .unwrap_or_else(|| "first".into())
}

pub fn load(folder: &Path) -> Value {
    std::fs::read_to_string(folder.join("grants.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

pub(crate) fn save(folder: &Path, g: &Value) -> Result<(), String> {
    let tmp = folder.join("grants.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(g).unwrap()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, folder.join("grants.json")).map_err(|e| e.to_string())
}

#[derive(Debug, PartialEq)]
pub enum Grant {
    Yes,
    /// Granted for an earlier version.
    Stale,
    No,
}

pub fn state(folder: &Path, name: &str) -> Grant {
    let g = load(folder);
    let on = match name {
        "folder" => g.get("folder").and_then(Value::as_str).is_some(),
        _ => g.get(name).and_then(Value::as_bool).unwrap_or(false),
    };
    if !on {
        Grant::No
    } else if g.get("version").and_then(Value::as_str) == Some(version_key(folder).as_str()) {
        Grant::Yes
    } else {
        Grant::Stale
    }
}

/// Records what the person allowed just now, for this version. Grants for
/// other powers from an earlier version are dropped, not carried over.
pub(crate) fn grant(folder: &Path, set: &[(&str, Value)]) -> Result<(), String> {
    let mut g = load(folder);
    let v = version_key(folder);
    if g.get("version").and_then(Value::as_str) != Some(v.as_str()) {
        g = json!({ "version": v });
    }
    for (k, val) in set {
        g[*k] = val.clone();
    }
    save(folder, &g)
}

pub mod ask;
pub mod extras;
pub mod files;
pub mod net;

// The parts share their helpers through here.
pub(crate) use ask::*;
pub(crate) use extras::*;
pub(crate) use files::*;
pub(crate) use net::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-powers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("root/sub")).unwrap();
        std::fs::create_dir_all(d.join("outside")).unwrap();
        std::fs::write(d.join("root/a.txt"), "a").unwrap();
        std::fs::write(d.join("outside/secret.txt"), "s").unwrap();
        d
    }

    #[test]
    fn unsaved_code_does_not_inherit_grants() {
        let d = tmp("pending");
        let f = d.join("root");
        std::fs::write(f.join("version.json"), r#"{"t":1}"#).unwrap();
        grant(&f, &[("system", json!(true))]).unwrap();
        assert_eq!(state(&f, "system"), Grant::Yes);
        // A change is waiting: its code runs, and must ask again.
        std::fs::create_dir_all(f.join(".pending/app")).unwrap();
        std::fs::write(f.join(".pending/app/index.html"), "x").unwrap();
        assert_eq!(state(&f, "system"), Grant::Stale);
        // Discarded: the saved version's grant holds again.
        std::fs::remove_dir_all(f.join(".pending")).unwrap();
        assert_eq!(state(&f, "system"), Grant::Yes);
    }

    #[test]
    fn refuses_parent_and_absolute() {
        let d = tmp("dots");
        let r = d.join("root");
        assert!(resolve(&r, "../outside/secret.txt", true).is_err());
        assert!(resolve(&r, "sub/../../outside/secret.txt", false).is_err());
        assert!(resolve(&r, "/etc/passwd", true).is_err());
        assert!(resolve(&r, "a.txt", true).is_ok());
        assert!(resolve(&r, "sub/new.txt", false).is_ok());
        assert!(resolve(&r, "missing/new.txt", false).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn refuses_symlink_escapes() {
        let d = tmp("links");
        let r = d.join("root");
        std::os::unix::fs::symlink(d.join("outside"), r.join("out")).unwrap();
        std::os::unix::fs::symlink(d.join("outside/secret.txt"), r.join("s.txt")).unwrap();
        std::os::unix::fs::symlink(r.join("a.txt"), r.join("inner.txt")).unwrap();
        assert!(resolve(&r, "out/secret.txt", true).is_err());
        assert!(resolve(&r, "out/new.txt", false).is_err());
        assert!(resolve(&r, "s.txt", true).is_err());
        // Writing through a link that points outside is refused too.
        assert!(file_op(&r, "writeText", &json!({"path": "s.txt", "text": "x"})).is_err());
        assert_eq!(
            std::fs::read_to_string(d.join("outside/secret.txt")).unwrap(),
            "s"
        );
        assert!(resolve(&r, "inner.txt", true).is_ok());
    }

    #[test]
    fn file_ops_stay_inside() {
        let d = tmp("ops");
        let r = d.join("root");
        file_op(&r, "mkdir", &json!({"path": "Sorted/Docs"})).unwrap();
        file_op(
            &r,
            "move",
            &json!({"from": "a.txt", "to": "Sorted/Docs/a.txt"}),
        )
        .unwrap();
        assert!(r.join("Sorted/Docs/a.txt").exists());
        assert!(file_op(
            &r,
            "move",
            &json!({"from": "Sorted/Docs/a.txt", "to": "../outside/a.txt"})
        )
        .is_err());
        assert!(file_op(&r, "mkdir", &json!({"path": "../evil"})).is_err());
        assert!(!d.join("evil").exists());
        let list = file_op(&r, "list", &json!({"path": ""})).unwrap();
        assert!(list
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "Sorted" && e["dir"] == true));
        assert!(file_op(&r, "trash", &json!({"path": ""})).is_err());
        assert_eq!(b64(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn runs_a_program_with_a_time_limit() {
        let tmp = std::env::temp_dir();
        let r = run_program("echo", &["hello".into()], &tmp, None, 5000).unwrap();
        assert_eq!(r["code"], 0);
        assert_eq!(r["stdout"], "hello\n");
        let r = run_program("cat", &[], &tmp, Some("piped".into()), 5000).unwrap();
        assert_eq!(r["stdout"], "piped");
        let r = run_program("sleep", &["5".into()], &tmp, None, 200).unwrap();
        assert_eq!(r["timedOut"], true);
        // Through a shell: its children stop too, promptly.
        let t = std::time::Instant::now();
        let r = run_program(
            "/bin/zsh",
            &["-c".into(), "sleep 5; echo x".into()],
            &tmp,
            None,
            200,
        )
        .unwrap();
        assert_eq!(r["timedOut"], true);
        assert!(t.elapsed().as_millis() < 1500, "took {:?}", t.elapsed());
        // More than the cap: finishes, capped, without waiting for the limit.
        let t = std::time::Instant::now();
        let r = run_program(
            "/bin/zsh",
            &[
                "-c".into(),
                "head -c 20000000 /dev/zero | tr '\\0' a".into(),
            ],
            &tmp,
            None,
            20000,
        )
        .unwrap();
        assert_eq!(r["timedOut"], false);
        assert_eq!(r["stdout"].as_str().unwrap().len(), 8 << 20);
        assert!(t.elapsed().as_secs() < 10);
        assert!(run_program("no-such-program-xyz", &[], &tmp, None, 1000).is_err());
    }

    #[test]
    fn shortcuts_need_a_strong_modifier() {
        assert!(shortcut_ok("CommandOrControl+Shift+R"));
        assert!(!shortcut_ok("Alt+Space"));
        assert!(!shortcut_ok("Command+C"));
        assert!(!shortcut_ok("CommandOrControl+Q"));
        assert!(shortcut_ok("Alt+Shift+Space"));
        assert!(!shortcut_ok("Shift+R"));
        assert!(!shortcut_ok("R"));
        assert!(!shortcut_ok("Space"));
    }

    #[test]
    fn wide_folders_are_refused() {
        let home = dirs::home_dir().unwrap();
        let app = home.join("Library/Application Support/OpenAs/apps/x");
        assert!(too_wide(Path::new("/"), &app));
        assert!(too_wide(Path::new("/Users"), &app));
        assert!(too_wide(&home, &app));
        assert!(too_wide(&home.join("Library"), &app));
        assert!(too_wide(
            &home.join("Library/Application Support/OpenAs"),
            &app
        ));
        assert!(too_wide(Path::new("/Applications"), &app));
        assert!(too_wide(&home.join("Applications"), &app));
        assert!(too_wide(&home.join("Applications/Tea Timer.app"), &app));
        assert!(!too_wide(&home.join("Downloads"), &app));
        assert!(!too_wide(&home.join("Documents/Invoices"), &app));
        let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
        assert!(too_wide(&icloud, &app));
        assert!(!too_wide(&icloud.join("Receipts"), &app));
        assert!(too_wide(&home.join("Library/CloudStorage/Dropbox"), &app));
        assert!(!too_wide(
            &home.join("Library/CloudStorage/Dropbox/Photos"),
            &app
        ));
        assert!(too_wide(&home.join("Library/Mobile Documents"), &app));
    }

    #[test]
    fn grants_belong_to_a_version() {
        let d = tmp("grants");
        let f = d.join("app");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("version.json"), r#"{"t":1}"#).unwrap();
        assert_eq!(state(&f, "menu-bar"), Grant::No);
        grant(
            &f,
            &[("menu-bar", json!(true)), ("folder", json!("/tmp/x"))],
        )
        .unwrap();
        assert_eq!(state(&f, "menu-bar"), Grant::Yes);
        assert_eq!(state(&f, "folder"), Grant::Yes);
        std::fs::write(f.join("version.json"), r#"{"t":2}"#).unwrap();
        assert_eq!(state(&f, "menu-bar"), Grant::Stale);
        // A new grant for the new version drops the old ones.
        grant(&f, &[("notifications", json!(true))]).unwrap();
        assert_eq!(state(&f, "notifications"), Grant::Yes);
        assert_eq!(state(&f, "menu-bar"), Grant::No);
    }
}
