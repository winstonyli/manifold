use std::cell::RefCell;
use std::rc::Rc;

use manifold_view_protocol::nav_state::NavState;
use servo::{EmbedderControl, JSValue, WebView, WebViewDelegate};
use url::Url;

pub(crate) struct Delegate {
    pub(crate) frame_ready: RefCell<bool>,
    pub(crate) nav: RefCell<NavState>,
    /// `(x, y, href)` for a right-click that landed on a link -- see
    /// `manifold_view_protocol::ViewToHost::ContextMenuLink`'s doc comment. Servo
    /// hands this over directly via `ContextMenuElementInformation::link_url`
    /// on the `EmbedderControl::ContextMenu` it requests for every right-
    /// click, unlike WebKit/WebView2's own `document.elementFromPoint`
    /// JS-query workaround -- no script round-trip needed here.
    pub(crate) context_menu_link: RefCell<Option<(f64, f64, String)>>,
    /// See `manifold_view_protocol::ViewToHost::JsonBody`'s own doc comment --
    /// filled in once per navigation by `notify_load_status_changed`'s own
    /// `WebView::evaluate_javascript` call. `Rc`-wrapped (unlike every
    /// other field here) specifically so its own callback -- a `'static`
    /// closure `evaluate_javascript` stores and invokes later, well after
    /// this method's `&self` borrow has ended -- can hold an owned clone of
    /// just this one cell, not the whole `Delegate`.
    pub(crate) json_body: Rc<RefCell<Option<String>>>,
    /// See `manifold_view_protocol::ViewToHost::RedirectableContent`'s own doc
    /// comment -- same `evaluate_javascript` call as `json_body`, just the
    /// other branch of the same `document.contentType` check, `Rc`-wrapped
    /// for the same reason.
    pub(crate) redirect_content_type: Rc<RefCell<Option<String>>>,
}

impl Delegate {
    pub(crate) fn new(url: String) -> Rc<Self> {
        Rc::new(Delegate {
            frame_ready: RefCell::new(false),
            nav: RefCell::new(NavState::new(url, false)),
            context_menu_link: RefCell::new(None),
            json_body: Rc::new(RefCell::new(None)),
            redirect_content_type: Rc::new(RefCell::new(None)),
        })
    }
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        *self.frame_ready.borrow_mut() = true;
    }

    /// Fires for every kind of native-ish prompt Servo wants to show
    /// (`<select>`/color/file pickers, IME, `alert()`/`confirm()`, and a
    /// right-click's own context menu) -- only `ContextMenu` is handled
    /// here, since none of the others are wired up by this view-host at
    /// all. Dropping `menu` without calling `.select()`/`.dismiss()`
    /// explicitly still tells Servo the menu was dismissed with no
    /// selection (see `ContextMenu`'s own `Drop` impl) -- exactly the
    /// "suppress the real menu, use it only as a data source" posture
    /// WebKit/WebView2/CEF's own equivalents already have, so a real
    /// native-looking popup never appears over the page.
    fn show_embedder_control(&self, _webview: WebView, embedder_control: EmbedderControl) {
        if let EmbedderControl::ContextMenu(menu) = embedder_control {
            if let Some(url) = menu.element_info().link_url.clone() {
                let position = menu.position();
                *self.context_menu_link.borrow_mut() =
                    Some((position.min.x as f64, position.min.y as f64, url.to_string()));
            }
        }
    }

    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        let mut nav = self.nav.borrow_mut();
        nav.url = url.to_string();
        nav.dirty = true;
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        let mut nav = self.nav.borrow_mut();
        nav.title = title.unwrap_or_default();
        nav.dirty = true;
    }

    fn notify_load_status_changed(&self, webview: WebView, status: servo::LoadStatus) {
        let complete = matches!(status, servo::LoadStatus::Complete | servo::LoadStatus::Stopped);
        let mut nav = self.nav.borrow_mut();
        nav.loading = !complete;
        nav.dirty = true;
        drop(nav);

        // See `manifold_view_protocol::ViewToHost::JsonBody`/`RedirectableContent`'s
        // own doc comments. Unlike WebKit/WebView2's `ExecuteScript`,
        // Servo's own `evaluate_javascript` hands back a real structured
        // `JSValue`, not a JSON-encoded string -- no unescaping step needed
        // here. The `"json\n"`/`"type\n"` prefixes still disambiguate the
        // two outcomes over this one script round trip, same as the other
        // two script-eval engines.
        if complete {
            let json_body = self.json_body.clone();
            let redirect_content_type = self.redirect_content_type.clone();
            webview.evaluate_javascript(manifold_view_protocol::PAGE_KIND_SCRIPT, move |result| {
                if let Ok(JSValue::String(text)) = result {
                    match manifold_view_protocol::PageKindReply::parse(&text) {
                        Some(manifold_view_protocol::PageKindReply::Json(body)) => *json_body.borrow_mut() = Some(body),
                        Some(manifold_view_protocol::PageKindReply::Other(t)) => {
                            *redirect_content_type.borrow_mut() = Some(t)
                        }
                        None => {}
                    }
                }
            });
        }
    }

    fn notify_history_changed(&self, _webview: WebView, entries: Vec<Url>, current: usize) {
        let mut nav = self.nav.borrow_mut();
        nav.can_go_back = current > 0;
        nav.can_go_forward = current + 1 < entries.len();
        nav.dirty = true;
    }
}
