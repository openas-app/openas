//! The moment a draft becomes an app. On Save as App, the making window
//! folds its column away and closes in around the app's icon; the installed
//! app's first window then grows out of that same spot to the size the
//! draft had. Two processes, one motion: the frames are handed over in
//! `<draft>.becoming`, which the installer copies to the app's `.grow.json`.

use crate::*;
use std::sync::atomic::AtomicBool;

/// While the window animates, `layout` leaves the webviews where this puts them.
pub(crate) static BECOMING: AtomicBool = AtomicBool::new(false);

/// The icon's side, in points, when the window has closed in on it.
const TILE: f64 = 128.0;
const FRAME: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    fn lerp(a: Frame, b: Frame, t: f64) -> Frame {
        let m = |p: f64, q: f64| p + (q - p) * t;
        Frame {
            x: m(a.x, b.x),
            y: m(a.y, b.y),
            w: m(a.w, b.w),
            h: m(a.h, b.h),
        }
    }
}

fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// Runs `f` with eased progress from 0 to 1 over `ms`.
fn animate(ms: u64, ease: fn(f64) -> f64, mut f: impl FnMut(f64)) {
    let start = std::time::Instant::now();
    loop {
        let t = (start.elapsed().as_secs_f64() * 1000.0 / ms as f64).min(1.0);
        f(ease(t));
        if t >= 1.0 {
            break;
        }
        std::thread::sleep(FRAME);
    }
}

/// The window's outer frame in points.
fn outer(win: &tauri::Window) -> Option<(Frame, f64)> {
    let s = win.scale_factor().ok()?;
    let p = win.outer_position().ok()?;
    let o = win.outer_size().ok()?;
    let i = win.inner_size().ok()?;
    let f = Frame {
        x: p.x as f64 / s,
        y: p.y as f64 / s,
        w: o.width as f64 / s,
        h: o.height as f64 / s,
    };
    // The title bar's height: what the frame has that the content doesn't.
    Some((f, (o.height as f64 - i.height as f64) / s))
}

fn place(win: &tauri::Window, f: Frame) {
    let _ = win.set_position(tauri::LogicalPosition::new(f.x, f.y));
    let _ = win.set_size(tauri::LogicalSize::new(f.w, f.h));
}

/// Gives the window its title bar back without changing its outer frame.
fn with_title_bar(win: &tauri::Window, f: Frame) {
    let _ = win.set_decorations(true);
    let bar = outer(win).map_or(0.0, |(_, bar)| bar);
    let _ = win.set_size(tauri::LogicalSize::new(f.w, f.h - bar));
    let _ = win.set_position(tauri::LogicalPosition::new(f.x, f.y));
}

/// The cover the app's page gets while the window closes in: the app's icon
/// on a plain ground, fading in over whatever the app shows.
fn cover_js(icon: &str) -> String {
    format!(
        r#"(()=>{{if(document.getElementById('__oa_become'))return;const d=document.createElement('div');d.id='__oa_become';
d.style.cssText='position:fixed;inset:0;z-index:2147483647;display:grid;place-items:center;opacity:0;transition:opacity .26s ease;background:'+(matchMedia('(prefers-color-scheme: dark)').matches?'#18181B':'#F4F4F5');
const i=new Image();i.src={icon:?};i.style.cssText='width:min(100vw,100vh,{TILE}px);height:min(100vw,100vh,{TILE}px);display:block;border-radius:22%';
d.append(i);(document.body||document.documentElement).append(d);requestAnimationFrame(()=>requestAnimationFrame(()=>d.style.opacity=1))}})()"#
    )
}

const UNCOVER_JS: &str = "document.getElementById('__oa_become')?.remove()";

