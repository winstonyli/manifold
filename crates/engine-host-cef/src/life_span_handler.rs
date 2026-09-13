use std::cell::RefCell;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};

/// Set by `on_before_popup` when the page asks to open a new tab
/// (`target=_blank`/`window.open()`/ctrl+click) — polled and drained by
/// the main loop each tick, same pattern as `nav_handler`'s `NavState`.
pub type SharedPendingOpenTab = Rc<RefCell<Option<String>>>;

pub fn new_shared_pending_open_tab() -> SharedPendingOpenTab {
    Rc::new(RefCell::new(None))
}

#[derive(Clone)]
pub struct EngineLifeSpanHandler {
    pending: SharedPendingOpenTab,
}

impl EngineLifeSpanHandler {
    pub fn new(pending: SharedPendingOpenTab) -> Self {
        Self { pending }
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
                *self.handler.pending.borrow_mut() = Some(url.to_string());
            }
            // Non-zero cancels popup creation - we spawn a real child tab
            // (a separate engine-host process) instead of letting CEF
            // create an in-process popup browser we have no IPC contract
            // for.
            true as _
        }
    }
}

impl LifeSpanHandlerBuilder {
    pub fn build(handler: EngineLifeSpanHandler) -> LifeSpanHandler {
        Self::new(handler)
    }
}
