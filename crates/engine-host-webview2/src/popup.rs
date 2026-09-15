//! Popup interception via WebView2's real `NewWindowRequested` event --
//! mirrors CEF/WebKit's own `OpenTabRequested` path: a page asking to open
//! a new window (`target=_blank`, `window.open()`, ctrl+click) hands over
//! the target URL here before WebView2 would otherwise create anything, so
//! `browser-ui` can spawn a real child tab for it instead of a stray
//! WebView2 window escaping this project's off-screen-host model.
//!
//! Confirmed live: clicking a real `target="_blank"` link (via `input.rs`'s
//! `el.click()` `MouseUp` handling) took the engine-host-webview2.exe
//! process count from 1 to 2 -- a real child tab spawned, not a stray
//! window. No second window ever became visible/capturable, consistent
//! with `SetHandled(true)` suppressing WebView2's own default popup
//! creation as intended.

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::{take_pwstr, NewWindowRequestedEventHandler};
use windows::core::PWSTR;

pub type PendingPopup = Rc<RefCell<Option<String>>>;

/// Registers the `NewWindowRequested` handler on `webview`. Every request is
/// marked `Handled` (via `SetHandled`) and `SetNewWindow` is never called,
/// so WebView2 never creates a real window of its own for it -- the target
/// URL lands in the returned cell instead, drained by `main.rs`'s tick loop
/// the same way `favicon::register`'s result is.
pub fn register(webview: &ICoreWebView2) -> PendingPopup {
    let pending: PendingPopup = Rc::new(RefCell::new(None));
    let mut token = Default::default();

    let pending_for_handler = pending.clone();
    let handler = NewWindowRequestedEventHandler::create(Box::new(move |_sender, args| {
        if let Some(args) = args {
            let mut uri = PWSTR::null();
            if unsafe { args.Uri(&mut uri) }.is_ok() {
                *pending_for_handler.borrow_mut() = Some(take_pwstr(uri));
            }
            unsafe {
                let _ = args.SetHandled(true);
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_NewWindowRequested(&handler, &mut token);
    }

    pending
}
