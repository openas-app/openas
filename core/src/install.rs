//! Install = copy the prebuilt Runner and make the copy this app: its own name,
//! bundle id, executable name and icon. Nothing is compiled per app.

use std::path::{Path, PathBuf};

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

/// A fingerprint of an installed copy (code-directory hash on a Mac, a hash
/// of the folder on Windows): what the Helper checks a caller against, so
/// only the exact copy it installed is trusted.
pub fn cdhash(app: &Path) -> Option<String> {
    openas_platform::seal(app)
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
        return "New App".into();
    }
    if openas_platform::allowed_name(n) {
        return n.into();
    }
    // Windows refuses more: <>"|?* in names, a trailing dot, CON and friends.
    let n: String = n.chars().filter(|c| !"<>\"|?*".contains(*c)).collect();
    let n = n.trim_end_matches(['.', ' ']).trim();
    if n.is_empty() {
        "New App".into()
    } else if openas_platform::allowed_name(n) {
        n.into()
    } else {
        format!("{n} App")
    }
}

/// Whether an installed copy belongs to the app kept in `folder`.
pub fn owned_by(app: &Path, folder: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    openas_platform::read_identity(app)
        .is_some_and(|who| who.folder == folder || canon(&who.folder) == canon(folder))
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
            "no Runner at {} (build it first: see the README)",
            runner.display()
        );
    }
    let dest_dir = paths::install_dir();
    std::fs::create_dir_all(&dest_dir)?;
    let safe_name = clean_name(name);
    let app = openas_platform::install_path(&dest_dir, &safe_name);
    // Only ever replace this app's own copy, never another app of that name.
    if app.exists() && !owned_by(&app, folder) {
        bail!("An app called \u{201c}{safe_name}\u{201d} is already installed");
    }
    // The model's icon, or a plain one when it won't draw.
    let svg = if shaped(svg, 64).is_ok() {
        svg.to_string()
    } else {
        fallback_icon(app_id)
    };
    let who = openas_platform::Identity {
        folder: folder.to_path_buf(),
        name: safe_name.clone(),
        id: bundle_id(app_id),
        // Where to find the Helper when the app's menu needs it.
        helper: std::env::current_exe()
            .ok()
            .map(|exe| paths::helper_exe(&exe)),
        manager,
        // Apps opened from the Finder or Start menu get no environment, so a
        // non-default setup (a test root) travels in the copy and on to the
        // Helper it starts.
        env: openas_platform::CARRIED_ENV
            .iter()
            .filter_map(|var| Some((var.to_string(), std::env::var(var).ok()?)))
            .collect(),
    };
    let icon = |px: u32| -> Result<Vec<u8>> { Ok(shaped(&svg, px)?.encode_png()?) };
    openas_platform::make_copy(&runner, &app, &who, &icon)?;
    let _ = render_png(&svg, &folder.join("icon.png"));
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
        if cfg!(windows) {
            assert_eq!(clean_name("What?"), "What");
            assert_eq!(clean_name("CON"), "CON App");
            assert_eq!(clean_name("Notes."), "Notes");
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
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
