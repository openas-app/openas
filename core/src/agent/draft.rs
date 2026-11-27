use super::*;

// ---- Making and changing ------------------------------------------------------

/// A new app as a draft (not installed): returns the draft folder.
pub fn new_draft(request: &str, ev: &make::Events) -> Result<(PathBuf, Finished)> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let id = format!(".draft-{nanos}");
    let ws = paths::apps().join(&id);
    std::fs::create_dir_all(ws.join("app"))?;
    std::fs::write(ws.join("prompt.txt"), request)?;
    std::fs::write(
        ws.join("draft.json"),
        json!({ "provider": "anthropic", "model": model() }).to_string(),
    )?;
    let f = run(&ws, &ws, request, &id, ev)?;
    Ok((ws, f))
}

/// A change, made on a copy. The live app only changes when the builder
/// finishes; the old version is kept whole in history/<n>/.
/// Where a change waits to be saved: the app's files as changed, beside the
/// saved ones, until Save as App moves them in or Discard throws them away.
pub fn pending_dir(folder: &Path) -> PathBuf {
    folder.join(".pending")
}

/// Makes a change as a try-out. The app shows it, but nothing is kept until
/// `save_change`; a second change builds on the first. The app's data is
/// never touched (the builder works on a copy).
pub fn change(app: &str, request: &str, ev: &make::Events) -> Result<(PathBuf, Finished)> {
    let folder = make::find(app)?;
    let pending = pending_dir(&folder);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let stage = paths::apps().join(format!(".stage-{app}-{nanos}"));
    // On top of an edit not yet saved, or the saved app.
    let base = if pending.join("app/index.html").exists() {
        pending.join("app")
    } else {
        folder.join("app")
    };
    copy_dir(&base, &stage.join("app"))?;
    for f in ["icon.svg", "permissions.json"] {
        if folder.join(f).exists() {
            std::fs::copy(folder.join(f), stage.join(f))?;
        }
    }
    if folder.join("data").exists() {
        copy_dir(&folder.join("data"), &stage.join("data"))?;
    }
    let job = folder
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let result = run(&stage, &folder, request, &job, ev);
    let f = match result {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
    };
    // The new try-out replaces the last one; what was asked accumulates for
    // the version's label.
    let mut asked = std::fs::read_to_string(pending.join("requests.txt")).unwrap_or_default();
    asked.push_str(request.trim());
    asked.push('\n');
    let next = folder.join(format!(".pending-{nanos}"));
    std::fs::create_dir_all(&next)?;
    std::fs::rename(stage.join("app"), next.join("app"))?;
    if stage.join("window.json").exists() {
        std::fs::copy(stage.join("window.json"), next.join("window.json"))?;
    }
    std::fs::write(next.join("requests.txt"), asked)?;
    let _ = std::fs::remove_dir_all(&pending);
    std::fs::rename(&next, &pending)?;
    // The build's log goes with the app, after any earlier builds' logs.
    if let Ok(t) = std::fs::read_to_string(stage.join("agent.jsonl")) {
        use std::io::Write;
        if let Ok(mut o) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(folder.join("agent.jsonl"))
        {
            let _ = o.write_all(t.as_bytes());
        }
    }
    let _ = std::fs::remove_dir_all(&stage);
    Ok((folder, f))
}

/// Save as App after edits: the try-out becomes the app's new version. The
/// version before it is kept, so Undo and Versions can go back.
pub fn save_change(app: &str) -> Result<()> {
    let folder = make::find(app)?;
    let pending = pending_dir(&folder);
    if !pending.join("app/index.html").exists() {
        bail!("there are no edits to save");
    }
    let tried_online = make::net_tried(&folder);
    make::snapshot(&folder)?;
    let old = folder.join(".app-old");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(folder.join("app"), &old)?;
    std::fs::rename(pending.join("app"), folder.join("app"))?;
    let _ = std::fs::remove_dir_all(&old);
    let asked = std::fs::read_to_string(pending.join("requests.txt")).unwrap_or_default();
    let label = asked
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    make::set_label(&folder, if label.is_empty() { "Changes" } else { &label })?;
    if pending.join("window.json").exists() {
        std::fs::copy(pending.join("window.json"), folder.join("window.json"))?;
    }
    std::fs::remove_dir_all(&pending)?;
    make::net_follow_save(&folder, tried_online)?;
    Ok(())
}

/// Discard after edits: back to the saved app, as if they never happened.
pub fn discard_change(app: &str) -> Result<()> {
    let pending = pending_dir(&make::find(app)?);
    if pending.exists() {
        std::fs::remove_dir_all(&pending)?;
    }
    Ok(())
}

pub(super) fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

// ---- A build that runs on its own --------------------------------------------

/// A fresh draft folder for a request (nothing runs yet).
pub fn prepare_draft(request: &str) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let ws = paths::apps().join(format!(".draft-{nanos}"));
    std::fs::create_dir_all(ws.join("app"))?;
    std::fs::write(ws.join("prompt.txt"), request)?;
    std::fs::write(
        ws.join("draft.json"),
        json!({ "provider": "anthropic", "model": model() }).to_string(),
    )?;
    let _ = Live::new(&ws, request, "new");
    Ok(ws)
}

/// What the person chose for a finished draft.
pub(super) enum Choice {
    Save(String),
    Discard,
}

