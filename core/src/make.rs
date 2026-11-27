//! Making and changing apps: ask the model, take the file out of its reply,
//! load-check it, and give the model one chance to fix what broke.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{self, Client, Message};
use crate::{check, extract, install, paths};

const FORMAT: &str = include_str!("../prompts/format.md");
/// One retry: a fix costs a full new file, and most failures are fixed in one.
const ATTEMPTS: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub model: String,
    pub created: u64,
    #[serde(default)]
    pub installed: Option<PathBuf>,
    /// The installed copy's code hash; the Helper trusts only this exact copy.
    #[serde(default)]
    pub cdhash: Option<String>,
    /// Came from someone else as a shared file: it can never get full access.
    #[serde(default)]
    pub shared: bool,
}

pub struct Spend {
    pub cost: f64,
    pub tokens: u64,
    /// Some provider didn't report a cost, so `cost` is a lower bound.
    pub cost_unknown: bool,
}

impl Spend {
    pub fn zero() -> Self {
        Spend {
            cost: 0.0,
            tokens: 0,
            cost_unknown: false,
        }
    }

    pub fn describe(&self) -> String {
        if self.cost_unknown {
            format!("{} tokens · cost unknown", self.tokens)
        } else {
            format!("{} tokens · ${:.3}", self.tokens, self.cost)
        }
    }
}

/// What a long job tells the person while it runs, and how they stop it.
pub struct Events<'a> {
    /// A short line about what is happening now.
    pub say: &'a dyn Fn(&str),
    /// The page as written so far (to show without running it).
    pub preview: &'a dyn Fn(&str),
    /// True once the person pressed Stop (or went away).
    pub stopped: &'a dyn Fn() -> bool,
    /// The end of the model's reasoning so far, while it thinks.
    pub thought: &'a dyn Fn(&str),
}

fn no_preview(_: &str) {}
fn never() -> bool {
    false
}

impl<'a> Events<'a> {
    /// Just lines, for the command line.
    pub fn lines(say: &'a dyn Fn(&str)) -> Self {
        Events {
            say,
            preview: &no_preview,
            stopped: &never,
            thought: &no_preview,
        }
    }
}

pub struct Outcome {
    pub info: AppInfo,
    pub folder: PathBuf,
    pub spend: Spend,
    /// Problems still there after the retry. Empty means it passed the check.
    pub problems: Vec<String>,
    /// Something to tell the person even though it worked.
    pub warning: Option<String>,
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() {
        "app".into()
    } else {
        s
    }
}

