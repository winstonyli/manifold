//! Popup interception via WebView2's real `NewWindowRequested` event --
//! mirrors CEF/WebKit's own `OpenTabRequested` path: a page asking to open
//! a new window (`target=_blank`, `window.open()`, ctrl+click) hands over
//! the target URL here before WebView2 would otherwise create anything, so
//! `manifold` can spawn a real child tab for it instead of a stray
//! WebView2 window escaping this project's off-screen-host model.
//!
//! Confirmed live: clicking a real `target="_blank"` link (via `input.rs`'s
//! `el.click()` `MouseUp` handling) took the manifold-view-webview2.exe
//! process count from 1 to 2 -- a real child tab spawned, not a stray
//! window. No second window ever became visible/capturable, consistent
//! with `SetHandled(true)` suppressing WebView2's own default popup
//! creation as intended.

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::{take_pwstr, NewWindowRequestedEventHandler};
use windows::core::{BOOL, PWSTR};

use crate::nav_state::SharedNavState;

/// A queue, not a single slot -- a page opening several popups
/// synchronously (e.g. back-to-back `window.open()` calls) fires this
/// handler more than once before `main.rs`'s tick loop next drains it, and
/// a single `Option` would silently lose all but the last one. Each entry is
/// `(url, user_gesture)`; the host drops popups without a gesture.
pub type PendingPopup = Rc<RefCell<Vec<(String, bool)>>>;

/// Registers the `NewWindowRequested` handler on `webview`. Every request is
/// marked `Handled` (via `SetHandled`) and `SetNewWindow` is never called,
/// so WebView2 never creates a real window of its own for it -- the target
/// URL lands in the returned queue instead, drained by `main.rs`'s tick loop
/// the same way `favicon::register`'s result is.
///
/// A request counts as the user's only when WebView2's `IsUserInitiated`
/// says so *and* the page got a click or key press within the gesture
/// window (`NavState::had_recent_input`): `IsUserInitiated` on navigations
/// was true even for a `setTimeout` navigation in a tab that never got
/// input (see `NavState::label`), so it isn't trusted alone.
pub fn register(webview: &ICoreWebView2, nav_state: SharedNavState) -> PendingPopup {
    let pending: PendingPopup = Rc::new(RefCell::new(Vec::new()));
    let mut token = Default::default();

    let pending_for_handler = pending.clone();
    let handler = NewWindowRequestedEventHandler::create(Box::new(move |_sender, args| {
        if let Some(args) = args {
            let mut uri = PWSTR::null();
            let mut user = BOOL::default();
            let user_initiated = unsafe { args.IsUserInitiated(&mut user) }.is_ok() && user.as_bool();
            // `try_borrow`: a WebView2 call made while `nav_state` is borrowed
            // can dispatch this event re-entrantly; treat that as no gesture.
            let user_gesture = user_initiated && nav_state.try_borrow().is_ok_and(|s| s.had_recent_input());
            if unsafe { args.Uri(&mut uri) }.is_ok() {
                pending_for_handler.borrow_mut().push((take_pwstr(uri), user_gesture));
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
