//! Everything in OpenAs that differs between macOS and Windows lives here,
//! behind one set of functions; no other crate checks which system it's on
//! (the Runner's window code aside).
//!
//! - An installed app is a copy of the prebuilt Runner made into that app:
//!   on a Mac `<dir>/<Name>.app`, on Windows `<dir>\<Name>\<Name>.exe` with
//!   `openas-app.json` beside it and a Start menu shortcut.
//! - The Helper listens on a Unix socket (Mac) or a named pipe (Windows) and
//!   names its caller by the caller's program file.
//! - Programs an app runs are stopped with everything they started: a process
//!   group (Mac) or a Job Object (Windows).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as os;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as os;

pub mod ipc;
pub mod proc;

/// What makes a Runner copy a particular app. Written into the copy at
/// install (Info.plist on a Mac, openas-app.json on Windows) and read by the
/// Runner when it starts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    /// The app's folder (`<root>/apps/<id>`).
    pub folder: PathBuf,
    /// The name people see.
    pub name: String,
    /// `app.openas.<id>`: Launch Services, notifications, the taskbar.
    pub id: String,
    /// The `openas` program that runs the Helper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper: Option<PathBuf>,
    /// The OpenAs window itself (may ask the Helper to make apps).
    #[serde(default)]
    pub manager: bool,
    /// A non-default setup (OPENAS_ROOT, OPENAS_INSTALL_DIR,
    /// OPENAS_ID_PREFIX), carried to apps opened without an environment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
}

/// The environment variables a copy carries (see `Identity::env`).
pub const CARRIED_ENV: [&str; 3] = ["OPENAS_ROOT", "OPENAS_INSTALL_DIR", "OPENAS_ID_PREFIX"];

/// Where installed apps go by default: ~/Applications on a Mac,
/// %LOCALAPPDATA%\Programs\OpenAs on Windows.
pub fn default_install_dir() -> PathBuf {
    os::default_install_dir()
}

/// The prebuilt Runner in a development tree's target folder:
/// `Runner.app` on a Mac, the `Runner` folder (holding Runner.exe) on Windows.
pub fn runner_bundle_in(target: &Path) -> PathBuf {
    os::runner_bundle_in(target)
}

/// The Runner program inside a bundle (or an installed copy).
pub fn exe_in(bundle: &Path) -> PathBuf {
    os::exe_in(bundle)
}

/// The installed copy a program file belongs to, if it is one.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    os::bundle_of(exe)
}

/// Where an app called `name` is installed in `dir`.
pub fn install_path(dir: &Path, name: &str) -> PathBuf {
    os::install_path(dir, name)
}

/// Whether `path` looks like an installed copy (so cleanup only ever removes those).
pub fn is_install(path: &Path) -> bool {
    os::is_install(path)
}

/// Makes `dest` a copy of `runner` that is this app. `icon(px)` draws the
/// app's icon as PNG bytes at a size. Replaces `dest` if it exists (the
/// caller has checked it belongs to this app).
pub fn make_copy(
    runner: &Path,
    dest: &Path,
    who: &Identity,
    icon: &dyn Fn(u32) -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<()> {
    os::make_copy(runner, dest, who, icon)
}

/// A plain copy of the Runner called "New app" inside `dir`, so the Dock or
/// taskbar shows that rather than "Runner" while an app is being made.
/// Returns the program to start.
pub fn making_copy(runner: &Path, dir: &Path) -> anyhow::Result<PathBuf> {
    os::making_copy(runner, dir)
}

/// The identity written into an installed copy.
pub fn read_identity(install: &Path) -> Option<Identity> {
    os::read_identity(install)
}

/// The identity of the copy this program is (None for the plain Runner).
pub fn own_identity() -> Option<Identity> {
    let exe = std::env::current_exe().ok()?;
    read_identity(&bundle_of(&exe)?)
}

/// A fingerprint of an installed copy, taken at install: the code-directory
/// hash on a Mac, a SHA-256 of every file in the folder on Windows. The
/// Helper trusts a caller only while its copy still matches.
pub fn seal(install: &Path) -> Option<String> {
    os::seal(install)
}

/// Whether the copy is intact and still matches the recorded seal.
pub fn sealed(install: &Path, recorded: &str) -> bool {
    os::sealed(install, recorded)
}

/// Removes an installed copy and anything install added for it (the Start
/// menu shortcut on Windows).
pub fn remove_install(install: &Path) {
    os::remove_install(install)
}

/// Opens an installed app (or any copy of the Runner). `quiet`: never in
/// front of the person (test runs).
pub fn launch(install: &Path, quiet: bool) -> anyhow::Result<()> {
    os::launch(install, quiet)
}

/// Shows a file in the Finder / Explorer.
pub fn reveal(path: &Path) {
    os::reveal(path)
}

/// Called first thing by the Runner: on Windows the process takes the app's
/// own taskbar identity (it groups with its shortcut and shows its name).
pub fn adopt_identity(id: &str) {
    os::adopt_identity(id)
}

/// System folders whose insides an app may never be handed (the system,
/// programs, other apps' settings). The home itself and the disk are refused
/// separately, as anything that contains the home is.
pub fn closed_folders(home: &Path) -> Vec<PathBuf> {
    os::closed_folders(home)
}

/// A path resolved (links, `..`) in the form people and other paths use: on
/// Windows without the `\\?\` prefix canonicalize adds, so it compares
/// with ordinary paths. Falls back to the path itself when it doesn't exist.
pub fn canonical(p: &Path) -> PathBuf {
    let c = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    #[cfg(windows)]
    {
        let s = c.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if rest.as_bytes().get(1) == Some(&b':') {
                return PathBuf::from(rest);
            }
        }
    }
    c
}