/// Save as App was pressed in the making window: fold the column, cover the
/// app with its icon, close in on it, and leave the frames for the app.
/// If the app never arrives, the window opens back up as it was.
pub(crate) fn close_in(app: AppHandle, folder: PathBuf) {
    let Some(win) = app.get_window("main") else {
        return;
    };
    let Some((start, bar)) = outer(&win) else {
        return;
    };
    let talk_w = if app
        .try_state::<Talk>()
        .is_some_and(|t| t.open.load(Ordering::SeqCst))
    {
        TALK_W.min(start.w * 0.5)
    } else {
        0.0
    };
    // The app's part of the window: where the app will open, at this size.
    let to = Frame {
        w: start.w - talk_w,
        ..start
    };
    let content_h = start.h - bar;
    let tile = Frame {
        x: to.x + (to.w - TILE) / 2.0,
        y: to.y + bar + (content_h - TILE) / 2.0,
        w: TILE,
        h: TILE,
    };
    let _ = std::fs::write(
        folder.with_extension("becoming"),
        json!({ "from": tile, "to": to }).to_string(),
    );
    let icon = std::fs::read(folder.join("icon.svg"))
        .map(|b| format!("data:image/svg+xml;base64,{}", powers::b64(&b)))
        .unwrap_or_default();
    let (app_view, talk_view) = (app.get_webview("app"), app.get_webview("talk"));
    BECOMING.store(true, Ordering::SeqCst);

    // 1. The column folds away; the app keeps its width.
    animate(260, ease_in_out, |t| {
        let w = to.w + talk_w * (1.0 - t);
        let _ = win.set_size(tauri::LogicalSize::new(w, content_h));
        if let Some(tv) = &talk_view {
            let _ = tv.set_bounds(tauri::Rect {
                position: tauri::LogicalPosition::new(to.w, 0.0).into(),
                size: tauri::LogicalSize::new((w - to.w).max(1.0), content_h).into(),
            });
        }
    });
    if let Some(tv) = &talk_view {
        let _ = tv.hide();
    }
    if let Some(t) = app.try_state::<Talk>() {
        t.open.store(false, Ordering::SeqCst);
    }

    // 2. The app gives way to its icon.
    if let Some(a) = &app_view {
        let _ = a.eval(cover_js(&icon));
    }
    std::thread::sleep(Duration::from_millis(280));

    // 3. The frame closes in on the icon. Without the title bar, the
    //    content is the whole frame, so the icon ends up filling it.
    let _ = win.set_decorations(false);
    place(&win, to);
    let fit = |f: Frame| {
        if let Some(a) = &app_view {
            let _ = a.set_bounds(tauri::Rect {
                position: tauri::LogicalPosition::new(0.0, 0.0).into(),
                size: tauri::LogicalSize::new(f.w, f.h).into(),
            });
        }
    };
    animate(420, ease_in_out, |t| {
        let f = Frame::lerp(to, tile, t);
        place(&win, f);
        fit(f);
    });

    // 4. The installed app takes over from here; the making window quits
    //    when it sees `<draft>.moved`. If it never comes, open back up.
    let moved = folder.with_extension("moved");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if moved.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(folder.with_extension("becoming"));
    animate(300, ease_out, |t| {
        let f = Frame::lerp(tile, start, t);
        place(&win, f);
        fit(f);
    });
    with_title_bar(&win, start);
    if let Some(a) = &app_view {
        let _ = a.eval(UNCOVER_JS);
    }
    BECOMING.store(false, Ordering::SeqCst);
    talk::show_talk(&app, true);
    layout(&app);
}

/// Read once at start: the frames a just-saved app grows between.
pub(crate) fn take_grow(folder: &Path) -> Option<(Frame, Frame)> {
    let p = folder.join(".grow.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
    let _ = std::fs::remove_file(&p);
    let from: Frame = serde_json::from_value(v.get("from")?.clone()).ok()?;
    let to: Frame = serde_json::from_value(v.get("to")?.clone()).ok()?;
    (to.w > 200.0 && to.h > 150.0).then_some((from, to))
}

/// The app's first window, built hidden and without a title bar, grows from
/// the icon to its size, then gets its title bar back.
pub(crate) fn grow_out(win: tauri::Window, from: Frame, to: Frame) {
    place(&win, from);
    let _ = win.show();
    // A test window never takes focus from the person at the Mac.
    if std::env::var("OPENAS_QUIET_WINDOW").is_err() {
        let _ = win.set_focus();
    }
    std::thread::sleep(Duration::from_millis(60));
    animate(460, ease_out, |t| place(&win, Frame::lerp(from, to, t)));
    with_title_bar(&win, to);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_starts_and_ends_in_place() {
        for e in [ease_in_out as fn(f64) -> f64, ease_out] {
            assert!(e(0.0).abs() < 1e-9 && (e(1.0) - 1.0).abs() < 1e-9);
            assert!(e(0.5) > 0.0 && e(0.5) < 1.0);
        }
    }

    #[test]
    fn frames_travel_in_a_straight_line() {
        let a = Frame {
            x: 0.0,
            y: 0.0,
            w: 900.0,
            h: 640.0,
        };
        let b = Frame {
            x: 386.0,
            y: 256.0,
            w: 128.0,
            h: 128.0,
        };
        assert_eq!(Frame::lerp(a, b, 0.0), a);
        assert_eq!(Frame::lerp(a, b, 1.0), b);
        assert_eq!(Frame::lerp(a, b, 0.5).w, 514.0);
    }

    #[test]
    fn grow_frames_are_read_once() {
        let d = std::env::temp_dir().join(format!("oa-grow-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join(".grow.json"),
            r#"{"from":{"x":1,"y":2,"w":128,"h":128},"to":{"x":0,"y":0,"w":900,"h":668}}"#,
        )
        .unwrap();
        assert!(take_grow(&d).is_some());
        assert!(take_grow(&d).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
