use super::*;

// ---- Live state ---------------------------------------------------------------

/// What the person sees while an app is made or changed, written to
/// `<dir>/making.json` (atomically, at most a few times a second). The window
/// reads it; notes come back in `<dir>/notes.jsonl`, Stop as `<dir>/stop`.
pub struct Live {
    pub(super) dir: PathBuf,
    pub(super) state: std::sync::Mutex<Value>,
    last: std::sync::Mutex<std::time::Instant>,
}

impl Live {
    pub fn new(dir: &Path, request: &str, kind: &str) -> Self {
        let _ = std::fs::remove_file(dir.join("stop"));
        let _ = std::fs::remove_file(dir.join("notes.jsonl"));
        let st = json!({ "kind": kind, "phase": "planning", "request": request, "started": now(), "step": 0, "checks": 0,
                         "thinking": "", "plan": null, "part": 0, "doing": "", "kb": 0, "notes": [], "ghost": null, "files_seq": 0 });
        let l = Live {
            dir: dir.to_path_buf(),
            state: std::sync::Mutex::new(st),
            last: std::sync::Mutex::new(std::time::Instant::now()),
        };
        l.flush(true);
        l
    }
    pub fn set(&self, k: &str, v: Value) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[k] = v;
        self.flush(false);
    }
    pub fn set_now(&self, k: &str, v: Value) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[k] = v;
        self.flush(true);
    }
    pub fn get(&self, k: &str) -> Value {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[k]
            .clone()
    }
    pub fn bump(&self, k: &str) -> u64 {
        let mut st = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = st[k].as_u64().unwrap_or(0) + 1;
        st[k] = json!(n);
        n
    }
    /// Remembers what a step of the plan did and which file it wrote, for
    /// the step's own line to open into: `steps["2"] = {did: [...], files: [...]}`.
    pub fn note_step(&self, part: u64, did: Option<&str>, file: Option<&str>) {
        if part == 0 {
            return;
        }
        let mut st = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !st["steps"].is_object() {
            st["steps"] = json!({});
        }
        let e = &mut st["steps"][part.to_string()];
        if !e.is_object() {
            *e = json!({ "did": [], "files": [] });
        }
        for (k, v) in [("did", did), ("files", file)] {
            let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else { continue };
            if let Some(a) = e[k].as_array_mut() {
                // Said once each, in the order they happened, and never a wall.
                if !a.iter().any(|x| x == v) && a.len() < 12 {
                    a.push(json!(v));
                }
            }
        }
    }
    pub fn flush(&self, force: bool) {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !force && last.elapsed().as_millis() < 250 {
            return;
        }
        *last = std::time::Instant::now();
        let mut st = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        st["now"] = json!(now());
        let tmp = self.dir.join(".making.tmp");
        if std::fs::write(&tmp, st.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, self.dir.join("making.json"));
        }
    }
    pub fn stopped(&self) -> bool {
        self.dir.join("stop").exists()
    }
    /// Notes the person typed since the last call.
    pub fn new_notes(&self) -> Vec<String> {
        let all: Vec<String> = std::fs::read_to_string(self.dir.join("notes.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                serde_json::from_str::<Value>(l)
                    .ok()?
                    .get("text")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        let mut st = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seen = st["notes"].as_array().map(|a| a.len()).unwrap_or(0);
        let fresh: Vec<String> = all.into_iter().skip(seen).collect();
        if let Some(notes) = st["notes"].as_array_mut() {
            notes.extend(fresh.iter().map(|n| json!({ "text": n, "t": now() })));
        }
        drop(st);
        if !fresh.is_empty() {
            self.flush(true);
        }
        fresh
    }
}
