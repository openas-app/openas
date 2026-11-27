//! Windows: an installed app is a folder `<dir>\<Name>\` holding `<Name>.exe`
//! (a copy of Runner.exe), `openas-app.json` (its identity) and `app.ico`,
//! plus a Start menu shortcut that carries the app's AppUserModelID, so the
//! taskbar shows and groups it as its own program.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, SetCurrentProcessExplicitAppUserModelID, ShellLink};

use crate::Identity;

pub(crate) const IDENTITY_FILE: &str = "openas-app.json";
const ICON_FILE: &str = "app.ico";

pub fn default_install_dir() -> PathBuf {
    local_app_data().join("Programs").join("OpenAs")
}

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Local"))
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn runner_bundle_in(target: &Path) -> PathBuf {
    target.join("Runner")
}

/// `<X>\<X>.exe`; the prebuilt bundle's is Runner.exe.
pub fn exe_in(bundle: &Path) -> PathBuf {
    let name = bundle
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Runner".into());
    bundle.join(format!("{name}.exe"))
}

/// The folder an exe sits in, when the exe is that folder's own program.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let stem = exe.file_stem()?;
    let ext = exe.extension()?;
    (ext.eq_ignore_ascii_case("exe")
        && dir
            .file_name()
            .is_some_and(|d| d.eq_ignore_ascii_case(stem)))
    .then(|| dir.to_path_buf())
}

pub fn install_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

pub fn is_install(path: &Path) -> bool {
    path.join(IDENTITY_FILE).is_file()
}

pub fn make_copy(
    runner: &Path,
    dest: &Path,
    who: &Identity,
    icon: &dyn Fn(u32) -> Result<Vec<u8>>,
) -> Result<()> {
    let runner_exe = exe_in(runner);
    if !runner_exe.is_file() {
        bail!("no Runner.exe in {}", runner.display());
    }
    if dest.exists() {
        remove_install(dest);
        if dest.exists() {
            bail!(
                "couldn't replace {} (is the app still open?)",
                dest.display()
            );
        }
    }
    std::fs::create_dir_all(dest)?;
    // Everything the Runner ships with, its program taking the app's name.
    for e in std::fs::read_dir(runner)? {
        let e = e?;
        if !e.file_type()?.is_file() {
            continue;
        }
        let to = if e.path() == runner_exe {
            exe_in(dest)
        } else {
            dest.join(e.file_name())
        };
        std::fs::copy(e.path(), &to).with_context(|| format!("copying {}", e.path().display()))?;
    }
    std::fs::write(dest.join(IDENTITY_FILE), serde_json::to_vec_pretty(who)?)?;
    let ico = dest.join(ICON_FILE);
    std::fs::write(&ico, make_ico(icon)?)?;
    // Only the real setup gets Start menu entries and the file type; a test
    // setup (its own install folder) leaves the person's menu alone.
    if std::env::var_os("OPENAS_INSTALL_DIR").is_none() {
        shortcut(&exe_in(dest), &ico, &who.name, &who.id)?;
        if who.manager {
            let _ = register_shared_files(&exe_in(dest), &ico);
        }
    }
    Ok(())
}

/// An .ico holding PNGs at the sizes Windows uses (Vista and later read PNG
/// entries directly).
fn make_ico(icon: &dyn Fn(u32) -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 256];
    let pngs = sizes.iter().map(|&s| icon(s)).collect::<Result<Vec<_>>>()?;
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, png) in sizes.iter().zip(&pngs) {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[b, b, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in &pngs {
        out.extend_from_slice(png);
    }
    Ok(out)
}

fn start_menu() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Roaming"))
        .join(r"Microsoft\Windows\Start Menu\Programs")
}

fn shortcut_path(name: &str) -> PathBuf {
    start_menu().join(format!("{name}.lnk"))
}

/// The Start menu shortcut: the app's program, icon and AppUserModelID (the
/// id its windows and notifications carry, so the taskbar groups them).
fn shortcut(exe: &Path, ico: &Path, name: &str, id: &str) -> Result<()> {
    // SAFETY: plain COM calls on this thread; every pointer passed is a live
    // HSTRING or a local, and the interfaces are released when dropped.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetIconLocation(&HSTRING::from(ico.as_os_str()), 0)?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        let store: IPropertyStore = link.cast()?;
        // PKEY_AppUserModel_ID
        let key = PROPERTYKEY {
            fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
            pid: 5,
        };
        store.SetValue(
            &key,
            &windows::Win32::System::Com::StructuredStorage::PROPVARIANT::from(id),
        )?;
        store.Commit()?;
        let file: IPersistFile = link.cast()?;
        let path = shortcut_path(name);
        std::fs::create_dir_all(start_menu())?;
        file.Save(&HSTRING::from(path.as_os_str()), true)?;
    }
    Ok(())
}

