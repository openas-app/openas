use super::*;

// ---- Tools ------------------------------------------------------------------

pub(super) fn tool_schemas() -> Value {
    let path = json!({ "type": "string", "description": "File name inside the app folder, e.g. index.html, editor.js, styles.css, icon.svg" });
    let doing = json!({ "type": "string", "description": "A few plain words the person sees while this runs, e.g. \"Writing the brush engine\" or \"Trying the eraser in dark mode\". No file names or code words." });
    let part = json!({ "type": "integer", "description": "Which part of your plan this works on (1 = the first)." });
    json!([
        { "name": "plan", "description": "Call this first, before writing any code: what the app is in one or two plain sentences, and its main parts in the order you'll build them (3 to 9 short names a non-developer understands, e.g. \"Layers panel\"). The person sees this as the progress list.",
          "input_schema": { "type": "object", "properties": { "summary": { "type": "string" }, "parts": { "type": "array", "items": { "type": "string" } } },
                            "required": ["summary", "parts"], "additionalProperties": false } },
        { "name": "list", "description": "List the app's files with their sizes and line counts.",
          "input_schema": { "type": "object", "properties": {}, "additionalProperties": false } },
        { "name": "view", "description": "Read a file with line numbers. Give a line range for large files (at most 600 lines per call).",
          "input_schema": { "type": "object", "properties": { "path": path, "start": { "type": "integer" }, "end": { "type": "integer" } },
                            "required": ["path"], "additionalProperties": false } },
        { "name": "create", "description": "Create a new file (or replace a small file entirely). For changes to an existing large file, use replace or insert.",
          "input_schema": { "type": "object", "properties": { "doing": doing, "part": part, "path": path, "content": { "type": "string" } },
                            "required": ["path", "content"], "additionalProperties": false } },
        { "name": "replace", "description": "Replace one exact piece of text in a file with new text. `old` must appear exactly once; include enough surrounding text to make it unique.",
          "input_schema": { "type": "object", "properties": { "doing": doing, "part": part, "path": path, "old": { "type": "string" }, "new": { "type": "string" } },
                            "required": ["path", "old", "new"], "additionalProperties": false } },
        { "name": "insert", "description": "Insert text after a line number (0 inserts at the top).",
          "input_schema": { "type": "object", "properties": { "doing": doing, "part": part, "path": path, "after_line": { "type": "integer" }, "text": { "type": "string" } },
                            "required": ["path", "after_line", "text"], "additionalProperties": false } },
        { "name": "check", "description": "Open the app in a real window and return a screenshot, console errors and load problems. Optional `script`: the body of an async JavaScript function run in the page after it loads. Use it like a person would, with the `ui` helpers (each moves a visible pointer the person watches): `await ui.click(target)`, `await ui.drag(target, [[dx,dy], ...])` (a path relative to where the drag starts), `await ui.type(target, text)`, `await ui.key(key, {meta, shift, alt})`, `await ui.wait(ms)`; a target is a CSS selector or {x, y} in window pixels. Then `return` what you want to verify. Read app state directly only to verify, not to act. `appearance`: light (default) or dark. `width`/`height`: window size (default 1200x800). Desktop powers are simulated in the check, never real: the person says yes to every ask (call `desk.deny = true` first to test a refusal), the folder is a sample Downloads folder in memory, `desk.menu(id)` clicks a menu bar item, `desk.shortcut(text)` presses a global shortcut, programs never really run (`app.system.run` returns a placeholder unless the test sets `desk.programs.top = 'its output'` or a function `(args) => ({ code, stdout, stderr })`; with full access the sample folder is ~/Downloads), and the result's `desktop` shows the grants, the menu bar, notifications, the file calls made and the camera, microphone or screen asks. In the check, the screen and camera are a moving test picture and the microphone a quiet tone, so recording really records.",
          "input_schema": { "type": "object", "properties": { "doing": doing, "script": { "type": "string" }, "appearance": { "type": "string", "enum": ["light", "dark"] },
                            "width": { "type": "integer" }, "height": { "type": "integer" } }, "additionalProperties": false } },
        { "name": "finish", "description": "Call when the app is genuinely done and checked, and icon.svg exists. `name`: the app's name, 1 to 3 words. `summary`: two or three plain sentences for the person about what it does (plain text: no markdown, no lists, no asterisks). `width`/`height`: the window size it should open at.",
          "input_schema": { "type": "object", "properties": { "name": { "type": "string" }, "summary": { "type": "string" },
                            "width": { "type": "integer" }, "height": { "type": "integer" } },
                            "required": ["name", "summary"], "additionalProperties": false } }
    ])
}

