//! Which live tabs render (`HostToView::SetVisible`): one rule, `tab_visible`, applied once per frame by
//! `sync_tab_visibility`, instead of each focus, split, transition and minimize path sending its own.

use super::*;

impl App {
    /// Whether live tab `id` should be capturing and sending frames: the split's pane; the focused tab while Tab
    /// view shows it or a transition animates it; and any tab whose engine has not yet sent a frame of its loaded
    /// page, so a tab opened in the background gets its card's thumbnail (M5 lets only shown tabs send frames, and
    /// the card spins until that frame is in, `ViewTab::framed`). Nothing renders behind a minimized window.
    pub(crate) fn tab_visible(&self, id: NodeId, minimized: bool) -> bool {
        let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_tab()) else { return false };
        let shown = self.focused == Some(id)
            && self.viewed_image.is_none()
            && (self.view_mode == ViewMode::Item || self.transition.as_ref().is_some_and(|t| t.node_id == id));
        !minimized && (self.split_tab == Some(id) || shown || !tab.tab.framed())
    }

    /// Sends each live tab `tab_visible`'s answer; `ViewTab::set_visible` sends only a change. Runs once per frame
    /// after the UI (and on the minimized tick), so a new engine host, which starts out visible, is corrected on the
    /// frame it lands.
    pub(crate) fn sync_tab_visibility(&mut self, minimized: bool) {
        let ids: Vec<NodeId> = self.nodes.iter().filter(|(_, n)| n.as_tab().is_some()).map(|(&id, _)| id).collect();
        for id in ids {
            let visible = self.tab_visible(id, minimized);
            if let Some(tab) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                tab.tab.set_visible(visible);
            }
        }
    }

    /// A frame came in from tab `id`'s engine: once one arrives after its page has loaded, the tab has its
    /// thumbnail (`tab_visible`).
    pub(crate) fn note_tab_frame(&mut self, id: NodeId) {
        if let Some(tab) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            if !tab.loading {
                tab.tab.mark_framed();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_manager::ViewKind;
    use crate::session_sync::tests::live_tab;
    use manifold_view_protocol::HostToView;

    fn visibility(sent: Vec<HostToView>) -> Vec<bool> {
        sent.into_iter().filter_map(|m| if let HostToView::SetVisible(v) = m { Some(v) } else { None }).collect()
    }

    /// A tab opened in the background keeps rendering until a frame of its loaded page is in, then stops, once.
    #[test]
    fn a_background_tab_renders_until_a_frame_of_its_loaded_page() {
        let mut app = App::default();
        let (id, mut peer) = live_tab(&mut app, ViewKind::Webview2, "https://a/");
        app.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()).unwrap().loading = true;
        app.sync_tab_visibility(false);
        assert!(app.tab_visible(id, false), "no frame yet");
        app.note_tab_frame(id);
        app.sync_tab_visibility(false);
        assert!(app.tab_visible(id, false), "a frame from before the load finished doesn't count");
        assert_eq!(visibility(peer.sent()), Vec::<bool>::new(), "a new host starts visible; nothing to send");
        app.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()).unwrap().loading = false;
        app.note_tab_frame(id);
        app.sync_tab_visibility(false);
        app.sync_tab_visibility(false);
        assert_eq!(visibility(peer.sent()), vec![false], "hidden once its loaded frame is in");
    }

    /// Each reason a framed tab renders, and minimizing overriding them all.
    #[test]
    fn tab_visible_follows_focus_split_transition_and_minimize() {
        let mut app = App::default();
        let (id, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://a/");
        app.note_tab_frame(id);
        app.view_mode = ViewMode::Graph;
        assert!(!app.tab_visible(id, false), "a framed background tab");
        app.focused = Some(id);
        assert!(!app.tab_visible(id, false), "focused, but the Graph shows");
        app.transition =
            Some(Transition { node_id: id, kind: TransitionKind::ToGraph, start: std::time::Instant::now() });
        assert!(app.tab_visible(id, false), "animating to or from its card");
        app.transition = None;
        app.view_mode = ViewMode::Item;
        assert!(app.tab_visible(id, false), "Tab view shows it");
        app.viewed_image = Some(crate::card_id::testutil::node(99));
        assert!(!app.tab_visible(id, false), "an image is shown in front of it");
        app.viewed_image = None;
        app.focused = None;
        app.split_tab = Some(id);
        assert!(app.tab_visible(id, false), "the split's pane");
        assert!(!app.tab_visible(id, true), "minimized");
    }

    /// Minimizing stops a tab still waiting for its first loaded frame too.
    #[test]
    fn minimizing_stops_an_unframed_tab() {
        let mut app = App::default();
        let (id, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://a/");
        assert!(app.tab_visible(id, false));
        assert!(!app.tab_visible(id, true));
    }
}
