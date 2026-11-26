use super::*;

// ---- Commands --------------------------------------------------------------

pub struct Tray(pub Mutex<Option<TrayIcon>>);

/// Only an app's own page: the "app" webview, or the whole window when there's
/// no column. Never OpenAs' own window or the column.
pub(crate) fn only_app(webview: &tauri::Webview, me: &App) -> Result<(), String> {
    if !me.manager && (webview.label() == "app" || webview.label() == "main") {
        Ok(())
    } else {
        Err("not allowed".into())
    }
}

pub(crate) fn folder_name(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.into())
}

#[tauri::command]
pub fn perm_has(name: String, webview: tauri::Webview, me: State<App>) -> Result<Value, String> {
    only_app(&webview, &me)?;
    if me.check.is_some() || me.making {
        return Ok(json!(false));
    }
    // A grant for an earlier version counts as not granted; asking again
    // explains that the app changed.
    Ok(json!(state(&me.folder, &name) == Grant::Yes))
}

/// Asks the person, in native dialogs: one for the plain powers together, and
/// one for a folder (which they pick). Runs off the main thread (async), so
/// the dialogs can block.
#[tauri::command]
pub async fn perm_request(
    names: Vec<String>,
    why: Option<String>,
    webview: tauri::Webview,
    app: AppHandle,
) -> Result<Value, String> {
    let me = app.state::<App>();
    only_app(&webview, &me)?;
    let mut out = serde_json::Map::new();
    if me.check.is_some() || me.making {
        for n in &names {
            out.insert(n.clone(), json!(false));
        }
        return Ok(Value::Object(out));
    }
    let folder = me.folder.clone();
    let why = why
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect::<String>();
    let mut plain = vec![];
    for n in &names {
        match KNOWN.iter().find(|(k, _)| k == n) {
            None => {
                out.insert(n.clone(), json!(false));
            }
            Some(("folder", _)) | Some(("full", _)) => {}
            Some((k, _)) if state(&folder, k) == Grant::Yes => {
                out.insert(n.clone(), json!(true));
            }
            Some(_) => plain.push(n.clone()),
        }
    }
    // Full access is never asked together with anything else.
    if names.iter().any(|n| n == "full") {
        // Like Claude: only apps you made yourself can have full access. A
        // shared app (someone else's code) never gets it, and isn't asked.
        let shared = std::fs::read_to_string(folder.join("app.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["shared"].as_bool())
            .unwrap_or(false);
        let yes =
            !shared && (state(&folder, "full") == Grant::Yes || ask_full(&app, &me, &folder, &why));
        out.insert("full".into(), json!(yes));
    }
    if names.iter().any(|n| n == "folder") {
        let got = ask_folder(&app, &me, &folder, &why)?;
        out.insert("folder".into(), json!(got.is_some()));
        if let Some(name) = got {
            out.insert("folderName".into(), json!(name));
        }
    }
    let win = app.get_window("main");
    if !plain.is_empty() {
        let list: Vec<String> = plain
            .iter()
            .map(|n| format!("• {}", KNOWN.iter().find(|(k, _)| k == n).unwrap().1))
            .collect();
        let changed = plain.iter().any(|n| state(&folder, n) == Grant::Stale);
        let mut text = list.join("\n");
        if !why.is_empty() {
            text = format!("{text}\n\n“{why}”");
        }
        if changed {
            text = format!(
                "{text}\n\nYou allowed this before. The app has changed since, so it asks again."
            );
        }
        let mut d = app
            .dialog()
            .message(text)
            .title(format!("{} would like to:", me.name))
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Allow".into(),
                "Don't Allow".into(),
            ));
        if let Some(w) = &win {
            d = d.parent(w);
        }
        let yes = match d.blocking_show_with_result() {
            MessageDialogResult::Ok | MessageDialogResult::Yes => true,
            MessageDialogResult::Custom(s) => s == "Allow",
            _ => false,
        };
        let set: Vec<(&str, Value)> = plain.iter().map(|n| (n.as_str(), json!(yes))).collect();
        if yes {
            grant(&folder, &set)?;
        }
        for n in &plain {
            out.insert(n.clone(), json!(yes));
        }
    }
    Ok(Value::Object(out))
}

