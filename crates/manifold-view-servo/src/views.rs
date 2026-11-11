//! The host's Servo views. Servo drops a load sent to a view while another load may be pending there, and a Back
//! there is overridden by that pending load, so such a load or Back goes to a new hidden view, which replaces the
//! shown one once it commits. Otherwise it goes to the shown view, which keeps its history.

use std::cell::RefCell;
use std::rc::Rc;

use dpi::PhysicalSize;
use manifold_view_protocol::nav_state::NavState;
use servo::{
    EmbedderControl, JSValue, NavigationRequest, RenderingContext, Servo, SoftwareRenderingContext, TraversalId,
    WebResourceLoad, WebView, WebViewBuilder, WebViewDelegate,
};
use url::Url;

#[cfg(test)]
mod tests;

/// Whether a load or a traversal sent to a view now reaches it.
#[derive(Default)]
pub(crate) struct LoadState {
    /// A load may be pending, and which of its arrival's two signals have come since: set when the view is built, at
    /// each load the host sends, at each one the page asks for (a link, a form, `location`, an iframe; Servo
    /// doesn't say which) and at each document fetch (a page's own Back to a discarded entry asks for none); cleared once a document has parsed its head and the history has changed, in either order.
    /// Neither alone: the old document sends `HeadParsed` whenever a script binds a new body, and a history change at
    /// `pushState`.
    maybe_pending: Option<Arrival>,
    /// Traversals sent and not yet complete.
    traversals: u32,
}

#[derive(Clone, Copy, Default)]
struct Arrival {
    head_parsed: bool,
    history_changed: bool,
}

impl LoadState {
    pub(crate) fn load_sent(&mut self) {
        self.maybe_pending = Some(Arrival::default());
    }

    pub(crate) fn head_parsed(&mut self) {
        self.arrive(|a| a.head_parsed = true);
    }

    pub(crate) fn history_changed(&mut self) {
        self.arrive(|a| a.history_changed = true);
    }

    fn arrive(&mut self, signal: impl FnOnce(&mut Arrival)) {
        if let Some(a) = self.maybe_pending.as_mut() {
            signal(a);
            if a.head_parsed && a.history_changed {
                self.maybe_pending = None;
            }
        }
    }

    pub(crate) fn traversal_started(&mut self) {
        self.traversals += 1;
    }

    pub(crate) fn traversal_complete(&mut self) {
        self.traversals = self.traversals.saturating_sub(1);
    }

    /// A load sent now is not dropped.
    pub(crate) fn load_in_place(&self) -> bool {
        self.maybe_pending.is_none() && self.traversals == 0
    }

    /// A traversal sent now is not overridden by a pending load.
    pub(crate) fn history_in_place(&self) -> bool {
        self.maybe_pending.is_none()
    }
}

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
    pub(crate) load: RefCell<LoadState>,
    /// The last `notify_history_changed`; the view has committed once it is non-empty.
    pub(crate) entries: RefCell<(Vec<Url>, usize)>,
}

impl Delegate {
    pub(crate) fn new(url: String) -> Rc<Self> {
        Rc::new(Delegate {
            frame_ready: RefCell::new(false),
            nav: RefCell::new(NavState::new(url, false)),
            context_menu_link: RefCell::new(None),
            json_body: Rc::new(RefCell::new(None)),
            redirect_content_type: Rc::new(RefCell::new(None)),
            load: RefCell::new(LoadState::default()),
            entries: RefCell::new((Vec::new(), 0)),
        })
    }
}

/// One Servo `WebView` and its delegate. Dropping it closes the view in Servo.
pub(crate) struct View {
    pub webview: WebView,
    pub delegate: Rc<Delegate>,
}

impl View {
    /// A shown view loading `url`, its load marked sent; a caller that wants it pending calls `hide()`.
    pub(crate) fn build(
        servo: &Servo,
        ctx: &Rc<SoftwareRenderingContext>,
        url: Url,
        size: PhysicalSize<u32>,
        zoom: f32,
    ) -> View {
        let delegate = Delegate::new(url.to_string());
        delegate.load.borrow_mut().load_sent();
        let webview = WebViewBuilder::new(servo, ctx.clone() as Rc<dyn RenderingContext>)
            .url(url)
            .delegate(delegate.clone())
            .build();
        webview.resize(size);
        webview.set_page_zoom(zoom);
        View { webview, delegate }
    }

    /// The page the view is on: its current entry (which moves when a traversal starts), else before the first
    /// commit the url it was built at.
    fn current_url(&self) -> Option<Url> {
        let entries = self.delegate.entries.borrow();
        entries
            .0
            .get(entries.1)
            .cloned()
            .or_else(|| self.webview.url())
            .or_else(|| Url::parse(&self.delegate.nav.borrow().url).ok())
    }
}

pub(crate) enum Load {
    Url(Url),
    Reload,
}

