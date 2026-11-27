//! The Helper: one background process that holds the provider settings and
//! the key, makes and changes apps, and answers only callers it can name.
//!
//! Transport: a Unix socket at `<root>/helper.sock`, one JSON request per
//! connection, answered with `{"progress": "..."}` lines and then one final
//! `{"ok": ...}` or `{"error": "..."}` line.
//!
//! Who's calling (S4): the peer's process id, its executable, and that
//! executable's bundle. A caller is trusted only as
//! - the owner: this same `openas` binary (the command line);
//! - the manager: the "OpenAs" copy recorded in `<root>/manager.json`;
//! - an app: an installed copy recorded in some app's `app.json`, whose code
//!   hash still matches the one taken at install. An app may change and undo
//!   only itself.
//!
//! This gives per-app attribution (a usage log; no caps yet); it is not protection from
//! malware running as the same user, which could read the key file directly.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use openas_core::{make, model, paths};
use openas_platform::ipc::{Conn, Listener};
use serde_json::{json, Value};

const IDLE_EXIT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq)]
pub enum Caller {
    Owner,
    Manager,
    App(String),
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn same(a: &Path, b: &Path) -> bool {
    a.canonicalize()
        .ok()
        .zip(b.canonicalize().ok())
        .is_some_and(|(a, b)| a == b)
}

fn trusted_copy(bundle: &Path, recorded: Option<&str>) -> bool {
    recorded.is_some_and(|want| openas_platform::sealed(bundle, want))
}

pub fn manager_record() -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(paths::root().join("manager.json")).ok()?).ok()
}

fn identify(conn: &Conn) -> Result<Caller> {
    let exe = conn
        .peer_exe()
        .ok_or_else(|| anyhow!("can't see who is calling"))?;
    if same(&exe, &std::env::current_exe()?) {
        return Ok(Caller::Owner);
    }
    let bundle = openas_platform::bundle_of(&exe)
        .ok_or_else(|| anyhow!("{} is not an OpenAs app", exe.display()))?;
    if let Some(m) = manager_record() {
        if m.get("installed")
            .and_then(Value::as_str)
            .is_some_and(|p| same(Path::new(p), &bundle))
            && trusted_copy(&bundle, m.get("cdhash").and_then(Value::as_str))
        {
            return Ok(Caller::Manager);
        }
    }
    for info in make::list()? {
        if info.installed.as_deref().is_some_and(|p| same(p, &bundle))
            && trusted_copy(&bundle, info.cdhash.as_deref())
        {
            return Ok(Caller::App(info.id));
        }
    }
    bail!(
        "{} is not an app OpenAs installed (or it was modified)",
        bundle.display()
    )
}

fn allowed(caller: &Caller, op: &str, app: Option<&str>) -> bool {
    match caller {
        Caller::Owner | Caller::Manager => true,
        Caller::App(id) => {
            matches!(
                op,
                "change" | "save_change" | "discard_change" | "undo" | "versions" | "restore"
            ) && app == Some(id.as_str())
        }
    }
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn yes() -> bool {
    true
}

/// One request to the Helper, as its `op` names it. Unknown fields are
/// ignored; a missing one is an error before anything runs.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The step-wise builder: a draft that builds in its own process, shown
    /// in its own window while it's made.
    Make {
        request: String,
        /// Open the separate making window. OpenAs's own window sends false:
        /// it shows the draft in place.
        #[serde(default = "yes")]
        window: bool,
    },
    Rename {
        app: String,
        name: String,
    },
    Change {
        app: String,
        request: String,
    },
    /// Keep the edits made since the last save (Save as App), or drop them.
    SaveChange {
        app: String,
    },
    DiscardChange {
        app: String,
    },
    Versions {
        app: String,
    },
    Restore {
        app: String,
        version: u64,
    },
    Undo {
        app: String,
    },
    List,
    Open {
        app: String,
    },
    Settings,
    /// A shared file someone sent: what it is, and its code, before installing.
    SharePeek {
        file: String,
    },
    ShareInstall {
        file: String,
        #[serde(default)]
        name: String,
    },
    Models {
        provider: String,
        #[serde(default)]
        key: Option<String>,
    },
    Connect {
        provider: String,
        model: String,
        #[serde(default)]
        key: Option<String>,
    },
    SettingsSet {
        #[serde(default)]
        provider: Option<String>,
        #[serde(default)]
        build_model: Option<String>,
    },
    /// The spending limits, in dollars: per calendar month and per app.
    LimitsSet {
        monthly: f64,
        per_app: f64,
    },
}

