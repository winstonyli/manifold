//! Navigation state tracking -- mirrors `manifold-engine-cef/src/nav_handler.rs`'s
//! `SharedNavState`/`dirty`-flag pattern exactly, since WebView2's real
//! native events (`NavigationStarting`/`NavigationCompleted`/
//! `DocumentTitleChanged`/`HistoryChanged`) play the same role CEF's
//! `DisplayHandler`/`LoadHandler` callbacks do: each just mutates this
//! shared cell and sets `dirty`, coalesced into one `NavigationStateChanged`
//! message per tick by `main.rs`'s loop rather than one message per event.

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::{
    take_pwstr, DocumentTitleChangedEventHandler, ExecuteScriptCompletedHandler, HistoryChangedEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
};
use windows::core::{BOOL, HSTRING, PWSTR};

pub struct NavState {
    pub url: String,
    pub title: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub loading: bool,
    pub dirty: bool,
}

pub type SharedNavState = Rc<RefCell<NavState>>;

pub fn new_shared_nav_state(initial_url: String) -> SharedNavState {
    Rc::new(RefCell::new(NavState {
        url: initial_url,
        title: String::new(),
        can_go_back: false,
        can_go_forward: false,
        loading: false,
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

/// Registers all four nav-state event handlers on `webview`. Each handler
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
                .and_then(|a| {
                    let mut uri = PWSTR::null();
                    unsafe { a.Uri(&mut uri) }.ok()?;
                    Some(take_pwstr(uri))
                })
                .unwrap_or_default();
            let mut s = state.borrow_mut();
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
}
