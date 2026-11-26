//! Providers and the one call every provider can answer: a plain chat
//! completion (OpenAI-compatible). No tools, no JSON mode, no vendor features,
//! so any endpoint the user points at works.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths;

const PRESETS: &str = include_str!("../prompts/providers.json");
const TESTED: &str = include_str!("../prompts/models.json");

/// Models that passed the generation test with a provider, with what they
/// cost per app when measured. Shown first; anything else is "your own".
pub fn tested_models(provider_id: &str) -> Value {
    serde_json::from_str::<Value>(TESTED)
        .ok()
        .and_then(|v| v.get(provider_id).cloned())
        .unwrap_or(json!([]))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    /// Environment variable holding the key; none for local servers.
    pub key_env: Option<String>,
    /// "anthropic" for the Messages API; otherwise OpenAI-compatible.
    #[serde(default)]
    pub api: Option<String>,
    /// "claude", "cloud" or "local": how the first-run screen groups it.
    #[serde(default)]
    pub kind: Option<String>,
    /// Where to get a key.
    #[serde(default)]
    pub key_url: Option<String>,
}

/// What the user chose. Kept in `<root>/settings.json`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    pub provider: String,
    pub build_model: String,
    /// Thinking models spend much of this on reasoning before writing the
    /// file; some endpoints cap it lower, so it is a setting.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Spending stops for the rest of the calendar month at this many
    /// dollars. It catches a build that runs away; the person sets it.
    #[serde(default = "default_monthly_limit")]
    pub monthly_limit: f64,
    /// One app (or one change) stops at this many dollars.
    #[serde(default = "default_app_limit")]
    pub app_limit: f64,
}

fn default_max_tokens() -> u32 {
    48_000
}

fn default_monthly_limit() -> f64 {
    50.0
}

fn default_app_limit() -> f64 {
    10.0
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            provider: "anthropic".into(),
            build_model: "claude-opus-5-5".into(),
            max_tokens: default_max_tokens(),
            monthly_limit: default_monthly_limit(),
            app_limit: default_app_limit(),
        }
    }
}

pub fn save_settings(s: &Settings) -> Result<()> {
    std::fs::create_dir_all(paths::root())?;
    std::fs::write(
        paths::root().join("settings.json"),
        serde_json::to_string_pretty(s)?,
    )?;
    Ok(())
}

/// The presets, with any of the user's own (`<root>/providers.json`) on top.
pub fn providers() -> Result<BTreeMap<String, Provider>> {
    let mut all: BTreeMap<String, Provider> = serde_json::from_str(PRESETS)?;
    if let Ok(text) = std::fs::read_to_string(paths::root().join("providers.json")) {
        let mine: BTreeMap<String, Provider> =
            serde_json::from_str(&text).context("providers.json")?;
        all.extend(mine);
    }
    Ok(all)
}

/// Whether the person has chosen a model yet (the first-run screen until then).
pub fn configured() -> bool {
    paths::root().join("settings.json").exists()
}

pub fn settings() -> Settings {
    std::fs::read_to_string(paths::root().join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// A provider's key: from the environment (for scripts), else the Keychain
/// (entered in the OpenAs window).
pub fn key_for(provider_id: &str, var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| crate::keys::get(provider_id))
}

/// A key left in an old `.env` file (before keys moved to the Keychain).
pub fn legacy_key(var: &str) -> Option<String> {
    let files: [PathBuf; 2] = [
        paths::root().join(".env"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.env"),
    ];
    files.iter().find_map(|f| {
        std::fs::read_to_string(f).ok()?.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == var).then(|| v.trim().to_string())
        })
    })
}

