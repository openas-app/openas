use super::*;

#[tauri::command]
pub fn menubar_set(
    spec: Value,
    webview: tauri::Webview,
    app: AppHandle,
    me: State<App>,
    tray: State<Tray>,
) -> Result<bool, String> {
    only_app(&webview, &me)?;
    if me.check.is_some() || me.making {
        return Ok(true);
    }
    need(&me, "menu-bar")?;
    let title: String = spec
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(40)
        .collect();
    let tip = spec
        .get("tooltip")
        .and_then(Value::as_str)
        .unwrap_or(&me.name)
        .to_string();
    let menu = Menu::new(&app).map_err(|e| e.to_string())?;
    for it in spec
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .take(40)
    {
        if it.as_str() == Some("-") || it.get("separator").and_then(Value::as_bool) == Some(true) {
            menu.append(&PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            continue;
        }
        let id = it.get("id").and_then(Value::as_str).unwrap_or("");
        let label: String = it
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .chars()
            .take(80)
            .collect();
        let enabled = it.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let label = if it.get("checked").and_then(Value::as_bool) == Some(true) {
            format!("✓ {label}")
        } else {
            label
        };
        menu.append(
            &MenuItem::with_id(&app, format!("mb:{id}"), label, enabled, None::<&str>)
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    menu.append(&PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    menu.append(
        &MenuItem::with_id(
            &app,
            "mb-open",
            format!("Open {}", me.name),
            true,
            None::<&str>,
        )
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    menu.append(
        &MenuItem::with_id(
            &app,
            "mb-quit",
            format!("Quit {}", me.name),
            true,
            None::<&str>,
        )
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut slot = tray
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(t) = slot.as_ref() {
        t.set_title(Some(&title)).map_err(|e| e.to_string())?;
        t.set_tooltip(Some(&tip)).map_err(|e| e.to_string())?;
        t.set_menu(Some(menu)).map_err(|e| e.to_string())?;
    } else {
        // Text only, like other menu bar items (the app's colour icon would
        // look out of place up there; a symbol can go in the title).
        let t = TrayIconBuilder::with_id("app")
            .title(&title)
            .tooltip(&tip)
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_tray_icon_event(|t, e| {
                if let TrayIconEvent::Click { .. } = e {
                    emit(t.app_handle(), "menu-bar-click", json!(null));
                }
            })
            .build(&app)
            .map_err(|e| e.to_string())?;
        *slot = Some(t);
    }
    Ok(true)
}

#[tauri::command]
pub fn menubar_clear(
    webview: tauri::Webview,
    app: AppHandle,
    me: State<App>,
    tray: State<Tray>,
) -> Result<bool, String> {
    only_app(&webview, &me)?;
    if let Some(t) = tray
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        let _ = app.remove_tray_by_id(t.id());
    }
    Ok(true)
}

#[tauri::command]
pub fn window_op(
    op: String,
    on: Option<bool>,
    webview: tauri::Webview,
    app: AppHandle,
    me: State<App>,
) -> Result<bool, String> {
    only_app(&webview, &me)?;
    if me.check.is_some() || me.making {
        return Ok(true);
    }
    let w = app.get_window("main").ok_or("no window")?;
    match op.as_str() {
        "show" => {
            w.show().map_err(|e| e.to_string())?;
            w.set_focus().map_err(|e| e.to_string())?;
        }
        "hide" => w.hide().map_err(|e| e.to_string())?,
        "alwaysOnTop" => {
            need(&me, "overlay")?;
            w.set_always_on_top(on.unwrap_or(true))
                .map_err(|e| e.to_string())?;
        }
        _ => return Err(format!("unknown window call “{op}”")),
    }
    Ok(true)
}

/// Runs a program (never through a shell unless the app asks for one) and
/// returns its exit code and output. Needs full access.
#[tauri::command]
pub async fn system_run(
    program: String,
    args: Option<Vec<String>>,
    opts: Option<Value>,
    webview: tauri::Webview,
    app: AppHandle,
) -> Result<Value, String> {
    let me = app.state::<App>();
    only_app(&webview, &me)?;
    if me.check.is_some() || me.making {
        return Err("not-allowed: programs don't run while the app is being made".into());
    }
    need(&me, "full")?;
    let o = opts.unwrap_or(Value::Null);
    let timeout = o
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(30_000)
        .clamp(100, 600_000);
    let home = dirs::home_dir().unwrap_or_default();
    let cwd = match o.get("cwd").and_then(Value::as_str) {
        Some("~") | None => home.clone(),
        Some(p) if p.starts_with("~/") => home.join(&p[2..]),
        Some(p) => PathBuf::from(p),
    };
    let stdin = o.get("stdin").and_then(Value::as_str).map(str::to_string);
    let args = args.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || run_program(&program, &args, &cwd, stdin, timeout))
        .await
        .map_err(|e| e.to_string())?
}

pub fn run_program(
    program: &str,
    args: &[String],
    cwd: &Path,
    stdin: Option<String>,
    timeout_ms: u64,
) -> Result<Value, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    const CAP: usize = 8 << 20;
    // Apps opened from the Finder get a bare PATH; give them the usual one.
    let path = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(program)
        .args(args)
        // Its own process group, so a time limit stops everything it started.
        .process_group(0)
        .current_dir(cwd)
        .env("PATH", path)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run “{program}”: {e}"))?;
    if let (Some(text), Some(mut w)) = (stdin, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = w.write_all(text.as_bytes());
        });
    }
    let grab = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            // Keep reading past the cap (and drop it), so a chatty program
            // never blocks on a full pipe.
            let (mut buf, mut chunk) = (vec![], [0u8; 65536]);
            if let Some(mut r) = r {
                while let Ok(n) = r.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if buf.len() < CAP {
                        buf.extend_from_slice(&chunk[..n.min(CAP - buf.len())]);
                    }
                }
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = grab(
        child
            .stdout
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    );
    let err = grab(
        child
            .stderr
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    );
    let start = std::time::Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(st);
        }
        if start.elapsed().as_millis() as u64 > timeout_ms {
            // SAFETY: kill(2) takes no pointers. The child was started as
            // the leader of its own group (pgid = pid), so the negative pid
            // reaches it and everything it started, and nothing else.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.kill();
            timed_out = true;
            break child.wait().ok();
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    };
    Ok(json!({
        "code": status.and_then(|s| s.code()),
        "stdout": out.join().unwrap_or_default(),
        "stderr": err.join().unwrap_or_default(),
        "timedOut": timed_out,
    }))
}

