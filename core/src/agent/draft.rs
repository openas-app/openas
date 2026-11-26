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

/// Waits for Save as App or Discard from the making window. A window that is
/// gone (closed, crashed, never opened) counts as Save: finished work is
/// never lost by closing a window.
pub(super) fn wait_for_choice(ws: &Path, name: &str) -> Choice {
    let (keep, discard, open) = (
        ws.with_extension("keep"),
        ws.with_extension("discard"),
        ws.with_extension("open"),
    );
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
        let alive = std::fs::read_to_string(&open)
            .ok()
            .and_then(|p| p.trim().parse::<u32>().ok())
            .is_some_and(|pid| {
                Command::new("/bin/kill")
                    .args(["-0", &pid.to_string()])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
            });
        if !alive {
            return Choice::Save(name.to_string());
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
    let request = std::fs::read_to_string(ws.join("prompt.txt"))?;
    let id = ws
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let say = |s: &str| eprintln!("{s}");
    let f = run(ws, ws, &request, &id, &make::Events::lines(&say))?;
    let name = if std::env::var("OPENAS_AUTO_KEEP").is_ok() {
        f.name.clone()
    } else {
        match wait_for_choice(ws, &f.name) {
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
    let becoming = ws.with_extension("becoming");
    if becoming.exists() {
        // Saved from the making window, which closed in on the app's icon:
        // the app's first window grows out of that spot.
        let _ = std::fs::copy(&becoming, paths::apps().join(&info.id).join(".grow.json"));
        let _ = std::fs::remove_file(&becoming);
    } else {
        // Otherwise it opens once with its conversation beside it.
        let _ = std::fs::write(paths::apps().join(&info.id).join(".just-made"), "");
    }
    std::fs::write(
        ws.with_extension("moved"),
        json!({ "id": info.id, "app": app }).to_string(),
    )?;
    if !app.as_os_str().is_empty() {
        // A test run never brings a window in front of the person.
        let quiet = std::env::var("OPENAS_QUIET_WINDOW").is_ok();
        let _ = Command::new("/usr/bin/open")
            .args(if quiet { &["-g"][..] } else { &[][..] })
            .arg(&app)
            .status();
    }
    Ok(())
}

/// Starts making an app: the build in its own process and the making window
/// beside it. Returns the draft folder.
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
    let app = dir.join("New app.app");
    let ok = (|| -> Result<()> {
        std::fs::create_dir_all(&dir)?;
        let st = Command::new("/usr/bin/ditto")
            .arg(paths::runner_bundle())
            .arg(&app)
            .status()?;
        if !st.success() {
            bail!("ditto");
        }
        let plist = app.join("Contents/Info.plist");
        for cmd in [
            "Set :CFBundleName New app",
            "Set :CFBundleIdentifier app.openas.making",
        ] {
            let _ = Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", cmd])
                .arg(&plist)
                .status();
        }
        let _ = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Add :CFBundleDisplayName string New app"])
            .arg(&plist)
            .stderr(std::process::Stdio::null())
            .status();
        let st = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app)
            .stderr(std::process::Stdio::null())
            .status()?;
        if !st.success() {
            bail!("codesign");
        }
        Ok(())
    })();
    match ok {
        Ok(()) => app.join("Contents/MacOS/Runner"),
        Err(_) => paths::runner_exe(),
    }
}

pub fn start_make(request: &str, openas: &Path) -> Result<PathBuf> {
    let ws = prepare_draft(request)?;
    let log = std::fs::File::create(ws.join("make.log"))?;
    Command::new(openas)
        .arg("make-run")
        .arg(&ws)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
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
