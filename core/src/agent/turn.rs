use super::*;

// ---- The loop -----------------------------------------------------------------

/// Builds (or changes) the app in `ws` until the model calls finish.
/// `job` names the ledger line (one per app).
pub fn run(
    ws: &Path,
    state_dir: &Path,
    request: &str,
    job: &str,
    ev: &make::Events,
) -> Result<Finished> {
    run_shown(ws, state_dir, request, request, job, ev)
}

/// `run`, with `shown` as what the person asked, in the conversation, when
/// the model is told more than that (carrying on after a stop).
pub fn run_shown(
    ws: &Path,
    state_dir: &Path,
    request: &str,
    shown: &str,
    job: &str,
    ev: &make::Events,
) -> Result<Finished> {
    let existing = ws.join("app/index.html").exists();
    let live = Live::new(state_dir, shown, if existing { "change" } else { "new" });
    // Which model really builds it, shown in the column.
    live.set("model", json!(model()));
    let r = run_live(ws, request, job, ev, &live);
    match &r {
        Ok(f) => {
            let st = live
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let started = st["started"].as_u64().unwrap_or(now());
            live.set("phase", json!("done"));
            live.set("summary", json!(f.summary));
            live.set("unfinished", json!(f.unfinished));
            live.set("name", json!(f.name));
            live.set(
                "minutes",
                json!(((now() - started) as f64 / 60.0).round().max(1.0)),
            );
            let parts = st["plan"]["parts"].as_array().map(|a| a.len()).unwrap_or(0);
            live.set_now("part", json!(parts + 1));
            // The app's story: each request and what came of it.
            let entry = json!({ "kind": st["kind"], "request": request, "summary": f.summary, "name": f.name, "t": now(),
                                "minutes": live.get("minutes"), "steps": st["step"], "usd": live.get("usd") });
            use std::io::Write;
            if let Ok(mut o) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(state_dir.join("story.jsonl"))
            {
                let _ = writeln!(o, "{entry}");
            }
        }
        Err(e) => {
            let stopped = e.to_string().starts_with("Stopped");
            live.set("error", json!(e.to_string()));
            live.set_now("phase", json!(if stopped { "stopped" } else { "failed" }));
        }
    }
    r
}

/// Claude, or an open model through an OpenAI-compatible endpoint
/// (OPENAS_AGENT_BASE, default the Hugging Face router; the key named by
/// OPENAS_AGENT_KEY in the Keychain, default "huggingface").
pub(super) enum Backend {
    Claude(Claude),
    Open(crate::oai::Oai),
}

impl Backend {
    /// A model running on this Mac: its price is zero.
    fn is_free(&self) -> bool {
        matches!(self, Backend::Open(o) if o.price == (0.0, 0.0))
    }

    fn for_model(m: &str) -> Result<Self> {
        if m.starts_with("claude-") {
            let key = crate::model::key_for("anthropic", "ANTHROPIC_API_KEY")
                .ok_or_else(|| anyhow!("No Anthropic key yet. Choose a model in OpenAs first."))?;
            return Ok(Backend::Claude(Claude::new(key)));
        }
        // Scripts may point anywhere; otherwise the provider the person chose.
        if let Ok(base) = std::env::var("OPENAS_AGENT_BASE") {
            let name = std::env::var("OPENAS_AGENT_KEY").unwrap_or_else(|_| "huggingface".into());
            let key = keys::get(&name).ok_or_else(|| anyhow!("no {name} key in the Keychain"))?;
            return Ok(Backend::Open(crate::oai::Oai::new(base, key, m)));
        }
        let id = crate::model::settings().provider;
        let p = crate::model::providers()?
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow!("no provider called {id}"))?;
        let key = match &p.key_env {
            Some(var) => crate::model::key_for(&id, var)
                .ok_or_else(|| anyhow!("No {} key yet. Choose a model in OpenAs first.", p.name))?,
            None => String::new(),
        };
        Ok(Backend::Open(crate::oai::Oai::new(
            p.base_url.clone(),
            key,
            m,
        )))
    }
    fn turn(&self, body: Value, on: &mut dyn FnMut(Ev) -> Result<()>) -> Result<claude::Turn> {
        match self {
            Backend::Claude(c) => c.turn(body, on),
            Backend::Open(o) => o.turn(body, on),
        }
    }
    fn cost(&self, m: &str, u: &Value) -> f64 {
        match self {
            Backend::Claude(_) => claude::cost(m, u),
            Backend::Open(o) => {
                let _ = m;
                crate::oai::cost(o.price, u)
            }
        }
    }
}