/// The name an installed copy may have on this system: no path or
/// reserved characters, nothing Windows refuses (CON, a trailing dot).
pub fn allowed_name(name: &str) -> bool {
    os::allowed_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grey square PNG of `px` pixels (stored deflate, no compressor).
    fn solid_png(px: u32) -> Vec<u8> {
        fn crc(data: &[u8]) -> u32 {
            let mut c = 0xffff_ffffu32;
            for &b in data {
                c ^= b as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 {
                        0xedb8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc(&body).to_be_bytes());
        }
        let mut raw = Vec::new();
        for _ in 0..px {
            raw.push(0);
            raw.extend(std::iter::repeat_n([128u8, 128, 128, 255], px as usize).flatten());
        }
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
        for (i, b) in blocks.iter().enumerate() {
            z.push((i + 1 == blocks.len()) as u8);
            z.extend_from_slice(&(b.len() as u16).to_le_bytes());
            z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
            z.extend_from_slice(b);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &raw {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&px.to_be_bytes());
        ihdr.extend_from_slice(&px.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn a_program_belongs_to_its_copy() {
        #[cfg(target_os = "macos")]
        assert_eq!(
            bundle_of(Path::new("/A/Plants.app/Contents/MacOS/Plants")),
            Some(PathBuf::from("/A/Plants.app"))
        );
        #[cfg(windows)]
        {
            assert_eq!(
                bundle_of(Path::new(r"C:\P\Plants\Plants.exe")),
                Some(PathBuf::from(r"C:\P\Plants"))
            );
            assert_eq!(bundle_of(Path::new(r"C:\P\Plants\Other.exe")), None);
        }
        assert_eq!(bundle_of(Path::new("/usr/bin/python3")), None);
    }

    #[test]
    fn an_install_round_trips_its_identity() {
        let tmp = std::env::temp_dir().join(format!("oa-plat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let target = tmp.join("target");
        let runner = runner_bundle_in(&target);
        // A stand-in Runner: on a Mac a minimal bundle, on Windows a folder.
        #[cfg(target_os = "macos")]
        {
            let c = runner.join("Contents");
            std::fs::create_dir_all(c.join("MacOS")).unwrap();
            std::fs::create_dir_all(c.join("Resources")).unwrap();
            std::fs::copy("/usr/bin/true", c.join("MacOS/Runner")).unwrap();
            let mut d = plist::Dictionary::new();
            d.insert(
                "CFBundleExecutable".into(),
                plist::Value::String("Runner".into()),
            );
            d.insert(
                "CFBundleIdentifier".into(),
                plist::Value::String("app.openas.runner".into()),
            );
            plist::Value::Dictionary(d)
                .to_file_xml(c.join("Info.plist"))
                .unwrap();
        }
        #[cfg(windows)]
        {
            std::fs::create_dir_all(&runner).unwrap();
            std::fs::write(exe_in(&runner), b"MZ stand-in").unwrap();
        }
        std::env::set_var("OPENAS_INSTALL_DIR", &tmp);
        let who = Identity {
            folder: tmp.join("apps/tea"),
            name: "Tea Timer".into(),
            id: "app.openas.test.tea".into(),
            helper: Some(tmp.join("openas")),
            manager: false,
            env: vec![("OPENAS_ROOT".into(), tmp.to_string_lossy().into_owned())],
        };
        let dest = install_path(&tmp.join("Applications"), &who.name);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let icon = |px: u32| -> anyhow::Result<Vec<u8>> { Ok(solid_png(px)) };
        make_copy(&runner, &dest, &who, &icon).unwrap();
        assert!(is_install(&dest));
        assert_eq!(read_identity(&dest), Some(who.clone()));
        assert_eq!(bundle_of(&exe_in(&dest)), Some(dest.clone()));
        let s = seal(&dest).unwrap();
        assert!(sealed(&dest, &s));
        // Anything added beside the program breaks the seal (on a Mac,
        // codesign --verify refuses extra files in the bundle too).
        #[cfg(windows)]
        {
            std::fs::write(dest.join("extra.dll"), b"x").unwrap();
            assert!(!sealed(&dest, &s));
        }
        remove_install(&dest);
        assert!(!dest.exists());
        std::env::remove_var("OPENAS_INSTALL_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
