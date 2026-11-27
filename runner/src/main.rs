//! OpenAs runner: the one window type. It opens an app folder
//! (`<folder>/app/index.html`), serves it from its own `app://` origin, and
//! gives the page `window.app`: `store` (JSON in `<folder>/data/`, which
//! changes never touch) and `notify`. When the app's file changes on disk (a
//! change or an undo), the window reloads.
//!
//! Its menu has "Change This App…" (a small sheet; the Helper does the work)
//! and "Undo Last Change", plus the standard Edit menu (copy and paste).
//!
//! Which app: an installed copy reads its own Info.plist (`OAAppFolder`,
//! `CFBundleName`, `CFBundleIdentifier`, `OAHelper`, `OAManager`) and ignores
//! every flag, so nothing can launch it on another folder under its name. Only
//! the bare Runner takes flags: `--app <folder> [--id <id>]`, and
//! `--check <folder> --report <file>` for the load check (no window shown,
//! nothing saved, exits when done).
//!
//! The manager ("OpenAs" itself) is a copy with `OAManager`: its window
//! shows the built-in list and New App page and may make apps. An ordinary
//! app's page can't reach the Helper at all; only its change sheet can, and
//! only for this app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{
    http, AppHandle, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl,
    WebviewWindowBuilder, Window, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;

mod powers;

