//! Favicon support via WebView2's real native API
//! (`ICoreWebView2_15::add_FaviconChanged` + `GetFavicon`) -- unlike CEF
//! (favicon candidates are always empty in windowless/Alloy-runtime mode,
//! a confirmed platform limitation, not fixable from this project's side)
//! or WebKit (needed an extra "actively request the fetch" step to work at
//! all), this is expected to be the cleanest of the four engines here,
//! since WebView2 exposes a direct, real async fetch API for exactly this.

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
};
use webview2_com::{FaviconChangedEventHandler, GetFaviconCompletedHandler};
use windows::Win32::System::Com::{IStream, STATFLAG_NONAME, STREAM_SEEK_SET};

pub type PendingFavicon = Rc<RefCell<Option<(Vec<u8>, u32, u32)>>>;

/// Reads an `IStream` (WebView2 hands back a PNG-encoded image this way,
/// not a plain byte buffer) fully into memory. `Stat` gives the exact size
/// up front, so this is one seek-to-start + one sized read, not a
/// grow-as-you-go loop.
fn read_stream_to_vec(stream: &IStream) -> windows::core::Result<Vec<u8>> {
    unsafe {
        let mut stat = Default::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        let size = stat.cbSize as usize;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut buf = vec![0u8; size];
        let mut read: u32 = 0;
        stream
            .Read(buf.as_mut_ptr() as *mut _, size as u32, Some(&mut read))
            .ok()?;
        buf.truncate(read as usize);
        Ok(buf)
    }
}

/// Registers the `FaviconChanged` handler on `webview`, requesting the
/// actual PNG bytes (via `GetFavicon`) every time it fires and decoding
/// them into raw RGBA -- `EngineToHost::FaviconChanged` carries decoded
/// pixels, not encoded image bytes, matching Servo/WebKit's convention.
/// Decoded results land in the returned cell, drained by `main.rs`'s tick
/// loop same as `nav_state`'s `dirty` flag.
pub fn register(webview: &ICoreWebView2) -> windows::core::Result<PendingFavicon> {
    let webview15: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_15 =
        windows::core::Interface::cast(webview)?;

    let pending: PendingFavicon = Rc::new(RefCell::new(None));
    let mut token = Default::default();

    let webview15_for_handler = webview15.clone();
    let pending_for_handler = pending.clone();
    let handler = FaviconChangedEventHandler::create(Box::new(move |_sender, _args| {
        let pending = pending_for_handler.clone();
        let completed = GetFaviconCompletedHandler::create(Box::new(move |_error, stream| {
            if let Some(stream) = stream {
                if let Ok(bytes) = read_stream_to_vec(&stream) {
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        let rgba = image.into_rgba8();
                        let (width, height) = rgba.dimensions();
                        *pending.borrow_mut() = Some((rgba.into_raw(), width, height));
                    }
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = webview15_for_handler
                .GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &completed);
        }
        Ok(())
    }));
    unsafe {
        webview15.add_FaviconChanged(&handler, &mut token)?;
    }

    Ok(pending)
}
