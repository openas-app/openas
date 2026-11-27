//! Install = copy the prebuilt Runner and make the copy this app: its own name,
//! bundle id, executable name and icon. Nothing is compiled per app.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::paths;

/// `app.openas.<slug>`: stable for the app's folder, so reinstalling keeps
/// its notification settings and Dock place.
/// `OPENAS_ID_PREFIX` keeps a test setup's apps apart from the real ones
/// (Launch Services knows apps by this id).
pub fn bundle_id(app_id: &str) -> String {
    let prefix = std::env::var("OPENAS_ID_PREFIX").unwrap_or_else(|_| "app.openas.".into());
    format!("{prefix}{app_id}")
}

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

/// The icon as macOS draws app icons: the artwork inside the system's rounded
/// shape (a superellipse, 824 of 1024 points, centred) with a soft shadow,
/// so every app sits in the Dock like any other Mac app. The model draws
/// square artwork edge to edge; the shape is ours.
pub fn shaped(svg: &str, px: u32) -> Result<resvg::tiny_skia::Pixmap> {
    use resvg::tiny_skia::{FillRule, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Transform};
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())
        .context("reading the icon")?;
    let u = px as f32 / 1024.0;
    let c = px as f32;
    let body = 824.0 * u;
    let size = tree.size();
    // Where the artwork actually draws: an icon that already has its own
    // margins or shape is fitted by what it draws, not by its canvas.
    let mut probe = Pixmap::new(256, 256).ok_or_else(|| anyhow!("pixmap"))?;
    resvg::render(
        &tree,
        Transform::from_scale(256.0 / size.width(), 256.0 / size.height()),
        &mut probe.as_mut(),
    );
    let (mut x0, mut y0, mut x1, mut y1) = (256u32, 256u32, 0u32, 0u32);
    for (i, p) in probe.pixels().iter().enumerate() {
        if p.alpha() > 24 {
            let (x, y) = (i as u32 % 256, i as u32 / 256);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        (x0, y0, x1, y1) = (0, 0, 256, 256);
    }
    let side = (x1 - x0).max(y1 - y0) as f32 / 256.0;
    let (cx, cy) = ((x0 + x1) as f32 / 512.0, (y0 + y1) as f32 / 512.0);
    // Scale so the drawn part (as a square around its centre) fills the body.
    let k = body / (side * size.width().max(size.height()));
    let mut art = Pixmap::new(px, px).ok_or_else(|| anyhow!("pixmap"))?;
    resvg::render(
        &tree,
        Transform::from_scale(k, k).post_translate(
            c / 2.0 - cx * size.width() * k,
            c / 2.0 - cy * size.height() * k,
        ),
        &mut art.as_mut(),
    );
    // The shape: |x|^n + |y|^n = 1 with n = 5, close to Apple's continuous corners.
    let shape = |cx: f32, cy: f32, half: f32| {
        let mut pb = PathBuilder::new();
        let n = 5.0f32;
        for i in 0..=360 {
            let t = (i as f32).to_radians();
            let (c, s) = (t.cos(), t.sin());
            let x = cx + half * c.signum() * c.abs().powf(2.0 / n);
            let y = cy + half * s.signum() * s.abs().powf(2.0 / n);
            if i == 0 {
                pb.move_to(x, y)
            } else {
                pb.line_to(x, y)
            }
        }
        pb.close();
        pb.finish().unwrap()
    };
    let mut out = Pixmap::new(px, px).ok_or_else(|| anyhow!("pixmap"))?;
    // A soft shadow below, built from widening, faint copies of the shape.
    for i in 0..(if px >= 64 { 8 } else { 0 }) {
        let mut p = Paint::default();
        p.set_color_rgba8(0, 0, 0, 6);
        p.anti_alias = true;
        let path = shape(
            c / 2.0,
            c / 2.0 + 8.0 * u,
            body / 2.0 + (i as f32 * 1.5 - 4.0) * u,
        );
        out.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
    let mut mask = Mask::new(px, px).ok_or_else(|| anyhow!("mask"))?;
    mask.fill_path(
        &shape(c / 2.0, c / 2.0, body / 2.0),
        FillRule::Winding,
        true,
        Transform::identity(),
    );
    out.draw_pixmap(
        0,
        0,
        art.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        Some(&mask),
    );
    Ok(out)
}

/// Renders the icon at every size macOS wants, then packs it.
fn make_icns(svg: &str, out: &Path) -> Result<()> {
    let set = out.with_extension("iconset");
    let _ = std::fs::remove_dir_all(&set);
    std::fs::create_dir_all(&set)?;
    for s in [16u32, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            // Each size drawn from the artwork itself, so small ones stay sharp.
            shaped(svg, s * scale)?.save_png(set.join(format!("icon_{s}x{s}{suffix}.png")))?;
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

/// A plain icon when the model gave none or one that won't draw.
pub fn fallback_icon(seed: &str) -> String {
    let h = seed
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32))
        % 360;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><rect width="1024" height="1024" fill="hsl({h},45%,42%)"/><circle cx="512" cy="512" r="200" fill="none" stroke="#fff" stroke-width="64"/></svg>"##
    )
}