pub(super) fn run_live(
    ws: &Path,
    request: &str,
    job: &str,
    ev: &make::Events,
    live: &Live,
) -> Result<Finished> {
    let (cap, app_cap) = limits();
    let client = Backend::for_model(&model())?;
    // A model on this Mac costs nothing, so no limit applies to it.
    let free = client.is_free();
    std::fs::create_dir_all(ws.join("app"))?;
    let log_path = ws.join("agent.jsonl");
    let log = |v: &Value| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = writeln!(f, "{v}");
        }
    };
    let existing = ws.join("app/index.html").exists();
    let first = if existing {
        format!("This app already exists (use `list` and `view` to see it). The person asks for this change:\n\n{request}\n\nMake the change carefully, keep everything else working and keep the person's saved data (same store keys). Check it, then call finish (keep the name unless the person asked otherwise).")
    } else {
        format!("The person describes the app they want:\n\n{request}")
    };
    let mut messages: Vec<Value> =
        vec![json!({ "role": "user", "content": [{ "type": "text", "text": first }] })];
    log(&json!({ "request": request, "model": model() }));
    let mut finished: Option<Finished> = None;
    let mut job_spent = 0.0;
    let stopped = || (ev.stopped)() || live.stopped();
    let mut usd = 0.0;
    // Progress: the last failing call and how often it repeated, and steps
    // since a file last changed.
    let (mut last_fail, mut same_fail, mut idle) = (String::new(), 0usize, 0usize);
    let new_app = !ws.join("app/index.html").exists();
    for turn in 1..=MAX_TURNS {
        if stopped() {
            bail!("Stopped.");
        }
        live.set_now("step", json!(turn));
        let month = spent_this_month();
        if !free && month >= cap {
            bail!("{LIMIT_MARK} You've reached this month's spending limit (${month:.2} of ${cap:.2}). Raise it in OpenAs: click the model name under the text box, then Spending.");
        }
        if !free && job_spent >= app_cap {
            bail!("{LIMIT_MARK} This app reached its spending limit (${job_spent:.2} of ${app_cap:.2}). Raise it in OpenAs: click the model name under the text box, then Spending.");
        }
        let mut tools = tool_schemas();
        // Big file contents stream as they're written, not in one burst at the end.
        for t in tools.as_array_mut().into_iter().flatten() {
            t["eager_input_streaming"] = json!(true);
        }
        let body = json!({
            "model": model(),
            "max_tokens": 64000,
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": "high" },
            "cache_control": { "type": "ephemeral" },
            "system": [{ "type": "text", "text": builder_prompt(), "cache_control": { "type": "ephemeral" } }],
            "tools": tools,
            "messages": messages,
        });
        let mut thought = String::new();
        let mut last_say = std::time::Instant::now() - std::time::Duration::from_secs(5);
        let doing_re = &*DOING_RE;
        let mut said = String::new();
        let t = client.turn(body, &mut |e| {
            match e {
                Ev::Thinking(t) => {
                    thought.push_str(t);
                    let tail: String = thought
                        .chars()
                        .rev()
                        .take(1500)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    (ev.thought)(
                        &tail
                            .chars()
                            .rev()
                            .take(400)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect::<String>(),
                    );
                    live.set("thinking", json!(tail));
                }
                Ev::Text(t) => {
                    said.push_str(t);
                    live.set("said", json!(said));
                }
                Ev::Tool(n) => {
                    (ev.say)(&format!("Step {turn}: {n}"));
                    live.set_now("tool", json!(n));
                    live.set("kb", json!(0));
                }
                Ev::Writing(n, head) => {
                    if let Some(c) = doing_re.captures(head) {
                        live.set("doing", json!(c[1].replace("\\\"", "\"")));
                    }
                    live.set("kb", json!(n / 1024));
                    if last_say.elapsed().as_millis() > 1500 {
                        (ev.say)(&format!("Step {turn}: writing… {} KB", n / 1024));
                        last_say = std::time::Instant::now();
                    }
                }
            }
            if stopped() {
                bail!("Stopped.");
            }
            Ok(())
        })?;
        let dollars = client.cost(&model(), &t.usage);
        job_spent = add_spend(job, dollars, &t.usage)?;
        usd += dollars;
        live.set("usd", json!((usd * 100.0).round() / 100.0));
        (ev.say)(&format!(
            "Step {turn} done · this app ${job_spent:.2} · this month ${:.2}",
            spent_this_month()
        ));
        log(
            &json!({ "turn": turn, "stop": t.stop_reason, "usd": dollars, "usage": t.usage,
                     "content": t.content.iter().map(|b| { let mut b = b.clone(); if b.get("signature").is_some() { b["signature"] = json!("…"); } b }).collect::<Vec<_>>() }),
        );
        match t.stop_reason.as_str() {
            "refusal" => bail!("the model declined this request"),
            "pause_turn" => {
                messages.push(json!({ "role": "assistant", "content": t.content }));
                continue;
            }
            _ => {}
        }
        let calls: Vec<Value> = t
            .content
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .cloned()
            .collect();
        messages.push(json!({ "role": "assistant", "content": t.content }));
        if calls.is_empty() {
            if finished.is_some() {
                break;
            }
            if t.stop_reason == "max_tokens" {
                messages.push(json!({ "role": "user", "content": [{ "type": "text", "text": "You reached the output limit. Continue, in smaller pieces." }] }));
            } else {
                messages.push(json!({ "role": "user", "content": [{ "type": "text", "text": "Continue working with the tools until the app is done, then call finish." }] }));
            }
            continue;
        }
        let mut results = vec![];
        let mut changed = false;
        for c in &calls {
            let id = c.get("id").cloned().unwrap_or(Value::Null);
            let name = c.get("name").and_then(Value::as_str).unwrap_or("");
            let input = c.get("input").cloned().unwrap_or(json!({}));
            let r = run_tool(ws, name, &input, &mut finished, ev, live);
            if r.is_ok() && matches!(name, "create" | "replace" | "insert") {
                changed = true;
                let part = input
                    .get("part")
                    .and_then(Value::as_u64)
                    .or(live.get("part").as_u64())
                    .unwrap_or(0);
                live.note_step(
                    part,
                    input.get("doing").and_then(Value::as_str),
                    input.get("path").and_then(Value::as_str),
                );
                live.bump("files_seq");
                live.flush(true);
            }
            match r {
                Ok(content) => results
                    .push(json!({ "type": "tool_result", "tool_use_id": id, "content": content })),
                Err(e) => {
                    let key = format!("{name}{input}");
                    same_fail = if key == last_fail { same_fail + 1 } else { 1 };
                    last_fail = key;
                    log(&json!({ "tool_error": name, "message": e.to_string() }));
                    results.push(json!({ "type": "tool_result", "tool_use_id": id, "content": e.to_string(), "is_error": true }))
                }
            }
        }
        idle = if changed { 0 } else { idle + 1 };
        if changed {
            same_fail = 0;
        }
        if finished.is_none() {
            let stuck = if same_fail >= SAME_FAIL_STOP {
                Some("It kept repeating a step that didn't work.")
            } else if idle >= IDLE_STOP {
                Some("It went a long time without changing anything.")
            } else {
                None
            };
            if let Some(why) = stuck {
                log(&json!({ "stuck": why, "turn": turn }));
                return keep_what_was_made(ws, new_app, why);
            }
            if same_fail == SAME_FAIL_NUDGE {
                results.push(json!({ "type": "text", "text": "You have sent the same call several times and it failed each time. Don't send it again: view the file to see what is really there, or take another approach." }));
            }
            if idle == IDLE_NUDGE {
                results.push(json!({ "type": "text", "text": format!("{IDLE_NUDGE} steps without changing a file. If the app works, call finish now; otherwise fix the one thing that is broken.") }));
            }
        }
        if turn == MAX_TURNS - WRAP_UP && finished.is_none() {
            let icon = if ws.join("icon.svg").exists() {
                ""
            } else {
                " Create icon.svg first;"
            };
            results.push(json!({ "type": "text", "text": format!("You have {WRAP_UP} steps left.{icon} Stop polishing: fix only what is broken, then call finish.") }));
        }
        // Notes the person typed while it worked go in with the results.
        for n in live.new_notes() {
            results.push(json!({ "type": "text", "text": format!("The person added a note while you work: {n}\nTake it into account from now on.") }));
        }
        messages.push(json!({ "role": "user", "content": results }));
        if finished.is_some() && t.stop_reason != "max_tokens" {
            // Let the model see "Done." only if it wants another turn; a
            // finish is final.
            break;
        }
    }
    match finished {
        Some(f) => Ok(f),
        None => keep_what_was_made(ws, new_app, "It reached the step limit before finishing."),
    }
}