/// What a connection test found, in words the person can act on.
pub enum Test {
    Ok,
    Refused(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Message {
            role: role.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug)]
pub struct Reply {
    pub text: String,
    /// "stop" when the model finished; "length" means it was cut off.
    pub finish: String,
    pub output_tokens: u64,
    /// In US dollars, when the provider reports it.
    pub cost: Option<f64>,
}

/// What a streamed reply is doing right now.
pub enum Delta<'a> {
    /// The model is reasoning: the newest piece of it.
    Thinking(&'a str),
    /// New answer text.
    Text(&'a str),
}

pub struct Client {
    provider: Provider,
    key: Option<String>,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(provider_id: &str) -> Result<Self> {
        let all = providers()?;
        let provider = all
            .get(provider_id)
            .cloned()
            .ok_or_else(|| anyhow!("no provider called {provider_id}"))?;
        let key = match &provider.key_env {
            Some(var) => Some(key_for(provider_id, var).ok_or_else(|| {
                anyhow!(
                    "No key for {} yet. Add one in OpenAs ▸ Model.",
                    provider.name
                )
            })?),
            None => None,
        };
        Ok(Self::with_key(provider, key))
    }

    /// For trying a key before it is saved.
    pub fn trial(provider_id: &str, key: Option<String>) -> Result<Self> {
        let provider = providers()?
            .get(provider_id)
            .cloned()
            .ok_or_else(|| anyhow!("no provider called {provider_id}"))?;
        Ok(Self::with_key(provider, key))
    }

    fn with_key(provider: Provider, key: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(900)))
            .http_status_as_error(false)
            .build()
            .into();
        Client {
            provider,
            key,
            agent,
        }
    }

