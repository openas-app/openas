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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use openas_core::{install, make, model, paths};
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

#[cfg(target_os = "macos")]
fn peer_pid(s: &std::os::unix::net::UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `len` are live locals of the size we pass; the
    // kernel writes at most `len` bytes into `pid`. LOCAL_PEERPID asks a
    // Unix socket for the process on its other end (sys/un.h).
    let r = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    (r == 0 && pid > 0).then_some(pid)
}

#[cfg(target_os = "macos")]
fn pid_exe(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is a live buffer of the length we pass; proc_pidpath
    // writes at most that many bytes and returns how many it wrote.
    let n =
        unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    (n > 0).then(|| PathBuf::from(String::from_utf8_lossy(&buf[..n as usize]).into_owned()))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn peer_pid(s: &std::os::unix::net::UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;
    // SAFETY: ucred is plain integers, so all zeroes is a valid value.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are live locals of the size we pass.
    let r = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    (r == 0).then_some(cred.pid)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pid_exe(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// `<X>.app/Contents/MacOS/<exe>` -> `<X>.app`
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let b = exe.parent()?.parent()?.parent()?;
    (b.extension()? == "app").then(|| b.to_path_buf())
}

fn same(a: &Path, b: &Path) -> bool {
    a.canonicalize()
        .ok()
        .zip(b.canonicalize().ok())
        .is_some_and(|(a, b)| a == b)
}

fn trusted_copy(bundle: &Path, recorded: Option<&str>) -> bool {
    let Some(want) = recorded else { return false };
    let verified = std::process::Command::new("codesign")
        .arg("--verify")
        .arg(bundle)
        .output()
        .is_ok_and(|o| o.status.success());
    verified && install::cdhash(bundle).as_deref() == Some(want)
}

pub fn manager_record() -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(paths::root().join("manager.json")).ok()?).ok()
}

fn identify(s: &std::os::unix::net::UnixStream) -> Result<Caller> {
    let pid = peer_pid(s).ok_or_else(|| anyhow!("can't see who is calling"))?;
    let exe = pid_exe(pid).ok_or_else(|| anyhow!("can't see the caller's program"))?;
    if same(&exe, &std::env::current_exe()?) {
        return Ok(Caller::Owner);
    }
    let bundle =
        bundle_of(&exe).ok_or_else(|| anyhow!("{} is not an OpenAs app", exe.display()))?;
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

fn log_usage(app: &str, op: &str, spend: &make::Spend, caller: &Caller) {
    let line = json!({ "t": now(), "app": app, "op": op, "tokens": spend.tokens, "cost": spend.cost, "by": format!("{caller:?}") });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::usage_log())
    {
        let _ = writeln!(f, "{line}");
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
    /// Built as a draft; it becomes an app with `Install`.
    New {
        prompt: String,
    },
    Install {
        draft: String,
        name: String,
    },
    AllowNetwork {
        draft: String,
    },
    Discard {
        draft: String,
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
    Usage,
}

impl Request {
    /// The `op` name, for the permission check and the usage log.
    pub fn op(&self) -> &'static str {
        match self {
            Request::Make { .. } => "make",
            Request::New { .. } => "new",
            Request::Install { .. } => "install",
            Request::AllowNetwork { .. } => "allow_network",
            Request::Discard { .. } => "discard",
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
            Request::Usage => "usage",
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
        Request::New { prompt } => {
            let d = make::build_draft(&prompt, ev)?;
            log_usage(&d.name, op, &d.spend, caller);
            let id = d
                .dir
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let png = d.dir.join("icon.png");
            let icon = install::render_png(&d.svg, &png)
                .ok()
                .and_then(|_| std::fs::read(&png).ok())
                .map(|b| format!("data:image/png;base64,{}", b64(&b)));
            Ok(
                json!({ "draft": id, "name": d.name, "problems": d.problems, "icon": icon,
                       "wants_network": openas_core::check::wanted_hosts(&d.problems),
                       "cost": if d.spend.cost_unknown { Value::Null } else { json!(d.spend.cost) } }),
            )
        }
        Request::Install { draft, name } => {
            say("Adding it to your Applications…");
            let info = make::install_draft(&make::draft_dir(&draft)?, &name)?;
            if let Some(p) = &info.installed {
                std::process::Command::new("open").arg(p).status()?;
            }
            Ok(json!({ "id": info.id, "name": info.name }))
        }
        Request::AllowNetwork { draft } => {
            say("Checking it again with internet access…");
            let problems = make::allow_network_draft(&make::draft_dir(&draft)?)?;
            Ok(json!({ "problems": problems }))
        }
        Request::Discard { draft } => {
            std::fs::remove_dir_all(make::draft_dir(&draft)?)?;
            Ok(json!({ "discarded": true }))
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
            std::process::Command::new("open").arg(path).status()?;
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
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg(app)
                    .status();
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
        Request::Usage => {
            let text = std::fs::read_to_string(paths::usage_log()).unwrap_or_default();
            let (mut cost, mut n) = (0.0, 0);
            for l in text.lines() {
                if let Ok(v) = serde_json::from_str::<Value>(l) {
                    cost += v.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
                    n += 1;
                }
            }
            Ok(json!({ "requests": n, "cost": cost }))
        }
    }
}

pub fn serve() -> Result<()> {
    use std::os::unix::net::{UnixListener, UnixStream};
    let sock = paths::socket();
    std::fs::create_dir_all(paths::root())?;
    if sock.exists() {
        if UnixStream::connect(&sock).is_ok() {
            bail!("a Helper is already running");
        }
        std::fs::remove_file(&sock)?;
    }
    let listener = UnixListener::bind(&sock).context("opening the Helper socket")?;
    make::sweep_drafts();
    // Only this user can connect.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;

    let last = Arc::new(AtomicU64::new(now()));
    let busy = Arc::new(AtomicUsize::new(0));
    // One change at a time per app.
    let locks: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();
    {
        let (last, busy, sock) = (last.clone(), busy.clone(), sock.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(15));
            if busy.load(Ordering::SeqCst) == 0
                && now() - last.load(Ordering::SeqCst) > IDLE_EXIT.as_secs()
            {
                let _ = std::fs::remove_file(&sock);
                std::process::exit(0);
            }
        });
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
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
    Ok(())
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
            "new",
            "install",
            "allow_network",
            "discard",
            "rename",
            "list",
            "open",
            "settings",
            "settings_set",
            "connect",
            "usage",
        ] {
            assert!(!allowed(&me, op, Some("plants")), "{op}");
        }
        assert!(allowed(&Caller::Manager, "new", None));
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

    #[test]
    fn bundle_of_finds_the_app() {
        assert_eq!(
            bundle_of(Path::new("/A/Plants.app/Contents/MacOS/Plants")),
            Some(PathBuf::from("/A/Plants.app"))
        );
        assert_eq!(bundle_of(Path::new("/usr/bin/python3")), None);
    }
}
