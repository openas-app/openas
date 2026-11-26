use super::*;

// ---- Paths ---------------------------------------------------------------------

/// Resolves a page's relative path inside `root`. `must_exist` paths are
/// resolved fully; a new path's parent must exist and be inside.
pub fn resolve(root: &Path, rel: &str, must_exist: bool) -> Result<PathBuf, String> {
    if rel.contains('\0') {
        return Err("bad path".into());
    }
    let rel_p = Path::new(rel);
    for c in rel_p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(format!("“{rel}” is outside the folder")),
        }
    }
    let root = root
        .canonicalize()
        .map_err(|_| "the folder is gone".to_string())?;
    let p = root.join(rel_p);
    let inside = |c: &Path| -> Result<(), String> {
        if c.starts_with(&root) {
            Ok(())
        } else {
            Err(format!("“{rel}” is outside the folder"))
        }
    };
    if must_exist || p.symlink_metadata().is_ok() {
        let c = p
            .canonicalize()
            .map_err(|_| format!("“{rel}” doesn't exist"))?;
        inside(&c)?;
        return Ok(c);
    }
    let parent = p.parent().ok_or("bad path")?;
    let pc = parent
        .canonicalize()
        .map_err(|_| format!("the folder for “{rel}” doesn't exist"))?;
    inside(&pc)?;
    Ok(pc.join(p.file_name().ok_or("bad path")?))
}

pub(crate) fn entry(root: &Path, p: &Path) -> Value {
    let m = std::fs::metadata(p).ok();
    let rel = p
        .strip_prefix(root)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_default();
    let time = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    };
    json!({
        "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "path": rel,
        "dir": m.as_ref().is_some_and(|m| m.is_dir()),
        "size": m.as_ref().map(|m| m.len()).unwrap_or(0),
        "modified": m.as_ref().and_then(|m| time(m.modified())),
        "created": m.as_ref().and_then(|m| time(m.created())),
        "ext": p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default(),
    })
}

pub(crate) fn mime_of(p: &Path) -> &'static str {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("heic") => "image/heic",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("txt" | "md") => "text/plain",
        _ => "application/octet-stream",
    }
}

pub(crate) fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// One file operation inside `root`.
pub fn file_op(root: &Path, op: &str, a: &Value) -> Result<Value, String> {
    let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("");
    match op {
        "list" => {
            let dir = resolve(root, s("path"), true)?;
            let root_c = root.canonicalize().map_err(|e| e.to_string())?;
            let mut out = vec![];
            for e in std::fs::read_dir(&dir)
                .map_err(|e| e.to_string())?
                .flatten()
            {
                let name = e.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                // A link that leads outside the folder isn't shown at all.
                if !e
                    .path()
                    .canonicalize()
                    .is_ok_and(|c| c.starts_with(&root_c))
                {
                    continue;
                }
                out.push(entry(&root_c, &e.path()));
                if out.len() >= MAX_LIST {
                    break;
                }
            }
            Ok(json!(out))
        }
        "stat" => {
            let p = resolve(root, s("path"), true)?;
            Ok(entry(&root.canonicalize().map_err(|e| e.to_string())?, &p))
        }
        "readText" => {
            let p = resolve(root, s("path"), true)?;
            if std::fs::metadata(&p).map_err(|e| e.to_string())?.len() > MAX_READ {
                return Err("that file is too big to read as text".into());
            }
            let b = std::fs::read(&p).map_err(|e| e.to_string())?;
            Ok(json!(String::from_utf8_lossy(&b)))
        }
        "readDataURL" => {
            let p = resolve(root, s("path"), true)?;
            if std::fs::metadata(&p).map_err(|e| e.to_string())?.len() > MAX_DATA_URL {
                return Err("that file is too big".into());
            }
            let b = std::fs::read(&p).map_err(|e| e.to_string())?;
            Ok(json!(format!("data:{};base64,{}", mime_of(&p), b64(&b))))
        }
        "writeText" => {
            let text = s("text");
            if text.len() > MAX_WRITE {
                return Err("too much to write at once".into());
            }
            let p = resolve(root, s("path"), false)?;
            if p.is_dir() {
                return Err("that's a folder".into());
            }
            std::fs::write(&p, text).map_err(|e| e.to_string())?;
            Ok(json!(true))
        }
        "mkdir" => {
            // One level at a time, so every parent is checked.
            let rel = s("path");
            let mut cur = PathBuf::new();
            for c in Path::new(rel).components() {
                cur.push(c);
                let p = resolve(root, &cur.to_string_lossy(), false)?;
                if !p.exists() {
                    std::fs::create_dir(&p).map_err(|e| e.to_string())?;
                }
            }
            Ok(json!(true))
        }
        "move" | "copy" => {
            let from = resolve(root, s("from"), true)?;
            let to = resolve(root, s("to"), false)?;
            if to.symlink_metadata().is_ok() {
                return Err(format!("“{}” already exists", s("to")));
            }
            if op == "move" {
                std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
            } else {
                if from.is_dir() {
                    return Err("only files can be copied".into());
                }
                std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
            }
            Ok(json!(true))
        }
        "trash" => {
            let p = resolve(root, s("path"), true)?;
            if p == root.canonicalize().map_err(|e| e.to_string())? {
                return Err("the folder itself can't be trashed".into());
            }
            trash::delete(&p).map_err(|e| e.to_string())?;
            Ok(json!(true))
        }
        "reveal" => {
            let p = resolve(root, s("path"), true)?;
            std::process::Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&p)
                .status()
                .map_err(|e| e.to_string())?;
            Ok(json!(true))
        }
        _ => Err(format!("unknown file call “{op}”")),
    }
}
