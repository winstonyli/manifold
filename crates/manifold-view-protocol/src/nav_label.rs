//! Whether a navigation was started by the user or by the host (manifold
//! itself), shared by the CEF and WebView2 engine hosts. Only user-started
//! navigations are shared with the tab's other instances.

use std::time::{Duration, Instant};

/// How long after the host asks for a navigation its label applies: long
/// enough for a navigation to start, short enough that a click after a
/// request that changed nothing isn't taken for the host's.
pub const HOST_START_WINDOW: Duration = Duration::from_secs(2);

/// How long after a click or key press a navigation counts as the user's:
/// Chromium's own lifespan for transient user activation. It covers a click
/// whose handler navigates only after a fetch, and a page's own timer
/// navigation within 5 s of a click or key press, as Chromium does: CEF's
/// `user_gesture` labelled a fetch-then-navigate click, a timer 3 s after a
/// click and one 2 s after a key press all as user-started (runtime tested
/// 2026-09-26). Too long only shares one navigation too many: the one it
/// starts in another instance is the host's, and never shared back.
pub const GESTURE_WINDOW: Duration = Duration::from_secs(5);

/// Who started the current navigation, and the marks that decide it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NavLabel {
    /// Whether the current navigation was user-started, sent as
    /// `NavigationStateChanged::user_started`. Set when a navigation starts
    /// and kept through its redirects; cleared once its load's end has been
    /// sent (`finished`), so a later change nothing started isn't labelled.
    pub user_started: Option<bool>,
    /// The label of a navigation the host just asked for, and when:
    /// `false` for `Navigate` (a typed url, another instance's url, the
    /// initial load), `true` for back/forward. An engine's own gesture flag
    /// reports only the page's gestures, so an API call needs its label from
    /// here. It applies to the next navigation to start within
    /// `HOST_START_WINDOW`, same-document url changes included: a request
    /// can change nothing (loading the current url), and a stale label
    /// would mislabel the user's next click.
    pub host_started: Option<(bool, Instant)>,
    /// When the page last got a click or a key press. A navigation within
    /// `GESTURE_WINDOW` of it is the user's (for CEF, only where Chromium's
    /// `user_gesture` doesn't answer: same-document url changes). Any
    /// navigation uses it up.
    pub last_input: Option<Instant>,
    /// The current navigation's load ended; the engine's main loop clears
    /// `user_started` after sending it.
    pub finished: bool,
}

impl NavLabel {
    /// A browser just created at its initial URL: that first navigation is the host's.
    pub fn created_by_host(now: Instant) -> Self {
        Self { host_started: Some((false, now)), ..Self::default() }
    }

    /// A navigation is started and not yet finished.
    pub fn under_way(&self) -> bool {
        self.user_started.is_some() && !self.finished
    }

    /// Labels a navigation starting at `now`: the host's label if it just
    /// asked for one, else `gesture` (CEF's `user_gesture`) if the engine
    /// has one, else the user's if they just clicked or typed.
    pub fn label_new(&mut self, gesture: Option<bool>, now: Instant) {
        let host = self.host_started.take().filter(|(_, at)| now.duration_since(*at) < HOST_START_WINDOW);
        let input = self.last_input.take().is_some_and(|at| now.duration_since(at) < GESTURE_WINDOW);
        self.user_started = Some(host.map_or(gesture.unwrap_or(input), |(label, _)| label));
        self.finished = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_hosts_label_wins_inside_its_window() {
        let t0 = Instant::now();
        let mut l = NavLabel { host_started: Some((false, t0)), last_input: Some(t0), ..NavLabel::default() };
        l.label_new(Some(true), t0 + Duration::from_secs(1));
        assert_eq!(l.user_started, Some(false));
        assert!(l.under_way());
    }

    #[test]
    fn a_stale_host_label_falls_back_to_the_gesture_then_input() {
        let t0 = Instant::now();
        let late = t0 + HOST_START_WINDOW;
        let mut l = NavLabel { host_started: Some((false, t0)), ..NavLabel::default() };
        l.label_new(Some(true), late);
        assert_eq!(l.user_started, Some(true), "CEF's user_gesture");
        let mut l = NavLabel { last_input: Some(t0), ..NavLabel::default() };
        l.label_new(None, t0 + Duration::from_secs(4));
        assert_eq!(l.user_started, Some(true), "a click 4 s ago");
        let mut l = NavLabel { last_input: Some(t0), ..NavLabel::default() };
        l.label_new(None, t0 + GESTURE_WINDOW);
        assert_eq!(l.user_started, Some(false), "a click too long ago");
    }

    #[test]
    fn labelling_consumes_both_marks_and_reopens_the_navigation() {
        let t0 = Instant::now();
        let mut l =
            NavLabel { host_started: Some((true, t0)), last_input: Some(t0), finished: true, ..NavLabel::default() };
        l.label_new(None, t0);
        assert_eq!((l.host_started, l.last_input, l.finished), (None, None, false));
    }

    #[test]
    fn a_new_browser_counts_as_the_hosts_navigation() {
        let t0 = Instant::now();
        let mut l = NavLabel::created_by_host(t0);
        l.label_new(None, t0);
        assert_eq!(l.user_started, Some(false));
    }
}
