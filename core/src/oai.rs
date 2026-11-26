//! Open models through any OpenAI-compatible chat endpoint (the Hugging Face
//! router by default), driving the same builder loop as Claude.
//!
//! The loop speaks in Messages-API shapes (content blocks, tool_use,
//! tool_result). This module turns a request in that shape into a chat
//! completion request, streams the reply, and turns it back into content
//! blocks, so the loop doesn't know which kind of model it is talking to.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::claude::{Ev, Turn};

pub const HF_ROUTER: &str = "https://router.huggingface.co/v1";

pub struct Oai {
    base: String,
    key: String,
    agent: ureq::Agent,
    /// Whether the model can look at screenshots.
    pub vision: bool,
    /// $ per million tokens (input, output), fixed for the run.
    pub price: (f64, f64),
}

/// $ per million tokens (input, output) for a model. On the Hugging Face
/// router the price is read from its live model list, taking the dearest
/// provider that supports tools (the router may pick any of them), so the
/// ledger can only overcount. Elsewhere OPENAS_AGENT_PRICE="in,out", or a
/// high default so the caps still hold.
pub fn price(base: &str, model: &str) -> (f64, f64) {
    known_price(base, model).unwrap_or((10.0, 50.0))
}

/// The real price if it can be found: the env override, the Hugging Face
/// router's or OpenRouter's live lists, or free for a model on this Mac.
pub fn known_price(base: &str, model: &str) -> Option<(f64, f64)> {
    if let Some((i, o)) = std::env::var("OPENAS_AGENT_PRICE").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    }) {
        return Some((i, o));
    }
    if base.contains("://localhost") || base.contains("://127.0.0.1") {
        return Some((0.0, 0.0));
    }
    if base.contains("openrouter.ai") {
        let list: Option<Value> = ureq::get(&format!("{}/models", base.trim_end_matches('/')))
            .call()
            .ok()
            .and_then(|mut r| r.body_mut().read_json().ok());
        let m = list.as_ref().and_then(|l| {
            l.get("data")?
                .as_array()?
                .iter()
                .find(|m| m["id"] == model)
                .cloned()
        })?;
        let per = |k: &str| {
            m.pointer(&format!("/pricing/{k}")).and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .or(v.as_f64())
            })
        };
        return Some((per("prompt")? * 1e6, per("completion")? * 1e6));
    }
    if base.contains("router.huggingface.co") {
        let id = model.split(':').next().unwrap_or(model);
        let cheapest = model.ends_with(":cheapest");
        let list: Option<Value> = ureq::get(&format!("{}/models", base.trim_end_matches('/')))
            .call()
            .ok()
            .and_then(|mut r| r.body_mut().read_json().ok());
        let found = list.as_ref().and_then(|l| {
            l.get("data")?
                .as_array()?
                .iter()
                .find(|m| m["id"] == id)
                .cloned()
        });
        if let Some(m) = found {
            let mut worst: Option<(f64, f64)> = None;
            for p in m["providers"].as_array().into_iter().flatten() {
                if p.get("supports_tools").and_then(Value::as_bool) == Some(false) {
                    continue;
                }
                let (Some(i), Some(o)) = (
                    p.pointer("/pricing/input").and_then(Value::as_f64),
                    p.pointer("/pricing/output").and_then(Value::as_f64),
                ) else {
                    continue;
                };
                // ":cheapest" asks the router for the cheapest provider, so it is
                // priced at that one; otherwise at the dearest, which may be picked.
                worst = Some(worst.map_or((i, o), |(a, b)| {
                    if cheapest {
                        (a.min(i), b.min(o))
                    } else {
                        (a.max(i), b.max(o))
                    }
                }));
            }
            if let Some(w) = worst {
                return Some(w);
            }
        }
    }
    None
}

