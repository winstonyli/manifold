use std::cell::RefCell;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};

/// Set by `on_before_popup` when the page asks to open a new tab
/// (`target=_blank`/`window.open()`/ctrl+click) — polled and drained by
/// the main loop each tick, same pattern as `nav_handler`'s `NavState`. A
/// queue, not a single slot -- a page opening several popups synchronously
/// (e.g. back-to-back `window.open()` calls) fires `on_before_popup` more
/// than once before the next drain, and a single `Option` would silently
/// lose all but the last one.
pub type SharedPendingOpenTab = Rc<RefCell<Vec<String>>>;

pub fn new_shared_pending_open_tab() -> SharedPendingOpenTab {
    Rc::new(RefCell::new(Vec::new()))
}

/// Set by `on_before_close`, once CEF has actually finished tearing down
/// the browser -- `HostToEngine::Shutdown`'s handler polls this before
/// calling `cef::shutdown()`, per CEF's own documented contract (call
/// `close_browser`, wait for `on_before_close`, only then shut down the
/// whole CEF runtime).
pub type SharedClosed = Rc<RefCell<bool>>;

pub fn new_shared_closed() -> SharedClosed {
    Rc::new(RefCell::new(false))
}

#[derive(Clone)]
pub struct EngineLifeSpanHandler {
    pending: SharedPendingOpenTab,
    closed: SharedClosed,
}

impl EngineLifeSpanHandler {
    pub fn new(pending: SharedPendingOpenTab, closed: SharedClosed) -> Self {
        Self { pending, closed }
    }
}

wrap_life_span_handler! {
    pub struct LifeSpanHandlerBuilder {
        handler: EngineLifeSpanHandler,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(url) = target_url {
                self.handler.pending.borrow_mut().push(url.to_string());
            }
            // Non-zero cancels popup creation - we spawn a real child tab
            // (a separate engine-host process) instead of letting CEF
            // create an in-process popup browser we have no IPC contract
            // for.
            true as _
        }

        // CEF's documented shutdown contract: `close_browser` must be
        // called, and `cef::shutdown()` must not run until this fires --
        // calling it earlier, with the browser/renderer still alive, risks
        // a crash or hang inside libcef itself.
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            *self.handler.closed.borrow_mut() = true;
        }
    }
}

impl LifeSpanHandlerBuilder {
    pub fn build(handler: EngineLifeSpanHandler) -> LifeSpanHandler {
        Self::new(handler)
    }
}