/// The strongest ask: its own dialog that says plainly what it means.
pub(crate) fn ask_full(app: &AppHandle, me: &App, folder: &Path, why: &str) -> bool {
    let changed = state(folder, "full") == Grant::Stale;
    let mut text = format!(
        "{} could run programs, and read, change or delete any of your files, without asking each time.",
        me.name
    );
    if !why.is_empty() {
        text = format!("{text}\n\n“{why}”");
    }
    text = format!("{text}\n\nOnly allow this if you trust what the app does. It asks again after every change to the app.");
    if changed {
        text = format!(
            "{text}\n\nYou allowed this before. The app has changed since, so it asks again."
        );
    }
    let mut d = app
        .dialog()
        .message(text)
        .title(format!("Give {} full access to your Mac?", me.name))
        .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
        // "Don't Allow" is the default (Return), so full access always takes a deliberate click.
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Don't Allow".into(),
            "Allow Full Access".into(),
        ));
    if let Some(w) = app.get_window("main") {
        d = d.parent(&w);
    }
    let r = d.blocking_show_with_result();
    eprintln!("full access dialog: {r:?}");
    let yes = matches!(r, MessageDialogResult::Custom(ref s) if s == "Allow Full Access");
    if yes {
        let _ = grant(folder, &[("full", json!(true))]);
    }
    yes
}

pub(crate) fn ask_folder(
    app: &AppHandle,
    me: &App,
    folder: &Path,
    why: &str,
) -> Result<Option<String>, String> {
    let g = load(folder);
    let win = app.get_window("main");
    let before = g.get("folder").and_then(Value::as_str).map(str::to_string);
    if let (Some(path), Grant::Yes) = (&before, state(folder, "folder")) {
        return Ok(Some(folder_name(path)));
    }
    if let (Some(path), Grant::Stale) = (&before, state(folder, "folder")) {
        // Changed since: offer the same folder again, or another.
        let mut d = app
            .dialog()
            .message(format!(
                "You let it use “{}” before. The app has changed since, so it asks again.",
                folder_name(path)
            ))
            .title(format!(
                "Let {} use “{}” again?",
                me.name,
                folder_name(path)
            ))
            .buttons(MessageDialogButtons::YesNoCancelCustom(
                "Allow".into(),
                "Choose Another…".into(),
                "Don't Allow".into(),
            ));
        if let Some(w) = &win {
            d = d.parent(w);
        }
        match d.blocking_show_with_result() {
            MessageDialogResult::Yes => {
                grant(folder, &[("folder", json!(path))])?;
                return Ok(Some(folder_name(path)));
            }
            MessageDialogResult::Custom(s) if s == "Allow" => {
                grant(folder, &[("folder", json!(path))])?;
                return Ok(Some(folder_name(path)));
            }
            MessageDialogResult::No => {}
            MessageDialogResult::Custom(s) if s.starts_with("Choose") => {}
            _ => return Ok(None),
        }
    } else {
        let mut text = "It can see, change and move to the Trash what's inside the folder you choose, and nothing else on your Mac.".to_string();
        if !why.is_empty() {
            text = format!("“{why}”\n\n{text}");
        }
        let mut d = app
            .dialog()
            .message(text)
            .title(format!("{} would like to use a folder", me.name))
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Choose Folder…".into(),
                "Not Now".into(),
            ));
        if let Some(w) = &win {
            d = d.parent(w);
        }
        match d.blocking_show_with_result() {
            MessageDialogResult::Ok | MessageDialogResult::Yes => {}
            MessageDialogResult::Custom(s) if s.starts_with("Choose") => {}
            _ => return Ok(None),
        }
    }
    // Starts in Downloads, the folder people most often mean (tests start elsewhere).
    let start = std::env::var("OPENAS_PICK_START")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join("Downloads"));
    let mut f = app
        .dialog()
        .file()
        .set_title(format!("Choose a folder for {}", me.name))
        .set_directory(start);
    if let Some(w) = &win {
        f = f.set_parent(w);
    }
    let Some(picked) = f.blocking_pick_folder() else {
        return Ok(None);
    };
    let p = picked.into_path().map_err(|e| e.to_string())?;
    let p = p.canonicalize().unwrap_or(p);
    if too_wide(&p, folder) {
        let _ = app
            .dialog()
            .message("That folder covers too much of your Mac. Choose a folder inside it.")
            .title("Choose a smaller folder")
            .blocking_show();
        return Ok(None);
    }
    grant(folder, &[("folder", json!(p.to_string_lossy()))])?;
    Ok(Some(folder_name(&p.to_string_lossy())))
}

