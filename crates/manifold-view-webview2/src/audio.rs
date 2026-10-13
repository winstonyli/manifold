//! Audio-playing indicator via WebView2's real native
//! `ICoreWebView2_8::IsDocumentPlayingAudio` property and its matching
//! `IsDocumentPlayingAudioChanged` event -- a passive read of state
//! Chromium already tracks internally, no different in kind from reading
//! `title`, unlike CEF's only equivalent (see `ipc_protocol::EngineToHost::
//! AudioPlayingChanged`'s doc comment for why CEF doesn't get this).

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::IsDocumentPlayingAudioChangedEventHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2_8};
use windows::core::BOOL;

pub type PendingAudioPlaying = Rc<RefCell<Option<bool>>>;

/// Best-effort like favicon/find's own version-gated APIs -- an
/// `ICoreWebView2_8`-less WebView2 Runtime just means this tab never
/// reports whether it's playing audio.
pub fn register(webview: &ICoreWebView2) -> windows::core::Result<PendingAudioPlaying> {
    let webview8: ICoreWebView2_8 = windows::core::Interface::cast(webview)?;

    let pending: PendingAudioPlaying = Rc::new(RefCell::new(None));
    let mut token = Default::default();

    let pending_for_handler = pending.clone();
    let handler = IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |sender, _args| {
        if let Some(sender) = &sender {
            let webview8_for_read: windows::core::Result<ICoreWebView2_8> = windows::core::Interface::cast(sender);
            if let Ok(sender8) = webview8_for_read {
                let mut playing = BOOL(0);
                if unsafe { sender8.IsDocumentPlayingAudio(&mut playing) }.is_ok() {
                    *pending_for_handler.borrow_mut() = Some(playing.as_bool());
                }
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = webview8.add_IsDocumentPlayingAudioChanged(&handler, &mut token);
    }

    Ok(pending)
}
