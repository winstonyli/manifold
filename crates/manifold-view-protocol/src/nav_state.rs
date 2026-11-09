//! The navigation state a view host reports, used by the CEF, WebView2 and Servo hosts. The engine's callbacks (CEF's
//! `DisplayHandler`/`LoadHandler`/`RequestHandler`, WebView2's navigation events, Servo's `WebViewDelegate`) each
//! change one part of it as it arrives and set `dirty`; the host's main loop sends one
//! `ViewToHost::NavigationStateChanged` per tick for all of them (`take_message`), not one per callback.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use crate::nav_label::NavLabel;
use crate::ViewToHost;

#[derive(Clone, Default)]
pub struct NavState {
    pub url: String,
    pub title: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub loading: bool,
    /// Who started the current navigation. CEF reports Chromium's own `user_gesture` only for a new document
    /// (`on_before_browse`); a same-document url change (`history.pushState`, a new `#fragment`) falls back to
    /// `last_input`, set from the host's `main.rs`. WebView2 has no gesture flag to trust: `IsUserInitiated` was true
    /// even for a page's `setTimeout` navigation in a tab that never got any input (runtime tested 2026-09-26), so it
    /// rests on `last_input` alone, which a new document uses up as it starts. Servo leaves it at the default, so its
    /// messages say `user_started: None`.
    pub label: NavLabel,
    /// At least one part changed since the main loop last sent the state.
    pub dirty: bool,
}

impl NavState {
    /// The state of a browser the host created at `url`: that first navigation is the host's
    /// (`NavLabel::created_by_host`), and under way, so a report sent before the engine says so does not read as a
    /// finished load (the trail would take it as the page settling). `dirty` says whether it is sent on the first
    /// tick: CEF sends it at once, WebView2 waits for its first navigation event.
    pub fn new(url: String, dirty: bool) -> Self {
        Self { url, label: NavLabel::created_by_host(Instant::now()), loading: true, dirty, ..Self::default() }
    }

    /// If dirty: clears it and returns the message, then applies the label's reset (a finished navigation's label is
    /// sent with its load's end and cleared after, so a later change nothing started isn't labelled).
    pub fn take_message(&mut self) -> Option<ViewToHost> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let message = ViewToHost::NavigationStateChanged {
            url: self.url.clone(),
            title: self.title.clone(),
            can_go_back: self.can_go_back,
            can_go_forward: self.can_go_forward,
            loading: self.loading,
            user_started: self.label.user_started,
        };
        if std::mem::take(&mut self.label.finished) {
            self.label.user_started = None;
        }
        Some(message)
    }
}

/// A `NavState` the engine's callbacks and the main loop share.
pub type SharedNavState = Rc<RefCell<NavState>>;

pub fn new_shared_nav_state(url: String, dirty: bool) -> SharedNavState {
    Rc::new(RefCell::new(NavState::new(url, dirty)))
}

/// A page's raw JSON body (`ViewToHost::JsonBody`): filled once per navigation by the engine's page-kind check when
/// the page is JSON, drained by the main loop.
pub type SharedJsonBody = Rc<RefCell<Option<String>>>;

pub fn new_shared_json_body() -> SharedJsonBody {
    Rc::new(RefCell::new(None))
}

/// The content type of a page manifold may show in its own viewer (`ViewToHost::RedirectableContent`): the other
/// answer of the same page-kind check, drained the same way.
pub type SharedRedirectContentType = Rc<RefCell<Option<String>>>;

pub fn new_shared_redirect_content_type() -> SharedRedirectContentType {
    Rc::new(RefCell::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The message's fields in order, so a test compares them in one assert (`ViewToHost` has no `PartialEq`).
    fn fields(m: ViewToHost) -> (String, String, bool, bool, bool, Option<bool>) {
        match m {
            ViewToHost::NavigationStateChanged { url, title, can_go_back, can_go_forward, loading, user_started } => {
                (url, title, can_go_back, can_go_forward, loading, user_started)
            }
            other => panic!("not a NavigationStateChanged: {other:?}"),
        }
    }

    #[test]
    fn a_clean_state_sends_nothing() {
        assert!(NavState::default().take_message().is_none());
    }

    #[test]
    fn a_dirty_state_sends_once_then_is_clean() {
        let mut s = NavState {
            url: String::from("https://a.test/"),
            title: String::from("A"),
            can_go_back: true,
            loading: true,
            dirty: true,
            ..NavState::default()
        };
        let m = s.take_message().expect("a dirty state sends");
        assert_eq!(fields(m), (String::from("https://a.test/"), String::from("A"), true, false, true, None));
        assert!(!s.dirty);
        assert!(s.take_message().is_none());
    }

    #[test]
    fn a_finished_navigation_clears_its_label_after_sending_it() {
        let mut s = NavState { dirty: true, ..NavState::default() };
        s.label.user_started = Some(true);
        s.label.finished = true;
        assert_eq!(fields(s.take_message().unwrap()).5, Some(true), "the load's end still carries the label");
        assert_eq!((s.label.user_started, s.label.finished), (None, false));
    }

    #[test]
    fn an_unfinished_navigation_keeps_its_label() {
        let mut s = NavState { dirty: true, ..NavState::default() };
        s.label.user_started = Some(false);
        assert_eq!(fields(s.take_message().unwrap()).5, Some(false));
        assert_eq!(s.label.user_started, Some(false));
    }

    /// A browser just created has not loaded its page: its first report must not read as a finished load.
    #[test]
    fn a_host_created_state_is_loading() {
        let mut s = NavState::new(String::from("https://a.test/"), true);
        assert!(fields(s.take_message().unwrap()).4);
    }

    #[test]
    fn a_host_created_state_labels_its_first_navigation_the_hosts() {
        let s = NavState::new(String::from("https://a.test/"), true);
        assert_eq!(s.url, "https://a.test/");
        assert!(s.dirty);
        assert!(s.label.host_started.is_some_and(|(back_forward, _)| !back_forward));
        assert!(!NavState::new(String::new(), false).dirty);
    }
}