fn unique_id(name: &str) -> String {
    let base = slug(name);
    let mut id = base.clone();
    let mut n = 2;
    while paths::apps().join(&id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

pub fn read_info(folder: &Path) -> Result<AppInfo> {
    Ok(serde_json::from_str(
        &std::fs::read_to_string(folder.join("app.json")).context("app.json")?,
    )?)
}

pub fn write_info(folder: &Path, info: &AppInfo) -> Result<()> {
    std::fs::write(folder.join("app.json"), serde_json::to_string_pretty(info)?)?;
    Ok(())
}

/// The shape of what the app saved, without its values: keys and types, one
/// example item per list. The user's data never goes to the model.
pub fn data_shape(folder: &Path) -> Option<String> {
    // A field name looks like `lastWatered` or `due_date`. Keys that are data
    // themselves (names, dates, ids) are summarised, never sent.
    fn field_like(k: &str) -> bool {
        k.len() <= 32
            && k.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    fn shape(v: &Value) -> Value {
        match v {
            Value::Object(m) if m.len() > 12 || !m.keys().all(|k| field_like(k)) => {
                let mut o = serde_json::Map::new();
                if let Some(first) = m.values().next() {
                    o.insert("<key>".into(), shape(first));
                }
                o.insert(
                    "…".into(),
                    Value::String(format!("{} keys like this, written by the app", m.len())),
                );
                Value::Object(o)
            }
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, v)| (k.clone(), shape(v))).collect())
            }
            Value::Array(a) => match a.first() {
                Some(first) => Value::Array(vec![
                    shape(first),
                    Value::String(format!("... {} items", a.len())),
                ]),
                None => Value::Array(vec![]),
            },
            Value::String(_) => Value::String("string".into()),
            Value::Number(_) => Value::String("number".into()),
            Value::Bool(_) => Value::String("boolean".into()),
            Value::Null => Value::Null,
        }
    }
    let text = std::fs::read_to_string(folder.join("data/store.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    serde_json::to_string_pretty(&shape(&v)).ok()
}

/// Asks, extracts, writes and checks, with one retry. Returns the svg (if any)
/// and the problems left.
fn produce(
    client: &Client,
    model: &str,
    mut messages: Vec<Message>,
    folder: &Path,
    spend: &mut Spend,
    ev: &Events,
) -> Result<(Option<String>, Vec<String>)> {
    let log = ev.say;
    let mut svg = None;
    let mut problems = vec![];
    for attempt in 1..=ATTEMPTS {
        let fixing = attempt > 1;
        log(if fixing {
            "Fixing it…"
        } else {
            "Starting…"
        });
        let started = std::time::Instant::now();
        let mut last = std::time::Instant::now() - std::time::Duration::from_secs(5);
        let mut text = String::new();
        let mut thought = String::new();
        let mut on = |d: model::Delta| -> Result<()> {
            if (ev.stopped)() {
                bail!("Stopped.");
            }
            let due = last.elapsed().as_millis() >= 700;
            match d {
                model::Delta::Thinking(r) => {
                    thought.push_str(r);
                    if thought.len() > 4000 {
                        let cut = thought.len() - 2000;
                        let cut = (cut..thought.len())
                            .find(|i| thought.is_char_boundary(*i))
                            .unwrap_or(0);
                        thought.drain(..cut);
                    }
                    if due {
                        last = std::time::Instant::now();
                        let what = if fixing {
                            "Working out the fix"
                        } else {
                            "Thinking it through"
                        };
                        log(&format!("{what}… {} s", started.elapsed().as_secs()));
                        let tail = &thought[thought.len().saturating_sub(400)..];
                        let tail = &thought[(thought.len() - tail.len()..)
                            .find(|i| thought.is_char_boundary(*i))
                            .unwrap_or(0)..];
                        (ev.thought)(tail);
                    }
                }
                model::Delta::Text(t) => {
                    text.push_str(t);
                    if due {
                        last = std::time::Instant::now();
                        if let Some(html) = extract::partial_html(&text) {
                            log(&format!(
                                "{}… {} KB",
                                if fixing { "Rewriting it" } else { "Writing it" },
                                html.len() / 1024 + 1
                            ));
                            (ev.preview)(html);
                        }
                    }
                }
            }
            Ok(())
        };
        let reply = client.chat_stream(model, &messages, model::settings().max_tokens, &mut on)?;
        if let Some(html) = extract::partial_html(&reply.text) {
            (ev.preview)(html);
        }
        match reply.cost {
            Some(c) => spend.cost += c,
            None => spend.cost_unknown = true,
        }
        spend.tokens += reply.output_tokens;
        let parts = extract::parts(&reply.text);
        problems.clear();
        if reply.finish == "length" {
            problems.push("The model was cut off before finishing.".into());
        }
        match &parts.html {
            Some(html) => {
                std::fs::create_dir_all(folder.join("app"))?;
                std::fs::write(folder.join("app/index.html"), html)?;
                problems.extend(extract::static_problems(html));
                if problems.is_empty() {
                    log("Checking that it opens…");
                    problems.extend(check::run(folder)?.problems());
                }
            }
            None => problems.push("The reply had no ```html block.".into()),
        }
        if parts.svg.is_some() {
            svg = parts.svg;
        }
        // Wanting the internet isn't a bug to fix: the person decides.
        if problems.is_empty() || !check::wanted_hosts(&problems).is_empty() {
            break;
        }
        if attempt < ATTEMPTS {
            log(&format!("Found a problem: {}", problems[0]));
        }
        if reply.finish != "length" && parts.html.is_some() {
            messages.push(Message::new("assistant", reply.text));
            messages.push(Message::new(
                "user",
                format!("The app has these problems:\n- {}\nReply with the complete corrected file, in the same format.", problems.join("\n- ")),
            ));
        }
    }
    Ok((svg, problems))
}

/// A built app that isn't an app yet: it lives in a hidden folder until it
/// passes and is installed, so a failed build never shows up in the list.
pub struct Draft {
    pub dir: PathBuf,
    pub name: String,
    pub svg: String,
    pub problems: Vec<String>,
    pub spend: Spend,
}

pub fn build_draft(prompt: &str, ev: &Events) -> Result<Draft> {
    let settings = model::settings();
    let client = Client::new(&settings.provider)?;
    std::fs::create_dir_all(paths::apps())?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = paths::apps().join(format!(".draft-{nanos}"));
    std::fs::create_dir_all(&dir)?;
    let mut spend = Spend::zero();
    let messages = vec![Message::new("system", FORMAT), Message::new("user", prompt)];
    let made = produce(
        &client,
        &settings.build_model,
        messages,
        &dir,
        &mut spend,
        ev,
    );
    let (svg, problems) = match made {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
    };
    let html = std::fs::read_to_string(dir.join("app/index.html")).unwrap_or_default();
    let name = extract::title(&html).unwrap_or_else(|| "New App".into());
    let svg = svg.unwrap_or_else(|| install::fallback_icon(&slug(&name)));
    std::fs::write(dir.join("prompt.txt"), prompt)?;
    std::fs::write(dir.join("icon.svg"), &svg)?;
    std::fs::write(
        dir.join("draft.json"),
        serde_json::to_string(
            &serde_json::json!({ "provider": settings.provider, "model": settings.build_model }),
        )?,
    )?;
    Ok(Draft {
        dir,
        name,
        svg,
        problems,
        spend,
    })
}

/// The hidden folder of a draft, by the id the window was given.
pub fn draft_dir(id: &str) -> Result<PathBuf> {
    if !id.starts_with(".draft-") || id.contains('/') || id.contains("..") {
        bail!("no such draft");
    }
    let dir = paths::apps().join(id);
    if !dir.is_dir() {
        bail!("that draft is gone");
    }
    Ok(dir)
}

/// A name no other app's .app already has ("Habit Tracker 2"), so installing
/// one app never replaces another. An app's own earlier copy doesn't count.
fn free_name(name: &str, folder: &Path) -> String {
    let taken = |n: &str| {
        let app = paths::install_dir().join(format!("{n}.app"));
        app.exists() && !install::owned_by(&app, folder)
    };
    let name = &install::clean_name(name);
    let mut n = name.to_string();
    let mut i = 2;
    while taken(&n) {
        n = format!("{name} {i}");
        i += 1;
    }
    n
}

/// Makes a draft a real app: its own folder, installed, with this name.
pub fn install_draft(dir: &Path, name: &str) -> Result<AppInfo> {
    let name = &install::clean_name(name);
    let made: Value = std::fs::read_to_string(dir.join("draft.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let id = unique_id(name);
    let folder = paths::apps().join(&id);
    let name = &free_name(name, &folder);
    std::fs::rename(dir, &folder)?;
    let _ = std::fs::remove_file(folder.join("draft.json"));
    let svg = std::fs::read_to_string(folder.join("icon.svg"))
        .unwrap_or_else(|_| install::fallback_icon(&id));
    let s = |k: &str| {
        made.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let mut info = AppInfo {
        id: id.clone(),
        name: name.into(),
        provider: s("provider"),
        model: s("model"),
        created: now(),
        installed: None,
        cdhash: None,
        shared: false,
    };
    let app = install::install(&folder, &id, name, &svg)?;
    info.cdhash = install::cdhash(&app);
    info.installed = Some(app);
    write_info(&folder, &info)?;
    Ok(info)
}

/// The person allowed this draft to use the internet: note it beside the
/// app (the model never writes this) and check it again with access.
pub fn allow_network_draft(dir: &Path) -> Result<Vec<String>> {
    std::fs::write(dir.join("permissions.json"), r#"{"network":true}"#)?;
    check_folder(dir)
}

/// Leftovers nobody installed (a crash, a closed window) go after a day. A
/// draft from the step-wise builder (it has making.json) is on OpenAs's shelf,
/// finished or not, and stays until the person saves or discards it.
pub fn sweep_drafts() {
    for e in std::fs::read_dir(paths::apps())
        .into_iter()
        .flatten()
        .flatten()
    {
        let n = e.file_name().to_string_lossy().into_owned();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|d| d.as_secs() > 86_400);
        let on_shelf = n.starts_with(".draft-") && e.path().join("making.json").exists();
        if (n.starts_with(".draft-") || n.starts_with(".stage-") || n.starts_with(".new-"))
            && old
            && !on_shelf
        {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// The command line's one-step version: build, and install if it passed.
pub fn new_app(prompt: &str, ev: &Events) -> Result<Outcome> {
    let d = build_draft(prompt, ev)?;
    let settings = model::settings();
    if !d.problems.is_empty() {
        let _ = std::fs::remove_dir_all(&d.dir);
        let info = AppInfo {
            id: String::new(),
            name: d.name,
            provider: settings.provider,
            model: settings.build_model,
            created: now(),
            installed: None,
            cdhash: None,
            shared: false,
        };
        return Ok(Outcome {
            info,
            folder: PathBuf::new(),
            spend: d.spend,
            problems: d.problems,
            warning: None,
        });
    }
    (ev.say)("Installing…");
    let info = install_draft(&d.dir, &d.name)?;
    let folder = paths::apps().join(&info.id);
    Ok(Outcome {
        info,
        folder,
        spend: d.spend,
        problems: vec![],
        warning: None,
    })
}

/// A new name everywhere the app shows: its .app, the Dock, the menu bar.
pub fn rename(app: &str, name: &str) -> Result<AppInfo> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name.contains(':') {
        bail!("Choose a name without / or :");
    }
    let folder = find(app)?;
    let mut info = read_info(&folder)?;
    if info.name == name {
        return Ok(info);
    }
    let name = &free_name(name, &folder);
    let svg = std::fs::read_to_string(folder.join("icon.svg"))
        .unwrap_or_else(|_| install::fallback_icon(&info.id));
    let path = install::install(&folder, &info.id, name, &svg)?;
    if let Some(old) = info.installed.take().filter(|o| {
        *o != path
            && o.extension().is_some_and(|e| e == "app")
            && o.starts_with(paths::install_dir())
    }) {
        let _ = std::fs::remove_dir_all(old);
    }
    info.name = name.into();
    info.cdhash = install::cdhash(&path);
    info.installed = Some(path);
    write_info(&folder, &info)?;
    Ok(info)
}

fn history(folder: &Path) -> PathBuf {
    folder.join("history")
}

fn snapshots(folder: &Path) -> Vec<u64> {
    let mut v: Vec<u64> = std::fs::read_dir(history(folder))
        .map(|d| {
            d.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// How many earlier versions an app keeps.
const KEEP_VERSIONS: usize = 20;

/// What made the version that is live now ("The first version", or the
/// request that changed it).
fn current_label(folder: &Path) -> String {
    std::fs::read_to_string(folder.join("version.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("made_by").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "The first version".into())
}

pub fn set_label(folder: &Path, label: &str) -> Result<()> {
    std::fs::write(
        folder.join("version.json"),
        serde_json::json!({ "made_by": label, "t": now() }).to_string(),
    )?;
    Ok(())
}

/// Keeps the current code (for Undo and Versions), what made it, and a copy
/// of the data at that moment (for recovery by hand; going back puts back
/// code only, so nothing entered since is lost).
pub fn snapshot(folder: &Path) -> Result<()> {
    let n = snapshots(folder).last().map(|n| n + 1).unwrap_or(1);
    let dir = history(folder).join(n.to_string());
    std::fs::create_dir_all(&dir)?;
    if multi_file(folder) {
        copy_tree(&folder.join("app"), &dir.join("app"))?;
    } else {
        std::fs::copy(folder.join("app/index.html"), dir.join("index.html"))?;
    }
    let data = folder.join("data/store.json");
    if data.exists() {
        std::fs::copy(data, dir.join("store.json"))?;
    }
    // When the version being kept was made (not when it was replaced).
    let made = std::fs::read_to_string(folder.join("version.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("t").and_then(Value::as_u64))
        .or_else(|| read_info(folder).ok().map(|i| i.created))
        .unwrap_or_else(now);
    std::fs::write(
        dir.join("meta.json"),
        serde_json::json!({ "made_by": current_label(folder), "t": made }).to_string(),
    )?;
    let all = snapshots(folder);
    for old in all.iter().take(all.len().saturating_sub(KEEP_VERSIONS)) {
        let _ = std::fs::remove_dir_all(history(folder).join(old.to_string()));
    }
    Ok(())
}

/// The app's versions, newest first: the live one, then earlier ones.
pub fn versions(app: &str) -> Result<Vec<Value>> {
    let folder = find(app)?;
    let t = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };
    let mut out = vec![
        serde_json::json!({ "version": 0, "label": current_label(&folder), "t": t(&folder.join("app/index.html")), "live": true }),
    ];
    for n in snapshots(&folder).into_iter().rev() {
        let dir = history(&folder).join(n.to_string());
        let meta: Value = std::fs::read_to_string(dir.join("meta.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let label = meta
            .get("made_by")
            .and_then(Value::as_str)
            .unwrap_or("An earlier version")
            .to_string();
        out.push(serde_json::json!({ "version": n, "label": label, "t": meta.get("t").and_then(Value::as_u64).unwrap_or(t(&dir)), "live": false }));
    }
    Ok(out)
}

/// Goes back to an earlier version. The version left behind is kept too, so
/// this can be undone like any change.
pub fn restore(app: &str, version: u64) -> Result<()> {
    let folder = find(app)?;
    let dir = history(&folder).join(version.to_string());
    if !dir.join("app").is_dir() && !dir.join("index.html").exists() {
        bail!("that version is gone");
    }
    let label: String = std::fs::read_to_string(dir.join("meta.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("made_by").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "An earlier version".into());
    // A copy first: keeping the current version prunes the oldest one, which
    // may be the one being restored.
    let aside = history(&folder).join(format!(".restoring-{version}"));
    let _ = std::fs::remove_dir_all(&aside);
    copy_tree(&dir, &aside)?;
    let done = snapshot(&folder).and_then(|_| put_back(&folder, &aside));
    let _ = std::fs::remove_dir_all(&aside);
    done?;
    set_label(
        &folder,
        &format!("Back to: {}", label.trim_start_matches("Back to: ")),
    )
}

/// An app made in steps has several files; one made in one go has index.html only.
fn multi_file(folder: &Path) -> bool {
    std::fs::read_dir(folder.join("app"))
        .map(|d| d.flatten().any(|e| e.file_name() != "index.html"))
        .unwrap_or(false)
}

pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// Puts a kept version's files back as the live app: a whole folder swapped
/// in at once, or the single page written whole.
fn put_back(folder: &Path, dir: &Path) -> Result<()> {
    if dir.join("app").is_dir() {
        let new = folder.join(".app-new");
        let old = folder.join(".app-old");
        let _ = std::fs::remove_dir_all(&new);
        let _ = std::fs::remove_dir_all(&old);
        copy_tree(&dir.join("app"), &new)?;
        std::fs::rename(folder.join("app"), &old)?;
        std::fs::rename(&new, folder.join("app"))?;
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    } else {
        write_atomic(
            &folder.join("app/index.html"),
            &std::fs::read_to_string(dir.join("index.html"))?,
        )
    }
}

/// Writes a file whole or not at all, so an open window never loads half of it.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A private place to try a new version: the new file and a copy of the data,
/// so the load check sees real data while the live app (maybe open) is
/// untouched. Removed when dropped, also when a model call fails midway.
struct Stage(PathBuf);

impl Stage {
    fn new(folder: &Path) -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let id = folder
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dir = paths::apps().join(format!(".stage-{id}-{nanos}"));
        std::fs::create_dir_all(dir.join("app"))?;
        std::fs::create_dir_all(dir.join("data"))?;
        let data = folder.join("data/store.json");
        if data.exists() {
            std::fs::copy(data, dir.join("data/store.json"))?;
        }
        // Checked under the app's own rules (internet on or off).
        if folder.join("permissions.json").exists() {
            std::fs::copy(
                folder.join("permissions.json"),
                dir.join("permissions.json"),
            )?;
        }
        Ok(Stage(dir))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Puts a checked version live: snapshot first, then swap the file in whole.
/// `label` says what made it (the person's request).
fn go_live(folder: &Path, stage: &Stage, label: &str) -> Result<()> {
    let html = std::fs::read_to_string(stage.0.join("app/index.html"))?;
    snapshot(folder)?;
    write_atomic(&folder.join("app/index.html"), &html)?;
    set_label(folder, label)
}

/// Keys of the saved data (top level).
fn data_keys(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect()))
        .unwrap_or_default()
}

/// A change that starts saving under new names may leave old entries
/// unread. Only what the app saves as it opens can be seen, so the note
/// says that much and no more.
fn data_warning(folder: &Path, stage: &Stage) -> Option<String> {
    let before = data_keys(&folder.join("data/store.json"));
    if before.is_empty() {
        return None;
    }
    let after: Vec<String> = std::fs::read_to_string(stage.0.join(".store-keys.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())?;
    let new: Vec<&String> = after.iter().filter(|k| !before.contains(k)).collect();
    (!new.is_empty()).then(|| {
        let names = new.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ");
        format!("This version started saving under a new name ({names}) when it opened. If something you entered is missing, use Undo Last Change.")
    })
}

pub fn find(app: &str) -> Result<PathBuf> {
    let direct = paths::apps().join(app);
    if direct.join("app.json").exists() {
        return Ok(direct);
    }
    let want = app.to_lowercase();
    for e in std::fs::read_dir(paths::apps())?.flatten() {
        if let Ok(info) = read_info(&e.path()) {
            if info.name.to_lowercase() == want {
                return Ok(e.path());
            }
        }
    }
    bail!("no app called {app}")
}

pub fn change(app: &str, request: &str, ev: &Events) -> Result<Outcome> {
    let log = ev.say;
    let folder = find(app)?;
    let info = read_info(&folder)?;
    let settings = model::settings();
    let client = Client::new(&settings.provider)?;
    let current = std::fs::read_to_string(folder.join("app/index.html"))?;
    let shape = data_shape(&folder)
        .map(|s| format!("\n\nIt has saved data with this shape (values hidden). Keep reading and writing the same keys so nothing is lost:\n```json\n{s}\n```"))
        .unwrap_or_default();
    let messages = vec![
        Message::new("system", FORMAT),
        Message::new(
            "user",
            format!("Here is the app as it is now:\n```html\n{current}\n```{shape}\n\nChange it: {request}\n\nReply with the complete new file only, in one ```html block (no icon: the app keeps its icon)."),
        ),
    ];
    let mut spend = Spend::zero();
    // Tried in a stage; the live app only ever sees a version that passed.
    // The icon stays: it is in the Dock and the user knows the app by it.
    let stage = Stage::new(&folder)?;
    let (_svg, mut problems) = produce(
        &client,
        &settings.build_model,
        messages,
        &stage.0,
        &mut spend,
        ev,
    )?;
    let hosts = check::wanted_hosts(&problems);
    if !hosts.is_empty() {
        // Only the person can let it online, from the app's own menu.
        problems = vec![format!(
            "the change needs the internet (to reach {}). Turn on Allow Internet Access in this app's menu, then ask again.",
            hosts.join(", ")
        )];
    }
    let mut warning = None;
    if problems.is_empty() {
        warning = data_warning(&folder, &stage);
        go_live(&folder, &stage, request.trim())?;
    } else {
        log("Kept the previous version; the change didn't pass the check.");
    }
    Ok(Outcome {
        info,
        folder,
        spend,
        problems,
        warning,
    })
}

/// Puts back the version before the last change.
pub fn undo(app: &str) -> Result<PathBuf> {
    let folder = if Path::new(app).join("app.json").exists() {
        PathBuf::from(app)
    } else {
        find(app)?
    };
    let last = *snapshots(&folder)
        .last()
        .ok_or_else(|| anyhow!("nothing to undo"))?;
    let dir = history(&folder).join(last.to_string());
    put_back(&folder, &dir)?;
    // The version put back keeps its own name and time.
    let meta = std::fs::read_to_string(dir.join("meta.json")).ok();
    std::fs::remove_dir_all(dir)?;
    match meta {
        Some(m) => std::fs::write(folder.join("version.json"), m)?,
        None => {
            let _ = std::fs::remove_file(folder.join("version.json"));
        }
    }
    Ok(folder)
}

pub fn list() -> Result<Vec<AppInfo>> {
    let mut out = vec![];
    for e in std::fs::read_dir(paths::apps())
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Ok(i) = read_info(&e.path()) {
            out.push(i);
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.created));
    Ok(out)
}

/// The source checks and the load check for whatever is now in `folder/app/`.
pub fn check_folder(folder: &Path) -> Result<Vec<String>> {
    let html = std::fs::read_to_string(folder.join("app/index.html")).unwrap_or_default();
    let mut problems = extract::static_problems(&html);
    if problems.is_empty() {
        problems.extend(check::run(folder)?.problems());
    }
    Ok(problems)
}

/// An app someone else wrote (an MCP client's own model): check it and, if it
/// passes, install it. Nothing is kept when it fails, so the writer can fix
/// and try again.
pub fn create_from(html: &str, svg: Option<&str>, source: &str) -> Result<Outcome> {
    std::fs::create_dir_all(paths::apps())?;
    let tmp = paths::apps().join(format!(".new-{}-{}", now(), std::process::id()));
    std::fs::create_dir_all(tmp.join("app"))?;
    std::fs::write(tmp.join("app/index.html"), html)?;
    let problems = check_folder(&tmp)?;
    let name = extract::title(html).unwrap_or_else(|| "New App".into());
    let mut info = AppInfo {
        id: String::new(),
        name: name.clone(),
        provider: source.into(),
        model: source.into(),
        created: now(),
        installed: None,
        cdhash: None,
        shared: false,
    };
    if !problems.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Ok(Outcome {
            info,
            folder: PathBuf::new(),
            spend: Spend::zero(),
            problems,
            warning: None,
        });
    }
    let id = unique_id(&name);
    let folder = paths::apps().join(&id);
    let name = free_name(&name, &folder);
    info.name = name.clone();
    std::fs::rename(&tmp, &folder)?;
    let svg = svg
        .filter(|s| s.contains("<svg"))
        .map(str::to_string)
        .unwrap_or_else(|| install::fallback_icon(&id));
    std::fs::write(folder.join("icon.svg"), &svg)?;
    let app = install::install(&folder, &id, &name, &svg)?;
    info.id = id;
    info.cdhash = install::cdhash(&app);
    info.installed = Some(app);
    write_info(&folder, &info)?;
    Ok(Outcome {
        info,
        folder,
        spend: Spend::zero(),
        problems,
        warning: None,
    })
}

/// A new version written by someone else: snapshot, check, and put the old
/// version back if it fails. An open window reloads on success.
pub fn update_from(app: &str, html: &str) -> Result<Outcome> {
    let folder = find(app)?;
    let info = read_info(&folder)?;
    let stage = Stage::new(&folder)?;
    std::fs::write(stage.0.join("app/index.html"), html)?;
    let problems = check_folder(&stage.0)?;
    let mut warning = None;
    if problems.is_empty() {
        warning = data_warning(&folder, &stage);
        go_live(&folder, &stage, "A new version from your chat")?;
    }
    Ok(Outcome {
        info,
        folder,
        spend: Spend::zero(),
        problems,
        warning,
    })
}

pub fn format_spec() -> &'static str {
    FORMAT
}

/// Copies the current Runner into an app again (after the Runner improves),
/// keeping its name, icon and folder; records the new code hash.
pub fn reinstall(app: &str) -> Result<AppInfo> {
    let folder = find(app)?;
    let mut info = read_info(&folder)?;
    let svg = std::fs::read_to_string(folder.join("icon.svg"))
        .unwrap_or_else(|_| install::fallback_icon(&info.id));
    let path = install::install(&folder, &info.id, &info.name, &svg)?;
    info.cdhash = install::cdhash(&path);
    info.installed = Some(path);
    write_info(&folder, &info)?;
    Ok(info)
}

#[cfg(test)]
mod tests {

    #[test]
    fn versions_are_named_capped_and_reversible() {
        let root = std::env::temp_dir().join(format!("oa-versions-{}", std::process::id()));
        std::env::set_var("OPENAS_ROOT", &root);
        let folder = paths::apps().join("notes");
        std::fs::create_dir_all(folder.join("app")).unwrap();
        let info = AppInfo {
            id: "notes".into(),
            name: "Notes".into(),
            provider: "p".into(),
            model: "m".into(),
            created: 1,
            installed: None,
            cdhash: None,
            shared: false,
        };
        write_info(&folder, &info).unwrap();
        let html = |n: usize| format!("<html>v{n}</html>");
        std::fs::write(folder.join("app/index.html"), html(0)).unwrap();
        for n in 1..=22 {
            snapshot(&folder).unwrap();
            write_atomic(&folder.join("app/index.html"), &html(n)).unwrap();
            set_label(&folder, &format!("change {n}")).unwrap();
        }
        let v = versions("notes").unwrap();
        assert_eq!(v.len(), 1 + KEEP_VERSIONS, "live + 20 kept");
        assert_eq!(v[0]["label"], "change 22");
        assert_eq!(v[1]["label"], "change 21");
        // Go back to the version made by "change 10", then undo that.
        let n = v.iter().find(|x| x["label"] == "change 10").unwrap()["version"]
            .as_u64()
            .unwrap();
        restore("notes", n).unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.join("app/index.html")).unwrap(),
            html(10)
        );
        assert_eq!(versions("notes").unwrap()[0]["label"], "Back to: change 10");
        undo("notes").unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.join("app/index.html")).unwrap(),
            html(22)
        );
        assert_eq!(versions("notes").unwrap()[0]["label"], "change 22");
        // The oldest kept version can be restored too, though keeping the
        // current one prunes it from the list.
        let v = versions("notes").unwrap();
        let oldest = v.last().unwrap();
        let (n, label) = (
            oldest["version"].as_u64().unwrap(),
            oldest["label"].as_str().unwrap().to_string(),
        );
        let want = label
            .trim_start_matches("change ")
            .parse::<usize>()
            .map(html)
            .ok();
        restore("notes", n).unwrap();
        if let Some(want) = want {
            assert_eq!(
                std::fs::read_to_string(folder.join("app/index.html")).unwrap(),
                want
            );
        }
        std::env::remove_var("OPENAS_ROOT");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_app_never_takes_another_apps_name() {
        let dir = std::env::temp_dir().join(format!("oa-names-{}", std::process::id()));
        let apps = dir.join("Applications");
        std::fs::create_dir_all(apps.join("Habits.app/Contents")).unwrap();
        let mut d = plist::Dictionary::new();
        d.insert(
            "OAAppFolder".into(),
            plist::Value::String("/x/apps/habits".into()),
        );
        plist::Value::Dictionary(d)
            .to_file_xml(apps.join("Habits.app/Contents/Info.plist"))
            .unwrap();
        std::env::set_var("OPENAS_INSTALL_DIR", &apps);
        assert_eq!(free_name("Habits", Path::new("/x/apps/other")), "Habits 2");
        assert_eq!(free_name("Habits", Path::new("/x/apps/habits")), "Habits"); // its own copy
        assert_eq!(free_name("Notes", Path::new("/x/apps/other")), "Notes");
        std::env::remove_var("OPENAS_INSTALL_DIR");
        let _ = std::fs::remove_dir_all(dir);
    }

    use super::*;

    #[test]
    fn shape_hides_values() {
        let dir = std::env::temp_dir().join(format!("oa-shape-{}", now()));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(
            dir.join("data/store.json"),
            r#"{"orders":[{"text":"3 loaves for Mia","done":false}],"n":3}"#,
        )
        .unwrap();
        let s = data_shape(&dir).unwrap();
        assert!(!s.contains("Mia"));
        assert!(s.contains("\"text\": \"string\"") && s.contains("1 items"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Bakery Orders!"), "bakery-orders");
        assert_eq!(slug("✨"), "app");
    }
}