/// A path inside the workspace: `icon.svg` sits at the top, the rest in app/.
/// A path's last part, for messages to the model.
pub(super) fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(super) fn resolve(ws: &Path, p: &str) -> Result<PathBuf> {
    let p = p.trim().trim_start_matches("./").trim_start_matches("app/");
    if p.is_empty() || p.contains("..") || p.starts_with('/') || p.contains('\\') {
        bail!("use a plain file name inside the app folder");
    }
    Ok(if p == "icon.svg" {
        ws.join("icon.svg")
    } else {
        ws.join("app").join(p)
    })
}

pub(super) enum Loose {
    One(String),
    Many(usize),
    None(String),
}

/// `old` matched line by line with the whitespace at each line's ends
/// ignored. On one match, `new` goes in with its indentation moved to the
/// file's. On none, a hint naming the closest line.
pub(super) fn loose_replace(text: &str, old: &str, new: &str) -> Loose {
    let want: Vec<&str> = old.lines().map(str::trim).collect();
    let (a, b) = (
        want.iter().position(|l| !l.is_empty()),
        want.iter().rposition(|l| !l.is_empty()),
    );
    let (Some(a), Some(b)) = (a, b) else {
        return Loose::None(String::new());
    };
    let want = &want[a..=b];
    // Each line of the file with its byte range (without the newline).
    let mut lines = vec![];
    let mut at = 0;
    for l in text.split_inclusive('\n') {
        let body = l.trim_end_matches('\n').trim_end_matches('\r');
        lines.push((at, at + body.len(), body));
        at += l.len();
    }
    let hits: Vec<usize> = (0..lines.len().saturating_sub(want.len() - 1))
        .filter(|&i| {
            want.iter()
                .enumerate()
                .all(|(k, w)| lines[i + k].2.trim() == *w)
        })
        .collect();
    match hits.len() {
        1 => {
            let i = hits[0];
            let (start, end) = (lines[i].0, lines[i + want.len() - 1].1);
            let indent_of = |l: &str| l[..l.len() - l.trim_start().len()].to_string();
            let have = indent_of(lines[i].2);
            let gave = old
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(indent_of)
                .unwrap_or_default();
            let moved: Vec<String> = new
                .lines()
                .map(|l| match l.strip_prefix(gave.as_str()) {
                    Some(rest) if !l.trim().is_empty() => format!("{have}{rest}"),
                    _ => l.to_string(),
                })
                .collect();
            let mut body = moved.join("\n");
            if new.ends_with('\n') {
                body.push('\n');
            }
            Loose::One(format!("{}{}{}", &text[..start], body, &text[end..]))
        }
        0 => {
            // The closest line to `old`'s first line, by shared words.
            let first: std::collections::HashSet<&str> = want[0]
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .filter(|w| w.len() > 2)
                .collect();
            let best = lines
                .iter()
                .enumerate()
                .map(|(n, l)| {
                    (
                        l.2.split(|c: char| !c.is_alphanumeric() && c != '_')
                            .filter(|w| first.contains(w))
                            .count(),
                        n,
                    )
                })
                .max();
            match best {
                Some((score, n)) if score > 0 => Loose::None(format!(
                    " The closest line is {}: `{}`.",
                    n + 1,
                    lines[n].2.trim().chars().take(120).collect::<String>()
                )),
                _ => Loose::None(String::new()),
            }
        }
        n => Loose::Many(n),
    }
}

