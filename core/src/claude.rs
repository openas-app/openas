//! Claude through Anthropic's Messages API, over plain HTTP (there is no Rust
//! SDK). Streams one turn of an agent loop and gives back the reply's content
//! blocks exactly as the API sent them, so the loop can pass them back
//! unchanged (thinking blocks carry signatures and must not be edited).

use std::io::{BufRead, BufReader};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

pub const API: &str = "https://api.anthropic.com/v1/messages";
pub const VERSION: &str = "2026-10-21";

/// What happened during a turn, for the person watching.
pub enum Ev<'a> {
    /// A piece of the readable thinking summary.
    Thinking(&'a str),
    /// A piece of text the model wrote between tool calls.
    Text(&'a str),
    /// A tool call has started (its name).
    Tool(&'a str),
    /// Bytes of tool input so far in this turn, and the start of the current
    /// tool's input (where a caption like `doing` usually sits).
    Writing(usize, &'a str),
}

pub struct Turn {
    /// The assistant's content blocks, as sent.
    pub content: Vec<Value>,
    pub stop_reason: String,
    /// Merged usage: input, cache writes and reads, output.
    pub usage: Value,
}

pub struct Claude {
    key: String,
    agent: ureq::Agent,
}

impl Claude {
    pub fn new(key: String) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(1800)))
            .http_status_as_error(false)
            .build()
            .into();
        Claude { key, agent }
    }

    /// One streamed request. `body` is the full Messages request without
    /// `stream`. Returning an error from `on` stops the request.
    pub fn turn(&self, mut body: Value, on: &mut dyn FnMut(Ev) -> Result<()>) -> Result<Turn> {
        body["stream"] = json!(true);
        let mut resp = self
            .agent
            .post(API)
            .header("content-type", "application/json")
            .header("x-api-key", &self.key)
            .header("anthropic-version", VERSION)
            .send_json(&body)
            .context("calling Anthropic")?;
        let status = resp.status().as_u16();
        if status != 200 {
            let v: Value = resp.body_mut().read_json().unwrap_or(Value::Null);
            let msg = v
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            bail!("{}", crate::oai::plain_error("Anthropic", status, &msg));
        }
        let mut blocks: Vec<Value> = vec![];
        let mut partial: Vec<String> = vec![];
        let mut usage = Map::new();
        let mut stop_reason = String::new();
        let mut written = 0usize;
        let mut event = String::new();
        for line in BufReader::new(resp.body_mut().as_reader()).lines() {
            let line = line.context("reading Claude's reply")?;
            if let Some(e) = line.strip_prefix("event:") {
                event = e.trim().to_string();
                continue;
            }
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let kind = v
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or(&event)
                .to_string();
            match kind.as_str() {
                "message_start" => {
                    if let Some(u) = v.pointer("/message/usage").and_then(Value::as_object) {
                        for (k, x) in u {
                            usage.insert(k.clone(), x.clone());
                        }
                    }
                }
                "content_block_start" => {
                    let i = v
                        .get("index")
                        .and_then(Value::as_u64)
                        .unwrap_or(blocks.len() as u64) as usize;
                    let b = v.get("content_block").cloned().unwrap_or(Value::Null);
                    if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                        on(Ev::Tool(
                            b.get("name").and_then(Value::as_str).unwrap_or(""),
                        ))?;
                    }
                    while blocks.len() <= i {
                        blocks.push(Value::Null);
                        partial.push(String::new());
                    }
                    blocks[i] = b;
                }
                "content_block_delta" => {
                    let i = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let d = v.get("delta").cloned().unwrap_or(Value::Null);
                    let Some(b) = blocks.get_mut(i) else { continue };
                    match d.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text_delta" => {
                            let t = d.get("text").and_then(Value::as_str).unwrap_or("");
                            append(b, "text", t);
                            written += t.len();
                            on(Ev::Text(t))?;
                        }
                        "thinking_delta" => {
                            let t = d.get("thinking").and_then(Value::as_str).unwrap_or("");
                            append(b, "thinking", t);
                            on(Ev::Thinking(t))?;
                        }
                        "signature_delta" => {
                            append(
                                b,
                                "signature",
                                d.get("signature").and_then(Value::as_str).unwrap_or(""),
                            );
                        }
                        "input_json_delta" => {
                            let t = d.get("partial_json").and_then(Value::as_str).unwrap_or("");
                            partial[i].push_str(t);
                            written += t.len();
                            let head = &partial[i][..partial[i]
                                .char_indices()
                                .nth(400)
                                .map(|(k, _)| k)
                                .unwrap_or(partial[i].len())];
                            on(Ev::Writing(written, head))?;
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    let i = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    if let Some(b) = blocks.get_mut(i) {
                        if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                            let raw = partial[i].trim();
                            // A cut-off or invalid input stays visible as such;
                            // the loop answers it with an error result.
                            b["input"] = if raw.is_empty() {
                                json!({})
                            } else {
                                serde_json::from_str(raw)
                                    .unwrap_or_else(|_| json!({ "__invalid_json": true }))
                            };
                        }
                    }
                }
                "message_delta" => {
                    if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                        stop_reason = r.to_string();
                    }
                    if let Some(u) = v.get("usage").and_then(Value::as_object) {
                        for (k, x) in u {
                            if !x.is_null() {
                                usage.insert(k.clone(), x.clone());
                            }
                        }
                    }
                }
                "error" => {
                    let m = v
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("error");
                    bail!("Claude stopped: {m}");
                }
                _ => {}
            }
        }
        // A tool_use whose input never closed (cut off) still needs an input.
        for (i, b) in blocks.iter_mut().enumerate() {
            if b.get("type").and_then(Value::as_str) == Some("tool_use") && b.get("input").is_none()
            {
                b["input"] = if partial[i].trim().is_empty() {
                    json!({})
                } else {
                    json!({ "__invalid_json": true })
                };
            }
        }
        blocks.retain(|b| !b.is_null());
        Ok(Turn {
            content: blocks,
            stop_reason,
            usage: Value::Object(usage),
        })
    }
}

fn append(b: &mut Value, field: &str, t: &str) {
    let cur = b
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    b[field] = json!(cur + t);
}

/// Dollars for one turn's usage on Claude Opus 5.5: $4 in, $20 out per
/// million tokens; cache writes (5 minutes) 1.25x input, reads $0.20.
pub fn cost(model: &str, u: &Value) -> f64 {
    let (inp, out, read) = match model {
        "claude-opus-5-5" => (4.0, 20.0, 0.20),
        "claude-sonnet-5-5" => (2.0, 10.0, 0.10),
        "claude-haiku-5-5" => (0.10, 0.50, 0.01),
        // Unknown: price it as the dearest current model so a cap still holds.
        _ => (10.0, 50.0, 1.0),
    };
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0) as f64 / 1e6;
    n("input_tokens") * inp
        + n("cache_creation_input_tokens") * inp * 1.25
        + n("cache_read_input_tokens") * read
        + n("output_tokens") * out
}