/// A shortcut needs two modifiers, one of them ⌘, ⌃ or ⌥, so an app can never
/// listen to plain typing or take everyday keys like ⌘C or ⌘Q from other apps.
pub(crate) fn shortcut_ok(acc: &str) -> bool {
    let parts: Vec<String> = acc.split('+').map(|p| p.trim().to_lowercase()).collect();
    let strong = [
        "cmd",
        "command",
        "super",
        "commandorcontrol",
        "cmdorctrl",
        "ctrl",
        "control",
        "alt",
        "option",
    ];
    let mods = &parts[..parts.len().saturating_sub(1)];
    mods.len() >= 2
        && mods.iter().any(|p| strong.contains(&p.as_str()))
        && !parts.last().is_some_and(|k| k.is_empty())
}

/// Global shortcuts: heard while the person is in other apps. At most 8,
/// each with ⌘, ⌃ or ⌥. Pressing one sends "shortcut" with its text.
/// Returns the ones that are now active; one another app already owns fails.
#[tauri::command]
pub fn shortcuts_set(
    list: Vec<String>,
    webview: tauri::Webview,
    app: AppHandle,
    me: State<App>,
) -> Result<Value, String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    only_app(&webview, &me)?;
    if me.check.is_some() || me.making {
        return Ok(json!({ "active": list, "failed": [] }));
    }
    need(&me, "shortcuts")?;
    if list.len() > 8 {
        return Err("too-many: an app can have at most 8 shortcuts".into());
    }
    if let Some(bad) = list.iter().find(|a| !shortcut_ok(a)) {
        return Err(format!("bad-shortcut: “{bad}” needs two modifiers, one of them Command, Control or Alt (like \"CommandOrControl+Shift+R\")"));
    }
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let (mut active, mut failed) = (vec![], vec![]);
    for acc in list {
        let text = acc.clone();
        match gs.on_shortcut(acc.as_str(), move |app, _s, ev| {
            if ev.state == ShortcutState::Pressed {
                emit(app, "shortcut", json!(text));
            }
        }) {
            Ok(()) => active.push(acc),
            Err(e) => failed.push(json!({ "shortcut": acc, "error": e.to_string() })),
        }
    }
    Ok(json!({ "active": active, "failed": failed }))
}

#[tauri::command]
pub fn shortcuts_clear(
    webview: tauri::Webview,
    app: AppHandle,
    me: State<App>,
) -> Result<bool, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    only_app(&webview, &me)?;
    let _ = app.global_shortcut().unregister_all();
    Ok(true)
}

/// The JS side, added to the page's `window.app`.
pub const BRIDGE_JS: &str = r#"
  const handlers = {};
  Object.defineProperty(window, "__oaEmit", { value: (ev, data) => (handlers[ev] || []).forEach(f => { try { f(data); } catch (e) { console.error(e); } }), writable: false, configurable: false });
  const fop = (op, args) => call("files_op", { op, args: args || {} });
  const powers = {
    permissions: Object.freeze({ has: name => call("perm_has", { name }), request: (names, why) => call("perm_request", { names: [].concat(names), why: why || null }) }),
    files: Object.freeze({
      folder: () => fop("folder"), list: (path = "") => fop("list", { path }), stat: path => fop("stat", { path }),
      readText: path => fop("readText", { path }), readDataURL: path => fop("readDataURL", { path }),
      writeText: (path, text) => fop("writeText", { path, text: String(text) }), mkdir: path => fop("mkdir", { path }),
      move: (from, to) => fop("move", { from, to }), copy: (from, to) => fop("copy", { from, to }),
      trash: path => fop("trash", { path }), reveal: path => fop("reveal", { path }) }),
    menuBar: Object.freeze({ set: spec => call("menubar_set", { spec }), clear: () => call("menubar_clear") }),
    system: Object.freeze({ run: (program, args, opts) => call("system_run", { program: String(program), args: [].concat(args || []).map(String), opts: opts || null }) }),
    shortcuts: Object.freeze({ set: list => call("shortcuts_set", { list: [].concat(list || []) }), clear: () => call("shortcuts_clear") }),
    window: Object.freeze({ show: () => call("window_op", { op: "show" }), hide: () => call("window_op", { op: "hide" }), setAlwaysOnTop: on => call("window_op", { op: "alwaysOnTop", on: !!on }) }),
    on: (ev, f) => { (handlers[ev] = handlers[ev] || []).push(f); },
  };
"#;