/// Runs one tool. Returns the tool_result content (text, maybe an image).
pub(super) fn run_tool(
    ws: &Path,
    name: &str,
    input: &Value,
    finished: &mut Option<Finished>,
    ev: &make::Events,
    live: &Live,
) -> Result<Value> {
    if input.get("__invalid_json").is_some() {
        bail!("your tool input was cut off or not valid JSON (probably the output limit). Send it again in smaller pieces.");
    }
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    if let Some(p) = input.get("part").and_then(Value::as_u64) {
        live.set("part", json!(p));
    }
    if let Some(d) = s("doing") {
        live.set("doing", json!(d));
    }
    match name {
        "plan" => {
            let parts: Vec<String> = input
                .get("parts")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if parts.is_empty() {
                bail!("give the parts as a list of short names");
            }
            live.set(
                "plan",
                json!({ "summary": s("summary").unwrap_or(""), "parts": parts }),
            );
            live.set_now("phase", json!("building"));
            if live.get("part").as_u64().unwrap_or(0) == 0 {
                live.set("part", json!(1));
            }
            Ok(json!("The person can see your plan now. Build it in this order; say which `part` each step works on."))
        }
        "list" => {
            let mut out = vec![];
            for (dir, prefix) in [(ws.join("app"), ""), (ws.to_path_buf(), "")] {
                for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_file()
                        && (dir.ends_with("app") || p.file_name().is_some_and(|n| n == "icon.svg"))
                    {
                        let t = std::fs::read_to_string(&p).unwrap_or_default();
                        out.push(format!(
                            "{prefix}{}  {} bytes, {} lines",
                            file_name(&p),
                            t.len(),
                            t.lines().count()
                        ));
                    }
                }
            }
            Ok(json!(if out.is_empty() {
                "No files yet.".to_string()
            } else {
                out.join("\n")
            }))
        }
        "view" => {
            let p = resolve(ws, s("path").ok_or_else(|| anyhow!("missing path"))?)?;
            let text = std::fs::read_to_string(&p).with_context(|| "no such file")?;
            let lines: Vec<&str> = text.lines().collect();
            let start = input
                .get("start")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .max(1) as usize;
            let end = input
                .get("end")
                .and_then(Value::as_u64)
                .map(|e| e as usize)
                .unwrap_or(lines.len())
                .min(lines.len())
                .min(start + 599);
            let mut out = String::new();
            for (i, l) in lines.iter().enumerate().take(end).skip(start - 1) {
                let l = if l.len() > 2000 {
                    &l[..l
                        .char_indices()
                        .nth(2000)
                        .map(|(i, _)| i)
                        .unwrap_or(l.len())]
                } else {
                    l
                };
                out.push_str(&format!("{:>5}  {l}\n", i + 1));
            }
            if end < lines.len() {
                out.push_str(&format!("(lines {start}-{end} of {})\n", lines.len()));
            }
            Ok(json!(out))
        }
        "create" => {
            let p = resolve(ws, s("path").ok_or_else(|| anyhow!("missing path"))?)?;
            let content = s("content").ok_or_else(|| anyhow!("missing content"))?;
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&p, content)?;
            (ev.say)(&format!(
                "Wrote {} ({} KB)",
                file_name(&p),
                content.len() / 1024
            ));
            Ok(json!(format!(
                "Created, {} lines.",
                content.lines().count()
            )))
        }
        "replace" => {
            let p = resolve(ws, s("path").ok_or_else(|| anyhow!("missing path"))?)?;
            let (old, new) = (
                s("old").ok_or_else(|| anyhow!("missing old"))?,
                s("new").ok_or_else(|| anyhow!("missing new"))?,
            );
            let text = std::fs::read_to_string(&p).with_context(|| "no such file")?;
            let file = file_name(&p);
            match text.matches(old).count() {
                1 => {
                    std::fs::write(&p, text.replacen(old, new, 1))?;
                    return Ok(json!("Replaced."));
                }
                0 => {}
                n => {
                    bail!("`old` appears {n} times; include more surrounding text so it is unique.")
                }
            }
            // Not exact: models often get indentation or trailing spaces
            // wrong. Match line by line ignoring the whitespace at each end;
            // if that finds one place, replace it there in the file's own
            // indentation.
            match loose_replace(&text, old, new) {
                Loose::One(t) => {
                    std::fs::write(&p, t)?;
                    Ok(json!("Replaced (the whitespace in `old` differed from the file; matched it line by line)."))
                }
                Loose::Many(n) => bail!("`old` matches {n} places (ignoring indentation); include more surrounding text so it is unique."),
                Loose::None(hint) => bail!("`old` was not found in {file}.{hint} View those lines and copy the text exactly, or use `create` to rewrite the file."),
            }
        }
        "insert" => {
            let p = resolve(ws, s("path").ok_or_else(|| anyhow!("missing path"))?)?;
            let after = input
                .get("after_line")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("missing after_line"))? as usize;
            let add = s("text").ok_or_else(|| anyhow!("missing text"))?;
            let text = std::fs::read_to_string(&p).with_context(|| "no such file")?;
            let mut lines: Vec<&str> = text.lines().collect();
            if after > lines.len() {
                bail!("the file has only {} lines", lines.len());
            }
            let block: Vec<&str> = add.lines().collect();
            lines.splice(after..after, block);
            std::fs::write(&p, lines.join("\n") + "\n")?;
            Ok(json!("Inserted."))
        }
        "check" => {
            live.set_now("phase", json!("checking"));
            live.bump("checks");
            let r = check_tool(ws, input, ev, live);
            live.set_now("phase", json!("building"));
            r
        }
        "finish" => {
            if !ws.join("icon.svg").exists() {
                bail!("create icon.svg first");
            }
            let w = input
                .get("width")
                .and_then(Value::as_u64)
                .unwrap_or(1100)
                .clamp(360, 2400);
            let h = input
                .get("height")
                .and_then(Value::as_u64)
                .unwrap_or(760)
                .clamp(300, 1600);
            std::fs::write(
                ws.join("window.json"),
                json!({ "width": w, "height": h }).to_string(),
            )?;
            *finished = Some(Finished {
                name: s("name").unwrap_or("New App").trim().to_string(),
                summary: s("summary").unwrap_or("").to_string(),
                unfinished: None,
            });
            Ok(json!("Done."))
        }
        _ => bail!("no tool called {name}"),
    }
}

