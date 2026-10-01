//! A tab's back/forward list as the host saw it, so a tab keeps its history
//! when its engine is replaced (crash restart, wake from sleep, engine
//! swap): the new engine starts with the one page it was told to load.
//!
//! Until the engine is replaced the engine's own history is used (`GoBack`,
//! scroll restoration and all) and this list only follows along. After a
//! replacement the tab is `hosted`: Back and Forward navigate the engine to
//! the list's neighbour URL, a plain load with no scroll or form state, and
//! the toolbar enables them from this list. Redirects and `pushState` are
//! only approximated (see `observe`).

/// Most entries kept per tab.
const MAX_ENTRIES: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dir {
    Back,
    Forward,
}

/// What the host should do for a Back/Forward click.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NavAction {
    /// Send the engine `GoBack`/`GoForward`.
    Native,
    /// Navigate the engine to this URL.
    Load(String),
    None,
}

#[derive(Debug, Default)]
pub(crate) struct NavHistory {
    entries: Vec<String>,
    index: usize,
    /// The engine's own history no longer matches `entries`.
    hosted: bool,
    /// The direction of the `GoBack`/`GoForward` just sent to the engine.
    pending: Option<Dir>,
    /// After a hosted load: the URL it should arrive at, and the one the
    /// tab is leaving (still reported until the load commits).
    expecting: Option<(String, String)>,
    /// The last report was still loading: a URL change then is a redirect.
    loading: bool,
}

impl NavHistory {
    /// Folds in one `NavigationStateChanged` (many repeat the same URL).
    pub(crate) fn observe(&mut self, url: &str, loading: bool) {
        let was_loading = std::mem::replace(&mut self.loading, loading);
        if self.entries.is_empty() {
            self.entries.push(url.to_string());
            return;
        }
        if self.entries[self.index] == url {
            self.expecting = None;
            self.pending = None;
            return;
        }
        if let Some((target, from)) = self.expecting.take() {
            if url == from {
                self.expecting = Some((target, from));
                return;
            }
            // Arrived, possibly after a redirect.
            self.entries[self.index] = url.to_string();
            return;
        }
        match self.pending.take() {
            Some(Dir::Back) if self.index > 0 && self.entries[self.index - 1] == url => self.index -= 1,
            Some(Dir::Forward) if self.entries.get(self.index + 1).is_some_and(|e| e == url) => self.index += 1,
            _ if was_loading => self.entries[self.index] = url.to_string(),
            _ => {
                self.entries.truncate(self.index + 1);
                self.entries.push(url.to_string());
                self.index += 1;
                if self.entries.len() > MAX_ENTRIES {
                    self.entries.remove(0);
                    self.index -= 1;
                }
            }
        }
    }

    /// The tab's engine was replaced and now knows only the current page.
    pub(crate) fn engine_reset(&mut self) {
        self.hosted = self.entries.len() > 1;
        self.pending = None;
        self.expecting = None;
        self.loading = false;
    }

    /// Whether Back is available, given what the engine reports.
    pub(crate) fn can_go_back(&self, engine: bool) -> bool {
        if self.hosted {
            self.index > 0
        } else {
            engine
        }
    }

    pub(crate) fn can_go_forward(&self, engine: bool) -> bool {
        if self.hosted {
            self.index + 1 < self.entries.len()
        } else {
            engine
        }
    }

    /// A Back or Forward click: what to send, with the list updated.
    pub(crate) fn go(&mut self, dir: Dir, engine_can: bool) -> NavAction {
        if !self.hosted {
            if !engine_can {
                return NavAction::None;
            }
            self.pending = Some(dir);
            return NavAction::Native;
        }
        let target = match dir {
            Dir::Back if self.index > 0 => self.index - 1,
            Dir::Forward if self.index + 1 < self.entries.len() => self.index + 1,
            _ => return NavAction::None,
        };
        self.expecting = Some((self.entries[target].clone(), self.entries[self.index].clone()));
        self.index = target;
        NavAction::Load(self.entries[target].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visit(h: &mut NavHistory, urls: &[&str]) {
        for url in urls {
            h.observe(url, true);
            h.observe(url, false);
        }
    }

    #[test]
    fn follows_navigation_and_native_back_forward() {
        let mut h = NavHistory::default();
        visit(&mut h, &["a", "b", "c"]);
        assert_eq!(h.go(Dir::Back, true), NavAction::Native);
        visit(&mut h, &["b"]);
        assert_eq!((h.index, h.entries.len()), (1, 3));
        assert_eq!(h.go(Dir::Forward, true), NavAction::Native);
        visit(&mut h, &["c"]);
        assert_eq!(h.index, 2);
        // A new page after going back drops the forward entries.
        h.go(Dir::Back, true);
        visit(&mut h, &["b", "d"]);
        assert_eq!(h.entries, ["a", "b", "d"]);
    }

    #[test]
    fn a_redirect_replaces_the_entry() {
        let mut h = NavHistory::default();
        visit(&mut h, &["a"]);
        h.observe("b", true);
        h.observe("b2", false);
        assert_eq!(h.entries, ["a", "b2"]);
    }

    #[test]
    fn a_replaced_engine_keeps_back_and_forward() {
        let mut h = NavHistory::default();
        visit(&mut h, &["x", "y", "z"]);
        h.engine_reset();
        // The new engine reports only the page it loaded, and no history.
        visit(&mut h, &["z"]);
        assert!(h.can_go_back(false));
        assert!(!h.can_go_forward(false));
        assert_eq!(h.go(Dir::Back, false), NavAction::Load("y".into()));
        // The old page is still reported until the load commits.
        h.observe("z", true);
        visit(&mut h, &["y"]);
        assert_eq!(h.index, 1);
        assert!(h.can_go_forward(false));
        assert_eq!(h.go(Dir::Back, false), NavAction::Load("x".into()));
        visit(&mut h, &["x"]);
        assert!(!h.can_go_back(false));
        assert_eq!(h.go(Dir::Back, false), NavAction::None);
        assert_eq!(h.go(Dir::Forward, false), NavAction::Load("y".into()));
    }

    #[test]
    fn a_hosted_load_that_redirects_replaces_the_entry() {
        let mut h = NavHistory::default();
        visit(&mut h, &["x", "y"]);
        h.engine_reset();
        visit(&mut h, &["y"]);
        h.go(Dir::Back, false);
        visit(&mut h, &["x2"]);
        assert_eq!(h.entries, ["x2", "y"]);
        assert_eq!(h.index, 0);
    }

    #[test]
    fn a_single_page_tab_stays_native_after_a_reset() {
        let mut h = NavHistory::default();
        visit(&mut h, &["a"]);
        h.engine_reset();
        assert!(h.can_go_back(true));
        assert_eq!(h.go(Dir::Back, false), NavAction::None);
    }

    #[test]
    fn the_list_is_capped() {
        let mut h = NavHistory::default();
        for n in 0..MAX_ENTRIES + 5 {
            visit(&mut h, &[&format!("p{n}")]);
        }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(h.index, MAX_ENTRIES - 1);
        assert_eq!(h.entries[MAX_ENTRIES - 1], format!("p{}", MAX_ENTRIES + 4));
    }
}