/// The model ids a provider offers (its `/models` list), for the picker.
pub fn list_models(base: &str, key: Option<&str>) -> Result<Vec<String>> {
    let mut req = ureq::get(&format!("{}/models", base.trim_end_matches('/')));
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let v: Value = req
        .call()
        .context("couldn't reach the provider")?
        .body_mut()
        .read_json()?;
    let mut ids: Vec<String> = v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    ids.sort();
    ids.truncate(800);
    Ok(ids)
}

/// Who the provider is, in words, from its address.
fn who(base: &str) -> String {
    let named = [
        ("huggingface", "Hugging Face"),
        ("openrouter", "OpenRouter"),
        ("together", "Together"),
        ("groq", "Groq"),
        ("fireworks", "Fireworks"),
    ];
    if base.contains("://localhost") || base.contains("://127.0.0.1") {
        return "The model on this Mac".into();
    }
    named
        .iter()
        .find(|(k, _)| base.contains(k))
        .map_or("The model's provider".into(), |(_, n)| n.to_string())
}

/// A provider's refusal in words the person can act on; anything unusual
/// keeps the provider's own message.
pub fn plain_error(who: &str, status: u16, msg: &str) -> String {
    match status {
        401 | 403 => format!(
            "{who} didn't accept the key. Click the model name in the OpenAs window to change it."
        ),
        402 => {
            format!("{who} says the account has no credit left. Add credit there, then try again.")
        }
        404 => format!("{who} doesn't have that model any more. Choose another one in OpenAs."),
        429 => format!("{who} is busy or rate-limited. Try again in a minute."),
        500..=599 => format!("{who} had a problem on its side ({status}). Try again in a minute."),
        _ => format!("{who} answered {status}: {msg}"),
    }
}

pub fn sees_images(model: &str) -> bool {
    if let Ok(v) = std::env::var("OPENAS_AGENT_VISION") {
        return v == "1";
    }
    let m = model.split(':').next().unwrap_or(model);
    matches!(
        m,
        "deepseek-ai/DeepSeek-V4.1-Flash"
            | "deepseek-ai/DeepSeek-V4.1-Flash-1017"
            | "moonshotai/Kimi-K3.1-Pro"
            | "moonshotai/Kimi-K3"
            | "moonshotai/Kimi-K2.6"
            | "zai-org/GLM-5.3-Flash"
            | "MiniMaxAI/MiniMax-M3"
    )
}

pub fn cost((i, o): (f64, f64), u: &Value) -> f64 {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0) as f64 / 1e6;
    // Cached input is billed by some providers at a discount; count it at
    // full price so the ledger never undercounts.
    (n("input_tokens") + n("cache_read_input_tokens")) * i + n("output_tokens") * o
}

