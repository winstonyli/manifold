//! Which live tabs render (`HostToView::SetVisible`): one rule, `tab_visible`, applied once per frame by
//! `sync_tab_visibility`, instead of each focus, split, transition and minimize path sending its own.

use super::*;

impl App {
    /// Whether live tab `id` should be capturing and sending frames: the split's pane; the focused tab while Tab
    /// view shows it or a transition animates it; and any tab whose loaded page has not yet settled into a frame
    /// (`ViewTab::first_frame`), so a tab opened in the background gets its card's thumbnail (M5 lets only shown tabs
    /// send frames, and the card spins until then). Nothing renders behind a minimized window.
    pub(crate) fn tab_visible(&self, id: NodeId, minimized: bool) -> bool {
        let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_tab()) else { return false };
        let shown = self.focused == Some(id)
            && self.viewed_image.is_none()
            && (self.view_mode == ViewMode::Item || self.transition.as_ref().is_some_and(|t| t.node_id == id));
        !minimized && (self.split_tab == Some(id) || shown || !tab.tab.first_frame.framed())
    }

    /// Settles each live tab's first frame as of `now`, then sends it `tab_visible`'s answer; `ViewTab::set_visible`
    /// sends only a change. Runs once per frame after the UI (and on the minimized tick), so a new engine host, which
    /// starts out visible, is corrected on the frame it lands.
    pub(crate) fn sync_tab_visibility(&mut self, minimized: bool, now: std::time::Instant) {
        let ids: Vec<NodeId> = self.nodes.iter().filter(|(_, n)| n.as_tab().is_some()).map(|(&id, _)| id).collect();
        for id in ids {
            if let Some(tab) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                tab.tab.first_frame.settle(now, minimized);
            }
            let visible = self.tab_visible(id, minimized);
            if let Some(tab) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                tab.tab.set_visible(visible);
            }
        }
    }

    /// A frame came in from tab `id`'s engine at `now`; one after its page has loaded starts or extends its settling
    /// (`FirstFrame`).
    pub(crate) fn note_tab_frame(&mut self, id: NodeId, now: std::time::Instant) {
        if let Some(tab) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            if !tab.loading {
                tab.tab.first_frame.note(now);
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
    use std::time::{Duration, Instant};

    fn visibility(sent: Vec<HostToView>) -> Vec<bool> {
        sent.into_iter().filter_map(|m| if let HostToView::SetVisible(v) = m { Some(v) } else { None }).collect()
    }

    fn ms(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    fn framed(app: &App, id: NodeId) -> bool {
        app.nodes.get(&id).and_then(|n| n.as_tab()).unwrap().tab.first_frame.framed()
    }

    /// A tab opened in the background keeps rendering until its loaded page has gone quiet (`FRAME_QUIET` with no new
    /// frame), then stops, once: its card keeps the settled page, not the first paint.
    #[test]
    fn a_background_tab_renders_until_its_loaded_page_goes_quiet() {
        let mut app = App::default();
        let (id, mut peer) = live_tab(&mut app, ViewKind::Webview2, "https://a/");
        let t0 = Instant::now();
        app.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()).unwrap().loading = true;
        app.note_tab_frame(id, t0);
        app.sync_tab_visibility(false, ms(t0, 1000));
        assert!(!framed(&app, id), "a frame from before the load finished doesn't count");
        app.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()).unwrap().loading = false;
        app.note_tab_frame(id, ms(t0, 1000));
        app.sync_tab_visibility(false, ms(t0, 1100));
        app.note_tab_frame(id, ms(t0, 1200));
        app.sync_tab_visibility(false, ms(t0, 1450));
        assert!(!framed(&app, id), "the page is still painting");
        assert_eq!(visibility(peer.sent()), Vec::<bool>::new(), "a new host starts visible; nothing to send");
        app.sync_tab_visibility(false, ms(t0, 1500));
        app.sync_tab_visibility(false, ms(t0, 1600));
        assert!(framed(&app, id));
        assert_eq!(visibility(peer.sent()), vec![false], "hidden once quiet");
    }

    /// A page that never stops changing is cut off `FRAME_CAP` after its first loaded frame.
    #[test]
    fn a_page_that_keeps_changing_stops_at_the_cap() {
        let mut app = App::default();
        let (id, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://a/");
        let t0 = Instant::now();
        for i in 0..30 {
            app.note_tab_frame(id, ms(t0, i * 100));
            app.sync_tab_visibility(false, ms(t0, i * 100 + 50));
        }
        assert!(!framed(&app, id), "2.95 s in");
        app.sync_tab_visibility(false, ms(t0, 3000));
        assert!(framed(&app, id));
    }

    /// Minimizing stops the settling: the host is hidden and sends nothing, so on restore it starts over from the
    /// whole frame the host sends when shown again.
    #[test]
    fn minimizing_restarts_the_settling() {
        let mut app = App::default();
        let (id, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://a/");
        let t0 = Instant::now();
        app.note_tab_frame(id, t0);
        app.sync_tab_visibility(true, ms(t0, 100));
        app.sync_tab_visibility(false, ms(t0, 5000));
        assert!(!framed(&app, id), "the frame from before the minimize doesn't count");
        app.note_tab_frame(id, ms(t0, 5000));
        app.sync_tab_visibility(false, ms(t0, 5400));
        assert!(framed(&app, id));
    }

    /// Each reason a framed tab renders, and minimizing overriding them all.
    #[test]
    fn tab_visible_follows_focus_split_transition_and_minimize() {
        let mut app = App::default();
        let (id, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://a/");
        let t0 = Instant::now();
        app.note_tab_frame(id, t0);
        app.sync_tab_visibility(false, ms(t0, 5000));
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
