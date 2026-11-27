//! macOS: an installed app is `<Name>.app`, a copy of Runner.app with its own
//! Info.plist, executable name and icon, ad-hoc signed.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::Identity;

fn run(cmd: &mut Command) -> Result<()> {
    let out = cmd
        .output()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!(
            "{:?} failed: {}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn default_install_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Applications")
}

pub fn runner_bundle_in(target: &Path) -> PathBuf {
    target.join("Runner.app")
}

/// `<X>.app/Contents/MacOS/<the one program>`: Runner in the prebuilt
/// bundle, the app's own name in an installed copy.
pub fn exe_in(bundle: &Path) -> PathBuf {
    let macos = bundle.join("Contents/MacOS");
    let named = plist::Value::from_file(bundle.join("Contents/Info.plist"))
        .ok()
        .and_then(|v| {
            v.as_dictionary()?
                .get("CFBundleExecutable")?
                .as_string()
                .map(str::to_owned)
        });
    macos.join(named.unwrap_or_else(|| "Runner".into()))
}

/// `<X>.app/Contents/MacOS/<exe>` -> `<X>.app`
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let b = exe.parent()?.parent()?.parent()?;
    (b.extension()? == "app").then(|| b.to_path_buf())
}

pub fn install_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.app"))
}

pub fn is_install(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "app")
}

pub fn make_copy(
    runner: &Path,
    app: &Path,
    who: &Identity,
    icon: &dyn Fn(u32) -> Result<Vec<u8>>,
) -> Result<()> {
    if app.exists() {
        std::fs::remove_dir_all(app)?;
    }
    run(Command::new("cp").arg("-R").arg(runner).arg(app))?;
    let name = &who.name;
    let info = app.join("Contents/Info.plist");
    let mut dict = plist::Value::from_file(&info)?
        .into_dictionary()
        .ok_or_else(|| anyhow!("Runner Info.plist"))?;
    let s = |v: &str| plist::Value::String(v.to_string());
    dict.insert("CFBundleName".into(), s(name));
    dict.insert("CFBundleDisplayName".into(), s(name));
    // What macOS shows when the app first asks for the camera or microphone
    // (a browser can use both after asking, so every app can).
    dict.insert(
        "NSCameraUsageDescription".into(),
        s(&format!("{name} uses the camera only when you ask it to.")),
    );
    dict.insert(
        "NSMicrophoneUsageDescription".into(),
        s(&format!(
            "{name} uses the microphone only when you ask it to."
        )),
    );
    dict.insert("CFBundleIdentifier".into(), s(&who.id));
    dict.insert("CFBundleExecutable".into(), s(name));
    dict.insert("OAAppFolder".into(), s(&who.folder.to_string_lossy()));
    // Apps opened from Finder get no environment, so a non-default setup
    // (a test root) travels in the bundle and on to the Helper it starts.
    for (var, v) in &who.env {
        if let Some(key) = plist_key(var) {
            dict.insert(key.into(), s(v));
        }
    }
    if let Some(h) = &who.helper {
        dict.insert("OAHelper".into(), s(&h.to_string_lossy()));
    }
    if who.manager {
        dict.insert("OAManager".into(), plist::Value::Boolean(true));
        // OpenAs opens shared apps (.openas files).
        let mut ty = plist::Dictionary::new();
        ty.insert("UTTypeIdentifier".into(), s("app.openas.shared"));
        ty.insert("UTTypeDescription".into(), s("OpenAs shared app"));
        ty.insert(
            "UTTypeConformsTo".into(),
            plist::Value::Array(vec![s("public.data"), s("public.archive")]),
        );
        let mut tags = plist::Dictionary::new();
        tags.insert(
            "public.filename-extension".into(),
            plist::Value::Array(vec![s("openas")]),
        );
        ty.insert(
            "UTTypeTagSpecification".into(),
            plist::Value::Dictionary(tags),
        );
        dict.insert(
            "UTExportedTypeDeclarations".into(),
            plist::Value::Array(vec![plist::Value::Dictionary(ty)]),
        );
        let mut doc = plist::Dictionary::new();
        doc.insert("CFBundleTypeName".into(), s("OpenAs shared app"));
        doc.insert("CFBundleTypeRole".into(), s("Viewer"));
        doc.insert("LSHandlerRank".into(), s("Owner"));
        doc.insert(
            "LSItemContentTypes".into(),
            plist::Value::Array(vec![s("app.openas.shared")]),
        );
        dict.insert(
            "CFBundleDocumentTypes".into(),
            plist::Value::Array(vec![plist::Value::Dictionary(doc)]),
        );
    }
    plist::Value::Dictionary(dict).to_file_xml(&info)?;
    // The process carries the app's own name (Activity Monitor, Force Quit).
    let macos = app.join("Contents/MacOS");
    std::fs::rename(macos.join("Runner"), macos.join(name))?;
    make_icns(icon, &app.join("Contents/Resources/AppIcon.icns"))?;
    // A copy of a downloaded Runner inherits its quarantine flag; this copy was
    // made here by the user's own action, so it is not a download.
    let _ = Command::new("xattr")
        .arg("-dr")
        .arg("com.apple.quarantine")
        .arg(app)
        .output();
    run(Command::new("codesign")
        .arg("--force")
        .arg("--sign")
        .arg("-")
        .arg(app))?;
    let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
        .arg("-f")
        .arg(app)
        .output();
    Ok(())
}

