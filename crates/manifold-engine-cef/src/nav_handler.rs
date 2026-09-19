use std::cell::RefCell;

use cef::{rc::Rc, *};

/// Latest known navigation state, updated incrementally as CEF's
/// `DisplayHandler`/`LoadHandler` callbacks fire independently of each
/// other (address, title, and loading/can-go-back-forward each arrive on
/// their own) — `dirty` marks that at least one of them changed since the
/// main loop last read it, so a single `NavigationStateChanged` snapshot
/// can be sent per tick rather than one message per individual callback.
#[derive(Clone)]
pub struct NavState {
    pub url: String,
    pub title: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub loading: bool,
    pub dirty: bool,
}

impl NavState {
    fn new(initial_url: String) -> Self {
        Self {
            url: initial_url,
            title: String::new(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            dirty: true,
        }
    }
}

pub type SharedNavState = std::rc::Rc<RefCell<NavState>>;

pub fn new_shared_nav_state(initial_url: String) -> SharedNavState {
    std::rc::Rc::new(RefCell::new(NavState::new(initial_url)))
}

/// The last `HostToEngine::SetMuted` value requested for this tab -- CEF's
/// own mute (`main.rs`'s `SetMuted` handler) is a one-shot JS write against
/// whatever `<video>`/`<audio>` elements exist at that instant, unlike
/// WebView2/WebKit's real view-level mute property, which persists through
/// navigation on its own. Re-applying this on every load completion (see
/// `on_loading_state_change` below) gets CEF the same "stays muted across
/// navigation" behavior the other two engines get for free.
pub type SharedMuted = std::rc::Rc<std::cell::Cell<bool>>;

pub fn new_shared_muted() -> SharedMuted {
    std::rc::Rc::new(std::cell::Cell::new(false))
}

#[derive(Clone)]
pub struct EngineDisplayHandler {
    state: SharedNavState,
}

impl EngineDisplayHandler {
    pub fn new(state: SharedNavState) -> Self {
        Self { state }
    }
}

wrap_display_handler! {
    pub struct DisplayHandlerBuilder {
        handler: EngineDisplayHandler,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if let Some(url) = url {
                let mut state = self.handler.state.borrow_mut();
                state.url = url.to_string();
                state.dirty = true;
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let mut state = self.handler.state.borrow_mut();
            state.title = title.map(|t| t.to_string()).unwrap_or_default();
            state.dirty = true;
        }

        // `on_favicon_urlchange` used to live here -- CEF's own favicon
        // plumbing only ever handed over a URL, and its `icon_urls` list
        // was confirmed live (a real page fully loaded and screenshotted,
        // not just glanced at early) to always be empty anyway, for
        // multiple real pages with real favicons (github.com, iana.org).
        // Root cause, per CEF's own issue tracker (chromiumembedded/cef
        // #3293/#3685): `windowless_rendering_enabled` forces CEF's legacy
        // "Alloy" runtime rather than the modern "Chrome" runtime (OSR
        // isn't supported by the Chrome runtime at all) — Alloy predates
        // and never fully wired up the chrome-layer favicon-candidate
        // collection a normal windowed Chrome/CEF browser gets, which is
        // why the callback fired (it was genuinely called) but never
        // carried real candidate URLs -- a CEF/Alloy-runtime limitation in
        // windowless mode, not fixable from this handler's side. Removed
        // once favicon fetching moved to `manifold`'s own uniform
        // `{origin}/favicon.ico` HTTP fetch, independent of any engine.
    }
}

impl DisplayHandlerBuilder {
    pub fn build(handler: EngineDisplayHandler) -> DisplayHandler {
        Self::new(handler)
    }
}

#[derive(Clone)]
pub struct EngineLoadHandler {
    state: SharedNavState,
    muted: SharedMuted,
}

impl EngineLoadHandler {
    pub fn new(state: SharedNavState, muted: SharedMuted) -> Self {
        Self { state, muted }
    }
}

wrap_load_handler! {
    pub struct LoadHandlerBuilder {
        handler: EngineLoadHandler,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            {
                let mut state = self.handler.state.borrow_mut();
                state.loading = is_loading != 0;
                state.can_go_back = can_go_back != 0;
                state.can_go_forward = can_go_forward != 0;
                state.dirty = true;
            }
            // CEF fires this only on a real transition, not per-frame -- so
            // `is_loading == 0` here already *is* the "loading: true -> false"
            // edge WebKit's own poll loop has to detect manually. Ask the
            // renderer whether the page that just finished loading is raw
            // JSON; see `render_process_handler.rs` for the reply side and
            // `ipc_protocol::EngineToHost::JsonBody`'s own doc comment.
            if is_loading == 0 {
                if let Some(frame) = browser.and_then(|b| b.main_frame()) {
                    if let Some(mut message) =
                        process_message_create(Some(&crate::render_process_handler::CHECK_JSON_BODY.into()))
                    {
                        frame.send_process_message(ProcessId::RENDERER, Some(&mut message));
                    }
                    // Re-apply the last requested mute state to this
                    // newly-loaded page's own elements -- see `SharedMuted`'s
                    // own doc comment for why CEF needs this and the other
                    // three engines don't.
                    if self.handler.muted.get() {
                        let code = "document.querySelectorAll('video,audio').forEach(function(el){el.muted=true;});";
                        frame.execute_java_script(Some(&code.into()), None, 0);
                    }
                }
            }
        }
    }
}

impl LoadHandlerBuilder {
    pub fn build(handler: EngineLoadHandler) -> LoadHandler {
        Self::new(handler)
    }
}