impl Oai {
    pub fn new(base: String, key: String, model: &str) -> Self {
        let vision = sees_images(model);
        let price = price(&base, model);
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(1800)))
            .http_status_as_error(false)
            .build()
            .into();
        Oai {
            base,
            key,
            agent,
            vision,
            price,
        }
    }

    /// One streamed turn. `body` is a Messages-API request.
    pub fn turn(&self, body: Value, on: &mut dyn FnMut(Ev) -> Result<()>) -> Result<Turn> {
        let req = to_chat(&body, self.vision);
        let mut resp = self
            .agent
            .post(&format!(
                "{}/chat/completions",
                self.base.trim_end_matches('/')
            ))
            .header("content-type", "application/json")
            .header("authorization", &format!("Bearer {}", self.key))
            .send_json(&req)
            .context("calling the model")?;
        let status = resp.status().as_u16();
        if status != 200 {
            let t = resp.body_mut().read_to_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
            let msg = v
                .pointer("/error/message")
                .or_else(|| v.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| t.chars().take(300).collect());
            bail!("{}", plain_error(&who(&self.base), status, &msg));
        }
        let mut text = String::new();
        // (id, name, arguments) per tool call index.
        let mut calls: Vec<(String, String, String)> = vec![];
        let mut finish = String::new();
        let mut usage = Value::Null;
        let mut written = 0usize;
        for line in BufReader::new(resp.body_mut().as_reader()).lines() {
            let line = line.context("reading the model's reply")?;
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(e) = v.get("error") {
                bail!(
                    "the model stopped: {}",
                    e.get("message").and_then(Value::as_str).unwrap_or("error")
                );
            }
            if v.get("usage").is_some_and(|u| !u.is_null()) {
                usage = v["usage"].clone();
            }
            let Some(c) = v.pointer("/choices/0") else {
                continue;
            };
            if let Some(r) = c.get("finish_reason").and_then(Value::as_str) {
                finish = r.to_string();
            }
            let d = c.get("delta").cloned().unwrap_or(Value::Null);
            for k in ["reasoning_content", "reasoning"] {
                if let Some(t) = d.get(k).and_then(Value::as_str) {
                    if !t.is_empty() {
                        on(Ev::Thinking(t))?;
                    }
                }
            }
            if let Some(t) = d.get("content").and_then(Value::as_str) {
                if !t.is_empty() {
                    text.push_str(t);
                    written += t.len();
                    on(Ev::Text(t))?;
                }
            }
            for tc in d
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let i = tc
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(calls.len() as u64) as usize;
                while calls.len() <= i {
                    calls.push(Default::default());
                }
                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    calls[i].0 = id.to_string();
                }
                if let Some(n) = tc.pointer("/function/name").and_then(Value::as_str) {
                    if !n.is_empty() && calls[i].1.is_empty() {
                        calls[i].1 = n.to_string();
                        on(Ev::Tool(n))?;
                    }
                }
                if let Some(a) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                    calls[i].2.push_str(a);
                    written += a.len();
                    let s = &calls[i].2;
                    let head = &s[..s.char_indices().nth(400).map(|(k, _)| k).unwrap_or(s.len())];
                    on(Ev::Writing(written, head))?;
                }
            }
        }
        let mut content = vec![];
        if !text.trim().is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
        for (n, (id, name, args)) in calls.into_iter().enumerate() {
            let input = if args.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str::<Value>(&args)
                    .unwrap_or_else(|_| json!({ "__invalid_json": true }))
            };
            let id = if id.is_empty() {
                format!("call_{n}")
            } else {
                id
            };
            content.push(json!({ "type": "tool_use", "id": id, "name": name, "input": input }));
        }
        let has_tools = content.iter().any(|b| b["type"] == "tool_use");
        let stop_reason = match finish.as_str() {
            "length" => "max_tokens",
            _ if has_tools => "tool_use",
            _ => "end_turn",
        }
        .to_string();
        // No usage means no way to count the cost: stop rather than count it as free.
        if usage.get("prompt_tokens").and_then(Value::as_u64).is_none() {
            bail!("the model's server didn't report token usage, so the cost can't be counted; stopping");
        }
        let cached = usage
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let prompt = usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let usage = json!({
            "input_tokens": prompt.saturating_sub(cached),
            "cache_read_input_tokens": cached,
            "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
        });
        Ok(Turn {
            content,
            stop_reason,
            usage,
        })
    }
}

