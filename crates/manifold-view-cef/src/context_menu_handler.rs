use std::cell::RefCell;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};

/// `(x, y, href)` for a right-click that landed on a link -- see
/// `ipc_protocol::EngineToHost::ContextMenuLink`'s doc comment. CEF hands
/// this over directly via `ContextMenuParams` (`link_url`/`xcoord`/
/// `ycoord`), unlike WebKit/WebView2's own `document.elementFromPoint`
/// JS-query workaround -- no script round-trip needed here.
pub type SharedContextMenuLink = Rc<RefCell<Option<(f64, f64, String)>>>;

pub fn new_shared_context_menu_link() -> SharedContextMenuLink {
    Rc::new(RefCell::new(None))
}

#[derive(Clone)]
pub struct EngineContextMenuHandler {
    link: SharedContextMenuLink,
}

impl EngineContextMenuHandler {
    pub fn new(link: SharedContextMenuLink) -> Self {
        Self { link }
    }
}

wrap_context_menu_handler! {
    pub struct ContextMenuHandlerBuilder {
        handler: EngineContextMenuHandler,
    }

    impl ContextMenuHandler {
        fn on_before_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            _model: Option<&mut MenuModel>,
        ) {
            if let Some(params) = params {
                let href = CefString::from(&params.link_url()).to_string();
                if !href.is_empty() {
                    *self.handler.link.borrow_mut() =
                        Some((params.xcoord() as f64, params.ycoord() as f64, href));
                }
            }
        }

        // Windowless/OSR mode has no real window to anchor a native popup
        // to anyway -- returning 1 (handled) here unconditionally means
        // CEF never even tries to show one, the same "suppress the real
        // menu, use it only as an x/y+href data source" posture WebKit/
        // WebView2's own equivalent already has.
        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            _model: Option<&mut MenuModel>,
            _callback: Option<&mut RunContextMenuCallback>,
        ) -> ::std::os::raw::c_int {
            1
        }
    }
}

impl ContextMenuHandlerBuilder {
    pub fn build(handler: EngineContextMenuHandler) -> ContextMenuHandler {
        Self::new(handler)
    }
}
