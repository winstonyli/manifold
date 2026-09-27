//! Navigation state tracking -- mirrors `manifold-engine-cef/src/nav_handler.rs`'s
//! `SharedNavState`/`dirty`-flag pattern exactly, since WebView2's real
//! native events (`NavigationStarting`/`NavigationCompleted`/
//! `SourceChanged`/`DocumentTitleChanged`/`HistoryChanged`) play the same role CEF's
//! `DisplayHandler`/`LoadHandler` callbacks do: each just mutates this
//! shared cell and sets `dirty`, coalesced into one `NavigationStateChanged`
//! message per tick by `main.rs`'s loop rather than one message per event.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2NavigationStartingEventArgs, ICoreWebView2SourceChangedEventArgs,
};
use webview2_com::{
    take_pwstr, DocumentTitleChangedEventHandler, ExecuteScriptCompletedHandler, HistoryChangedEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler, SourceChangedEventHandler,
};
use windows::core::{BOOL, HSTRING, PWSTR};

pub struct NavState {
    pub url: String,
    pub title: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub loading: bool,
    /// Whether the current navigation was user-started, sent as
    /// `NavigationStateChanged::user_started`. Set when a navigation starts
    /// and kept through its redirects; cleared once its completion has been
    /// sent (`finished`), so a later change nothing started isn't labelled.
    pub user_started: Option<bool>,
    /// The label of a navigation the host just asked for, and when:
    /// `false` for `Navigate` (a typed url, another instance's url, the
    /// initial load), `true` for back/forward. It applies to the next
    /// navigation or same-document url change within `HOST_START_WINDOW`:
    /// a request can change nothing (`Navigate` to the current url), and a
    /// stale label would mislabel the user's next click.
    pub host_started: Option<(bool, Instant)>,
    /// When the page last got a click or a key press (`main.rs`). A
    /// navigation or same-document url change that starts within
    /// `GESTURE_WINDOW` of it is the user's, and uses it up, as a new
    /// document starts without Chromium's transient user activation. WebView2's own `IsUserInitiated` can't
    /// say: it was true even for a page's `setTimeout` navigation in a tab
    /// that never got any input (runtime tested 2026-09-26).
    pub last_input: Option<Instant>,
    /// The current navigation completed; `main.rs` clears `user_started`
    /// after sending it.
    pub finished: bool,
    pub dirty: bool,
}

/// See `NavState::host_started`: long enough for a navigation to start,
/// short enough that a click after a request that changed nothing isn't
/// taken for the host's.
const HOST_START_WINDOW: Duration = Duration::from_secs(2);

/// See `NavState::last_input`: Chromium's own lifespan for transient user
/// activation, which covers a click whose handler navigates only after a
/// fetch. It also covers a page's own timer navigation within 5 s of a
/// click or key press, as Chromium does: CEF's `user_gesture` labelled a
/// fetch-then-navigate click, a timer 3 s after a click and one 2 s after a
/// key press all as user-started (runtime tested 2026-09-26). Too long
/// only shares one navigation too many: the one it starts in another
/// instance is the host's, and never shared back.
const GESTURE_WINDOW: Duration = Duration::from_secs(5);

impl NavState {
    /// A navigation is started and not yet finished.
    fn under_way(&self) -> bool {
        self.user_started.is_some() && !self.finished
    }

    /// Labels a navigation starting now: the host's label if it just asked
    /// for one, else the user's if they just clicked or typed.
    fn label_new(&mut self) {
        let host = self.host_started.take().filter(|(_, at)| at.elapsed() < HOST_START_WINDOW);
        let input = self.last_input.take().is_some_and(|at| at.elapsed() < GESTURE_WINDOW);
        self.user_started = Some(host.map_or(input, |(label, _)| label));
        self.finished = false;
    }
}

pub type SharedNavState = Rc<RefCell<NavState>>;

pub fn new_shared_nav_state(initial_url: String) -> SharedNavState {
    Rc::new(RefCell::new(NavState {
        url: initial_url,
        title: String::new(),
        can_go_back: false,
        can_go_forward: false,
        loading: false,
        user_started: None,
        host_started: Some((false, Instant::now())), // `main.rs` navigates to `initial_url` next
        last_input: None,
        finished: false,
        dirty: false,
    }))
}