fn plist_key(var: &str) -> Option<&'static str> {
    match var {
        "OPENAS_ROOT" => Some("OARoot"),
        "OPENAS_INSTALL_DIR" => Some("OAInstallDir"),
        "OPENAS_ID_PREFIX" => Some("OAIdPrefix"),
        _ => None,
    }
}

/// Renders the icon at every size macOS wants, then packs it.
fn make_icns(icon: &dyn Fn(u32) -> Result<Vec<u8>>, out: &Path) -> Result<()> {
    let set = out.with_extension("iconset");
    let _ = std::fs::remove_dir_all(&set);
    std::fs::create_dir_all(&set)?;
    for s in [16u32, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            // Each size drawn from the artwork itself, so small ones stay sharp.
            std::fs::write(
                set.join(format!("icon_{s}x{s}{suffix}.png")),
                icon(s * scale)?,
            )?;
        }
    }
    run(Command::new("iconutil")
        .arg("-c")
        .arg("icns")
        .arg(&set)
        .arg("-o")
        .arg(out))?;
    let _ = std::fs::remove_dir_all(&set);
    Ok(())
}

pub fn making_copy(runner: &Path, dir: &Path) -> Result<PathBuf> {
    let app = dir.join("New app.app");
    std::fs::create_dir_all(dir)?;
    run(Command::new("/usr/bin/ditto").arg(runner).arg(&app))?;
    let plist = app.join("Contents/Info.plist");
    for cmd in [
        "Set :CFBundleName New app",
        "Set :CFBundleIdentifier app.openas.making",
    ] {
        let _ = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", cmd])
            .arg(&plist)
            .status();
    }
    let _ = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Add :CFBundleDisplayName string New app"])
        .arg(&plist)
        .stderr(std::process::Stdio::null())
        .status();
    run(Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app))?;
    Ok(app.join("Contents/MacOS/Runner"))
}

pub fn read_identity(app: &Path) -> Option<Identity> {
    let d = plist::Value::from_file(app.join("Contents/Info.plist"))
        .ok()?
        .into_dictionary()?;
    let s = |k: &str| d.get(k).and_then(|v| v.as_string()).map(str::to_owned);
    let folder = s("OAAppFolder").filter(|f| !f.is_empty())?;
    Some(Identity {
        folder: PathBuf::from(folder),
        name: s("CFBundleName").unwrap_or_else(|| "App".into()),
        id: s("CFBundleIdentifier").unwrap_or_default(),
        helper: s("OAHelper").map(PathBuf::from),
        manager: d
            .get("OAManager")
            .and_then(|v| v.as_boolean())
            .unwrap_or(false),
        env: crate::CARRIED_ENV
            .iter()
            .filter_map(|var| Some((var.to_string(), s(plist_key(var)?)?)))
            .collect(),
    })
}

/// The code-directory hash of an installed bundle.
pub fn seal(app: &Path) -> Option<String> {
    let out = Command::new("codesign")
        .arg("-dvvv")
        .arg(app)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .find_map(|l| l.strip_prefix("CDHash=").map(str::to_string))
}

pub fn sealed(app: &Path, recorded: &str) -> bool {
    let verified = Command::new("codesign")
        .arg("--verify")
        .arg(app)
        .output()
        .is_ok_and(|o| o.status.success());
    verified && seal(app).as_deref() == Some(recorded)
}

pub fn remove_install(app: &Path) {
    let _ = std::fs::remove_dir_all(app);
}

pub fn launch(app: &Path, quiet: bool) -> Result<()> {
    Command::new("/usr/bin/open")
        .args(if quiet { &["-g"][..] } else { &[][..] })
        .arg(app)
        .status()?;
    Ok(())
}

pub fn open_url(url: &str) {
    let _ = Command::new("/usr/bin/open").arg(url).spawn();
}

pub fn reveal(path: &Path) {
    let _ = Command::new("/usr/bin/open").arg("-R").arg(path).status();
}

pub fn adopt_identity(_id: &str) {}

/// ~/Library is handled by the Runner (cloud drives inside it are fine).
pub fn closed_folders(home: &Path) -> Vec<PathBuf> {
    [
        "/System",
        "/Library",
        "/private",
        "/usr",
        "/bin",
        "/sbin",
        "/etc",
        "/Applications",
    ]
    .iter()
    .map(PathBuf::from)
    .chain([home.join("Applications")])
    .collect()
}

pub fn allowed_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', ':', '\\']) && !name.starts_with('.')
}