/// A folder that would give more than one folder's worth of access: the disk,
/// your home folder or anything above it, the Library folders, or anything
/// holding OpenAs' own data or the installed apps (an app could rewrite
/// its own grants there).
pub fn too_wide(p: &Path, app_folder: &Path) -> bool {
    let canon = |q: &Path| q.canonicalize().unwrap_or_else(|_| q.to_path_buf());
    let home = canon(&dirs::home_dir().unwrap_or_default());
    let data_root = canon(
        app_folder
            .parent()
            .and_then(Path::parent)
            .unwrap_or(app_folder),
    );
    let exe = std::env::current_exe().ok().map(|e| canon(&e));
    let bundle = exe
        .as_deref()
        .and_then(|e| {
            e.ancestors()
                .find(|a| a.extension().is_some_and(|x| x == "app"))
        })
        .map(Path::to_path_buf);
    // iCloud Drive and cloud-storage folders live in Library: a folder inside
    // one of them is fine; the drive itself is too much.
    let clouds = [
        home.join("Library/Mobile Documents/com~apple~CloudDocs"),
        home.join("Library/CloudStorage"),
    ];
    let in_cloud = clouds.iter().any(|c| {
        p.strip_prefix(c).is_ok_and(|rest| {
            rest.components().count() >= if c.ends_with("CloudStorage") { 2 } else { 1 }
        })
    });
    home.starts_with(p)
        || (p.starts_with(home.join("Library")) && !in_cloud)
        || p.starts_with("/System")
        || p.starts_with("/Library")
        || p.starts_with("/private")
        || p.starts_with("/usr")
        || p.starts_with("/bin")
        || p.starts_with("/sbin")
        || p.starts_with("/etc")
        || data_root.starts_with(p)
        || p.starts_with(&data_root)
        || bundle
            .as_ref()
            .is_some_and(|b| b.starts_with(p) || p.starts_with(b))
}

/// Every file call. The folder must be granted for this version.
#[tauri::command]
pub async fn files_op(
    op: String,
    args: Value,
    webview: tauri::Webview,
    app: AppHandle,
) -> Result<Value, String> {
    let me = app.state::<App>();
    only_app(&webview, &me)?;
    if me.check.is_some() || me.making {
        return Err("no-folder: no folder has been chosen yet".into());
    }
    // With full access, a path may be absolute (/…) or start at home (~/…).
    let abs = |v: &Value| {
        v.as_str()
            .is_some_and(|p| p.starts_with('/') || p == "~" || p.starts_with("~/"))
    };
    if ["path", "from", "to"]
        .iter()
        .any(|k| args.get(k).is_some_and(abs))
    {
        need(&me, "full").map_err(|_| "not-allowed: full paths need full access; call app.permissions.request(['full']) from a click, or use a path inside the chosen folder".to_string())?;
        let home = dirs::home_dir().unwrap_or_default();
        let mut a = args.clone();
        for k in ["path", "from", "to"] {
            if let Some(p) = args.get(k).and_then(Value::as_str) {
                let full = if p == "~" {
                    home.clone()
                } else if let Some(r) = p.strip_prefix("~/") {
                    home.join(r)
                } else {
                    PathBuf::from(p)
                };
                a[k] = json!(full.to_string_lossy().trim_start_matches('/'));
            }
        }
        if op == "folder" {
            return Ok(json!({ "name": "/" }));
        }
        return file_op(Path::new("/"), &op, &a);
    }
    let root = match (state(&me.folder, "folder"), load(&me.folder).get("folder").and_then(Value::as_str)) {
        (Grant::Yes, Some(p)) => PathBuf::from(p),
        (Grant::Stale, _) => return Err("ask-again: the app changed since the folder was allowed; call app.permissions.request(['folder'])".into()),
        _ => return Err("no-folder: no folder has been chosen yet; call app.permissions.request(['folder']) from a click".into()),
    };
    if op == "folder" {
        return Ok(json!({ "name": folder_name(&root.to_string_lossy()) }));
    }
    file_op(&root, &op, &args)
}

pub(crate) fn need(me: &App, name: &str) -> Result<(), String> {
    match state(&me.folder, name) {
        Grant::Yes => Ok(()),
        Grant::Stale => Err(format!("ask-again: call app.permissions.request(['{name}']) again; the app changed since it was allowed")),
        Grant::No => Err(format!("not-allowed: call app.permissions.request(['{name}']) first")),
    }
}

pub fn emit(app: &AppHandle, event: &str, data: Value) {
    if let Some(w) = app.get_webview("app").or_else(|| app.get_webview("main")) {
        let _ = w.eval(format!(
            "window.__oaEmit && window.__oaEmit({}, {})",
            json!(event),
            data
        ));
    }
}