/// See `ipc_protocol::EngineToHost::JsonBody`'s own doc comment -- filled
/// in once per navigation, from the same `NavigationCompleted` handler
/// below that also updates `NavState`.
pub type SharedJsonBody = Rc<RefCell<Option<String>>>;

pub fn new_shared_json_body() -> SharedJsonBody {
    Rc::new(RefCell::new(None))
}

/// See `ipc_protocol::EngineToHost::RedirectableContent`'s own doc comment
/// -- same `NavigationCompleted` handler as `SharedJsonBody`, just the
/// other branch of the same `document.contentType` check.
pub type SharedRedirectContentType = Rc<RefCell<Option<String>>>;

pub fn new_shared_redirect_content_type() -> SharedRedirectContentType {
    Rc::new(RefCell::new(None))
}

/// The raw COM binding's `CanGoBack`/`CanGoForward` are `[out, retval]
/// BOOL` params, not idiomatic return values -- this is the shared
/// out-param dance for both.
fn can_go_back(webview: &ICoreWebView2) -> bool {
    let mut value = BOOL(0);
    unsafe { webview.CanGoBack(&mut value) }.ok();
    value.as_bool()
}

fn can_go_forward(webview: &ICoreWebView2) -> bool {
    let mut value = BOOL(0);
    unsafe { webview.CanGoForward(&mut value) }.ok();
    value.as_bool()
}

fn is_redirected(args: &ICoreWebView2NavigationStartingEventArgs) -> bool {
    let mut value = BOOL(0);
    unsafe { args.IsRedirected(&mut value) }.ok();
    value.as_bool()
}

fn is_new_document(args: &ICoreWebView2SourceChangedEventArgs) -> bool {
    let mut value = BOOL(0);
    unsafe { args.IsNewDocument(&mut value) }.ok();
    value.as_bool()
}