/// The builder stopped without calling finish. A new app that has its page
/// is still offered, marked as unfinished, so the work isn't lost; a change
/// to an existing app is never applied half done.
pub(super) fn keep_what_was_made(ws: &Path, new_app: bool, why: &str) -> Result<Finished> {
    let page = std::fs::read_to_string(ws.join("app/index.html")).unwrap_or_default();
    if !new_app || page.trim().is_empty() {
        bail!("{why} It stopped before finishing.");
    }
    // The page's own title is the best name it gave the app.
    let title = page
        .split_once("<title>")
        .and_then(|(_, t)| t.split_once("</title>"))
        .map(|(t, _)| t.trim().to_string())
        .filter(|t| !t.is_empty() && t.len() <= 40);
    Ok(Finished {
        name: title.unwrap_or_else(|| "New App".into()),
        summary: String::new(),
        unfinished: Some(why.to_string()),
    })
}

#[cfg(test)]
mod keep_tests {
    use super::keep_what_was_made;

    fn draft(page: Option<&str>) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "oa-keep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(d.join("app")).unwrap();
        if let Some(p) = page {
            std::fs::write(d.join("app/index.html"), p).unwrap();
        }
        d
    }

    #[test]
    fn a_new_app_with_a_page_is_kept_under_its_title() {
        let d = draft(Some(
            "<html><head><title>Mood Journal</title></head></html>",
        ));
        let f =
            keep_what_was_made(&d, true, "It reached the step limit before finishing.").unwrap();
        assert_eq!(f.name, "Mood Journal");
        assert!(f.unfinished.is_some());
    }

    #[test]
    fn no_page_means_nothing_to_keep() {
        assert!(keep_what_was_made(&draft(None), true, "x").is_err());
    }

    #[test]
    fn a_half_done_change_is_never_kept() {
        assert!(keep_what_was_made(&draft(Some("<title>A</title>")), false, "x").is_err());
    }
}