impl Request {
    /// The `op` name, for the permission check and the usage log.
    pub fn op(&self) -> &'static str {
        match self {
            Request::Make { .. } => "make",
            Request::Rename { .. } => "rename",
            Request::Change { .. } => "change",
            Request::SaveChange { .. } => "save_change",
            Request::DiscardChange { .. } => "discard_change",
            Request::Versions { .. } => "versions",
            Request::Restore { .. } => "restore",
            Request::Undo { .. } => "undo",
            Request::List => "list",
            Request::Open { .. } => "open",
            Request::Settings => "settings",
            Request::SharePeek { .. } => "share_peek",
            Request::ShareInstall { .. } => "share_install",
            Request::Models { .. } => "models",
            Request::Connect { .. } => "connect",
            Request::SettingsSet { .. } => "settings_set",
            Request::LimitsSet { .. } => "limits_set",
        }
    }

    /// The app it acts on, if any.
    pub fn app(&self) -> Option<&str> {
        match self {
            Request::Rename { app, .. }
            | Request::Change { app, .. }
            | Request::SaveChange { app }
            | Request::DiscardChange { app }
            | Request::Versions { app }
            | Request::Restore { app, .. }
            | Request::Undo { app }
            | Request::Open { app } => Some(app),
            _ => None,
        }
    }
}

