use crate::*;

// ---- The conversation column (its own webview, label "talk") -------------------

pub(crate) struct Talk {
    pub(crate) open: std::sync::atomic::AtomicBool,
}

pub(crate) fn only_talk(webview: &tauri::Webview) -> Result<(), String> {
    if webview.label() == "talk" {
        Ok(())
    } else {
        Err("not allowed".into())
    }
}

pub(crate) fn read_json(p: &Path) -> Value {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// Everything the column shows, read from the app's own folder.
#[tauri::command]
pub fn talk_state(webview: tauri::Webview, who: State<Who>) -> Result<Value, String> {
    only_talk(&webview)?;
    let me = who.me(&webview)?;
    let moved = me.folder.with_extension("moved");
    let story: Vec<Value> = std::fs::read_to_string(me.folder.join("story.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    // The draft's icon, for the Save as App card.
    let icon = if me.making {
        std::fs::read(me.folder.join("icon.svg"))
            .ok()
            .map(|b| format!("data:image/svg+xml;base64,{}", powers::b64(&b)))
    } else {
        None
    };
    if me.making {
        crate::who::settle(&me.folder);
    }
    let making = read_json(&me.folder.join("making.json"));
    Ok(
        json!({ "mode": if me.making { "making" } else { "app" }, "name": me.name, "making": making, "icon": icon,
               "story": story, "moved": moved.exists().then(|| read_json(&moved)),
               "pending": me.folder.join(".pending/app/index.html").exists(),
               // Shown in OpenAs's own window, which has a way back to Home.
               "home": who.home.manager,
               // What the person first asked for: a build carried on after a
               // stop shows it above "Continue".
               "first": if me.making { std::fs::read_to_string(me.folder.join("prompt.txt")).ok().map(|t| t.trim().to_string()) } else { None } }),
    )
}

#[tauri::command]
pub fn talk_note(text: String, webview: tauri::Webview, who: State<Who>) -> Result<(), String> {
    only_talk(&webview)?;
    let me = who.me(&webview)?;
    use std::io::Write;
    let text: String = text.trim().chars().take(2000).collect();
    if text.is_empty() {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(me.folder.join("notes.jsonl"))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{}", json!({ "text": text })).map_err(|e| e.to_string())
}

/// Save as App (with the name the person gave it) or Discard, for a
/// finished draft in the making window.
#[tauri::command]
pub fn talk_choose(
    save: bool,
    name: String,
    webview: tauri::Webview,
    who: State<Who>,
) -> Result<(), String> {
    only_talk(&webview)?;
    let me = who.me(&webview)?;
    if !me.making {
        return Err("not allowed".into());
    }
    if !save {
        // A build that stopped or failed has no process waiting for the
        // choice: throw the draft away here.
        if build_ended(&me.folder) {
            std::fs::write(
                me.folder.with_extension("moved"),
                json!({ "discarded": true }).to_string(),
            )
            .map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&me.folder);
            return Ok(());
        }
        return std::fs::write(me.folder.with_extension("discard"), "").map_err(|e| e.to_string());
    }
    if build_ended(&me.folder) {
        return Err("it didn't finish: continue it first".into());
    }
    std::fs::write(me.folder.with_extension("keep"), name).map_err(|e| e.to_string())?;
    Ok(())
}

/// The draft's build stopped or failed, so no process is working on it.
fn build_ended(folder: &Path) -> bool {
    matches!(
        read_json(&folder.join("making.json"))["phase"].as_str(),
        Some("stopped" | "failed")
    )
}

/// Carries on with a draft whose build stopped or failed: the build starts
/// again in the same folder, from the files it had written, with the
/// person's note if they gave one.
#[tauri::command]
pub fn talk_continue(text: String, webview: tauri::Webview, who: State<Who>) -> Result<(), String> {
    only_talk(&webview)?;
    let me = who.me(&webview)?;
    if !me.making || !build_ended(&me.folder) {
        return Err("not allowed".into());
    }
    let helper = me
        .helper
        .clone()
        .ok_or("this window doesn't know where OpenAs is")?;
    let first = std::fs::read_to_string(me.folder.join("prompt.txt")).unwrap_or_default();
    let note: String = text.trim().chars().take(2000).collect();
    let started = me.folder.join("app/index.html").exists();
    let request = match (started, note.is_empty()) {
        (true, true) => format!("It stopped before it was finished. The person first asked for:\n\n{}\n\nFinish building it.", first.trim()),
        (true, false) => format!("It stopped before it was finished. The person first asked for:\n\n{}\n\nFinish building it. They add: {note}", first.trim()),
        (false, true) => first.trim().to_string(),
        (false, false) => format!("{}\n\n{note}", first.trim()),
    };
    let shown = if note.is_empty() {
        "Continue".to_string()
    } else {
        note.clone()
    };
    std::fs::write(
        me.folder.join("continue.json"),
        json!({ "request": request, "shown": shown }).to_string(),
    )
    .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(me.folder.join("stop"));
    std::process::Command::new(&helper)
        .arg("make-run")
        .arg(&me.folder)
        .envs(me.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start OpenAs ({}): {e}", helper.display()))?;
    Ok(())
}

#[tauri::command]
pub fn talk_stop(webview: tauri::Webview, who: State<Who>) -> Result<(), String> {
    only_talk(&webview)?;
    let me = who.me(&webview)?;
    std::fs::write(me.folder.join("stop"), "").map_err(|e| e.to_string())
}

/// The app's files, read only, for "Show code".
#[tauri::command]
pub fn talk_code(webview: tauri::Webview, who: State<Who>) -> Result<Value, String> {
    only_talk(&webview)?;
    let me = who.me(&webview)?;
    let mut files = vec![];
    for e in std::fs::read_dir(me.folder.join("app"))
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let p = e.path();
        if p.is_file() {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            files.push(json!({ "name": e.file_name().to_string_lossy(), "text": text.chars().take(400_000).collect::<String>() }));
        }
    }
    files.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!(files))
}

#[tauri::command]
pub fn talk_close(webview: tauri::Webview, app: AppHandle, who: State<Who>) -> Result<(), String> {
    only_talk(&webview)?;
    let me = who.me(&webview)?;
    if !me.making {
        show_talk(&app, false);
    }
    Ok(())
}

/// Lays out the app and the column side by side in the window.
pub(crate) fn layout(app: &AppHandle) {
    if app.state::<Who>().home.manager {
        return who::layout(app);
    }
    let Some(win) = app.get_window("main") else {
        return;
    };
    let (Ok(size), Ok(scale)) = (win.inner_size(), win.scale_factor()) else {
        return;
    };
    let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
    let open = app
        .try_state::<Talk>()
        .is_some_and(|t| t.open.load(Ordering::SeqCst));
    let tw = if open { TALK_W.min(w * 0.5) } else { 0.0 };
    if let Some(a) = app.get_webview("app") {
        let _ = a.set_bounds(tauri::Rect {
            position: tauri::LogicalPosition::new(0.0, 0.0).into(),
            size: tauri::LogicalSize::new(w - tw, h).into(),
        });
    }
    if let Some(t) = app.get_webview("talk") {
        let _ = t.set_bounds(tauri::Rect {
            position: tauri::LogicalPosition::new(w - tw, 0.0).into(),
            size: tauri::LogicalSize::new(tw.max(1.0), h).into(),
        });
        let _ = if open { t.show() } else { t.hide() };
    }
}

pub(crate) fn show_talk(app: &AppHandle, open: bool) {
    let was = app
        .try_state::<Talk>()
        .is_some_and(|t| t.open.swap(open, Ordering::SeqCst));
    // The window grows to make room for the column, so the app keeps its size.
    if was != open {
        if let Some(win) = app.get_window("main") {
            if let (Ok(size), Ok(scale)) = (win.inner_size(), win.scale_factor()) {
                let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
                let mut w = if open {
                    w + TALK_W
                } else {
                    (w - TALK_W).max(400.0)
                };
                // Keep the whole window on screen: slide it left to make room,
                // and if the screen is too narrow, the app gives up the width.
                if open {
                    if let (Ok(Some(mon)), Ok(pos)) = (win.current_monitor(), win.outer_position())
                    {
                        let wa = mon.work_area();
                        let left = wa.position.x as f64 / scale;
                        let right = left + wa.size.width as f64 / scale;
                        let (x, y) = (pos.x as f64 / scale, pos.y as f64 / scale);
                        w = w.min(right - left);
                        if x + w > right {
                            let _ = win.set_position(tauri::LogicalPosition::new(
                                (right - w).max(left),
                                y,
                            ));
                        }
                    }
                }
                let _ = win.set_size(tauri::LogicalSize::new(w, h));
            }
        }
    }
    layout(app);
    if let Some(t) = app.get_webview(if open { "talk" } else { "app" }) {
        let _ = t.set_focus();
    }
}

/// The test's pointer moves, replayed over the app as it's made, in OpenAs's
/// own look (an ink dot, never an arrow that could pass for your cursor),
/// with what it's trying in a caption at the bottom.
pub(crate) fn ghost_js(g: &Value) -> String {
    include_str!("../pages/ghost.js").replace("__OPENAS_GHOST__", &g.to_string())
}

/// Discard from the conversation (a build that stopped or failed, or the
/// name menu): the same sheet the menu bar opens.
#[tauri::command]
pub fn talk_ask(save: bool, webview: tauri::Webview, app: tauri::AppHandle) -> Result<(), String> {
    only_talk(&webview)?;
    crate::sheet::ask(&app, save);
    Ok(())
}
