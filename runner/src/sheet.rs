//! The sheets Save as App and Discard open on the build window: macOS's own
//! dialog, sliding down from the title bar, so choosing happens in the menu
//! and its sheet and never inside the conversation.

use crate::talk::read_json;
use crate::Who;
use tauri::{AppHandle, Manager};

/// Asks before saving (with the name to save under) or discarding the draft
/// the build window shows. The answer goes to the conversation page, which
/// shows the progress and makes the choice.
pub(crate) fn ask(app: &AppHandle, save: bool) {
    let Some(talk) = app.get_webview("talk") else {
        return;
    };
    let Ok(me) = app.state::<Who>().me_by_label("talk") else {
        return;
    };
    if !me.making {
        return;
    }
    let name = read_json(&me.folder.join("making.json"))["name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    // The dialog shows the app being saved, not OpenAs.
    let icon = me.folder.join("icon.svg");
    let icon = icon.exists().then(|| icon.to_string_lossy().into_owned());
    let Ok(win) = talk.window().ns_window() else {
        return;
    };
    let win = win as usize;
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        let answer = move |name: Option<String>| {
            let Some(t) = app2.get_webview("talk") else {
                return;
            };
            let js = match name {
                Some(n) => format!(
                    "window.__oaChose && window.__oaChose(true, {})",
                    serde_json::Value::from(n)
                ),
                None => "window.__oaChose && window.__oaChose(false, '')".into(),
            };
            let _ = t.eval(js);
        };
        #[cfg(target_os = "macos")]
        unsafe {
            mac::sheet(
                win as *mut std::ffi::c_void,
                save,
                &name,
                icon.as_deref(),
                answer,
            )
        };
        #[cfg(not(target_os = "macos"))]
        let _ = (win, icon, answer);
    });
}

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::encode::{Encode, Encoding};
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use std::ffi::{c_void, CString};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    unsafe impl Encode for Rect {
        const ENCODING: Encoding = Encoding::Struct(
            "CGRect",
            &[
                Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
                Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
            ],
        );
    }

    unsafe fn ns(s: &str) -> *mut AnyObject {
        let c = CString::new(s.replace('\0', "")).unwrap_or_default();
        msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()]
    }

    unsafe fn text(s: *mut AnyObject) -> String {
        if s.is_null() {
            return String::new();
        }
        let p: *const std::ffi::c_char = msg_send![s, UTF8String];
        if p.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }

    const FIRST_BUTTON: isize = 1000;

    /// `answer(Some(name))` for Save, `answer(None)` for Discard; nothing on Cancel.
    pub(super) unsafe fn sheet(
        win: *mut c_void,
        save: bool,
        name: &str,
        icon: Option<&str>,
        answer: impl Fn(Option<String>) + 'static,
    ) {
        let win = win.cast::<AnyObject>();
        if win.is_null() {
            return;
        }
        let alert: *mut AnyObject = msg_send![class!(NSAlert), new];
        let shown = if name.is_empty() { "this app" } else { name };
        let (title, info, ok) = if save {
            (
                "Save as App".to_string(),
                "It goes in your Applications folder with its own icon and window. It can't reach your files or run programs unless you allow it.",
                "Save as App",
            )
        } else {
            (
                format!("Discard {shown}?"),
                "The app and its conversation are deleted. Nothing is added to your Applications.",
                "Discard",
            )
        };
        if let Some(path) = icon {
            let img: *mut AnyObject = msg_send![class!(NSImage), alloc];
            let img: *mut AnyObject = msg_send![img, initWithContentsOfFile: ns(path)];
            if !img.is_null() {
                let _: () = msg_send![alert, setIcon: img];
                let _: () = msg_send![img, release];
            }
        }
        let _: () = msg_send![alert, setMessageText: ns(&title)];
        let _: () = msg_send![alert, setInformativeText: ns(info)];
        let b: *mut AnyObject = msg_send![alert, addButtonWithTitle: ns(ok)];
        if !save {
            let _: () = msg_send![b, setHasDestructiveAction: true];
        }
        let _: *mut AnyObject = msg_send![alert, addButtonWithTitle: ns("Cancel")];
        let field: *mut AnyObject = if save {
            let f: *mut AnyObject = msg_send![class!(NSTextField), alloc];
            let f: *mut AnyObject =
                msg_send![f, initWithFrame: Rect { x: 0.0, y: 0.0, w: 260.0, h: 24.0 }];
            let _: () = msg_send![f, setStringValue: ns(name)];
            let _: () = msg_send![f, setPlaceholderString: ns("Name")];
            let _: () = msg_send![alert, setAccessoryView: f];
            let aw: *mut AnyObject = msg_send![alert, window];
            let _: () = msg_send![aw, setInitialFirstResponder: f];
            f
        } else {
            std::ptr::null_mut()
        };
        let done = RcBlock::new(move |resp: isize| {
            if resp == FIRST_BUTTON {
                if field.is_null() {
                    answer(None);
                } else {
                    let v: *mut AnyObject = msg_send![field, stringValue];
                    answer(Some(text(v).trim().chars().take(60).collect()));
                }
            }
            if !field.is_null() {
                let _: () = msg_send![field, release];
            }
            let _: () = msg_send![alert, release];
        });
        let _: () = msg_send![alert, beginSheetModalForWindow: win, completionHandler: &*done];
    }
}
