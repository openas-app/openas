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
pub fn talk_state(webview: tauri::Webview, me: State<App>) -> Result<Value, String> {
    only_talk(&webview)?;
    let story: Vec<Value> = std::fs::read_to_string(me.folder.join("story.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let moved = me.folder.with_extension("moved");
    // The draft's icon, for the Save as App card.
    let icon = if me.making {
        std::fs::read(me.folder.join("icon.svg"))
            .ok()
            .map(|b| format!("data:image/svg+xml;base64,{}", powers::b64(&b)))
    } else {
        None
    };
    Ok(
        json!({ "mode": if me.making { "making" } else { "app" }, "name": me.name, "making": read_json(&me.folder.join("making.json")), "icon": icon,
               "story": story, "moved": moved.exists().then(|| read_json(&moved)) }),
    )
}

#[tauri::command]
pub fn talk_note(text: String, webview: tauri::Webview, me: State<App>) -> Result<(), String> {
    only_talk(&webview)?;
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
    me: State<App>,
) -> Result<(), String> {
    only_talk(&webview)?;
    if !me.making {
        return Err("not allowed".into());
    }
    let (path, text) = if save {
        (me.folder.with_extension("keep"), name)
    } else {
        (me.folder.with_extension("discard"), String::new())
    };
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn talk_stop(webview: tauri::Webview, me: State<App>) -> Result<(), String> {
    only_talk(&webview)?;
    std::fs::write(me.folder.join("stop"), "").map_err(|e| e.to_string())
}

/// The app's files, read only, for "Show code".
#[tauri::command]
pub fn talk_code(webview: tauri::Webview, me: State<App>) -> Result<Value, String> {
    only_talk(&webview)?;
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
pub fn talk_close(webview: tauri::Webview, app: AppHandle, me: State<App>) -> Result<(), String> {
    only_talk(&webview)?;
    if !me.making {
        show_talk(&app, false);
    }
    Ok(())
}

/// Lays out the app and the column side by side in the window.
pub(crate) fn layout(app: &AppHandle) {
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
