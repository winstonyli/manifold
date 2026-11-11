//! The WebView2 events that keep the host's `NavState` (`manifold_view_protocol::nav_state`) current: the real
//! native events (`NavigationStarting`/`NavigationCompleted`/`SourceChanged`/`DocumentTitleChanged`/
//! `HistoryChanged`) play the role CEF's `DisplayHandler`/`LoadHandler` callbacks do, each just mutating the shared
//! cell and setting `dirty`, coalesced into one `NavigationStateChanged` message per tick by `main.rs`'s loop
//! (`NavState::take_message`) rather than one message per event.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use manifold_view_protocol::nav_state::{SharedJsonBody, SharedNavState, SharedRedirectContentType};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2NavigationStartingEventArgs, ICoreWebView2SourceChangedEventArgs,
};
use webview2_com::{
    take_pwstr, DocumentTitleChangedEventHandler, ExecuteScriptCompletedHandler, HistoryChangedEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler, SourceChangedEventHandler,
};
use windows::core::{BOOL, HSTRING, PWSTR};

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

/// Registers all five nav-state event handlers on `webview`, and returns whether a document has committed since (the
/// engine starts on an uncommitted `about:blank`, see the `Reload` arm in `main.rs`). Each handler
/// closure is leaked into the event-registration token via `add_*` and
/// never explicitly removed -- matches this project's other view hosts,
/// which live exactly as long as the process and never unregister handlers
/// either (nothing to clean up, the whole process exits on `Shutdown`).
pub fn register_handlers(
    webview: &ICoreWebView2,
    state: SharedNavState,
    json_body: SharedJsonBody,
    redirect_content_type: SharedRedirectContentType,
) -> Rc<Cell<bool>> {
    let mut token = Default::default();
    let committed = Rc::new(Cell::new(false));

    {
        let state = state.clone();
        let webview_for_query = webview.clone();
        // Not the target url: the url stays the shown page's until a document commits (`SourceChanged`), so a load that
        // never commits (a 204, a download, one cancelled by the next) never reports a page the tab did not reach.
        let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
            let redirected = args.as_ref().is_some_and(is_redirected);
            let mut s = state.borrow_mut();
            // A redirect continues the navigation it came from, and keeps
            // its label, while that one is under way. WebView2 also calls a
            // page script's navigation just after the load a redirect (a
            // client redirect); that one gets its own.
            if !(redirected && s.label.under_way()) {
                s.label.label_new(None, Instant::now());
            }
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
        let committed = committed.clone();
        let handler = NavigationCompletedEventHandler::create(Box::new(move |sender, _args| {
            let mut s = state.borrow_mut();
            s.loading = false;
            s.label.finished = true;
            // Before any document commits, `Source` is the engine's own `about:blank`, no page: a first load that
            // never commits (a 204, or one the next load cancels, as a woken tab's Back does) leaves the url requested.
            if let Some(sender) = sender.as_ref().filter(|_| committed.get()) {
                let mut uri = PWSTR::null();
                if unsafe { sender.Source(&mut uri) }.is_ok() {
                    s.url = take_pwstr(uri);
                }
            }
            s.can_go_back = can_go_back(&webview_for_query);
            s.can_go_forward = can_go_forward(&webview_for_query);
            s.dirty = true;
            drop(s);

            // See `manifold_view_protocol::ViewToHost::JsonBody`/`RedirectableContent`'s
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
                    match manifold_view_protocol::PageKindReply::parse(&text) {
                        Some(manifold_view_protocol::PageKindReply::Json(body)) => *json_body.borrow_mut() = Some(body),
                        Some(manifold_view_protocol::PageKindReply::Other(t)) => {
                            *redirect_content_type.borrow_mut() = Some(t)
                        }
                        None => {}
                    }
                }
                Ok(())
            }));
            unsafe {
                let _ = webview_for_json
                    .ExecuteScript(&HSTRING::from(manifold_view_protocol::PAGE_KIND_SCRIPT), &exec_handler);
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
        // A new document committing, or a same-document url change (`history.pushState`/`replaceState`, a new
        // `#fragment`), which starts no navigation, so only this reports it.
        let state = state.clone();
        let committed = committed.clone();
        let handler = SourceChangedEventHandler::create(Box::new(move |sender, args| {
            let Some(sender) = &sender else { return Ok(()) };
            let mut uri = PWSTR::null();
            if unsafe { sender.Source(&mut uri) }.is_err() {
                return Ok(());
            }
            let mut s = state.borrow_mut();
            if args.as_ref().is_none_or(is_new_document) {
                committed.set(true);
            } else if !s.label.under_way() {
                // During a load it is part of that navigation (a router rewriting the url as the page starts);
                // otherwise it is a navigation of its own, already complete.
                s.label.label_new(None, Instant::now());
                s.label.finished = true;
            }
            s.url = take_pwstr(uri);
            s.dirty = true;
            Ok(())
        }));
        unsafe {
            let _ = webview.add_SourceChanged(&handler, &mut token);
        }
    }
    committed
}
