//! Sharing an app as one file (`<Name>.openas`): its code and icon, never
//! its data, permissions or history. Opening the file installs a copy that
//! starts with no permissions and can never get full access; after that it's
//! the receiver's own app, changed by asking like any other (remix).
//!
//! A shared file is someone else's code, so it's unpacked into a temporary
//! folder and checked before anything is installed: only the expected files,
//! no links, no paths outside, and size limits.

use std::path::{Path, PathBuf};

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
        zip_dir(&stage, &out).context("couldn't write the shared file")?;
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
    unzip(file, &dir)?;
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

/// A zip of the folder's contents (paths relative to it, `/` between
/// parts), the layout `ditto -c -k` wrote before.
fn zip_dir(dir: &Path, out: &Path) -> Result<()> {
    use std::io::Write;
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut z = zip::ZipWriter::new(std::fs::File::create(out)?);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let rel = p
                .strip_prefix(dir)?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.is_dir() {
                z.add_directory(format!("{rel}/"), opts)?;
                stack.push(p);
            } else if meta.is_file() {
                z.start_file(rel, opts)?;
                z.write_all(&std::fs::read(&p)?)?;
            }
        }
    }
    z.finish()?;
    Ok(())
}

/// Unpacks a shared file. Every entry must stay inside `dir` (no absolute
/// paths, no `..`), links are refused, and what comes out is capped, so a
/// small file can't unpack into a huge one.
fn unzip(file: &Path, dir: &Path) -> Result<()> {
    use std::io::Read;
    let bad = || anyhow::anyhow!("This isn't a shared app OpenAs can open.");
    let mut z = zip::ZipArchive::new(std::fs::File::open(file)?).map_err(|_| bad())?;
    if z.len() > MAX_FILES * 2 {
        bail!("This shared app is too big.");
    }
    let mut left = MAX_BYTES * 2;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|_| bad())?;
        let Some(rel) = f.enclosed_name() else {
            bail!("This shared app reaches outside itself.");
        };
        if f.is_symlink() {
            bail!("This shared app contains a link, which isn't allowed.");
        }
        let to = dir.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&to)?;
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::new();
        (&mut f)
            .take(left + 1)
            .read_to_end(&mut buf)
            .map_err(|_| bad())?;
        if buf.len() as u64 > left {
            bail!("This shared app is too big.");
        }
        left -= buf.len() as u64;
        std::fs::write(&to, buf)?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-share-{tag}-{}", unique()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(d: &Path) {
        std::fs::create_dir_all(d.join("app/img")).unwrap();
        std::fs::write(d.join("app/index.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(d.join("app/img/a.svg"), "<svg/>").unwrap();
        std::fs::write(d.join("icon.svg"), "<svg/>").unwrap();
        std::fs::write(d.join("openas.json"), r#"{"format":1,"name":"Tea"}"#).unwrap();
    }

    #[test]
    fn a_shared_file_round_trips() {
        let (src, out) = (tmp("src"), tmp("out"));
        sample(&src);
        let file = out.join("Tea.openas");
        zip_dir(&src, &file).unwrap();
        let r = open(&file).unwrap();
        assert_eq!(r.name, "Tea");
        assert_eq!(
            std::fs::read_to_string(r.dir.join("app/img/a.svg")).unwrap(),
            "<svg/>"
        );
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out);
    }

    /// Files shared from OpenAs before this change were written by ditto.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_file_written_by_ditto_still_opens() {
        let (src, out) = (tmp("dsrc"), tmp("dout"));
        sample(&src);
        let file = out.join("Tea.openas");
        assert!(std::process::Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--norsrc", "--noextattr"])
            .arg(&src)
            .arg(&file)
            .status()
            .unwrap()
            .success());
        let r = open(&file).unwrap();
        assert_eq!(r.name, "Tea");
        assert!(r.dir.join("app/index.html").is_file());
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out);
    }

    #[test]
    fn a_file_that_reaches_outside_is_refused() {
        use std::io::Write;
        let out = tmp("evil");
        let file = out.join("Evil.openas");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("app/index.html", o).unwrap();
        z.write_all(b"<h1>x</h1>").unwrap();
        z.start_file("../escaped.txt", o).unwrap();
        z.write_all(b"x").unwrap();
        z.finish().unwrap();
        let e = open(&file).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains("outside"), "{e}");
        assert!(!out.parent().unwrap().join("escaped.txt").exists());
        let _ = std::fs::remove_dir_all(out);
    }
}