/// A copy of a PNG no larger than `max` pixels on its longer side.
fn shrink_png(from: &Path, to: &Path, max: u32) -> Result<()> {
    use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
    let src = Pixmap::load_png(from)?;
    let scale = (max as f32 / src.width().max(src.height()) as f32).min(1.0);
    let (w, h) = (
        ((src.width() as f32 * scale).round() as u32).max(1),
        ((src.height() as f32 * scale).round() as u32).max(1),
    );
    let mut out = Pixmap::new(w, h).ok_or_else(|| anyhow::anyhow!("pixmap"))?;
    out.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..Default::default()
        },
        Transform::from_scale(scale, scale),
        None,
    );
    out.save_png(to)?;
    Ok(())
}

pub(super) fn snap_exe() -> PathBuf {
    std::env::var("OPENAS_SNAP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| paths::runner_bundle().with_file_name("snap"))
}

pub(super) fn check_tool(
    ws: &Path,
    input: &Value,
    ev: &make::Events,
    live: &Live,
) -> Result<Value> {
    if !ws.join("app/index.html").exists() {
        bail!("there is no index.html yet");
    }
    (ev.say)("Looking at it…");
    // The real runtime's load check (its content policy, its app API).
    let report = check::run(ws)?;
    let problems = report.problems();
    // The screenshot and the test script.
    let shot = ws.join(".shot.png");
    let small = ws.join(".shot-small.png");
    let _ = std::fs::remove_file(&shot);
    let script = input.get("script").and_then(Value::as_str).unwrap_or("");
    let sf = ws.join(".script.js");
    std::fs::write(&sf, script)?;
    let w = input
        .get("width")
        .and_then(Value::as_u64)
        .unwrap_or(1200)
        .clamp(360, 2400)
        .to_string();
    let h = input
        .get("height")
        .and_then(Value::as_u64)
        .unwrap_or(800)
        .clamp(300, 1600)
        .to_string();
    let look = input
        .get("appearance")
        .and_then(Value::as_str)
        .unwrap_or("light");
    let out = Command::new(snap_exe())
        .arg(ws.join("app"))
        .arg(&shot)
        .args([&w, &h])
        .arg(if script.is_empty() {
            PathBuf::new()
        } else {
            sf.clone()
        })
        .arg(look)
        .stderr(std::process::Stdio::null())
        .output()
        .context("starting the screenshot helper")?;
    let snap: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or(json!({ "errors": ["the screenshot window did not report back"] }));
    // What the test did with the pointer, for the person to watch.
    if snap
        .get("moves")
        .and_then(Value::as_array)
        .is_some_and(|m| !m.is_empty())
    {
        let seq = live
            .get("ghost")
            .get("seq")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        live.set_now("ghost", json!({ "seq": seq, "caption": input.get("doing").cloned().unwrap_or(json!("")), "moves": snap["moves"], "w": w, "h": h }));
    }
    let mut text = json!({
        "load_problems": problems,
        "console_errors": snap.get("errors").cloned().unwrap_or(json!([])),
        "script_result": snap.get("result").cloned().unwrap_or(json!("")),
        "visible_text_chars": report.text_chars,
    });
    // What the simulated desktop saw: grants, the menu bar, notifications, file calls.
    if let Some(d) = snap.get("desktop").filter(|d| d.is_object()) {
        let busy = d
            .get("grants")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
            || d.get("file_calls")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            || d.get("menu_bar").is_some_and(|m| !m.is_null())
            || d.get("media_asks")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            || d.get("shortcuts")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            || d.get("programs_run")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty());
        if busy {
            text["desktop"] = d.clone();
        }
    }
    let mut text = text.to_string();
    let mut content = vec![];
    if shot.exists() {
        let _ = shrink_png(&shot, &small, 1400);
        if let Ok(png) = std::fs::read(&small) {
            content.push(json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64(&png) } }));
            // Keep the latest look for the person watching.
            let _ = std::fs::copy(&small, ws.join("preview.png"));
        }
    } else {
        text.push_str("\n(no screenshot)");
    }
    content.insert(0, json!({ "type": "text", "text": text }));
    Ok(Value::Array(content))
}