    /// One tiny request. Judged by the HTTP status, not the text: a thinking
    /// model may send no text at all for so few tokens.
    pub fn test(&self, model: &str) -> Test {
        let claude = self.provider.api.as_deref() == Some("anthropic");
        let url = format!(
            "{}/{}",
            self.provider.base_url.trim_end_matches('/'),
            if claude {
                "messages"
            } else {
                "chat/completions"
            }
        );
        let body = json!({ "model": model, "messages": [{ "role": "user", "content": "Say ok." }], "max_tokens": 16 });
        let mut req = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json");
        if let Some(k) = &self.key {
            req = if claude {
                req.header("x-api-key", k)
                    .header("anthropic-version", "2023-06-01")
            } else {
                req.header("Authorization", &format!("Bearer {k}"))
            };
        }
        let name = &self.provider.name;
        let mut resp = match req.send_json(&body) {
            Ok(r) => r,
            Err(_) => {
                return Test::Refused(format!(
                    "Couldn't reach {name}. Check the address and your connection."
                ))
            }
        };
        let status = resp.status().as_u16();
        let v: Value = resp.body_mut().read_json().unwrap_or(Value::Null);
        let said = v
            .pointer("/error/message")
            .or_else(|| v.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(160)
            .collect::<String>();
        match status {
            200 => Test::Ok,
            401 | 403 => Test::Refused(format!("{name} didn't accept that key.")),
            402 => Test::Refused(format!("{name} says the account has no credit left.")),
            404 => Test::Refused(format!("{name} doesn't have a model called {model}.")),
            429 => Test::Refused(format!(
                "{name} is busy or rate-limited. Try again in a minute."
            )),
            _ => Test::Refused(format!("{name} answered {status}. {said}")),
        }
    }

    /// A chat completion streamed piece by piece, so the person can watch it
    /// and stop it. `on` returning an error stops the request. A provider
    /// that ignores `stream` and answers in one piece works too.
    /// With `OPENAS_RECORD=<file>`, the raw stream is saved (for replaying
    /// in tests without paying for a model).
    pub fn chat_stream(
        &self,
        model: &str,
        messages: &[Message],
        max_tokens: u32,
        on: &mut dyn FnMut(Delta) -> Result<()>,
    ) -> Result<Reply> {
        use std::io::{BufRead, BufReader, Write};
        let url = format!(
            "{}/chat/completions",
            self.provider.base_url.trim_end_matches('/')
        );
        let body = json!({ "model": model, "messages": messages, "max_tokens": max_tokens, "temperature": 0.4,
                           "stream": true, "stream_options": { "include_usage": true } });
        let mut req = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json");
        if let Some(k) = &self.key {
            req = req.header("Authorization", &format!("Bearer {k}"));
        }
        let mut resp = req
            .send_json(&body)
            .with_context(|| format!("calling {}", self.provider.name))?;
        let status = resp.status().as_u16();
        let streaming = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.contains("event-stream"));
        if status != 200 || !streaming {
            let v: Value = resp.body_mut().read_json().unwrap_or(Value::Null);
            if status != 200 {
                let msg = v
                    .pointer("/error/message")
                    .or_else(|| v.get("error"))
                    .cloned()
                    .unwrap_or(v.clone());
                bail!("{} said {status}: {}", self.provider.name, msg);
            }
            let reply = Self::reply_of(&v)?;
            on(Delta::Text(&reply.text))?;
            return Ok(reply);
        }
        let mut record = std::env::var("OPENAS_RECORD")
            .ok()
            .and_then(|p| std::fs::File::create(p).ok());
        let mut text = String::new();
        let mut finish = String::new();
        let (mut tokens, mut cost) = (0u64, None);
        for line in BufReader::new(resp.body_mut().as_reader()).lines() {
            let line = line.context("reading the model's reply")?;
            if let Some(f) = record.as_mut() {
                let _ = writeln!(f, "{line}");
            }
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
                    "{} stopped: {}",
                    self.provider.name,
                    e.get("message").and_then(Value::as_str).unwrap_or("error")
                );
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                tokens = u
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(tokens);
                cost = u.get("cost").and_then(Value::as_f64).or(cost);
            }
            let Some(c) = v.pointer("/choices/0") else {
                continue;
            };
            if let Some(f) = c.get("finish_reason").and_then(Value::as_str) {
                finish = f.to_string();
            }
            let d = c.get("delta").unwrap_or(&Value::Null);
            if let Some(r) = d
                .get("reasoning_content")
                .or_else(|| d.get("reasoning"))
                .and_then(Value::as_str)
            {
                on(Delta::Thinking(r))?;
            }
            if let Some(t) = d
                .get("content")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                text.push_str(t);
                on(Delta::Text(t))?;
            }
        }
        Ok(Reply {
            text,
            finish,
            output_tokens: tokens,
            cost,
        })
    }

    fn reply_of(v: &Value) -> Result<Reply> {
        let choice = v
            .pointer("/choices/0")
            .ok_or_else(|| anyhow!("no answer in the reply"))?;
        Ok(Reply {
            text: choice
                .pointer("/message/content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            finish: choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            output_tokens: v
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cost: v.pointer("/usage/cost").and_then(Value::as_f64),
        })
    }

    pub fn chat(&self, model: &str, messages: &[Message], max_tokens: u32) -> Result<Reply> {
        let url = format!(
            "{}/chat/completions",
            self.provider.base_url.trim_end_matches('/')
        );
        let body = json!({ "model": model, "messages": messages, "max_tokens": max_tokens, "temperature": 0.4 });
        let mut req = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json");
        if let Some(k) = &self.key {
            req = req.header("Authorization", &format!("Bearer {k}"));
        }
        let mut resp = req
            .send_json(&body)
            .with_context(|| format!("calling {}", self.provider.name))?;
        let status = resp.status().as_u16();
        let v: Value = resp.body_mut().read_json().unwrap_or(Value::Null);
        if status != 200 {
            let msg = v
                .pointer("/error/message")
                .or_else(|| v.get("error"))
                .cloned()
                .unwrap_or(v.clone());
            bail!("{} said {status}: {}", self.provider.name, msg);
        }
        let choice = v
            .pointer("/choices/0")
            .ok_or_else(|| anyhow!("no answer in the reply"))?;
        Ok(Reply {
            text: choice
                .pointer("/message/content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            finish: choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            output_tokens: v
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cost: v.pointer("/usage/cost").and_then(Value::as_f64),
        })
    }
}
