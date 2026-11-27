//! Provider keys. On Windows they live in Credential Manager (see the end
//! of this file). On a Mac they live in the login Keychain (service
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
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

const SERVICE: &str = "OpenAs";
/// Where keys were kept before the rename: read as a fallback, never copied.
#[cfg(target_os = "macos")]
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

// Elsewhere: the system's credential store (Windows Credential Manager),
// one item per provider under the same service name.
#[cfg(not(target_os = "macos"))]
fn entry(service: &str, provider: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(service, provider).ok()
}

#[cfg(not(target_os = "macos"))]
pub fn get(provider: &str) -> Option<String> {
    let read = |s: &str| {
        entry(s, provider)?
            .get_password()
            .ok()
            .filter(|k| !k.is_empty())
    };
    read(&service()).or_else(|| (service() != SERVICE).then(|| read(SERVICE)).flatten())
}

#[cfg(not(target_os = "macos"))]
pub fn set(provider: &str, key: &str) -> Result<()> {
    let ok = entry(&service(), provider).is_some_and(|e| e.set_password(key).is_ok());
    if !ok {
        bail!("couldn't save the key in the credential store");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn remove(provider: &str) {
    if let Some(e) = entry(&service(), provider) {
        let _ = e.delete_credential();
    }
}
