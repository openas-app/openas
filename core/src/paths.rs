//! Where things live. One root per user; each app is a plain folder in it.
//!
//! <root>/apps/<id>/app.json        name, icon, created, model
//! <root>/apps/<id>/app/index.html  the code (rewritten on each change)
//! <root>/apps/<id>/icon.png        the icon, 1024 px
//! <root>/apps/<id>/data/           what the app saved (never touched by changes)
//! <root>/apps/<id>/history/<n>/    earlier versions of the code, for undo

use std::path::PathBuf;

pub fn root() -> PathBuf {
    if let Ok(r) = std::env::var("OPENAS_ROOT") {
        return PathBuf::from(r);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OpenAs")
}

pub fn apps() -> PathBuf {
    root().join("apps")
}

/// Where installed apps go: ~/Applications on a Mac.
pub fn install_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OPENAS_INSTALL_DIR") {
        return PathBuf::from(d);
    }
    dirs::home_dir().unwrap_or_default().join("Applications")
}

/// The prebuilt Runner that installs copy. In development it sits in the
/// workspace's target folder; a release ships it inside OpenAs.app.
pub fn runner_bundle() -> PathBuf {
    if let Ok(r) = std::env::var("OPENAS_RUNNER") {
        return PathBuf::from(r);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/Runner.app")
}

/// The Runner binary itself, used for the load check.
pub fn runner_exe() -> PathBuf {
    runner_bundle().join("Contents/MacOS/Runner")
}

/// The `openas` binary that serves as the Helper: the one running now when
/// that is it, else the one beside it (the MCP server and the Helper are the
/// same binary).
pub fn helper_exe(current: &std::path::Path) -> PathBuf {
    if let Ok(h) = std::env::var("OPENAS_HELPER") {
        return PathBuf::from(h);
    }
    if current.file_name().is_some_and(|n| n == "openas") {
        return current.to_path_buf();
    }
    current.with_file_name("openas")
}

pub fn socket() -> PathBuf {
    socket_for(&root())
}

/// The Helper's socket: in the data folder, unless that path is too long
/// for a Unix socket (about 104 bytes on a Mac); then a short one in the
/// temp folder, named after the data folder so setups never share one.
pub fn socket_for(root: &std::path::Path) -> PathBuf {
    let p = root.join("helper.sock");
    if p.as_os_str().len() < 100 {
        return p;
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in root.to_string_lossy().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    std::env::temp_dir().join(format!("openas-{h:016x}.sock"))
}