pub(super) fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod path_tests {
    use super::*;
    #[test]
    fn builder_cannot_write_grants() {
        let ws = Path::new("/w");
        assert_eq!(
            resolve(ws, "grants.json").unwrap(),
            Path::new("/w/app/grants.json")
        );
        assert_eq!(
            resolve(ws, "version.json").unwrap(),
            Path::new("/w/app/version.json")
        );
        assert_eq!(resolve(ws, "icon.svg").unwrap(), Path::new("/w/icon.svg"));
        assert!(resolve(ws, "../grants.json").is_err());
        assert!(resolve(ws, "/w/grants.json").is_err());
    }
}

#[cfg(test)]
mod loose_tests {
    use super::{loose_replace, Loose};

    #[test]
    fn wrong_indentation_still_replaces_in_the_files_indentation() {
        let file = "function a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let Loose::One(t) = loose_replace(
            file,
            "  let x = 1;\n  let y = 2;",
            "  let x = 3;\n  let y = 4;",
        ) else {
            panic!()
        };
        assert_eq!(t, "function a() {\n    let x = 3;\n    let y = 4;\n}\n");
    }

    #[test]
    fn two_places_is_ambiguous() {
        let file = "a\n  b\nc\n  b\n";
        assert!(matches!(loose_replace(file, "b", "z"), Loose::Many(2)));
    }

    #[test]
    fn missing_text_names_the_closest_line() {
        let file = "let theme = 'light';\nconst count = 0;\n";
        let Loose::None(h) = loose_replace(file, "let theme = \"dark\";", "x") else {
            panic!()
        };
        assert!(h.contains("closest line is 1:"), "{h}");
    }

    #[test]
    fn crlf_files_keep_working() {
        let file = "a\r\n  b\r\nc\r\n";
        let Loose::One(t) = loose_replace(file, "b", "z") else {
            panic!()
        };
        assert_eq!(t, "a\r\n  z\r\nc\r\n");
    }
}
