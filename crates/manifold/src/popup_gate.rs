//! What a page may open as a new tab. A page-opened tab is a whole engine
//! process, and its URL becomes a browser-initiated navigation that skips the
//! renderer's own checks (a `file:` URL would open), so the host decides.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// At most this many page-opened tabs per source tab per `POPUP_WINDOW`.
pub(crate) const POPUP_BURST: usize = 5;
pub(crate) const POPUP_WINDOW: Duration = Duration::from_secs(10);

/// Whether a URL that came from page content (a popup, a context-menu link)
/// may become a new tab's first navigation.
pub(crate) fn page_tab_url_allowed(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower == "about:blank" || ["http://", "https://", "manifold://"].iter().any(|scheme| lower.starts_with(scheme))
}

/// A refused URL's scheme, for a log line or a toast: never the rest,
/// since a `data:` or `javascript:` URL carries page content. `None` when
/// what precedes the first `:` isn't a scheme (RFC 3986: a letter, then
/// letters, digits, `+`, `-`, `.`), so the caller words that case itself.
pub(crate) fn scheme_for_log(url: &str) -> Option<&str> {
    let (scheme, _) = url.trim().split_once(':')?;
    let valid = scheme.len() <= 32
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    valid.then_some(scheme)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admit {
    Open,
    /// `toast`: first drop for this source in the current window.
    Drop {
        toast: bool,
    },
}

#[derive(Default)]
pub(crate) struct PopupGate {
    /// Per source tab: opens in the current window, and when the last drop was toasted.
    by_source: HashMap<String, (Vec<Instant>, Option<Instant>)>,
}

impl PopupGate {
    pub(crate) fn admit(&mut self, source: &str, user_gesture: bool, now: Instant) -> Admit {
        let (opens, toasted) = self.by_source.entry(source.to_string()).or_default();
        opens.retain(|&t| now.duration_since(t) <= POPUP_WINDOW);
        if user_gesture && opens.len() < POPUP_BURST {
            opens.push(now);
            return Admit::Open;
        }
        let toast = toasted.is_none_or(|t| now.duration_since(t) > POPUP_WINDOW);
        if toast {
            *toasted = Some(now);
        }
        Admit::Drop { toast }
    }

    /// Forget a closed tab.
    pub(crate) fn forget(&mut self, source: &str) {
        self.by_source.remove(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn only_web_and_internal_schemes_may_open_a_tab() {
        for ok in ["https://a.org/x", "HTTP://a.org", "about:blank", "manifold://welcome"] {
            assert!(page_tab_url_allowed(ok), "{ok}");
        }
        for bad in
            ["file:///C:/x", "javascript:alert(1)", "about:config", "ms-settings:", "data:text/html,x", "", "blank"]
        {
            assert!(!page_tab_url_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn a_refused_url_is_logged_by_scheme_only() {
        assert_eq!(scheme_for_log("data:text/html,<p>secret</p>"), Some("data"));
        assert_eq!(scheme_for_log(" javascript:alert(1)"), Some("javascript"));
        assert_eq!(scheme_for_log("ms-settings:"), Some("ms-settings"));
        let long = format!("{}:x", "a".repeat(33));
        for none in ["", "blank", "<p>a:b", "1x:y", long.as_str()] {
            assert_eq!(scheme_for_log(none), None, "{none}");
        }
    }

    #[test]
    fn a_popup_without_a_gesture_is_dropped() {
        let mut gate = PopupGate::default();
        assert_eq!(gate.admit("t", false, Instant::now()), Admit::Drop { toast: true });
    }

    #[test]
    fn gestured_popups_are_capped_per_source_per_window() {
        let mut gate = PopupGate::default();
        let t0 = Instant::now();
        for _ in 0..POPUP_BURST {
            assert_eq!(gate.admit("t", true, t0), Admit::Open);
        }
        assert_eq!(gate.admit("t", true, t0), Admit::Drop { toast: true });
        assert_eq!(gate.admit("t", true, t0), Admit::Drop { toast: false }, "one toast per window");
        assert_eq!(gate.admit("other", true, t0), Admit::Open, "per source tab");
        assert_eq!(gate.admit("t", true, t0 + POPUP_WINDOW + Duration::from_millis(1)), Admit::Open);
    }
}
