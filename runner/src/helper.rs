use crate::*;

#[derive(Default)]
pub(crate) struct Job {
    pub(crate) lines: Vec<String>,
    /// The page as written so far, latest only, and how many times it changed.
    pub(crate) preview: Option<String>,
    pub(crate) preview_n: u64,
    pub(crate) thought: Option<String>,
    pub(crate) done: Option<Value>,
    /// The connection, so Stop can cut it (the Helper then stops the model).
    #[cfg(unix)]
    pub(crate) conn: Option<std::os::unix::net::UnixStream>,
}

/// Shared app files opened with OpenAs (Finder, a double-click), waiting
/// for the home window to show them.
#[derive(Default)]
pub(crate) struct Inbox(pub(crate) Mutex<Vec<String>>);

/// The home window takes the shared files that were opened.
#[tauri::command]
pub fn shared_pending(
    webview: tauri::Webview,
    me: State<App>,
    inbox: State<Inbox>,
) -> Result<Vec<String>, String> {
    if webview.label() != "main" || !me.manager {
        return Err("not allowed".into());
    }
    Ok(std::mem::take(
        &mut *inbox
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    ))
}

#[derive(Default)]
pub(crate) struct Jobs {
    pub(crate) next: AtomicU64,
    pub(crate) all: Mutex<HashMap<u64, Arc<Mutex<Job>>>>,
}

// ---- The Helper, for the change sheet and the manager -----------------------

#[cfg(unix)]
pub(crate) fn helper_request(
    me: &App,
    req: &Value,
    job: &Arc<Mutex<Job>>,
) -> Result<Value, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let sock = socket_for(&root(me));
    let connect = || UnixStream::connect(&sock);
    let stream = match connect() {
        Ok(s) => s,
        Err(_) => {
            let helper = me
                .helper
                .clone()
                .ok_or("this app doesn't know where OpenAs is")?;
            std::process::Command::new(&helper)
                .arg("serve")
                .envs(me.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| format!("couldn't start OpenAs ({}): {e}", helper.display()))?;
            let mut tries = 0;
            loop {
                std::thread::sleep(Duration::from_millis(100));
                if let Ok(s) = connect() {
                    break s;
                }
                tries += 1;
                if tries > 50 {
                    return Err("OpenAs didn't start".into());
                }
            }
        }
    };
    writeln!(&stream, "{req}").map_err(|e| e.to_string())?;
    job.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .conn = stream.try_clone().ok();
    for line in BufReader::new(&stream).lines() {
        let Ok(line) = line else { break };
        let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if let Some(p) = v.get("progress").and_then(Value::as_str) {
            job.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .lines
                .push(p.to_string());
        } else if let Some(t) = v.get("thought").and_then(Value::as_str) {
            job.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .thought = Some(t.to_string());
        } else if let Some(h) = v.get("preview").and_then(Value::as_str) {
            let mut j = job
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            j.preview = Some(h.to_string());
            j.preview_n += 1;
        } else {
            return Ok(v);
        }
    }
    Err(
        if job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .conn
            .is_none()
        {
            "Stopped.".into()
        } else {
            "OpenAs stopped answering".into()
        },
    )
}

#[cfg(not(unix))]
pub(crate) fn helper_request(
    _me: &App,
    _req: &Value,
    _job: &Arc<Mutex<Job>>,
) -> Result<Value, String> {
    Err("changing apps from their menu isn't on this system yet".into())
}

/// What a window may ask the Helper. The change sheet: change only this app.
/// The manager's own window: anything. An app's page: nothing.
/// Who may ask the Helper for what, by the calling webview's own label (the
/// app page and the column share a window, so the window says nothing).
pub(crate) fn vet(webview: &tauri::Webview, me: &App, mut req: Value) -> Result<Value, String> {
    match webview.label() {
        "talk"
            if !me.making
                && matches!(
                    req.get("op").and_then(Value::as_str),
                    Some(
                        "change"
                            | "save_change"
                            | "discard_change"
                            | "versions"
                            | "restore"
                            | "undo"
                    )
                ) =>
        {
            req["app"] = json!(me.app_id());
            Ok(req)
        }
        "main" if me.manager => Ok(req),
        _ => Err("not allowed".into()),
    }
}

/// Whether a window may follow (poll, stop) jobs: OpenAs' own sheets and
/// the manager's window. An app's page may not.
pub(crate) fn may_follow(webview: &tauri::Webview, me: &App) -> Result<(), String> {
    match webview.label() {
        "talk" => Ok(()),
        "main" if me.manager => Ok(()),
        _ => Err("not allowed".into()),
    }
}

pub(crate) fn start_job(app: &AppHandle, req: Value) -> u64 {
    let jobs = app.state::<Jobs>();
    let id = jobs.next.fetch_add(1, Ordering::SeqCst) + 1;
    let job = Arc::new(Mutex::new(Job::default()));
    jobs.all
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, job.clone());
    let handle = app.clone();
    std::thread::spawn(move || {
        let me = handle.state::<App>();
        let done = match helper_request(&me, &req, &job) {
            Ok(v) => v,
            Err(e) => json!({ "error": e }),
        };
        job.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .done = Some(done);
    });
    id
}

#[tauri::command]
pub fn helper_start(
    req: Value,
    webview: tauri::Webview,
    app: AppHandle,
    me: State<App>,
) -> Result<u64, String> {
    let req = vet(&webview, &me, req)?;
    Ok(start_job(&app, req))
}

#[tauri::command]
pub fn helper_poll(
    job: u64,
    seen: Option<u64>,
    webview: tauri::Webview,
    jobs: State<Jobs>,
    me: State<App>,
) -> Result<Value, String> {
    let seen = seen.unwrap_or(0);
    may_follow(&webview, &me)?;
    let j = jobs
        .all
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&job)
        .cloned()
        .ok_or("no such job")?;
    let j = j.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // The preview only when it's newer than what the window has.
    let preview = (j.preview_n > seen).then(|| j.preview.clone()).flatten();
    // Only the newest line: the window shows one at a time.
    Ok(
        json!({ "line": j.lines.last(), "done": j.done, "preview": preview, "preview_n": j.preview_n, "thought": j.thought }),
    )
}

#[tauri::command]
pub fn helper_stop(
    job: u64,
    webview: tauri::Webview,
    jobs: State<Jobs>,
    me: State<App>,
) -> Result<(), String> {
    may_follow(&webview, &me)?;
    if let Some(j) = jobs
        .all
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&job)
    {
        #[cfg(unix)]
        if let Some(c) = j
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .conn
            .take()
        {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }
    Ok(())
}