/// `.openas` files open with OpenAs (per user, no admin rights).
fn register_shared_files(exe: &Path, ico: &Path) -> Result<()> {
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    let set = |key: &str, value: &str| -> Result<()> {
        let v: Vec<u16> = value.encode_utf16().chain([0]).collect();
        // SAFETY: the key and value are NUL-terminated UTF-16 buffers that
        // outlive the call; the byte count matches the buffer.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                PCWSTR::null(),
                REG_SZ.0,
                Some(v.as_ptr() as *const _),
                (v.len() * 2) as u32,
            )
            .ok()?;
        }
        Ok(())
    };
    set(r"Software\Classes\.openas", "OpenAs.Shared")?;
    set(r"Software\Classes\OpenAs.Shared", "OpenAs shared app")?;
    set(
        r"Software\Classes\OpenAs.Shared\DefaultIcon",
        &ico.to_string_lossy(),
    )?;
    set(
        r"Software\Classes\OpenAs.Shared\shell\open\command",
        &format!("\"{}\" \"%1\"", exe.display()),
    )?;
    // SAFETY: no item pointers are passed with SHCNE_ASSOCCHANGED.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

pub fn making_copy(runner: &Path, dir: &Path) -> Result<PathBuf> {
    let copy = dir.join("New app");
    std::fs::create_dir_all(&copy)?;
    let runner_exe = exe_in(runner);
    for e in std::fs::read_dir(runner)? {
        let e = e?;
        if e.file_type()?.is_file() {
            let to = if e.path() == runner_exe {
                exe_in(&copy)
            } else {
                copy.join(e.file_name())
            };
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(exe_in(&copy))
}

pub fn read_identity(install: &Path) -> Option<Identity> {
    let who: Identity =
        serde_json::from_slice(&std::fs::read(install.join(IDENTITY_FILE)).ok()?).ok()?;
    (!who.folder.as_os_str().is_empty()).then_some(who)
}

/// SHA-256 over every file in the folder (names and contents, in order), so
/// a changed program, identity or a library dropped beside the program all
/// break it. A copy with a subfolder isn't one we made.
pub fn seal(install: &Path) -> Option<String> {
    let mut files: Vec<_> = std::fs::read_dir(install)
        .ok()?
        .map(|e| e.ok())
        .collect::<Option<Vec<_>>>()?;
    files.sort_by_key(|e| e.file_name());
    let mut h = Sha256::new();
    for e in files {
        if !e.file_type().ok()?.is_file() {
            return None;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        h.update((name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
        let bytes = std::fs::read(e.path()).ok()?;
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    Some(
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
}

pub fn sealed(install: &Path, recorded: &str) -> bool {
    !recorded.is_empty() && seal(install).as_deref() == Some(recorded)
}

pub fn remove_install(install: &Path) {
    if let Some(who) = read_identity(install) {
        let lnk = shortcut_path(&who.name);
        if lnk.exists() {
            let _ = std::fs::remove_file(lnk);
        }
    }
    let _ = std::fs::remove_dir_all(install);
}

pub fn launch(install: &Path, _quiet: bool) -> Result<()> {
    let exe = if install.is_dir() {
        exe_in(install)
    } else {
        install.to_path_buf()
    };
    Command::new(&exe)
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    Ok(())
}

pub fn reveal(path: &Path) {
    let _ = Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

pub fn adopt_identity(id: &str) {
    // SAFETY: the HSTRING outlives the call.
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(id));
    }
}

pub fn closed_folders(home: &Path) -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut v: Vec<PathBuf> = [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ]
    .iter()
    .filter_map(|k| env(k))
    .collect();
    v.push(home.join("AppData"));
    v.push(default_install_dir());
    v
}

pub fn allowed_name(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase();
    !name.is_empty()
        && !name.contains(['/', ':', '\\', '<', '>', '"', '|', '?', '*'])
        && !name.ends_with(['.', ' '])
        && !name.starts_with('.')
        && !RESERVED.contains(&stem.as_str())
        && !name.eq_ignore_ascii_case("Runner")
}