/// A key typed into a field: blank means "use the saved one".
fn typed_key(key: Option<String>) -> Option<String> {
    key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

/// Runs one request, telling the window how it goes through `ev`.
pub fn handle(caller: &Caller, req: &Value, ev: &make::Events) -> Result<Value> {
    let say = ev.say;
    let op_name = req.get("op").and_then(Value::as_str).unwrap_or("");
    let req: Request =
        serde_json::from_value(req.clone()).map_err(|e| anyhow!("bad request ({op_name}): {e}"))?;
    let op = req.op();
    if !allowed(caller, op, req.app()) {
        bail!("not allowed: {op}");
    }
    match req {
        Request::Make { request, window } => {
            let ws = openas_core::agent::start_make(&request, &std::env::current_exe()?, window)?;
            Ok(json!({ "draft": ws.file_name().map(|f| f.to_string_lossy().into_owned()) }))
        }
        Request::Rename { app, name } => {
            let info = make::rename(&app, &name)?;
            Ok(json!({ "id": info.id, "name": info.name }))
        }
        Request::Change { app, request } => {
            let (_, f) = openas_core::agent::change(&app, &request, ev)?;
            Ok(json!({ "id": app, "problems": [], "summary": f.summary, "pending": true }))
        }
        Request::SaveChange { app } => {
            openas_core::agent::save_change(&app)?;
            Ok(json!({ "saved": true }))
        }
        Request::DiscardChange { app } => {
            openas_core::agent::discard_change(&app)?;
            Ok(json!({ "discarded": true }))
        }
        Request::Versions { app } => Ok(json!({ "versions": make::versions(&app)? })),
        Request::Restore { app, version } => {
            openas_core::agent::discard_change(&app)?;
            make::restore(&app, version)?;
            Ok(json!({ "restored": true }))
        }
        Request::Undo { app } => {
            openas_core::agent::discard_change(&app)?;
            make::undo(&app)?;
            Ok(json!({ "undone": true }))
        }
        Request::List => {
            let apps: Vec<Value> = make::list()?
                .into_iter()
                .map(|i| {
                    let icon = std::fs::read(paths::apps().join(&i.id).join("icon.png")).ok().map(|b| format!("data:image/png;base64,{}", b64(&b)));
                    json!({ "id": i.id, "name": i.name, "model": i.model, "created": i.created, "installed": i.installed, "icon": icon })
                })
                .collect();
            Ok(json!({ "apps": apps }))
        }
        Request::Open { app } => {
            let folder = make::find(&app)?;
            let info = make::read_info(&folder)?;
            let path = info.installed.ok_or_else(|| anyhow!("not installed"))?;
            openas_platform::launch(&path, false)?;
            Ok(json!({ "opened": true }))
        }
        Request::Settings => {
            let s = model::settings();
            // Whether a key is set, never the key.
            let providers: Vec<Value> = model::providers()?
                .into_iter()
                .map(|(id, p)| {
                    let has_key = p.key_env.as_ref().map(|v| model::key_for(&id, v).is_some()).unwrap_or(true);
                    json!({ "id": id, "name": p.name, "needs_key": p.key_env.is_some(), "has_key": has_key,
                            "base_url": p.base_url, "kind": p.kind, "key_url": p.key_url })
                })
                .collect();
            let configured = model::configured();
            let ready = configured
                && providers
                    .iter()
                    .any(|p| p["id"] == json!(s.provider) && p["has_key"] == json!(true));
            Ok(
                json!({ "provider": s.provider, "build_model": s.build_model, "providers": providers, "ready": ready,
                       "first_run": !configured, "monthly_limit": s.monthly_limit, "app_limit": s.app_limit,
                       "spent_this_month": openas_core::agent::spent_this_month() }),
            )
        }
        Request::SharePeek { file } => {
            let r = openas_core::share::open(std::path::Path::new(&file))?;
            Ok(
                json!({ "name": r.name, "made_with": r.made_with, "icon": openas_core::share::icon(&r), "files": openas_core::share::code(&r) }),
            )
        }
        Request::ShareInstall { file, name } => {
            let r = openas_core::share::open(std::path::Path::new(&file))?;
            let info = openas_core::share::install(&r, &name)?;
            if let Some(app) = &info.installed {
                let _ = openas_platform::launch(app, false);
            }
            Ok(json!({ "id": info.id, "name": info.name }))
        }
        Request::Models { provider, key } => {
            // The ids a provider offers, with this key (or the saved one); never the key itself.
            let p = model::providers()?
                .get(&provider)
                .cloned()
                .ok_or_else(|| anyhow!("no provider called {provider}"))?;
            if p.api.as_deref() == Some("anthropic") {
                return Ok(
                    json!({ "models": ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"] }),
                );
            }
            let key = typed_key(key).or_else(|| {
                p.key_env
                    .as_ref()
                    .and_then(|v| model::key_for(&provider, v))
            });
            match openas_core::oai::list_models(&p.base_url, key.as_deref()) {
                Ok(m) => Ok(json!({ "models": m })),
                Err(_) => Ok(
                    json!({ "models": [], "message": format!("Couldn't get {}'s model list.", p.name) }),
                ),
            }
        }
        Request::Connect {
            provider,
            model: model_id,
            key,
        } => {
            // Try the provider with this key (or the saved one) and model;
            // save both only if it answers.
            let model_id = model_id.trim().to_string();
            let fresh = typed_key(key);
            let p = model::providers()?
                .get(&provider)
                .cloned()
                .ok_or_else(|| anyhow!("no provider called {provider}"))?;
            let key = fresh.clone().or_else(|| {
                p.key_env
                    .as_ref()
                    .and_then(|v| model::key_for(&provider, v))
            });
            if p.key_env.is_some() && key.is_none() {
                return Ok(
                    json!({ "connected": false, "message": format!("Paste your {} key first.", p.name) }),
                );
            }
            say(&format!("Trying {}…", p.name));
            match model::Client::trial(&provider, key)?.test(&model_id) {
                model::Test::Ok => {
                    if let Some(k) = &fresh {
                        openas_core::keys::set(&provider, k)?;
                    }
                    let mut s = model::settings();
                    s.provider = provider;
                    s.build_model = model_id;
                    model::save_settings(&s)?;
                    Ok(json!({ "connected": true }))
                }
                model::Test::Refused(why) => Ok(json!({ "connected": false, "message": why })),
            }
        }
        Request::SettingsSet {
            provider,
            build_model,
        } => {
            let mut s = model::settings();
            if let Some(p) = provider {
                s.provider = p;
            }
            if let Some(m) = build_model {
                s.build_model = m;
            }
            model::save_settings(&s)?;
            Ok(json!({ "saved": true }))
        }
        Request::LimitsSet { monthly, per_app } => {
            if !(monthly.is_finite() && per_app.is_finite() && monthly > 0.0 && per_app > 0.0) {
                bail!("limits must be more than $0");
            }
            let mut s = model::settings();
            s.monthly_limit = monthly;
            s.app_limit = per_app;
            model::save_settings(&s)?;
            Ok(json!({ "saved": true }))
        }
    }
}

pub fn serve() -> Result<()> {
    let listener = Arc::new(match Listener::bind(&paths::root()) {
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => bail!("a Helper is already running"),
        r => r.context("opening the Helper's socket")?,
    });
    make::sweep_drafts();

    let last = Arc::new(AtomicU64::new(now()));
    let busy = Arc::new(AtomicUsize::new(0));
    // One change at a time per app.
    let locks: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();
    {
        let (last, busy, listener) = (last.clone(), busy.clone(), listener.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(15));
            if busy.load(Ordering::SeqCst) == 0
                && now() - last.load(Ordering::SeqCst) > IDLE_EXIT.as_secs()
            {
                listener.close();
                std::process::exit(0);
            }
        });
    }
    loop {
        let Ok(stream) = listener.accept() else {
            continue;
        };
        last.store(now(), Ordering::SeqCst);
        let (last, busy, locks) = (last.clone(), busy.clone(), locks.clone());
        std::thread::spawn(move || {
            let Ok(mut out) = stream.try_clone() else {
                return;
            };
            busy.fetch_add(1, Ordering::SeqCst);
            // The window going away (closed, or Stop) is noticed on the next
            // line sent, at most a second later, and stops the model call.
            let gone = std::sync::atomic::AtomicBool::new(false);
            let send = |v: Value| {
                if writeln!(&stream, "{v}").is_err() {
                    gone.store(true, Ordering::SeqCst);
                }
            };
            let result = (|| -> Result<Value> {
                let caller = identify(&stream)?;
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line)?;
                let req: Value = serde_json::from_str(&line).context("bad request")?;
                let key = req
                    .get("app")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let writes = matches!(
                    req.get("op").and_then(Value::as_str),
                    Some("change" | "undo" | "restore")
                );
                if writes
                    && !locks
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(key.clone())
                {
                    bail!("this app is already being changed");
                }
                let say = |p: &str| send(json!({ "progress": p }));
                let preview = |h: &str| send(json!({ "preview": h }));
                let stopped = || gone.load(Ordering::SeqCst);
                let thought = |t: &str| send(json!({ "thought": t }));
                let r = handle(
                    &caller,
                    &req,
                    &make::Events {
                        say: &say,
                        preview: &preview,
                        stopped: &stopped,
                        thought: &thought,
                    },
                );
                if writes {
                    locks
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&key);
                }
                r
            })();
            let final_line = match result {
                Ok(v) => json!({ "ok": v }),
                Err(e) => json!({ "error": e.to_string() }),
            };
            let _ = writeln!(out, "{final_line}");
            last.store(now(), Ordering::SeqCst);
            busy.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_read_into_their_shapes() {
        let r: Request =
            serde_json::from_value(json!({ "op": "restore", "app": "plants", "version": 3 }))
                .unwrap();
        assert!(matches!(&r, Request::Restore { app, version: 3 } if app == "plants"));
        assert_eq!((r.op(), r.app()), ("restore", Some("plants")));
        // A blank key field means "use the saved one".
        let r: Request = serde_json::from_value(
            json!({ "op": "connect", "provider": "anthropic", "model": "m", "key": "  " }),
        )
        .unwrap();
        let Request::Connect { key, .. } = r else {
            panic!()
        };
        assert_eq!(typed_key(key), None);
        // Missing fields and unknown ops are refused before anything runs.
        assert!(
            serde_json::from_value::<Request>(json!({ "op": "restore", "app": "plants" })).is_err()
        );
        assert!(serde_json::from_value::<Request>(json!({ "op": "format_disk" })).is_err());
    }

    #[test]
    fn an_app_may_only_change_or_undo_itself() {
        let me = Caller::App("plants".into());
        assert!(allowed(&me, "change", Some("plants")));
        assert!(allowed(&me, "undo", Some("plants")));
        assert!(!allowed(&me, "change", Some("orders")));
        for op in [
            "make",
            "rename",
            "list",
            "open",
            "settings",
            "settings_set",
            "connect",
        ] {
            assert!(!allowed(&me, op, Some("plants")), "{op}");
        }
        assert!(allowed(&Caller::Manager, "make", None));
        assert!(allowed(&Caller::Owner, "settings_set", None));
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }
}
