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

/// Where installed apps go: ~/Applications on a Mac,
/// %LOCALAPPDATA%\Programs\OpenAs on Windows.
pub fn install_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OPENAS_INSTALL_DIR") {
        return PathBuf::from(d);
    }
    openas_platform::default_install_dir()
}

/// The prebuilt Runner that installs copy (Runner.app, or the Runner folder
/// on Windows). In development it sits in the workspace's target folder; a
/// release ships it inside OpenAs.
pub fn runner_bundle() -> PathBuf {
    if let Ok(r) = std::env::var("OPENAS_RUNNER") {
        return PathBuf::from(r);
    }
    openas_platform::runner_bundle_in(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"))
}

/// The Runner program itself, used for the load check.
pub fn runner_exe() -> PathBuf {
    openas_platform::exe_in(&runner_bundle())
}

/// The `openas` program that serves as the Helper: the one running now when
/// that is it, else the one beside it (the MCP server and the Helper are the
/// same program).
pub fn helper_exe(current: &std::path::Path) -> PathBuf {
    if let Ok(h) = std::env::var("OPENAS_HELPER") {
        return PathBuf::from(h);
    }
    if current.file_stem().is_some_and(|n| n == "openas") {
        return current.to_path_buf();
    }
    current.with_file_name(format!("openas{}", std::env::consts::EXE_SUFFIX))
}

/// Where the Helper of this setup listens (a socket, or a pipe on Windows).
pub fn socket() -> PathBuf {
    openas_platform::ipc::address(&root())
}