pub fn render_png(svg: &str, out: &Path) -> Result<()> {
    shaped(svg, 1024)?.save_png(out)?;
    Ok(())
}

/// Installs (or reinstalls) the app at `folder` as `<install_dir>/<name>.app`.
/// The code-directory hash of an installed bundle: what the Helper checks a
/// caller against, so only the exact copy it installed is trusted.
pub fn cdhash(app: &Path) -> Option<String> {
    let out = Command::new("codesign")
        .arg("-dvvv")
        .arg(app)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .find_map(|l| l.strip_prefix("CDHash=").map(str::to_string))
}

/// The name an app's bundle gets: no path characters, no invisible or control
/// characters, no leading dot, at most 60 characters.
pub fn clean_name(name: &str) -> String {
    let invisible = |c: char| {
        c.is_control()
            || matches!(
                c,
                '\u{00AD}'
                    | '\u{034F}'
                    | '\u{061C}'
                    | '\u{115F}'
                    | '\u{1160}'
                    | '\u{17B4}'
                    | '\u{17B5}'
                    | '\u{180E}'
                    | '\u{3164}'
                    | '\u{FEFF}'
                    | '\u{FFA0}'
            )
            || ('\u{200B}'..='\u{200F}').contains(&c)
            || ('\u{202A}'..='\u{202E}').contains(&c)
            || ('\u{2060}'..='\u{206F}').contains(&c)
            || ('\u{FE00}'..='\u{FE0F}').contains(&c)
    };
    let n: String = name
        .chars()
        .filter(|&c| !matches!(c, '/' | ':' | '\\') && !invisible(c))
        .take(60)
        .collect();
    let n = n.trim().trim_start_matches('.').trim();
    if n.is_empty() {
        "New App".into()
    } else {
        n.into()
    }
}

/// Whether an installed bundle belongs to the app kept in `folder`.
pub fn owned_by(app: &Path, folder: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    plist::Value::from_file(app.join("Contents/Info.plist"))
        .ok()
        .and_then(|v| {
            v.as_dictionary()
                .and_then(|d| d.get("OAAppFolder"))
                .and_then(|f| f.as_string())
                .map(PathBuf::from)
        })
        .is_some_and(|owner| owner == folder || canon(&owner) == canon(folder))
}

pub fn install(folder: &Path, app_id: &str, name: &str, svg: &str) -> Result<PathBuf> {
    install_as(folder, app_id, name, svg, false)
}

