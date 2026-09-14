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

/// The most recently reported favicon URL, drained once by the main loop
/// (which does the actual HTTP fetch + decode — see
/// `EngineToHost::FaviconChanged`'s doc comment for why CEF alone among
/// the three engines needs this extra step: `on_favicon_urlchange` only
/// ever hands over a URL, never pixels).
pub type SharedFaviconUrl = std::rc::Rc<RefCell<Option<String>>>;

pub fn new_shared_favicon_url() -> SharedFaviconUrl {
    std::rc::Rc::new(RefCell::new(None))
}

#[derive(Clone)]
pub struct EngineDisplayHandler {
    state: SharedNavState,
    favicon_url: SharedFaviconUrl,
}

impl EngineDisplayHandler {
    pub fn new(state: SharedNavState, favicon_url: SharedFaviconUrl) -> Self {
        Self { state, favicon_url }
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

        // CEF's own favicon plumbing only ever hands over a URL, never
        // decoded pixels (unlike Servo/WebKit's `favicon()` getters) —
        // take the first (an arbitrary but consistent choice; the list's
        // ordering isn't documented as size-sorted) and let the main loop
        // fetch+decode it, since that needs blocking I/O this handler
        // can't do.
        //
        // Confirmed via live logging (with a real page fully loaded and
        // screenshotted, not just glanced at early) that this callback
        // fires, but its `icon_urls` list is empty every time — for
        // multiple real pages with real favicons (github.com, iana.org).
        // Root cause, per CEF's own issue tracker (chromiumembedded/cef
        // #3293/#3685): `windowless_rendering_enabled` forces CEF's legacy
        // "Alloy" runtime rather than the modern "Chrome" runtime (OSR
        // isn't supported by the Chrome runtime at all) — Alloy predates
        // and never fully wired up the chrome-layer favicon-candidate
        // collection a normal windowed Chrome/CEF browser gets, which is
        // why the callback fires (it's genuinely called) but never carries
        // real candidate URLs. This is a CEF/Alloy-runtime limitation in
        // windowless mode, not something fixable from this handler's side
        // — kept as a graceful no-op (a missing favicon is cosmetic, never
        // load-bearing) with logging left in since favicon changes are
        // rare (a couple of times per navigation, not per frame) and this
        // is genuinely useful to see if CEF's behavior here ever changes.
        fn on_favicon_urlchange(
            &self,
            _browser: Option<&mut Browser>,
            icon_urls: Option<&mut CefStringList>,
        ) {
            let urls: Vec<String> = icon_urls
                .map(|l| l.clone().into_iter().collect())
                .unwrap_or_default();
            eprintln!("[favicon] on_favicon_urlchange fired, urls={urls:?}");
            if let Some(url) = urls.into_iter().next() {
                *self.handler.favicon_url.borrow_mut() = Some(url);
            }
        }
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
}

impl EngineLoadHandler {
    pub fn new(state: SharedNavState) -> Self {
        Self { state }
    }
}

wrap_load_handler! {
    pub struct LoadHandlerBuilder {
        handler: EngineLoadHandler,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            let mut state = self.handler.state.borrow_mut();
            state.loading = is_loading != 0;
            state.can_go_back = can_go_back != 0;
            state.can_go_forward = can_go_forward != 0;
            state.dirty = true;
        }
    }
}

impl LoadHandlerBuilder {
    pub fn build(handler: EngineLoadHandler) -> LoadHandler {
        Self::new(handler)
    }
}
