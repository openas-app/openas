//! Provider keys. On a Mac they live in the login Keychain (service
//! "OpenAs", one item per provider), entered once in the OpenAs window.
//! They are written and read through Apple's `security` tool, so rebuilding
//! OpenAs never brings up a Keychain prompt. That keeps keys out of files
//! and backups. It does not hide them from other programs running as you:
//! any of them can ask `security` for the item, and while a key is being
//! saved it is briefly visible in that child process's arguments.
//!
//! A key never appears in progress lines, logs or the usage file; the
//! settings only say whether one is set.

use anyhow::{bail, Result};
use std::process::{Command, Stdio};

const SERVICE: &str = "OpenAs";
/// Where keys were kept before the rename: read as a fallback, never copied.
const OLD_SERVICE: &str = "Open Apps";

fn service() -> String {
    // A test setup keeps its keys apart from the real ones.
    match std::env::var("OPENAS_ID_PREFIX") {
        Ok(p) if p != "app.openas." => format!("{SERVICE} ({p})"),
        _ => SERVICE.into(),
    }
}

#[cfg(target_os = "macos")]
fn read(service: &str, provider: &str) -> Option<String> {
    let out = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", provider, "-w"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let k = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !k.is_empty()).then_some(k)
}

/// A test setup reads its own item first, then the real one (read only, never
/// copied), so the one real key serves both.
#[cfg(target_os = "macos")]
pub fn get(provider: &str) -> Option<String> {
    read(&service(), provider)
        .or_else(|| {
            (service() != SERVICE)
                .then(|| read(SERVICE, provider))
                .flatten()
        })
        .or_else(|| read(OLD_SERVICE, provider))
}

#[cfg(target_os = "macos")]
pub fn set(provider: &str, key: &str) -> Result<()> {
    // -U updates an existing item. The key is passed as an argument to a
    // short-lived child, so for that moment `ps` can show it to this user's
    // processes (see the note at the top).
    let ok = Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            &service(),
            "-a",
            provider,
            "-l",
            &format!("{SERVICE}: {provider}"),
            "-w",
            key,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success();
    if !ok {
        bail!("couldn't save the key in the Keychain");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn remove(provider: &str) {
    let _ = Command::new("/usr/bin/security")
        .args(["delete-generic-password", "-s", &service(), "-a", provider])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// Elsewhere (until Windows Credential Manager is wired up): a file only this
// user can read, in the data folder.
#[cfg(not(target_os = "macos"))]
fn file() -> std::path::PathBuf {
    crate::paths::root().join("keys.json")
}

#[cfg(not(target_os = "macos"))]
pub fn get(provider: &str) -> Option<String> {
    let m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file()).ok()?).ok()?;
    m.get(provider)?.as_str().map(str::to_owned)
}

#[cfg(not(target_os = "macos"))]
pub fn set(provider: &str, key: &str) -> Result<()> {
    let mut m: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    m.insert(provider.into(), key.into());
    std::fs::create_dir_all(crate::paths::root())?;
    std::fs::write(file(), serde_json::to_string(&m)?)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn remove(provider: &str) {
    if let Some(mut m) = std::fs::read_to_string(file())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&t).ok())
    {
        m.remove(provider);
        let _ = std::fs::write(file(), serde_json::to_string(&m).unwrap_or_default());
    }
}