/// `manager`: the OpenAs window itself (the list and New App), which may
/// ask the Helper to make apps. Every other copy may only change itself.
pub fn install_as(
    folder: &Path,
    app_id: &str,
    name: &str,
    svg: &str,
    manager: bool,
) -> Result<PathBuf> {
    let runner = paths::runner_bundle();
    if !runner.exists() {
        bail!(
            "no Runner at {} (build it with runner/scripts/bundle_mac.sh)",
            runner.display()
        );
    }
    let dest_dir = paths::install_dir();
    std::fs::create_dir_all(&dest_dir)?;
    let safe_name = clean_name(name);
    let app = dest_dir.join(format!("{safe_name}.app"));
    if app.exists() {
        // Only ever replace this app's own bundle, never another app of that name.
        if !owned_by(&app, folder) {
            bail!(
                "An app called \u{201c}{safe_name}\u{201d} is already in your Applications folder"
            );
        }
        std::fs::remove_dir_all(&app)?;
    }
    run(Command::new("cp").arg("-R").arg(&runner).arg(&app))?;

    let info = app.join("Contents/Info.plist");
    let mut dict = plist::Value::from_file(&info)?
        .into_dictionary()
        .ok_or_else(|| anyhow!("Runner Info.plist"))?;
    let s = |v: &str| plist::Value::String(v.to_string());
    dict.insert("CFBundleName".into(), s(&safe_name));
    dict.insert("CFBundleDisplayName".into(), s(&safe_name));
    // What macOS shows when the app first asks for the camera or microphone
    // (a browser can use both after asking, so every app can).
    dict.insert(
        "NSCameraUsageDescription".into(),
        s(&format!(
            "{safe_name} uses the camera only when you ask it to."
        )),
    );
    dict.insert(
        "NSMicrophoneUsageDescription".into(),
        s(&format!(
            "{safe_name} uses the microphone only when you ask it to."
        )),
    );
    dict.insert("CFBundleIdentifier".into(), s(&bundle_id(app_id)));
    dict.insert("CFBundleExecutable".into(), s(&safe_name));
    dict.insert("OAAppFolder".into(), s(&folder.to_string_lossy()));
    // Apps opened from Finder get no environment, so a non-default setup
    // (a test root) travels in the bundle and on to the Helper it starts.
    for (key, var) in [
        ("OARoot", "OPENAS_ROOT"),
        ("OAInstallDir", "OPENAS_INSTALL_DIR"),
        ("OAIdPrefix", "OPENAS_ID_PREFIX"),
    ] {
        if let Ok(v) = std::env::var(var) {
            dict.insert(key.into(), s(&v));
        }
    }
    // Where to find the Helper when the app's menu needs it.
    if let Ok(exe) = std::env::current_exe() {
        dict.insert(
            "OAHelper".into(),
            s(&paths::helper_exe(&exe).to_string_lossy()),
        );
    }
    if manager {
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
    std::fs::rename(macos.join("Runner"), macos.join(&safe_name))?;

    let svg = if make_icns(svg, &app.join("Contents/Resources/AppIcon.icns")).is_ok() {
        svg.to_string()
    } else {
        let f = fallback_icon(app_id);
        make_icns(&f, &app.join("Contents/Resources/AppIcon.icns"))?;
        f
    };
    let _ = render_png(&svg, &folder.join("icon.png"));

    // A copy of a downloaded Runner inherits its quarantine flag; this copy was
    // made here by the user's own action, so it is not a download.
    let _ = Command::new("xattr")
        .arg("-dr")
        .arg("com.apple.quarantine")
        .arg(&app)
        .output();
    run(Command::new("codesign")
        .arg("--force")
        .arg("--sign")
        .arg("-")
        .arg(&app))?;
    let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
        .arg("-f")
        .arg(&app)
        .output();
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_path_and_invisible_characters() {
        assert_eq!(clean_name("OpenAs:"), "OpenAs");
        assert_eq!(clean_name("a/b\\c"), "abc");
        assert_eq!(clean_name("Open\u{200B}As"), "OpenAs");
        assert_eq!(clean_name("  ..hidden "), "hidden");
        assert_eq!(clean_name("/:"), "New App");
        assert_eq!(clean_name(&"x".repeat(80)).len(), 60);
        assert_eq!(clean_name("Tea Timer"), "Tea Timer");
    }

    #[test]
    fn only_an_apps_own_bundle_is_its_own() {
        let tmp = std::env::temp_dir().join(format!("oa-owned-{}", std::process::id()));
        let app = tmp.join("X.app/Contents");
        std::fs::create_dir_all(&app).unwrap();
        let mut d = plist::Dictionary::new();
        d.insert(
            "OAAppFolder".into(),
            plist::Value::String("/x/apps/mine".into()),
        );
        plist::Value::Dictionary(d)
            .to_file_xml(app.join("Info.plist"))
            .unwrap();
        assert!(owned_by(&tmp.join("X.app"), Path::new("/x/apps/mine")));
        assert!(!owned_by(&tmp.join("X.app"), Path::new("/x/apps/other")));
        // Not an OpenAs app at all.
        assert!(!owned_by(&tmp.join("Nope.app"), Path::new("/x/apps/mine")));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