pub(crate) const RUNNER_ID: &str = "app.openas.runner";
pub(crate) const CHANGE_PAGE: &str = include_str!("../pages/change.html");
pub(crate) const MANAGER_PAGE: &str = include_str!("../pages/manager.html");
pub(crate) const VERSIONS_PAGE: &str = include_str!("../pages/versions.html");
pub(crate) const TALK_PAGE: &str = include_str!("../pages/talk.html");
/// Shown where the app will be, until its first file exists.
pub(crate) const WAITING_PAGE: &str = r#"<!doctype html><meta charset="utf-8"><style>
:root{color-scheme:light dark;--bg:#F4F4F5;--fg:#52525B;--line:#D4D4D8}@media (prefers-color-scheme:dark){:root{--bg:#0B0B0C;--fg:#A1A1AA;--line:#2E2E33}}
html,body{margin:0;height:100%;background:var(--bg);color:var(--fg);font:13px/18px "Inter",-apple-system,system-ui,sans-serif;display:grid;place-items:center}
.g{width:220px;height:150px;border-radius:20px;border:1.5px dashed var(--line);display:grid;place-items:center;margin:0 auto 12px}
.g i{width:40px;height:40px;border-radius:11px;background:var(--line);opacity:.8;animation:p 2.4s ease-in-out infinite}@keyframes p{50%{opacity:.25}}
</style><div><div class="g"><i></i></div>Your app opens here as soon as it runs.</div>"#;
/// The conversation column's width (logical points).
pub(crate) const TALK_W: f64 = 380.0;

mod api;
mod app;
mod helper;
mod serve;
mod sheet;
mod talk;
mod who;

use api::*;
use app::*;
use helper::*;
use powers::{online, set_net};
use serve::*;
use talk::*;
pub(crate) use who::{Draft, Who};

fn main() {
    let me = which_app();
    let checking = me.check.clone();
    let manager = me.manager;
    let store = load_store(&me.folder, checking.is_none());
    let root_dir = me.folder.join("app");
    let folder = me.folder.clone();
    let index = root_dir.join("index.html");
    // Edits not yet saved: the app shows them until Save as App or Discard.
    let pending_root = me.folder.join(".pending/app");
    let pending_index = pending_root.join("index.html");
    let title = me.name.clone();
    let mut context = tauri::generate_context!();
    if let Some(id) = &me.id {
        context.config_mut().identifier = id.clone();
    }
    let downloads = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
    // Asked per file: the code that runs (saved, or a change being tried)
    // decides, and a change of answer reloads the page.
    let me_proto = me.clone();
    let me_web = me.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .register_uri_scheme_protocol("app", move |ctx, req| {
            let rel = req.uri().path().trim_start_matches('/');
            // OpenAs's window: the draft's page gets the draft's files and
            // nothing else; with no draft shown, nothing at all.
            if ctx.webview_label() == "app" {
                if let Some(who) = ctx.app_handle().try_state::<Who>().filter(|w| w.home.manager) {
                    return match who.shown() {
                        Some(Draft { app: d, .. }) => serve_dir(&d.folder.join("app"), rel, &policy(online(&d))),
                        None => not_found(),
                    };
                }
            }
            let csp = policy(online(&me_proto));
            let ok = respond;
            // OpenAs' own pages: never any internet (the build preview
            // inside the manager inherits this).
            match rel {
                "__oa/change.html" => return ok(CHANGE_PAGE.as_bytes().to_vec(), "text/html", &policy(false)),
                "__oa/manager.html" => return ok(MANAGER_PAGE.as_bytes().to_vec(), "text/html", &policy(false)),
                "__oa/versions.html" => return ok(VERSIONS_PAGE.as_bytes().to_vec(), "text/html", &policy(false)),
                "__oa/talk.html" => return ok(TALK_PAGE.as_bytes().to_vec(), "text/html", &policy(false)),
                _ => {}
            }
            // The app's files (or its edits waiting to be saved).
            let root_dir = if pending_root.join("index.html").exists() { &pending_root } else { &root_dir };
            serve_dir(root_dir, rel, &csp)
        })
        .manage(me.clone())
        .manage(Who::new(me, store))
        .manage(Jobs::default())
        .manage(Inbox::default())
        .manage(powers::Tray(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![store_get, store_set, notify, check_report, helper_start, helper_poll, helper_stop, close_sheet, who::drafts_list, who::draft_show, who::draft_home, who::draft_code_view, who::draft_full, talk_state, talk_note, talk_stop, talk_continue, talk_ask, talk_choose, shared_pending, talk_code, talk_close, powers::ask::perm_has, powers::perm_request, powers::files_op, powers::extras::menubar_set, powers::extras::menubar_clear, powers::extras::shortcuts_set, powers::extras::shortcuts_clear, powers::system_run, powers::extras::window_op])
        .on_menu_event(|app, event| match event.id().as_ref() {
            "change" | "versions" => show_talk(app, true),
            "share" => {
                // One file with the app's code and icon (never its data), shown in Finder.
                let me = app.state::<App>();
                let (Some(helper), id, env) = (me.helper.clone(), me.app_id(), me.env.clone()) else { return };
                std::thread::spawn(move || {
                    let out = std::process::Command::new(&helper).arg("share").arg(&id).envs(env).output();
                    match out {
                        Ok(o) if o.status.success() => {
                            let file = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(file).status();
                        }
                        other => eprintln!("share failed: {other:?}"),
                    }
                });
            }
            "internet" => {
                // Only the person, from the app's own menu, changes this
                // (for the code that runs now, like an answer to the ask).
                let me = app.state::<App>();
                let on = !online(&me);
                let _ = set_net(&me.folder, on);
                powers::apply(app, on);
            }
            "mb-open" => {
                if let Some(w) = app.get_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "mb-quit" => app.exit(0),
            id if id.starts_with("mb:") => powers::emit(app, "menu-bar", json!(&id[3..])),
            // A finished app's own edits (OpenAs's window only shows drafts).
            "save-app" | "discard-app" if !app.state::<App>().making && !app.state::<App>().manager => {
                let me = app.state::<App>();
                let op = if event.id().as_ref() == "save-app" { "save_change" } else { "discard_change" };
                let req = json!({ "op": op, "app": me.app_id() });
                start_job(app, req);
            }
            // A draft: the menu opens a sheet on the window that asks first.
            "save-app" | "discard-app" => {
                show_talk(app, true);
                sheet::ask(app, event.id().as_ref() == "save-app");
            }
            "undo-change" => {
                let me = app.state::<App>();
                let req = json!({ "op": "undo", "app": me.app_id() });
                start_job(app, req);
            }
            _ => {}
        })
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            if checking.is_some() || std::env::var("OPENAS_QUIET_WINDOW").is_ok() {
                // The load check never shows in the Dock; nor do test windows,
                // which also never take focus from the person at the Mac.
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            let page = if manager { "/__oa/manager.html" } else { "/index.html" };
            // The size the app was made for (window.json, written by the builder).
            let made: Value = std::fs::read_to_string(app.state::<App>().folder.join("window.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
            let (start_w, start_h) = match (made.get("width").and_then(Value::as_f64), made.get("height").and_then(Value::as_f64)) {
                (Some(w), Some(h)) if !manager => (w, h),
                _ => if manager { (900.0, 720.0) } else { (900.0, 640.0) },
            };
            let dl = downloads.clone();
            // The load check never sends anything to the browser: its page is
            // unchecked, and nobody is there to have clicked.
            // No browser tabs opened from a check or from the making window.
            let quiet = checking.is_some() || app.state::<App>().making;
            let (net_nav, net_win) = (me_web.clone(), me_web.clone());
            // OpenAs' own window may open its few help links (a new window,
            // like the token page) in the browser; it never navigates away.
            let own_links = manager;
            let making = app.state::<App>().making;
            let split = !manager && checking.is_none();
            macro_rules! wire {
                ($b:expr) => {{
                    let (net_nav, net_win, net_load, dl) = (net_nav.clone(), net_win.clone(), net_nav.clone(), dl.clone());
                    // The first page, too, starts with no peer connections.
                    #[cfg(target_os = "macos")]
                    let b = match powers::no_peers_config(false, online(&net_load)) {
                        Some(c) => $b.with_webview_configuration(c),
                        None => $b,
                    };
                    #[cfg(not(target_os = "macos"))]
                    let b = $b;
                    b
                // Links to the web open in the user's browser, never in the app window.
                .on_navigation(move |u| {
                    let web = matches!(u.scheme(), "http" | "https") && u.host_str() != Some("app.localhost");
                    // A web link opens in the browser only if this app may use
                    // the internet: otherwise a page could send data out in a URL.
                    if web && !quiet && online(&net_nav) {
                        let _ = std::process::Command::new(if cfg!(windows) { "explorer" } else { "open" }).arg(u.as_str()).spawn();
                    }
                    // The window stays on the app: web pages open in the browser,
                    // and other schemes (file:, ssh:, …) go nowhere.
                    let own = u.scheme() == "app" || u.host_str() == Some("app.localhost");
                    own && !web || u.scheme() == "about" || u.scheme() == "data" || u.scheme() == "blob"
                })
                .on_new_window(move |u, _| {
                    // Only web pages go to the browser; never file:, ssh:, other apps' schemes.
                    if matches!(u.scheme(), "http" | "https") && !quiet && (own_links || online(&net_win)) {
                        let _ = std::process::Command::new(if cfg!(windows) { "explorer" } else { "open" }).arg(u.as_str()).spawn();
                    }
                    NewWindowResponse::Deny
                })
                // Each page starts under the answer for the code it runs: no
                // peer connections offline, and an app that changed since
                // the person allowed the internet asks again.
                .on_page_load(move |w, p| {
                    if matches!(p.event(), tauri::webview::PageLoadEvent::Started) {
                        powers::peer_connections(&w, online(&net_load));
                    } else {
                        powers::sync_tick(w.app_handle(), online(&net_load));
                        powers::ask_if_changed(w.app_handle());
                    }
                })
                // Downloads (an app's Export button) land in Downloads.
                .on_download(move |_w, event| {
                    if let DownloadEvent::Requested { url, destination } = event {
                        let name = destination
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .filter(|n| !n.is_empty())
                            .or_else(|| url.path_segments().and_then(|mut s| s.next_back().map(str::to_string)))
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| "download".into());
                        let mut path = dl.join(&name);
                        let mut n = 2;
                        while path.exists() {
                            let p = Path::new(&name);
                            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                            path = dl.join(format!("{stem} {n}{ext}"));
                            n += 1;
                        }
                        *destination = path;
                    }
                    true
                })
                }};
            }
            let mut b = WebviewWindowBuilder::new(app, "main", url(page)).title(&title).inner_size(start_w, start_h);
            if !split {
                b = wire!(b);
            }
            b = if manager { b.initialization_script(oa_bridge("OpenAs")) } else { b.initialization_script(bridge()) };
            if let Some(report) = checking.clone() {
                b = b.initialization_script(CHECK).visible(false).focused(false).skip_taskbar(true);
                // A page that never finishes loading still gets a report.
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(20));
                    let _ = std::fs::write(&report, json!({ "timeout": true }).to_string());
                    handle.exit(0);
                });
            }
            // An app's window holds two webviews: the app, and the conversation
            // column beside it (hidden until asked for, open while it's made).
            let open_talk = making;
            let main: tauri::Window = if split {
                let quiet_win = std::env::var("OPENAS_QUIET_WINDOW").is_ok();
                let win = tauri::window::WindowBuilder::new(app, "main").title(&title).inner_size(start_w + if open_talk { TALK_W } else { 0.0 }, start_h).focused(!quiet_win).build()?;
                let ab = wire!(tauri::webview::WebviewBuilder::new("app", url("/index.html"))).initialization_script(bridge());
                win.add_child(ab, tauri::LogicalPosition::new(0.0, 0.0), tauri::LogicalSize::new(start_w, start_h))?;
                let tb = tauri::webview::WebviewBuilder::new("talk", url("/__oa/talk.html")).initialization_script(oa_bridge(&title));
                win.add_child(tb, tauri::LogicalPosition::new(start_w, 0.0), tauri::LogicalSize::new(TALK_W, start_h))?;
                app.manage(Talk { open: std::sync::atomic::AtomicBool::new(open_talk) });
                win
            } else if manager && checking.is_none() {
                // OpenAs's window: Home, and in its place an app being made
                // (its page and its conversation), added by `who::show`.
                // One bar across the whole window, drawn by the pages, with the
                // traffic lights inside it (as Claude's own window does).
                let cfg = tauri::utils::config::WindowConfig {
                    label: "main".into(),
                    title: title.clone(),
                    width: start_w,
                    height: start_h,
                    #[cfg(target_os = "macos")]
                    title_bar_style: tauri::TitleBarStyle::Overlay,
                    #[cfg(target_os = "macos")]
                    hidden_title: true,
                    #[cfg(target_os = "macos")]
                    traffic_light_position: Some(tauri::utils::config::LogicalPosition { x: 18.0, y: who::BAR_H / 2.0 + 1.0 }),
                    create: false,
                    ..Default::default()
                };
                let win = tauri::window::WindowBuilder::from_config(app, &cfg)?.build()?;
                let hb = wire!(tauri::webview::WebviewBuilder::new("main", url(page))).initialization_script(oa_bridge("OpenAs"));
                win.add_child(hb, tauri::LogicalPosition::new(0.0, 0.0), tauri::LogicalSize::new(start_w, start_h))?;
                win
            } else {
                let w = b.build()?;
                let win = w.as_ref().window();
                if checking.is_some() {
                    return Ok(());
                }
                win
            };
            restore_frame(&main, &folder);
            layout(app.handle());
            let frame_file = folder.join(".window.json");
            let w2 = main.clone();
            let lh = app.handle().clone();
            let ch = app.handle().clone();
            main.on_window_event(move |e| {
                // A menu bar app keeps running when its window closes.
                if let WindowEvent::CloseRequested { api, .. } = e {
                    if ch.state::<powers::Tray>().0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some() {
                        api.prevent_close();
                        if let Some(w) = ch.get_window("main") {
                            let _ = w.hide();
                        }
                    }
                }
                if matches!(e, WindowEvent::Resized(_)) {
                    layout(&lh);
                }
                if matches!(e, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
                    if let (Ok(p), Ok(s)) = (w2.outer_position(), w2.inner_size()) {
                        let _ = std::fs::write(&frame_file, json!({ "x": p.x, "y": p.y, "w": s.width, "h": s.height }).to_string());
                    }
                }
            });

            // The menu: the app's own items, then the standard Edit and Window menus.
            let h = app.handle();
            let change = MenuItem::with_id(h, "change", "Change This App…", !manager, None::<&str>)?;
            let undo = MenuItem::with_id(h, "undo-change", "Undo Last Change", !manager && has_history(&folder), None::<&str>)?;
            let versions = MenuItem::with_id(h, "versions", "Versions…", !manager, None::<&str>)?;
            let share = MenuItem::with_id(h, "share", "Share…", !manager, None::<&str>)?;
            let internet = tauri::menu::CheckMenuItem::with_id(h, "internet", "Allow Internet Access", !manager, online(&app.state::<App>()), None::<&str>)?;
            // While an app is being made, its menu is about the draft: save it
            // as an app once it's done, or discard it.
            let save_app = MenuItem::with_id(h, "save-app", "Save as App", false, None::<&str>)?;
            let discard_app = MenuItem::with_id(h, "discard-app", if making || manager { "Discard" } else { "Discard Changes" }, false, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(h)?;
            let (hide, hide_others, show_all, quit) = (
                PredefinedMenuItem::hide(h, None)?,
                PredefinedMenuItem::hide_others(h, None)?,
                PredefinedMenuItem::show_all(h, None)?,
                PredefinedMenuItem::quit(h, None)?,
            );
            let (sep2, sep3, sep4, sep5) = (PredefinedMenuItem::separator(h)?, PredefinedMenuItem::separator(h)?, PredefinedMenuItem::separator(h)?, PredefinedMenuItem::separator(h)?);
            let items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = if making || manager {
                vec![&save_app, &discard_app, &sep, &hide, &hide_others, &show_all, &sep2, &quit]
            } else {
                // Save as App is there in a finished app too, greyed out until
                // there are edits to keep.
                vec![&save_app, &discard_app, &sep5, &change, &undo, &versions, &sep, &share, &sep2, &internet, &sep3, &hide, &hide_others, &show_all, &sep4, &quit]
            };
            let app_menu = Submenu::with_items(h, &title, true, &items)?;
            let edit = Submenu::with_items(
                h,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(h, None)?,
                    &PredefinedMenuItem::redo(h, None)?,
                    &PredefinedMenuItem::separator(h)?,
                    &PredefinedMenuItem::cut(h, None)?,
                    &PredefinedMenuItem::copy(h, None)?,
                    &PredefinedMenuItem::paste(h, None)?,
                    &PredefinedMenuItem::select_all(h, None)?,
                ],
            )?;
            let window = Submenu::with_items(
                h,
                "Window",
                true,
                &[&PredefinedMenuItem::minimize(h, None)?, &PredefinedMenuItem::maximize(h, None)?, &PredefinedMenuItem::close_window(h, None)?],
            )?;
            app.set_menu(Menu::with_items(h, &[&app_menu, &edit, &window])?)?;
            app.manage(NetToggle { item: internet.clone() });

            if manager {
                who::watch(app.handle().clone(), save_app, discard_app);
                return Ok(());
            }
            // Reload when a change or an undo rewrites the app's file, and keep
            // Undo enabled only when there is something to undo.
            let handle = app.handle().clone();
            let watch = folder.clone();
            std::thread::spawn(move || {
                let version = watch.join("version.json");
                let mut seen = (modified(&index), modified(&version), modified(&pending_index));
                let (mut files_seq, mut ghost_seq, mut ghost_cleared) = (0u64, 0u64, false);
                let mut reload_at: Option<std::time::Instant> = None;
                loop {
                    std::thread::sleep(Duration::from_millis(250));
                    let app_view = handle.get_webview("app");
                    let now = (modified(&index), modified(&version), modified(&pending_index));
                    if !making {
                        let edited = pending_index.exists();
                        let _ = save_app.set_enabled(edited);
                        let _ = discard_app.set_enabled(edited);
                    }
                    if now.0.is_some() && now != seen {
                        seen = now;
                        reload_at = Some(std::time::Instant::now());
                    }
                    if making {
                        let m = read_json(&watch.join("making.json"));
                        let ready = m["phase"] == "done" && !watch.with_extension("keep").exists() && !watch.with_extension("discard").exists();
                        let _ = save_app.set_enabled(ready);
                        let _ = discard_app.set_enabled(ready);
                        let fs = m["files_seq"].as_u64().unwrap_or(0);
                        if fs != files_seq {
                            files_seq = fs;
                            // Reload once the writing pauses, not on every edit.
                            reload_at = Some(std::time::Instant::now());
                        }
                        // Done: the last check's pointer and caption leave the app.
                        if m["phase"] == "done" && !ghost_cleared {
                            ghost_cleared = true;
                            if let Some(a) = &app_view {
                                let _ = a.eval("document.getElementById('__oa_ghost')?.remove()");
                            }
                        }
                        let gs = m["ghost"]["seq"].as_u64().unwrap_or(0);
                        if gs != ghost_seq && m["phase"] != "done" {
                            ghost_seq = gs;
                            if let Some(a) = &app_view {
                                let _ = a.eval(ghost_js(&m["ghost"]));
                            }
                        }
                        let moved = watch.with_extension("moved");
                        // Installed: the finished app opens in this one's place.
                        // Discarded: long enough to read so, then gone.
                        if moved.exists() {
                            let saved = !std::fs::read_to_string(&moved).unwrap_or_default().contains("discarded");
                            std::thread::sleep(Duration::from_millis(if saved { 900 } else { 1800 }));
                            handle.exit(0);
                            return;
                        }
                    }
                    // Saved: the app view is on its way into the Dock; never reload it.
                    if making && !watch.exists() {
                        reload_at = None;
                    }
                    if reload_at.is_some_and(|t| t.elapsed() >= Duration::from_millis(if making { 900 } else { 300 })) {
                        reload_at = None;
                        if let Some(a) = &app_view {
                            let on = online(&a.app_handle().state::<App>());
                            powers::reload(a, on);
                        }
                    }
                    let _ = undo.set_enabled(!manager && !making && has_history(&watch));
                }
            });
            Ok(())
        })
        .build(context)
        .expect("runner failed to start")
        .run(|app, e| {
            // A click on the Dock icon brings back a window that was closed
            // while the app kept running in the menu bar.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { has_visible_windows: false, .. } = e {
                if let Some(w) = app.get_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            // A shared app file opened with OpenAs: the home window shows it.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &e {
                if app.state::<App>().manager {
                    let files: Vec<String> = urls.iter().filter_map(|u| u.to_file_path().ok()).filter(|p| p.extension().is_some_and(|x| x == "openas")).map(|p| p.to_string_lossy().into_owned()).collect();
                    if !files.is_empty() {
                        app.state::<Inbox>().0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend(files);
                        if let Some(w) = app.get_webview("main") {
                            let _ = w.eval("window.__shared && window.__shared()");
                        }
                        if let Some(w) = app.get_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                }
            }
            // The making window is gone: the build saves the app by itself.
            if let tauri::RunEvent::Exit = e {
                let me = app.state::<App>();
                if me.making {
                    let _ = std::fs::remove_file(me.folder.with_extension("open"));
                }
            }
            let _ = (app, e);
        });
}
