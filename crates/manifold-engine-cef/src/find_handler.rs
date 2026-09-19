use std::cell::RefCell;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};

/// `(count, active_match_ordinal)` — mirrors `ipc_protocol::EngineToHost::
/// FindResult` exactly. CEF's own `on_find_result` fires multiple times per
/// search as matches are found incrementally (`final_update == 0` for all
/// but the last) — only the final one is worth a message, so intermediate
/// callbacks are dropped rather than flooding `manifold` with partial
/// counts.
pub type SharedFindResult = Rc<RefCell<Option<(u32, u32)>>>;

pub fn new_shared_find_result() -> SharedFindResult {
    Rc::new(RefCell::new(None))
}

#[derive(Clone)]
pub struct EngineFindHandler {
    result: SharedFindResult,
}

impl EngineFindHandler {
    pub fn new(result: SharedFindResult) -> Self {
        Self { result }
    }
}

wrap_find_handler! {
    pub struct FindHandlerBuilder {
        handler: EngineFindHandler,
    }

    impl FindHandler {
        fn on_find_result(
            &self,
            _browser: Option<&mut Browser>,
            _identifier: ::std::os::raw::c_int,
            count: ::std::os::raw::c_int,
            _selection_rect: Option<&Rect>,
            active_match_ordinal: ::std::os::raw::c_int,
            final_update: ::std::os::raw::c_int,
        ) {
            if final_update != 0 {
                *self.handler.result.borrow_mut() =
                    Some((count.max(0) as u32, active_match_ordinal.max(0) as u32));
            }
        }
    }
}

impl FindHandlerBuilder {
    pub fn build(handler: EngineFindHandler) -> FindHandler {
        Self::new(handler)
    }
}
