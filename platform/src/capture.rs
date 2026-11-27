//! Windows: a screenshot of what a WebView2 shows (the builder's eyes).

use webview2_com::CapturePreviewCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
};
use windows::Win32::System::Com::{IStream, STREAM_SEEK_SET};
use windows::Win32::UI::Shell::SHCreateMemStream;

/// Asks the web view for a PNG of its contents; `done` gets the bytes (or
/// None) on the web view's thread once it's ready.
pub fn png(core: &ICoreWebView2, done: Box<dyn FnOnce(Option<Vec<u8>>)>) {
    // SAFETY: COM calls on the web view's own thread, with interfaces we
    // hold for the length of each call; the stream stays alive in the
    // handler until it has been read.
    unsafe {
        let Some(stream) = SHCreateMemStream(None) else {
            done(None);
            return;
        };
        let s2 = stream.clone();
        let done = std::cell::Cell::new(Some(done));
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |r| {
            let bytes = r.ok().and_then(|_| read_all(&s2));
            if let Some(d) = done.take() {
                d(bytes);
            }
            Ok(())
        }));
        if core
            .CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                &stream,
                &handler,
            )
            .is_err()
        {
            // The handler never runs; nothing was captured.
        }
    }
}

unsafe fn read_all(s: &IStream) -> Option<Vec<u8>> {
    s.Seek(0, STREAM_SEEK_SET, None).ok()?;
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let mut n = 0u32;
        let r = s.Read(buf.as_mut_ptr() as *mut _, buf.len() as u32, Some(&mut n));
        if r.is_err() || n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    (!out.is_empty()).then_some(out)
}