/// A Messages-API request as a chat-completions request.
pub fn to_chat(body: &Value, vision: bool) -> Value {
    let mut msgs = vec![];
    let system: String = match body.get("system") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    };
    if !system.is_empty() {
        msgs.push(json!({ "role": "system", "content": system }));
    }
    for m in body
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
        let blocks: Vec<Value> = match m.get("content") {
            Some(Value::String(s)) => vec![json!({ "type": "text", "text": s })],
            Some(Value::Array(a)) => a.clone(),
            _ => vec![],
        };
        if role == "assistant" {
            let text: String = blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let calls: Vec<Value> = blocks
                .iter()
                .filter(|b| b["type"] == "tool_use")
                .map(|b| json!({ "id": b["id"], "type": "function", "function": { "name": b["name"], "arguments": b["input"].to_string() } }))
                .collect();
            let mut a = json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) } });
            if !calls.is_empty() {
                a["tool_calls"] = json!(calls);
            }
            msgs.push(a);
            continue;
        }
        // A user turn: tool results become tool messages; their images, and
        // any plain text, follow in one user message.
        let mut after: Vec<Value> = vec![];
        for b in &blocks {
            match b["type"].as_str() {
                Some("tool_result") => {
                    let parts: Vec<Value> = match &b["content"] {
                        Value::String(s) => vec![json!({ "type": "text", "text": s })],
                        Value::Array(a) => a.clone(),
                        _ => vec![],
                    };
                    let mut t: String = parts
                        .iter()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if b.get("is_error").and_then(Value::as_bool) == Some(true) {
                        t = format!("Error: {t}");
                    }
                    let imgs: Vec<&Value> = parts.iter().filter(|p| p["type"] == "image").collect();
                    if !imgs.is_empty() {
                        if vision {
                            after.push(json!({ "type": "text", "text": "The screenshot from that check:" }));
                            for p in imgs {
                                let url = format!(
                                    "data:{};base64,{}",
                                    p["source"]["media_type"].as_str().unwrap_or("image/png"),
                                    p["source"]["data"].as_str().unwrap_or("")
                                );
                                after.push(
                                    json!({ "type": "image_url", "image_url": { "url": url } }),
                                );
                            }
                        } else {
                            t.push_str("\n(A screenshot was taken, but this model can't see images. Verify with the script's return value instead.)");
                        }
                    }
                    msgs.push(json!({ "role": "tool", "tool_call_id": b["tool_use_id"], "content": if t.is_empty() { "Done.".to_string() } else { t } }));
                }
                Some("text") => after.push(json!({ "type": "text", "text": b["text"] })),
                Some("image") if vision => {
                    let url = format!(
                        "data:{};base64,{}",
                        b["source"]["media_type"].as_str().unwrap_or("image/png"),
                        b["source"]["data"].as_str().unwrap_or("")
                    );
                    after.push(json!({ "type": "image_url", "image_url": { "url": url } }));
                }
                _ => {}
            }
        }
        if !after.is_empty() {
            let only_text = after.iter().all(|p| p["type"] == "text");
            let content = if only_text {
                json!(after
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n"))
            } else {
                json!(after)
            };
            msgs.push(json!({ "role": "user", "content": content }));
        }
    }
    let tools: Vec<Value> = body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| json!({ "type": "function", "function": { "name": t["name"], "description": t["description"], "parameters": t["input_schema"] } }))
        .collect();
    json!({
        "model": body["model"],
        "messages": msgs,
        "tools": tools,
        "tool_choice": "auto",
        "stream": true,
        "stream_options": { "include_usage": true },
        "max_tokens": 32000,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_a_tool_round_trip() {
        let body = json!({
            "model": "m", "system": [{ "type": "text", "text": "S" }],
            "tools": [{ "name": "view", "description": "d", "input_schema": { "type": "object" } }],
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "make it" }] },
                { "role": "assistant", "content": [{ "type": "thinking", "thinking": "x", "signature": "s" }, { "type": "text", "text": "ok" },
                    { "type": "tool_use", "id": "c1", "name": "view", "input": { "path": "a" } }] },
                { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "c1", "content": [{ "type": "text", "text": "R" },
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AA" } }] },
                    { "type": "text", "text": "a note" }] }
            ]
        });
        let c = to_chat(&body, true);
        let m = c["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(
            m[2]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a\"}"
        );
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["content"], "R");
        assert_eq!(m[4]["role"], "user");
        assert!(m[4]["content"].to_string().contains("image_url"));
        let blind = to_chat(&body, false);
        assert!(blind["messages"][3]["content"]
            .as_str()
            .unwrap()
            .contains("can't see images"));
        assert_eq!(blind["messages"][4]["content"], "a note");
        assert_eq!(c["tools"][0]["function"]["parameters"]["type"], "object");
    }
}
