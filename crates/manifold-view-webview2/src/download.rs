//! Download handling via WebView2's real native `DownloadStarting`/
//! `BytesReceivedChanged`/`StateChanged` events (`ICoreWebView2_4`). This
//! project never prompts for a save location -- WebView2 already computes a
//! real default path (its own equivalent of a normal Edge download's
//! default Downloads-folder location) before this handler ever runs, so
//! there's nothing to choose. `SetHandled(true)` suppresses WebView2's own
//! default download UI (a flyout a real windowed Edge would show) -- same
//! don't-let-the-engine-try-to-paint-its-own-chrome caution `popup.rs`/
//! `find.rs` already take, and this off-screen host has nowhere sensible
//! for a flyout to appear anyway.
//!
//! Reports live byte progress now, not just the final result: `path` is
//! read once, from `DownloadStartingEventArgs::ResultFilePath` at the
//! moment the download starts (already final by contract -- WebView2 has
//! decided the save location before this event fires), then reused as the
//! stable key for every later `Progress`/`Completed` event on the same
//! download, rather than re-reading it from the operation each time.
//!
//! The host holds each download's operation until it reports finished or
//! failed: one the host stops referencing never raises `StateChanged`
//! (runs 2026-10-27-dl-probe-*). A transfer that breaks is retried as a
//! second `DownloadStarting` under the same path, from 0 or, when the server
//! sent an ETag, from the bytes the engine kept; the retry reports the
//! outcome, and the first operation never reports again, so it stays held
//! until the host exits.

use std::cell::RefCell;
use std::rc::Rc;

use manifold_view_protocol::ViewToHost;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2DownloadOperation, ICoreWebView2_4, COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
    COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
};
use webview2_com::{
    take_pwstr, BytesReceivedChangedEventHandler, DownloadStartingEventHandler, StateChangedEventHandler,
};
use windows::core::{Interface, PWSTR};

/// A queue, not a single slot -- two downloads progressing within the same
/// tick would otherwise silently clobber each other (each gets its own
/// `BytesReceivedChangedEventHandler`/`StateChangedEventHandler` closures
/// below, all writing into whatever this type is).
pub type PendingDownload = Rc<RefCell<Vec<ViewToHost>>>;

/// Best-effort like this project's other version-gated WebView2 APIs -- an
/// `ICoreWebView2_4`-less Runtime just means downloads on this tab proceed
/// with WebView2's own default UI/behavior instead of being silently
/// tracked, rather than failing to download at all.
pub fn register(webview: &ICoreWebView2) -> windows::core::Result<PendingDownload> {
    let webview4: ICoreWebView2_4 = windows::core::Interface::cast(webview)?;

    let pending: PendingDownload = Rc::new(RefCell::new(Vec::new()));
    let mut token = Default::default();

    let pending_for_handler = pending.clone();
    // Every download that hasn't finished or failed yet (see the module doc).
    let held_for_handler: Rc<RefCell<Vec<ICoreWebView2DownloadOperation>>> = Rc::default();
    let handler = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let _ = args.SetHandled(true);
        }
        let mut path_raw = PWSTR::null();
        let path = if unsafe { args.ResultFilePath(&mut path_raw) }.is_ok() {
            take_pwstr(path_raw)
        } else {
            return Ok(());
        };
        if let Ok(operation) = unsafe { args.DownloadOperation() } {
            let mut total_bytes_raw: i64 = -1;
            let total_bytes = unsafe { operation.TotalBytesToReceive(&mut total_bytes_raw) }
                .is_ok()
                .then_some(total_bytes_raw)
                .filter(|n| *n >= 0)
                .map(|n| n as u64);
            let mut uri_raw = PWSTR::null();
            let url =
                unsafe { operation.Uri(&mut uri_raw) }.is_ok().then(|| take_pwstr(uri_raw)).filter(|u| !u.is_empty());
            pending_for_handler.borrow_mut().push(ViewToHost::DownloadStarted { path: path.clone(), total_bytes, url });
            // A retried download that resumed starts with the bytes the engine kept.
            let mut bytes_received: i64 = 0;
            if unsafe { operation.BytesReceived(&mut bytes_received) }.is_ok() && bytes_received > 0 {
                pending_for_handler
                    .borrow_mut()
                    .push(ViewToHost::DownloadProgress { path: path.clone(), bytes_received: bytes_received as u64 });
            }
            held_for_handler.borrow_mut().push(operation.clone());

            let pending_for_bytes = pending_for_handler.clone();
            let path_for_bytes = path.clone();
            let mut bytes_token = Default::default();
            let bytes_handler = BytesReceivedChangedEventHandler::create(Box::new(move |sender, _args| {
                if let Some(sender) = &sender {
                    let mut bytes_received: i64 = 0;
                    if unsafe { sender.BytesReceived(&mut bytes_received) }.is_ok() && bytes_received >= 0 {
                        pending_for_bytes.borrow_mut().push(ViewToHost::DownloadProgress {
                            path: path_for_bytes.clone(),
                            bytes_received: bytes_received as u64,
                        });
                    }
                }
                Ok(())
            }));
            unsafe {
                let _ = operation.add_BytesReceivedChanged(&bytes_handler, &mut bytes_token);
            }

            let pending_for_state = pending_for_handler.clone();
            let held_for_state = held_for_handler.clone();
            let mut state_token = Default::default();
            let state_handler = StateChangedEventHandler::create(Box::new(move |sender, _args| {
                if let Some(sender) = &sender {
                    let mut state = Default::default();
                    if unsafe { sender.State(&mut state) }.is_ok() {
                        let message = if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                            Some(ViewToHost::DownloadCompleted { path: path.clone() })
                        } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                            Some(ViewToHost::DownloadFailed { path: path.clone() })
                        } else {
                            None
                        };
                        if let Some(message) = message {
                            pending_for_state.borrow_mut().push(message);
                            // Over: nothing more will come from it.
                            held_for_state.borrow_mut().retain(|op| op.as_raw() != sender.as_raw());
                        }
                    }
                }
                Ok(())
            }));
            unsafe {
                let _ = operation.add_StateChanged(&state_handler, &mut state_token);
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = webview4.add_DownloadStarting(&handler, &mut token);
    }

    Ok(pending)
}