/// The shown view and, while a swap is under way, the hidden view that will replace it.
pub(crate) struct Views {
    shown: View,
    pending: Option<View>,
    size: PhysicalSize<u32>,
}

impl Views {
    pub(crate) fn new(servo: &Servo, ctx: &Rc<SoftwareRenderingContext>, url: Url, size: PhysicalSize<u32>) -> Views {
        Views { shown: View::build(servo, ctx, url, size, 1.0), pending: None, size }
    }

    pub(crate) fn shown(&self) -> &View {
        &self.shown
    }

    /// A Reload targets the page the user sees, so with a pending view it reloads that page, not the pending one.
    pub(crate) fn load(&mut self, servo: &Servo, ctx: &Rc<SoftwareRenderingContext>, load: Load) {
        if self.pending.is_none() && self.shown.delegate.load.borrow().load_in_place() {
            eprintln!("[views] load in place");
            self.shown.delegate.load.borrow_mut().load_sent();
            match load {
                Load::Url(url) => self.shown.webview.load(url),
                Load::Reload => self.shown.webview.reload(),
            }
            return;
        }
        let target = match load {
            Load::Url(url) => url,
            Load::Reload => match self.shown.current_url() {
                Some(url) => url,
                None => return,
            },
        };
        eprintln!("[views] swap to {target}");
        self.pending = Some(self.build_pending(servo, ctx, target));
    }

    /// In place, with no entry lookup: Servo completes an out-of-range traversal at once, so the count can't stick.
    pub(crate) fn traverse(&mut self, servo: &Servo, ctx: &Rc<SoftwareRenderingContext>, back: bool) {
        if self.shown.delegate.load.borrow().history_in_place() {
            eprintln!("[views] history in place");
            self.pending = None;
            self.shown.delegate.load.borrow_mut().traversal_started();
            if back {
                self.shown.webview.go_back(1);
            } else {
                self.shown.webview.go_forward(1);
            }
            return;
        }
        let (entries, current) = self.shown.delegate.entries.borrow().clone();
        let target = if back { current.checked_sub(1) } else { current.checked_add(1) };
        let Some(url) = target.and_then(|i| entries.get(i).cloned()) else { return };
        eprintln!("[views] history swap to {url}");
        self.pending = Some(self.build_pending(servo, ctx, url));
    }

    pub(crate) fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
        self.shown.webview.resize(size);
        if let Some(p) = &self.pending {
            p.webview.resize(size);
        }
    }

    pub(crate) fn set_zoom(&self, factor: f32) {
        self.shown.webview.set_page_zoom(factor);
        if let Some(p) = &self.pending {
            p.webview.set_page_zoom(factor);
        }
    }

    /// Once the pending view has committed it replaces the shown one, which is dropped. Never marked loading
    /// before: Servo says nothing of a pending view whose load ends in a 204, so the tab would stay loading.
    pub(crate) fn promote(&mut self) {
        if !self.pending.as_ref().is_some_and(|p| !p.delegate.entries.borrow().0.is_empty()) {
            return;
        }
        let next = self.pending.take().expect("checked");
        eprintln!("[views] promoted");
        next.webview.show();
        self.shown.webview.hide();
        *next.delegate.frame_ready.borrow_mut() = true;
        next.delegate.nav.borrow_mut().dirty = true;
        self.shown = next;
    }

    /// Replaces any older pending view: dropping it closes its load.
    fn build_pending(&mut self, servo: &Servo, ctx: &Rc<SoftwareRenderingContext>, url: Url) -> View {
        self.pending = None;
        let view = View::build(servo, ctx, url, self.size, self.shown.webview.page_zoom());
        view.webview.hide();
        view
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
        if matches!(status, servo::LoadStatus::HeadParsed) {
            self.load.borrow_mut().head_parsed();
        }
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

    /// The request is left to drop, which allows it.
    fn request_navigation(&self, _webview: WebView, _request: NavigationRequest) {
        self.load.borrow_mut().load_sent();
    }

    /// A document fetch the page started without asking: its own Back or Forward to an entry Servo discarded, which
    /// it refetches. Also every other document fetch, already marked; a frame's own navigation reads as one too, as in
    /// `request_navigation`. Dropping `load` lets it through.
    fn load_web_resource(&self, _webview: WebView, load: WebResourceLoad) {
        if load.request().is_for_main_frame {
            self.load.borrow_mut().load_sent();
        }
    }

    fn notify_traversal_complete(&self, _webview: WebView, _id: TraversalId) {
        self.load.borrow_mut().traversal_complete();
    }

    fn notify_history_changed(&self, _webview: WebView, entries: Vec<Url>, current: usize) {
        let mut nav = self.nav.borrow_mut();
        nav.can_go_back = current > 0;
        nav.can_go_forward = current + 1 < entries.len();
        nav.dirty = true;
        *self.entries.borrow_mut() = (entries, current);
        self.load.borrow_mut().history_changed();
    }
}