/// Registers all five nav-state event handlers on `webview`. Each handler
/// closure is leaked into the event-registration token via `add_*` and
/// never explicitly removed -- matches this project's other engine-hosts,
/// which live exactly as long as the process and never unregister handlers
/// either (nothing to clean up, the whole process exits on `Shutdown`).
pub fn register_handlers(
    webview: &ICoreWebView2,
    state: SharedNavState,
    json_body: SharedJsonBody,
    redirect_content_type: SharedRedirectContentType,
) {
    let mut token = Default::default();

    {
        let state = state.clone();
        let webview_for_query = webview.clone();
        let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
            let url = args
                .as_ref()
                .and_then(|a| {
                    let mut uri = PWSTR::null();
                    unsafe { a.Uri(&mut uri) }.ok()?;
                    Some(take_pwstr(uri))
                })
                .unwrap_or_default();
            let redirected = args.as_ref().is_some_and(is_redirected);
            let mut s = state.borrow_mut();
            // A redirect continues the navigation it came from, and keeps
            // its label, while that one is under way. WebView2 also calls a
            // page script's navigation just after the load a redirect (a
            // client redirect); that one gets its own.
            if !(redirected && s.under_way()) {
                s.label_new();
            }
            s.url = url;
            s.loading = true;
            s.can_go_back = can_go_back(&webview_for_query);
            s.can_go_forward = can_go_forward(&webview_for_query);
            s.dirty = true;
            Ok(())
        }));
        unsafe {
            let _ = webview.add_NavigationStarting(&handler, &mut token);
        }
    }

    {
        let state = state.clone();
        let webview_for_query = webview.clone();
        let webview_for_json = webview.clone();
        let json_body = json_body.clone();
        let redirect_content_type = redirect_content_type.clone();
        let handler = NavigationCompletedEventHandler::create(Box::new(move |sender, _args| {
            let mut s = state.borrow_mut();
            s.loading = false;
            s.finished = true;
            if let Some(sender) = &sender {
                let mut uri = PWSTR::null();
                if unsafe { sender.Source(&mut uri) }.is_ok() {
                    s.url = take_pwstr(uri);
                }
            }
            s.can_go_back = can_go_back(&webview_for_query);
            s.can_go_forward = can_go_forward(&webview_for_query);
            s.dirty = true;
            drop(s);

            // See `ipc_protocol::EngineToHost::JsonBody`/`RedirectableContent`'s
            // own doc comments. `ExecuteScript`'s own return value comes
            // back JSON-encoded (an already-quoted-and-escaped string, or
            // the literal text `null`) -- `serde_json::from_str::<Option<String>>`
            // decodes both cases correctly and un-escapes real page content
            // (quotes/newlines/etc a JSON body can genuinely contain),
            // unlike `input.rs`'s own plain strip-outer-quotes trick, which
            // only stays safe there because a link href effectively never
            // contains a literal `"`. The `"json\n"`/`"type\n"` prefixes
            // disambiguate the two outcomes over this one script round trip
            // -- safe since they're prepended before any real page content,
            // never something a real JSON body could itself start with by
            // coincidence.
            let json_body = json_body.clone();
            let redirect_content_type = redirect_content_type.clone();
            let exec_handler = ExecuteScriptCompletedHandler::create(Box::new(move |_error, result| {
                if let Ok(Some(text)) = serde_json::from_str::<Option<String>>(&result.to_string()) {
                    if let Some(body) = text.strip_prefix("json\n") {
                        *json_body.borrow_mut() = Some(body.to_string());
                    } else if let Some(content_type) = text.strip_prefix("type\n") {
                        *redirect_content_type.borrow_mut() = Some(content_type.to_string());
                    }
                }
                Ok(())
            }));
            unsafe {
                let _ = webview_for_json.ExecuteScript(
                    &HSTRING::from(
                        "(function(){var t=document.contentType;if(t==='application/json'){return 'json\\n'+document.body.innerText;}if(t){return 'type\\n'+t;}return null;})();",
                    ),
                    &exec_handler,
                );
            }
            Ok(())
        }));
        unsafe {
            let _ = webview.add_NavigationCompleted(&handler, &mut token);
        }
    }

    {
        let state = state.clone();
        let handler = DocumentTitleChangedEventHandler::create(Box::new(move |sender, _args| {
            if let Some(sender) = &sender {
                let mut title = PWSTR::null();
                if unsafe { sender.DocumentTitle(&mut title) }.is_ok() {
                    let mut s = state.borrow_mut();
                    s.title = take_pwstr(title);
                    s.dirty = true;
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = webview.add_DocumentTitleChanged(&handler, &mut token);
        }
    }

    {
        let state = state.clone();
        let webview_for_query = webview.clone();
        let handler = HistoryChangedEventHandler::create(Box::new(move |_sender, _args| {
            let mut s = state.borrow_mut();
            s.can_go_back = can_go_back(&webview_for_query);
            s.can_go_forward = can_go_forward(&webview_for_query);
            s.dirty = true;
            Ok(())
        }));
        unsafe {
            let _ = webview.add_HistoryChanged(&handler, &mut token);
        }
    }

    {
        // A same-document url change (`history.pushState`/`replaceState`,
        // a new `#fragment`) starts no navigation, so only this reports it.
        let state = state.clone();
        let handler = SourceChangedEventHandler::create(Box::new(move |sender, args| {
            if args.as_ref().is_none_or(is_new_document) {
                return Ok(()); // `NavigationStarting`/`NavigationCompleted` report it
            }
            let Some(sender) = &sender else { return Ok(()) };
            let mut uri = PWSTR::null();
            if unsafe { sender.Source(&mut uri) }.is_err() {
                return Ok(());
            }
            let mut s = state.borrow_mut();
            // During a load it is part of that navigation (a router
            // rewriting the url as the page starts); otherwise it is a
            // navigation of its own, already complete.
            if !s.under_way() {
                s.label_new();
                s.finished = true;
            }
            s.url = take_pwstr(uri);
            s.dirty = true;
            Ok(())
        }));
        unsafe {
            let _ = webview.add_SourceChanged(&handler, &mut token);
        }
    }
}