/// Waits for Save as App (with the name the person chose) or Discard. Only
/// the person's choice saves: a finished draft whose window is gone stays a
/// draft, waiting on OpenAs's shelf as "Ready to save".
pub(super) fn wait_for_choice(ws: &Path, name: &str) -> Choice {
    let (keep, discard) = (ws.with_extension("keep"), ws.with_extension("discard"));
    loop {
        if let Ok(n) = std::fs::read_to_string(&keep) {
            let _ = std::fs::remove_file(&keep);
            let n = n.trim();
            return Choice::Save(if n.is_empty() {
                name.to_string()
            } else {
                n.chars().take(60).collect()
            });
        }
        if discard.exists() {
            let _ = std::fs::remove_file(&discard);
            return Choice::Discard;
        }
        // Gone from disk: nothing to save. Otherwise it waits, however long,
        // for the person to choose: closing a window or quitting OpenAs is
        // never a Save. Its tile on OpenAs's shelf opens it again.
        if !ws.is_dir() {
            return Choice::Discard;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

/// Builds a prepared draft, waits for Save as App (or Discard) in the making
/// window, then installs it and opens the finished app. Runs as its own
/// process (`openas make-run <draft>`), so closing the making window doesn't
/// stop it. When it's installed (or discarded), `<draft>.moved` says so, so
/// the making window can step aside. OPENAS_AUTO_KEEP saves without asking
/// (scripts and tests).
pub fn make_run(ws: &Path) -> Result<()> {
    // Held for as long as this build runs. The system lets go of it however
    // the process ends (crash, kill, restart), so OpenAs can tell a build
    // that's working from one that died without saying so.
    let _alive = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ws.join(BUILD_LOCK))?;
    _alive.lock()?;
    let id = ws
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Finished already, but the process that waited for the choice is gone
    // (a restart, a crash): wait for Save as App or Discard again, no rebuild.
    let made: Value = std::fs::read_to_string(ws.join("making.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if made["phase"] == "done"
        && !ws.join("continue.json").exists()
        && ws.join("app/index.html").exists()
    {
        let name = made["name"].as_str().unwrap_or("New App").to_string();
        return keep_or_discard(ws, &name);
    }
    // Carrying on after a stop or a failure: what to do next, said once.
    // `continue.json`: {"request": for the model, "shown": for the person}.
    let cont = ws.join("continue.json");
    let (request, shown) = match std::fs::read_to_string(&cont) {
        Ok(t) => {
            let _ = std::fs::remove_file(&cont);
            let v: Value = serde_json::from_str(&t)?;
            let r = v["request"].as_str().unwrap_or_default().to_string();
            let s = v["shown"].as_str().unwrap_or(&r).to_string();
            (r, s)
        }
        Err(_) => {
            let r = std::fs::read_to_string(ws.join("prompt.txt"))?;
            (r.clone(), r)
        }
    };
    let say = |s: &str| eprintln!("{s}");
    let f = super::turn::run_shown(ws, ws, &request, &shown, &id, &make::Events::lines(&say))?;
    keep_or_discard(ws, &f.name)
}

/// Waits for the person's choice on a finished draft, then installs it (and
/// opens it) or throws it away.
fn keep_or_discard(ws: &Path, made_name: &str) -> Result<()> {
    let name = if std::env::var("OPENAS_AUTO_KEEP").is_ok() {
        made_name.to_string()
    } else {
        match wait_for_choice(ws, made_name) {
            Choice::Save(n) => n,
            Choice::Discard => {
                std::fs::write(
                    ws.with_extension("moved"),
                    json!({ "discarded": true }).to_string(),
                )?;
                let _ = std::fs::remove_dir_all(ws);
                return Ok(());
            }
        }
    };
    let info = make::install_draft(ws, &name)?;
    let app = info.installed.clone().unwrap_or_default();
    std::fs::write(
        ws.with_extension("moved"),
        json!({ "id": info.id, "app": app }).to_string(),
    )?;
    if !app.as_os_str().is_empty() {
        // A test run never brings a window in front of the person.
        let quiet = std::env::var("OPENAS_QUIET_WINDOW").is_ok();
        let _ = openas_platform::launch(&app, quiet);
    }
    Ok(())
}

/// A copy of the Runner named "New app", so the Dock and the app switcher
/// show that rather than "Runner" while an app is being made. It sits beside
/// the draft and old ones are cleared on the next make. Falls back to the
/// plain Runner if anything fails.
pub(super) fn making_window(ws: &Path) -> PathBuf {
    let Some(apps) = ws.parent() else {
        return paths::runner_exe();
    };
    if let Ok(rd) = std::fs::read_dir(apps) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|a| a.as_secs() > 6 * 3600);
            if let Some(draft) = n.strip_prefix(".window-") {
                if !apps.join(draft).exists() || old {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
    let dir = apps.join(format!(
        ".window-{}",
        ws.file_name().unwrap_or_default().to_string_lossy()
    ));
    openas_platform::making_copy(&paths::runner_bundle(), &dir)
        .unwrap_or_else(|_| paths::runner_exe())
}

/// The lock a running build holds in its draft folder (runner/src/who.rs
/// reads it under the same name).
const BUILD_LOCK: &str = "build.lock";

/// Starts making an app: the build in its own process, and (with `window`)
/// the making window beside it. Returns the draft folder.
/// `window`: open the making window too. OpenAs's own window shows the draft
/// itself and passes false; the command line and other callers pass true.
pub fn start_make(request: &str, openas: &Path, window: bool) -> Result<PathBuf> {
    let ws = prepare_draft(request)?;
    let log = std::fs::File::create(ws.join("make.log"))?;
    openas_platform::proc::background(&mut Command::new(openas))
        .arg("make-run")
        .arg(&ws)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    if !window {
        return Ok(ws);
    }
    Command::new(making_window(&ws))
        .arg("--app")
        .arg(&ws)
        .arg("--making")
        .env("OPENAS_HELPER", openas)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(ws)
}
