//! Selecting cards by clicking in the graph: a plain click on a card selects it alone (a double-click opens it), a
//! Ctrl-click toggles it in or out, and a plain click on empty canvas clears the selection.

use crate::App;

use crate::card_id::CardId;

impl App {
    /// Puts card `id` in the selection, or takes it out if it is in (Space on a focused card, Ctrl-click).
    pub(crate) fn toggle_selected(&mut self, id: CardId) {
        if !self.interaction.selected.remove(&id) {
            self.interaction.selected.insert(id);
        }
    }

    /// Makes card `id` the whole selection (a plain click on a card).
    pub(crate) fn select_only(&mut self, id: CardId) {
        self.interaction.selected.clear();
        self.interaction.selected.insert(id);
    }

    /// Whether a click on the graph's Scene background empties the selection. `clicked` is the Scene's own primary
    /// click (egui reports a pan as a drag, never a click, and a card, control or popup on top takes its own). Not when
    /// the click belongs to something else: a link line (`on_line`), the minimap (painted, not a widget), a glide the
    /// press stopped, a text field's focus, a drag or marquee, pick-target mode, or an open menu or popup
    /// (`line_click_allowed` lists those).
    pub(crate) fn canvas_click_deselects(
        &self,
        clicked: bool,
        on_line: bool,
        pointer: Option<egui::Pos2>,
        minimap: Option<egui::Rect>,
        typing: bool,
    ) -> bool {
        clicked
            && !on_line
            && !typing
            && self.line_click_allowed()
            && self.renaming_group.is_none()
            && self.menus.omni_popup.open.is_none()
            && !self.camera.is_moving()
            && !pointer.zip(minimap).is_some_and(|(p, rect)| rect.contains(p))
    }
}

#[cfg(test)]
mod tests {
    use crate::card_id::CardId;
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    use crate::App;
    use std::collections::HashSet;

    /// Headless `build_graph_ui` frames over two sleeping tabs side by side, fitted in view, both selected.
    struct Rig {
        ctx: egui::Context,
        app: App,
        ids: [crate::store::model::NodeId; 2],
    }

    impl Rig {
        fn new() -> Self {
            let ctx = crate::theme::testutil::ctx(true);
            let mut app = App::default();
            let ids = [(0.0, "https://a.test/"), (600.0, "https://b.test/")].map(|(x, url)| {
                let id = app.alloc_node_id();
                app.insert_placeholder_node(id, egui::vec2(x, 0.0), Kind::Tab, sleeping_tab(url, "t"));
                id
            });
            let mut rig = Self { ctx, app, ids };
            for _ in 0..3 {
                rig.frame(vec![], egui::Modifiers::NONE);
            }
            rig.app.camera.jump_to(rig.app.fit_all_nodes_scene_rect());
            rig.frame(vec![], egui::Modifiers::NONE);
            rig.app.interaction.selected.extend(rig.ids.iter().map(|i| CardId::from(*i)));
            rig
        }

        /// One frame; `modifiers` is the keyboard state it starts with (egui reads it off `ModifiersChanged`).
        fn frame(&mut self, mut events: Vec<egui::Event>, modifiers: egui::Modifiers) {
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let app = &mut self.app;
            let mut output = self.ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
        }

        fn click_with(&mut self, at: egui::Pos2, modifiers: egui::Modifiers) {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            };
            self.frame(vec![egui::Event::PointerMoved(at), press(true)], modifiers);
            self.frame(vec![press(false)], modifiers);
            self.frame(vec![], egui::Modifiers::NONE);
        }

