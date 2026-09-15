//! Download handling via WebView2's real native `DownloadStarting`/
//! `StateChanged` events (`ICoreWebView2_4`). This project never prompts
//! for a save location or shows progress -- WebView2 already computes a
//! real default path (its own equivalent of a normal Edge download's
//! default Downloads-folder location) before this handler ever runs, so
//! there's nothing to choose; only the *result* (once complete) is worth
//! reporting back to `browser-ui`. `SetHandled(true)` suppresses
//! WebView2's own default download UI (a flyout a real windowed Edge would
//! show) -- same don't-let-the-engine-try-to-paint-its-own-chrome caution
//! `popup.rs`/`find.rs` already take, and this off-screen host has nowhere
//! sensible for a flyout to appear anyway.

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2_4, COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
};
use webview2_com::{take_pwstr, DownloadStartingEventHandler, StateChangedEventHandler};
use windows::core::PWSTR;

pub type PendingDownload = Rc<RefCell<Option<String>>>;

/// Best-effort like this project's other version-gated WebView2 APIs -- an
/// `ICoreWebView2_4`-less Runtime just means downloads on this tab proceed
/// with WebView2's own default UI/behavior instead of being silently
/// tracked, rather than failing to download at all.
pub fn register(webview: &ICoreWebView2) -> windows::core::Result<PendingDownload> {
    let webview4: ICoreWebView2_4 = windows::core::Interface::cast(webview)?;

    let pending: PendingDownload = Rc::new(RefCell::new(None));
    let mut token = Default::default();

    let pending_for_handler = pending.clone();
    let handler = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let _ = args.SetHandled(true);
        }
        if let Ok(operation) = unsafe { args.DownloadOperation() } {
            let pending = pending_for_handler.clone();
            let mut op_token = Default::default();
            let state_handler = StateChangedEventHandler::create(Box::new(move |sender, _args| {
                if let Some(sender) = &sender {
                    let mut state = Default::default();
                    if unsafe { sender.State(&mut state) }.is_ok()
                        && state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
                    {
                        let mut path = PWSTR::null();
                        if unsafe { sender.ResultFilePath(&mut path) }.is_ok() {
                            *pending.borrow_mut() = Some(take_pwstr(path));
                        }
                    }
                }
                Ok(())
            }));
            unsafe {
                let _ = operation.add_StateChanged(&state_handler, &mut op_token);
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = webview4.add_DownloadStarting(&handler, &mut token);
    }

    Ok(pending)
}
