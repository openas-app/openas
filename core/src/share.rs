//! Sharing an app as one file (`<Name>.openas`): its code and icon, never
//! its data, permissions or history. Opening the file installs a copy that
//! starts with no permissions and can never get full access; after that it's
//! the receiver's own app, changed by asking like any other (remix).
//!
//! A shared file is someone else's code, so it's unpacked into a temporary
//! folder and checked before anything is installed: only the expected files,
//! no links, no paths outside, and size limits.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{install, make, paths};

const MAX_BYTES: u64 = 30 << 20;
const MAX_FILES: usize = 2000;

/// Writes `<Name>.openas` into `dir` (or Downloads) and returns its path.
pub fn export(app_folder: &Path, dir: Option<&Path>) -> Result<PathBuf> {
    let info = make::read_info(app_folder)?;
    let stage = std::env::temp_dir().join(format!("openas-share-{}", unique()));
    std::fs::create_dir_all(&stage)?;
    let r = (|| -> Result<PathBuf> {
        copy_tree(&app_folder.join("app"), &stage.join("app"))?;
        if let Ok(svg) = std::fs::read_to_string(app_folder.join("icon.svg")) {
            std::fs::write(stage.join("icon.svg"), svg)?;
        }
        let manifest = json!({ "format": 1, "name": info.name, "made_with": info.model, "shared": make::now() });
        std::fs::write(
            stage.join("openas.json"),
            serde_json::to_string_pretty(&manifest)?,
        )?;
        let out_dir = dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dirs::download_dir().unwrap_or_else(std::env::temp_dir));
        let safe: String = info
            .name
            .chars()
            .map(|c| if "/:\\".contains(c) { '-' } else { c })
            .collect();
        let mut out = out_dir.join(format!("{safe}.openas"));
        let mut n = 2;
        while out.exists() {
            out = out_dir.join(format!("{safe} {n}.openas"));
            n += 1;
        }
        let ok = Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--norsrc", "--noextattr"])
            .arg(&stage)
            .arg(&out)
            .status()?
            .success();
        if !ok {
            bail!("couldn't write the shared file");
        }
        Ok(out)
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

/// A received file, unpacked and checked, not yet installed.
pub struct Received {
    pub dir: PathBuf,
    pub name: String,
    pub made_with: String,
}

impl Drop for Received {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Unpacks a shared file into a temporary folder and checks every entry.
pub fn open(file: &Path) -> Result<Received> {
    if std::fs::metadata(file).context("the shared file")?.len() > MAX_BYTES {
        bail!("This file is too big to be a shared app.");
    }
    let dir = std::env::temp_dir().join(format!("openas-received-{}", unique()));
    std::fs::create_dir_all(&dir)?;
    let received = Received {
        dir: dir.clone(),
        name: String::new(),
        made_with: String::new(),
    };
    let ok = Command::new("/usr/bin/ditto")
        .args(["-x", "-k", "--norsrc", "--noextattr"])
        .arg(file)
        .arg(&dir)
        .status()?
        .success();
    if !ok {
        bail!("This isn't a shared app OpenAs can open.");
    }
    let (mut bytes, mut files) = (0u64, 0usize);
    check_tree(&dir, &dir, &mut bytes, &mut files)?;
    if bytes > MAX_BYTES * 2 || files > MAX_FILES {
        bail!("This shared app is too big.");
    }
    for e in std::fs::read_dir(&dir)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !matches!(n.as_str(), "app" | "icon.svg" | "openas.json") {
            bail!("This shared app has something unexpected in it ({n}).");
        }
    }
    if !dir.join("app/index.html").is_file() {
        bail!("This shared app has no page to show.");
    }
    let m: Value = std::fs::read_to_string(dir.join("openas.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let name: String = m["name"]
        .as_str()
        .unwrap_or("Shared app")
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    let made_with: String = m["made_with"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(80)
        .collect();
    let mut r = received;
    r.name = if name.is_empty() {
        "Shared app".into()
    } else {
        name
    };
    r.made_with = made_with;
    Ok(r)
}

/// Its files as text, for Show code before installing.
pub fn code(r: &Received) -> Vec<Value> {
    let mut out = vec![];
    let mut stack = vec![r.dir.join("app")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(t) = std::fs::read_to_string(&p) {
                let rel = p
                    .strip_prefix(r.dir.join("app"))
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .into_owned();
                out.push(
                    json!({ "name": rel, "text": t.chars().take(400_000).collect::<String>() }),
                );
            }
        }
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    out
}

pub fn icon(r: &Received) -> String {
    std::fs::read_to_string(r.dir.join("icon.svg"))
        .ok()
        .filter(|s| s.contains("<svg"))
        .unwrap_or_else(|| install::fallback_icon("shared"))
}

/// Installs a received app under `name`, marked as shared.
pub fn install(r: &Received, name: &str) -> Result<make::AppInfo> {
    let draft = paths::apps().join(format!(".import-{}", unique()));
    std::fs::create_dir_all(&draft)?;
    copy_tree(&r.dir.join("app"), &draft.join("app"))?;
    std::fs::write(draft.join("icon.svg"), icon(r))?;
    std::fs::write(
        draft.join("prompt.txt"),
        format!("Shared with you: {}", r.name),
    )?;
    std::fs::write(
        draft.join("draft.json"),
        json!({ "provider": "shared", "model": r.made_with }).to_string(),
    )?;
    let mut info = make::install_draft(
        &draft,
        if name.trim().is_empty() {
            &r.name
        } else {
            name
        },
    )?;
    info.shared = true;
    make::write_info(&paths::apps().join(&info.id), &info)?;
    Ok(info)
}

fn unique() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{n}", std::process::id())
}

fn check_tree(root: &Path, d: &Path, bytes: &mut u64, files: &mut usize) -> Result<()> {
    for e in std::fs::read_dir(d)?.flatten() {
        let p = e.path();
        let meta = std::fs::symlink_metadata(&p)?;
        if meta.file_type().is_symlink() {
            bail!("This shared app contains a link, which isn't allowed.");
        }
        if !p.starts_with(root) {
            bail!("This shared app reaches outside itself.");
        }
        if meta.is_dir() {
            check_tree(root, &p, bytes, files)?;
        } else if meta.is_file() {
            *bytes += meta.len();
            *files += 1;
        } else {
            bail!("This shared app contains something that isn't a file.");
        }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        let meta = std::fs::symlink_metadata(&p)?;
        if meta.is_dir() {
            copy_tree(&p, &to.join(e.file_name()))?;
        } else if meta.is_file() {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}