        fn click(&mut self, at: egui::Pos2) {
            self.click_with(at, egui::Modifiers::NONE);
        }

        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            let press = |at, pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![egui::Event::PointerMoved(from), press(from, true)], egui::Modifiers::NONE);
            for step in 1..=8 {
                let at = from + (to - from) * (step as f32 / 8.0);
                self.frame(vec![egui::Event::PointerMoved(at)], egui::Modifiers::NONE);
            }
            self.frame(vec![press(to, false)], egui::Modifiers::NONE);
            self.frame(vec![], egui::Modifiers::NONE);
        }

        fn card(&self, i: usize) -> egui::Rect {
            self.app.graph_world_rect_to_screen(self.app.card_rect(self.ids[i]).expect("a card"))
        }

        /// Empty canvas below card 0, clear of the minimap in the bottom-right corner.
        fn empty(&self) -> egui::Pos2 {
            let below = egui::pos2(self.card(0).center().x, self.card(0).bottom() + 60.0);
            assert!(!self.card(0).contains(below) && !self.card(1).contains(below));
            below
        }

        fn both(&self) -> HashSet<CardId> {
            self.ids.iter().map(|i| CardId::from(*i)).collect()
        }
    }

    #[test]
    fn a_click_on_empty_canvas_clears_the_selection() {
        let mut rig = Rig::new();
        let at = rig.empty();
        rig.click(at);
        assert!(rig.app.interaction.selected.is_empty());
    }

    #[test]
    fn a_pan_drag_keeps_the_selection() {
        let mut rig = Rig::new();
        let from = rig.empty();
        rig.drag(from, from + egui::vec2(60.0, -20.0));
        assert_eq!(rig.app.interaction.selected, rig.both());
    }

    #[test]
    fn a_click_that_deletes_a_link_line_keeps_the_selection() {
        let mut rig = Rig::new();
        let (a, b) = (rig.ids[0], rig.ids[1]);
        let link = rig.app.create_link(a, b);
        rig.frame(vec![], egui::Modifiers::NONE);
        let edges = rig.app.level_edges();
        let edge = edges.iter().find(|e| e.link == Some(link)).expect("drawn");
        let world = crate::link_gestures::bow_point(edge.from, edge.to, 0.5);
        let on = rig.app.graph_world_rect_to_screen(egui::Rect::from_min_max(world, world)).min;
        rig.click(on);
        assert!(rig.app.links_naming(a).is_empty(), "the click deleted the link");
        assert_eq!(rig.app.interaction.selected, rig.both());
    }

    #[test]
    fn a_click_that_ends_pick_mode_keeps_the_selection() {
        let mut rig = Rig::new();
        rig.app.link.connecting = Some((rig.ids[0], crate::caps::LINK));
        let at = rig.empty();
        rig.click(at);
        assert!(rig.app.link.connecting.is_none(), "the click ended pick mode");
        assert_eq!(rig.app.interaction.selected, rig.both());
    }

    #[test]
    fn a_click_on_the_minimap_keeps_the_selection() {
        let mut rig = Rig::new();
        // The minimap shows only while a card is out of view: zoom in on the first.
        let first = rig.app.card_rect(rig.ids[0]).expect("a card");
        rig.app.camera.jump_to(first.expand(100.0));
        rig.frame(vec![], egui::Modifiers::NONE);
        let minimap = rig.app.graph_scene_outer_rect().right_bottom() - egui::vec2(92.0, 67.0);
        rig.click(minimap);
        assert_eq!(rig.app.interaction.selected, rig.both());
    }

    #[test]
    fn ctrl_click_toggles_a_tab_without_opening_it() {
        let mut rig = Rig::new();
        let centre = rig.card(0).center();
        rig.click_with(centre, egui::Modifiers::CTRL);
        assert_eq!(rig.app.interaction.selected, HashSet::from([CardId::from(rig.ids[1])]), "out");
        rig.app.transition = None;
        rig.click_with(centre, egui::Modifiers::CTRL);
        assert_eq!(rig.app.interaction.selected, rig.both(), "and back in");
        assert!(rig.app.transition.is_none(), "neither opened the tab");
    }

    /// A plain click on a card selects that card alone and opens nothing.
    #[test]
    fn a_plain_click_selects_the_card_alone() {
        let mut rig = Rig::new();
        let centre = rig.card(0).center();
        rig.click(centre);
        assert_eq!(rig.app.interaction.selected, HashSet::from([CardId::from(rig.ids[0])]));
        assert!(rig.app.transition.is_none(), "a single click does not open the tab");
    }

    /// A double-click opens the tab (which ends the selection, as Open always has).
    #[test]
    fn a_double_click_opens_the_tab() {
        let mut rig = Rig::new();
        let centre = rig.card(0).center();
        rig.click(centre);
        rig.click(centre);
        assert!(rig.app.transition.is_some(), "opened");
        assert!(rig.app.interaction.selected.is_empty(), "Open ends the selection");
    }
}
