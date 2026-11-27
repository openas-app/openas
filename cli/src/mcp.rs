//! `openas mcp`: an MCP server over stdio, so any MCP client (Open WebUI,
//! Goose, Jan, LM Studio, Claude, Cursor…) can make apps with its own model.
//! The client's model writes the HTML; we check it, install it, and hand
//! back what broke so it can fix it. No key, no provider: the user's own
//! chat is the builder.

use std::io::{BufRead, Write};

use anyhow::{anyhow, Result};
use openas_core::make;
use serde_json::{json, Value};

const INSTRUCTIONS: &str = "OpenAs turns one HTML file into a real desktop app with its own window, name, icon and notifications. \
Call get_app_format first and follow it exactly. Then create_app with the file and an SVG icon. \
If it returns problems, fix them and call create_app again. To change an app, read_app, then update_app with the complete new file; \
the user's saved data is kept, so keep using the same app.store keys.";

fn tools() -> Value {
    let app = json!({ "type": "string", "description": "The app's id or name, from list_apps" });
    json!([
        { "name": "get_app_format", "description": "The rules an app file must follow (one HTML file, the app.* API, the icon). Read this before writing an app.",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "create_app", "description": "Check an app file and, if it runs, install it as a desktop app and open it. Returns the problems to fix if it doesn't.",
          "inputSchema": { "type": "object", "required": ["html"], "properties": {
              "html": { "type": "string", "description": "The complete app file. Its <title> becomes the app's name." },
              "icon_svg": { "type": "string", "description": "The icon: <svg viewBox=\"0 0 1024 1024\">, a rounded square with one bold white symbol, no text." } } } },
        { "name": "update_app", "description": "Replace an app's file with a new version (the icon stays). Checked first; if it fails, the old version stays and the problems come back. An open window reloads by itself.",
          "inputSchema": { "type": "object", "required": ["app", "html"], "properties": { "app": app, "html": { "type": "string" } } } },
        { "name": "read_app", "description": "An app's current file, and the shape of its saved data (values hidden).",
          "inputSchema": { "type": "object", "required": ["app"], "properties": { "app": app } } },
        { "name": "list_apps", "description": "The user's apps.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "undo_app", "description": "Put back the version before the last change.",
          "inputSchema": { "type": "object", "required": ["app"], "properties": { "app": app } } },
        { "name": "open_app", "description": "Open an installed app.",
          "inputSchema": { "type": "object", "required": ["app"], "properties": { "app": app } } }
    ])
}

fn call(name: &str, a: &Value) -> Result<String> {
    let s = |k: &str| {
        a.get(k)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing {k}"))
    };
    let opt = |k: &str| a.get(k).and_then(Value::as_str);
    Ok(match name {
        "get_app_format" => format!(
            "{}\n\nWhen calling create_app or update_app, pass the file as `html` and the icon as `icon_svg` instead of code blocks.",
            make::format_spec()
        ),
        "create_app" => {
            let o = make::create_from(s("html")?, opt("icon_svg"), "mcp")?;
            if !o.problems.is_empty() {
                return Ok(format!("Not installed. Fix these and call create_app again:\n- {}", o.problems.join("\n- ")));
            }
            if let Some(p) = &o.info.installed {
                let _ = openas_platform::launch(p, false);
            }
            format!("Installed \"{}\" (id {}) and opened it.", o.info.name, o.info.id)
        }
        "update_app" => {
            let o = make::update_from(s("app")?, s("html")?)?;
            if o.problems.is_empty() {
                format!("Updated \"{}\". Its open window reloads by itself.", o.info.name)
            } else {
                format!("Kept the old version. Fix these and call update_app again:\n- {}", o.problems.join("\n- "))
            }
        }
        "read_app" => {
            let folder = make::find(s("app")?)?;
            let html = std::fs::read_to_string(folder.join("app/index.html"))?;
            let shape = make::data_shape(&folder).unwrap_or_else(|| "(nothing saved yet)".into());
            format!("Saved data shape (values hidden):\n{shape}\n\nCurrent file:\n{html}")
        }
        "list_apps" => {
            let list: Vec<String> = make::list()?.iter().map(|i| format!("{} (id {})", i.name, i.id)).collect();
            if list.is_empty() { "No apps yet.".into() } else { list.join("\n") }
        }
        "undo_app" => {
            make::undo(s("app")?)?;
            "Put back the previous version.".into()
        }
        "open_app" => {
            let info = make::read_info(&make::find(s("app")?)?)?;
            let p = info.installed.ok_or_else(|| anyhow!("not installed"))?;
            openas_platform::launch(&p, false)?;
            format!("Opened \"{}\".", info.name)
        }
        _ => return Err(anyhow!("unknown tool {name}")),
    })
}

pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            continue;
        }; // a notification: nothing to answer
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "openas", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                Ok(match call(name, &args) {
                    Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                    Err(e) => {
                        json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true })
                    }
                })
            }
            _ => Err(json!({ "code": -32601, "message": format!("no method {method}") })),
        };
        let reply = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}
