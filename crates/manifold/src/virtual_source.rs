//! Virtual cards (spec `2026-10-16-virtual-cards-design.md`): cards that no stored node backs. A `VirtualSource`
//! supplies a level's extra `CardEntry`s, which draw after the stored ones, so on top. The trait grows with the plan:
//! each handler is added by the task that first calls it (the pre-commit clippy rejects dead code).

use crate::card::{CardBody, DropEffect};
use crate::level::{CardEntry, LevelEdge, Origin};
use crate::{App, GraphAction};
use std::rc::Rc;

/// Something that puts virtual cards on the current level. Registered in `App::virtual_sources`.
pub(crate) trait VirtualSource {
    /// The source's name: the `Origin::Virtual` of every entry it yields.
    fn name(&self) -> &'static str;
    /// The cards this source shows on the level being viewed, in draw order.
    fn entries(&self, app: &App) -> Vec<CardEntry>;
    /// The edges this source adds to the level; `rect_of` gives any entry's rect on the level (`None` off it).
    fn edges(&self, app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge>;
    /// Paints `entry`'s content inside the shared card frame; the body says which part of the card takes the pointer.
    fn paint_body(&self, app: &App, ui: &mut egui::Ui, entry: &CardEntry) -> CardBody;
    /// A click on `entry` (the shared card loop calls it for every virtual card, whatever its abilities).
    fn on_click(&self, app: &mut App, entry: &CardEntry) -> Option<GraphAction>;
    /// A right-click on `entry` at screen point `at`, only when its kind grants `menu`.
    fn on_menu(&self, app: &mut App, entry: &CardEntry, at: egui::Pos2);
    /// `entry`, a `drag_source` kind, was released: `target` is what `App::drop_target` found under the pointer (the
    /// effect and the card dropped on, or `None` for the back button), and `None` for a release over empty canvas.
    /// The source decides what either means; a returned action is applied like a click's.
    fn on_drop(
        &self,
        app: &mut App,
        entry: &CardEntry,
        target: Option<(DropEffect, Option<String>)>,
    ) -> Option<GraphAction>;
    /// The text the Graph toolbar filter matches `entry` against; `None` (the default) means the filter never dims it.
    fn searchable_text(&self, _entry: &CardEntry) -> Option<String> {
        None
    }
}

/// Whether `id` names a virtual card (`virtual:<source>:<key>`, spec decision 3), which never collides with a node id.
pub(crate) fn is_virtual_id(id: &str) -> bool {
    id.starts_with("virtual:")
}

impl App {
    /// Whether the toolbar filter dims virtual card `entry`: its source gives searchable text and the filter misses it.
    pub(crate) fn virtual_card_dimmed(&self, entry: &CardEntry) -> bool {
        let Origin::Virtual(name) = entry.origin else { return false };
        let text = self.virtual_source(name).and_then(|source| source.searchable_text(entry));
        text.is_some_and(|text| crate::card::graph_filter_dims(&self.graph_filter, &[&text]))
    }

    /// The registered source named `name` (the `Origin::Virtual` of the entries it yields). Shared, so a handler can
    /// take `&mut App` while the caller still holds the source.
    pub(crate) fn virtual_source(&self, name: &str) -> Option<Rc<dyn VirtualSource>> {
        self.virtual_sources.iter().find(|s| s.name() == name).cloned() // name rule: a source's identifier, never shown
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use crate::caps::TagSet;
    use crate::level::{EdgeCurve, EdgeKind, Origin};
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    use crate::CardKind;
    use std::cell::RefCell;

    /// The kind `FakeSource` entries carry unless `with_kind` says otherwise: the prefix row, no abilities.
    pub(crate) const FAKE: &str = "application/vnd.manifold.virtual.fake";

    /// One `on_drop`: the entry's id and the target it received.
    pub(crate) type Dropped = (String, Option<(DropEffect, Option<String>)>);

    /// Three cards `virtual:fake:a|b|c`, side by side, that records every handler call in `calls`.
    pub(crate) struct FakeSource {
        pub(crate) calls: RefCell<Vec<String>>,
        /// Where `on_menu` was last asked to open.
        pub(crate) menu_at: std::cell::Cell<Option<egui::Pos2>>,
        /// Every `on_drop`: the entry's id and the target it received.
        pub(crate) drops: RefCell<Vec<Dropped>>,
        /// What `searchable_text` answers for every entry.
        pub(crate) searchable: std::cell::Cell<Option<&'static str>>,
        ty: &'static str,
    }

    impl Default for FakeSource {
        fn default() -> Self {
            Self::with_kind(FAKE)
        }
    }

    impl FakeSource {
        /// Entries of kind `ty` (a test-only exact row in `caps::rows::builtin_rows` grants its abilities).
        pub(crate) fn with_kind(ty: &'static str) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                menu_at: std::cell::Cell::new(None),
                drops: RefCell::default(),
                searchable: std::cell::Cell::new(None),
                ty,
            }
        }
    }

    impl VirtualSource for FakeSource {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn entries(&self, _app: &App) -> Vec<CardEntry> {
            self.calls.borrow_mut().push("entries".to_string());
            [("a", 0.0), ("b", 200.0), ("c", 400.0)]
                .map(|(letter, x)| CardEntry {
                    id: format!("virtual:fake:{letter}"),
                    kind: CardKind { ty: self.ty, tags: TagSet::EMPTY },
                    origin: Origin::Virtual("fake"),
                    rect: egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(100.0, 60.0)),
                })
                .to_vec()
        }

        /// One edge, `a` to `b`, between the rects' midpoints.
        fn edges(&self, _app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
            let (Some(a), Some(b)) = (rect_of("virtual:fake:a"), rect_of("virtual:fake:b")) else {
                return Vec::new();
            };
            vec![LevelEdge {
                from: a.center(),
                to: b.center(),
                from_id: "virtual:fake:a".to_string(),
                to_id: "virtual:fake:b".to_string(),
                curve: EdgeCurve::Bow,
                highlight: false,
                kind: EdgeKind::Opened,
                link: None,
            }]
        }

        fn paint_body(&self, _app: &App, _ui: &mut egui::Ui, entry: &CardEntry) -> CardBody {
            self.calls.borrow_mut().push("paint_body".to_string());
            CardBody { interact_rect: entry.rect, tooltip: None }
        }

        fn on_click(&self, _app: &mut App, _entry: &CardEntry) -> Option<GraphAction> {
            self.calls.borrow_mut().push("on_click".to_string());
            None
        }

        fn on_menu(&self, _app: &mut App, _entry: &CardEntry, at: egui::Pos2) {
            self.calls.borrow_mut().push("on_menu".to_string());
            self.menu_at.set(Some(at));
        }

        fn on_drop(
            &self,
            _app: &mut App,
            entry: &CardEntry,
            target: Option<(DropEffect, Option<String>)>,
        ) -> Option<GraphAction> {
            self.calls.borrow_mut().push("on_drop".to_string());
            self.drops.borrow_mut().push((entry.id.clone(), target));
            None
        }

        fn searchable_text(&self, _entry: &CardEntry) -> Option<String> {
            self.searchable.get().map(str::to_owned)
        }
    }

    /// Headless `build_graph_ui` frames over one sleeping tab and a `FakeSource`, fitted in view.
    pub(crate) struct Rig {
        pub(crate) ctx: egui::Context,
        pub(crate) app: App,
        /// The registered source, shared so a test can read its call log.
        pub(crate) source: Rc<FakeSource>,
    }

    impl Rig {
        pub(crate) fn new() -> Self {
            Self::with_kind(FAKE)
        }

        /// A rig whose fake entries are of kind `ty`.
        pub(crate) fn with_kind(ty: &'static str) -> Self {
            let ctx = egui::Context::default();
            crate::theme::apply_fonts(&ctx);
            let mut app = App::default();
            let source = Rc::new(FakeSource::with_kind(ty));
            app.virtual_sources.push(source.clone());
            let id = app.alloc_node_id();
            app.insert_placeholder_node(id, egui::vec2(0.0, 300.0), Kind::Tab, sleeping_tab("https://a.test/", "t"));
            let mut rig = Self { ctx, app, source };
            for _ in 0..3 {
                rig.frame(vec![], egui::Modifiers::NONE);
            }
            rig.app.camera.jump_to(rig.app.fit_all_nodes_scene_rect());
            rig.frame(vec![], egui::Modifiers::NONE);
            rig
        }

        /// One frame; `modifiers` is the keyboard state it starts with (egui reads it off `ModifiersChanged`).
        pub(crate) fn frame(&mut self, mut events: Vec<egui::Event>, modifiers: egui::Modifiers) {
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

        fn press(at: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
            egui::Event::PointerButton { pos: at, button, pressed, modifiers: egui::Modifiers::NONE }
        }

        /// A primary click at `at`: move and press, release, settle.
        pub(crate) fn click(&mut self, at: egui::Pos2) {
            let primary = egui::PointerButton::Primary;
            self.frame(vec![egui::Event::PointerMoved(at), Self::press(at, primary, true)], egui::Modifiers::NONE);
            self.frame(vec![Self::press(at, primary, false)], egui::Modifiers::NONE);
            self.frame(vec![], egui::Modifiers::NONE);
        }

        /// Press at `from` and drag to `to`, button still held.
        pub(crate) fn hold_drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            let primary = egui::PointerButton::Primary;
            self.frame(vec![egui::Event::PointerMoved(from), Self::press(from, primary, true)], egui::Modifiers::NONE);
            for step in 1..=8 {
                let at = from + (to - from) * (step as f32 / 8.0);
                self.frame(vec![egui::Event::PointerMoved(at)], egui::Modifiers::NONE);
            }
        }

        /// Release the button at `at`, then settle.
        pub(crate) fn release(&mut self, at: egui::Pos2) {
            self.frame(vec![Self::press(at, egui::PointerButton::Primary, false)], egui::Modifiers::NONE);
            self.frame(vec![], egui::Modifiers::NONE);
        }

        /// A primary-button drag from `from` to `to`.
        pub(crate) fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            self.hold_drag(from, to);
            self.release(to);
        }

        /// A secondary click at `at`.
        pub(crate) fn right_click(&mut self, at: egui::Pos2) {
            let secondary = egui::PointerButton::Secondary;
            self.frame(vec![egui::Event::PointerMoved(at), Self::press(at, secondary, true)], egui::Modifiers::NONE);
            self.frame(vec![Self::press(at, secondary, false)], egui::Modifiers::NONE);
            self.frame(vec![], egui::Modifiers::NONE);
        }

        /// A right-drag (a marquee) over `rect` on screen: press at its top-left, move to its bottom-right, release.
        pub(crate) fn marquee(&mut self, rect: egui::Rect) {
            let secondary = egui::PointerButton::Secondary;
            let (from, to) = (rect.min, rect.max);
            self.frame(
                vec![egui::Event::PointerMoved(from), Self::press(from, secondary, true)],
                egui::Modifiers::NONE,
            );
            self.frame(vec![egui::Event::PointerMoved(to)], egui::Modifiers::NONE);
            self.frame(vec![Self::press(to, secondary, false)], egui::Modifiers::NONE);
            self.frame(vec![], egui::Modifiers::NONE);
        }

        /// The screen rect around all three fake cards, with a margin.
        pub(crate) fn around_virtuals(&self) -> egui::Rect {
            self.screen("virtual:fake:a").union(self.screen("virtual:fake:c")).expand(10.0)
        }

        /// The stored tab's id.
        pub(crate) fn tab(&self) -> String {
            self.app.nodes.keys().next().cloned().expect("the rig's tab")
        }

        /// Entry `id`'s card on screen.
        pub(crate) fn screen(&self, id: &str) -> egui::Rect {
            let entry = self.app.level_items().into_iter().find(|i| i.id == id).expect("an entry with that id");
            self.app.graph_world_rect_to_screen(entry.rect)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeSource, Rig, FAKE};
    use super::*;
    use crate::level::Origin;

    #[test]
    fn level_items_include_a_registered_virtual_source_after_the_stored_nodes() {
        let mut app = App::default();
        app.virtual_sources.push(Rc::new(fake::FakeSource::default()));
        let items = app.level_items();
        assert!(items.iter().any(|i| i.id == "virtual:fake:a" && matches!(i.origin, Origin::Virtual("fake"))));
        assert_eq!(items.last().unwrap().id, "virtual:fake:c", "virtual entries draw last, so on top");
    }

    #[test]
    fn a_virtual_entry_comes_after_a_stored_node_and_takes_the_sources_name_and_kind() {
        let rig = Rig::new();
        let items = rig.app.level_items();
        assert!(matches!(items[0].origin, Origin::Stored), "the tab is first: {items:?}");
        assert_eq!(items.len(), 4);
        assert!(items[1..].iter().all(|i| i.kind.ty == FAKE && rig.app.virtual_sources[0].name() == "fake"));
        let (a, b) = (rig.screen("virtual:fake:a"), rig.screen("virtual:fake:b"));
        assert!(a.width() > 0.0 && a.right() < b.left(), "{a:?} {b:?}");
    }

    const ALL: &str = "application/vnd.manifold.virtual.fake-all";
    const MENU: &str = "application/vnd.manifold.virtual.fake-menu";

    fn calls(rig: &Rig, name: &str) -> usize {
        rig.source.calls.borrow().iter().filter(|c| c.as_str() == name).count()
    }

    #[test]
    fn the_card_loop_draws_virtual_entries_and_hovers_them() {
        let mut rig = Rig::with_kind(ALL);
        let on_b = rig.screen("virtual:fake:b").center();
        rig.frame(vec![egui::Event::PointerMoved(on_b)], egui::Modifiers::NONE);
        rig.frame(vec![], egui::Modifiers::NONE);
        assert_eq!(rig.app.interaction.hovered.as_deref(), Some("virtual:fake:b"));
        assert!(calls(&rig, "paint_body") >= 3, "each of the three cards paints through the source");
    }

    #[test]
    fn a_click_on_a_virtual_card_reaches_the_source_and_touches_no_node() {
        let mut rig = Rig::with_kind(ALL);
        let (nodes, selected) = (rig.app.nodes.len(), rig.app.interaction.selected.clone());
        rig.click(rig.screen("virtual:fake:b").center());
        assert_eq!(calls(&rig, "on_click"), 1, "{:?}", rig.source.calls.borrow());
        assert_eq!((rig.app.nodes.len(), &rig.app.interaction.selected), (nodes, &selected));
        assert!(rig.app.menus.node_menu.is_none());
    }

    #[test]
    fn a_right_click_on_a_virtual_card_opens_no_node_menu_and_calls_on_menu_only_with_the_ability() {
        let mut rig = Rig::with_kind(FAKE);
        rig.right_click(rig.screen("virtual:fake:b").center());
        assert_eq!(calls(&rig, "on_menu"), 0);
        assert!(rig.app.menus.node_menu.is_none() && rig.app.menus.node_menu_page.is_none());

        let mut rig = Rig::with_kind(MENU);
        let at = rig.screen("virtual:fake:b").center();
        rig.right_click(at);
        assert_eq!(calls(&rig, "on_menu"), 1, "{:?}", rig.source.calls.borrow());
        assert!(rig.app.menus.node_menu.is_none() && rig.app.menus.node_menu_page.is_none());
        assert_eq!(rig.source.menu_at.get(), Some(at));
    }

    #[test]
    fn a_virtual_card_shows_no_stored_controls_and_the_tab_order_lists_it_last() {
        use crate::card::card_controls;
        let rig = Rig::new();
        let entry = rig.app.level_items().pop().unwrap();
        let view = rig.app.card_view(&entry, &Default::default()).unwrap();
        assert!(card_controls(&view).is_empty(), "{:?}", card_controls(&view));
        assert!(!view.closable && !view.in_group);
        let stored = rig.app.level_items().remove(0);
        assert!(!card_controls(&rig.app.card_view(&stored, &Default::default()).unwrap()).is_empty());
        let order = rig.app.card_tab_order();
        assert_eq!(order.last().map(String::as_str), Some("virtual:fake:c"));
        assert_eq!(order.len(), 4);
    }

    #[test]
    fn a_virtual_card_is_not_in_eguis_own_tab_traversal() {
        let rig = Rig::new();
        let sense = |id: &str| rig.ctx.read_response(egui::Id::new(("graph_card", id))).expect("drawn").sense;
        assert!(!sense("virtual:fake:b").is_focusable(), "only the first card or the focused one is focusable");
        let first = rig.app.card_tab_order()[0].clone();
        assert!(sense(&first).is_focusable());
    }

    #[test]
    fn tab_onto_a_virtual_card_pans_to_its_rect() {
        let mut rig = Rig::new();
        let order = rig.app.card_tab_order();
        let from = rig.app.card_rect(&order[0]).unwrap();
        rig.app.camera.jump_to(egui::Rect::from_center_size(from.center(), egui::vec2(120.0, 120.0)));
        rig.frame(vec![], egui::Modifiers::NONE);
        rig.ctx.memory_mut(|m| m.request_focus(egui::Id::new(("graph_card", order[0].as_str()))));
        rig.frame(vec![], egui::Modifiers::NONE);
        rig.app.camera.tween = None;
        let tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        rig.frame(vec![tab], egui::Modifiers::NONE);
        assert!(rig.app.camera.tween.is_some(), "Tab to a card out of view pans to it");
        let want = rig.app.level_items().iter().find(|e| e.id == order[1]).unwrap().rect;
        assert_eq!(rig.app.level_card_rect(&order[1]), Some(want));
    }

    const SELECTABLE: &str = "application/vnd.manifold.virtual.fake-selectable";

    fn virtual_ids() -> std::collections::HashSet<String> {
        ["a", "b", "c"].map(|l| format!("virtual:fake:{l}")).into_iter().collect()
    }

    #[test]
    fn a_marquee_selects_virtual_cards_only_when_the_kind_is_selectable() {
        let mut rig = Rig::with_kind(SELECTABLE);
        rig.marquee(rig.around_virtuals());
        assert_eq!(rig.app.interaction.selected, virtual_ids());
        rig.app.end_stale_selection();
        assert_eq!(rig.app.interaction.selected, virtual_ids(), "a selectable virtual id is not stale");

        for ty in [FAKE, MENU] {
            let mut rig = Rig::with_kind(ty);
            rig.marquee(rig.around_virtuals());
            assert!(rig.app.interaction.selected.is_empty(), "{ty}: {:?}", rig.app.interaction.selected);
            rig.app.interaction.selected.insert("virtual:fake:a".to_string());
            rig.app.end_stale_selection();
            assert!(rig.app.interaction.selected.is_empty(), "{ty}: a kind that cannot be selected is dropped");
        }
    }

    #[test]
    fn space_on_a_focused_virtual_card_toggles_it_only_if_selectable() {
        let space = || egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for (ty, expect) in [(SELECTABLE, true), (FAKE, false)] {
            let mut rig = Rig::with_kind(ty);
            let first = rig.app.card_tab_order()[0].clone();
            rig.ctx.memory_mut(|m| m.request_focus(egui::Id::new(("graph_card", first.as_str()))));
            rig.frame(vec![], egui::Modifiers::NONE);
            let tab = egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            rig.frame(vec![tab], egui::Modifiers::NONE);
            rig.frame(vec![], egui::Modifiers::NONE);
            let focused = rig.app.card_tab_order()[1].clone();
            assert!(rig.ctx.memory(|m| m.has_focus(egui::Id::new(("graph_card", focused.as_str())))), "{focused}");
            rig.frame(vec![space()], egui::Modifiers::NONE);
            assert_eq!(rig.app.interaction.selected.contains(&focused), expect, "{ty}");
        }
    }

    #[test]
    fn selection_actions_act_on_stored_cards_and_leave_a_virtual_id_alone() {
        let mut rig = Rig::with_kind(SELECTABLE);
        let (tab, virt) = (rig.tab(), "virtual:fake:a".to_string());
        let nodes = rig.app.nodes.len();
        rig.app.interaction.selected.extend([tab.clone(), virt.clone()]);
        rig.app.pin_shortcut();
        assert!(rig.app.mirror.pinned(&tab), "the stored tab is pinned");
        assert!(!rig.app.mirror.has(&virt), "no store write for the virtual id");
        assert_eq!(rig.app.interaction.selected.len(), 2);

        let ctx = rig.ctx.clone();
        rig.app.apply_graph_action(&ctx, GraphAction::CloseSelected);
        assert!(!rig.app.nodes.contains_key(&tab), "the stored tab is closed");
        assert_eq!(rig.app.nodes.len(), nodes - 1);
        assert_eq!(rig.app.interaction.selected, std::collections::HashSet::from([virt.clone()]));
        assert!(!rig.app.mirror.has(&virt));

        let (mut rig, virt) = (Rig::with_kind(SELECTABLE), "virtual:fake:a".to_string());
        let tab = rig.tab();
        rig.app.interaction.selected.extend([tab.clone(), virt.clone()]);
        let ctx = rig.ctx.clone();
        rig.app.apply_graph_action(&ctx, GraphAction::GroupSelected);
        assert!(rig.app.mirror.group(&tab).is_some(), "the stored tab is grouped");
        assert_eq!(rig.app.interaction.selected, std::collections::HashSet::from([virt.clone()]));
        assert!(!rig.app.mirror.has(&virt));
    }

    #[test]
    fn with_kind_picks_the_row_and_the_source_logs_its_calls() {
        let app = App::default();
        let source = FakeSource::with_kind("application/vnd.manifold.virtual.fake-all");
        let entries = source.entries(&app);
        assert_eq!(*source.calls.borrow(), ["entries"]);
        let caps = entries[0].caps();
        assert!(caps.selectable && caps.drag_source && caps.movable && caps.drop_target && caps.menu && caps.linkable);
        let none = FakeSource::default().entries(&app)[0].caps();
        assert!(!none.selectable && !none.menu && none.draw_layer == 8);
    }
}

#[cfg(test)]
mod reach_tests {
    use super::fake::{Rig, FAKE};
    use super::*;
    use crate::level::Origin;
    use crate::store::model::{field, FieldValue};

    const SELECTABLE: &str = "application/vnd.manifold.virtual.fake-selectable";

    /// Everything a stored write could touch for `id`, plus the queued writes and the node count.
    fn footprint(app: &App, id: &str) -> (bool, bool, usize, String, String) {
        (
            app.mirror.has(id),
            app.layout.contains_key(id),
            app.nodes.len(),
            format!("{:?}", app.pending),
            format!("{:?}", app.mirror.get(id, field::POS)),
        )
    }

    #[test]
    fn every_write_entry_point_leaves_a_virtual_id_out_of_the_store_and_the_layout() {
        let mut rig = Rig::with_kind(SELECTABLE);
        let virt = "virtual:fake:a";
        let before = footprint(&rig.app, virt);
        rig.app.edit_pos(virt, Some(egui::vec2(5.0, 7.0)));
        rig.app.edit_field(virt, field::PINNED, FieldValue::Bool(true));
        rig.app.interaction.dragging = Some(virt.to_string());
        rig.app.interaction.drag_moved = vec![virt.to_string()];
        rig.app.release_drag();
        rig.app.apply_drop(virt, Some((DropEffect::Join, Some(virt.to_string()))));
        rig.app.apply_drop(virt, Some((DropEffect::Eject, None)));
        rig.app.interaction.selected = std::collections::HashSet::from([virt.to_string()]);
        let ctx = rig.ctx.clone();
        rig.app.apply_graph_action(&ctx, GraphAction::CloseSelected);
        rig.app.pin_shortcut();
        assert_eq!(footprint(&rig.app, virt), before);
    }

    #[test]
    fn a_virtual_card_far_from_the_stored_ones_enlarges_the_bounds_and_the_fit() {
        struct Far;
        impl VirtualSource for Far {
            fn name(&self) -> &'static str {
                "far"
            }
            fn entries(&self, _: &App) -> Vec<CardEntry> {
                vec![CardEntry {
                    id: "virtual:far:x".to_string(),
                    kind: crate::CardKind { ty: FAKE, tags: crate::caps::TagSet::EMPTY },
                    origin: Origin::Virtual("far"),
                    rect: egui::Rect::from_min_size(egui::pos2(9000.0, 9000.0), egui::vec2(100.0, 60.0)),
                }]
            }
            fn edges(&self, _: &App, _: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
                Vec::new()
            }
            fn paint_body(&self, _: &App, _: &mut egui::Ui, entry: &CardEntry) -> CardBody {
                CardBody { interact_rect: entry.rect, tooltip: None }
            }
            fn on_click(&self, _: &mut App, _: &CardEntry) -> Option<GraphAction> {
                None
            }
            fn on_menu(&self, _: &mut App, _: &CardEntry, _: egui::Pos2) {}
            fn on_drop(
                &self,
                _: &mut App,
                _: &CardEntry,
                _: Option<(DropEffect, Option<String>)>,
            ) -> Option<GraphAction> {
                None
            }
        }
        let mut rig = Rig::new();
        let tab = rig.tab();
        let stored_only = rig.app.bounds_for_level([&tab]);
        let fit = rig.app.fit_all_nodes_scene_rect();
        let with_far = |app: &App| app.bounds_for_level([&tab, &"virtual:far:x".to_string()]);
        assert!(with_far(&rig.app).max.x < 1000.0, "unknown id is ignored before the source exists");
        rig.app.virtual_sources.push(Rc::new(Far));
        assert!(with_far(&rig.app).max.x > 9000.0, "{:?} vs {stored_only:?}", with_far(&rig.app));
        assert!(rig.app.fit_all_nodes_scene_rect().max.x > 9000.0 && fit.max.x < 1000.0);
    }

    #[test]
    fn export_state_lists_a_virtual_card_with_its_source_and_abilities() {
        let rig = Rig::with_kind(SELECTABLE);
        let state = rig.app.export_state();
        let card = state["cards"].as_array().unwrap().iter().find(|c| c["id"] == "virtual:fake:a").expect("listed");
        assert_eq!(card["virtual"], "fake");
        assert_eq!(card["type"], SELECTABLE);
        assert_eq!(card["json_card"], false);
        assert_eq!(card["abilities"]["selectable"], true);
        assert_eq!(card["abilities"]["menu"], false);
        assert!(card["rect"].is_array() && card["screen"].is_array());
        let stored = state["cards"].as_array().unwrap().iter().find(|c| c["id"] == rig.tab()).unwrap();
        assert!(stored.get("virtual").is_none());
    }

    #[test]
    fn a_filter_dims_a_virtual_card_only_when_its_source_gives_searchable_text() {
        let mut rig = Rig::new();
        let entry = rig.app.level_items().pop().unwrap();
        rig.app.graph_filter = "zzz".to_string();
        assert!(!rig.app.virtual_card_dimmed(&entry), "no searchable text: never dimmed");
        rig.app.virtual_sources.clear();
        let source = Rc::new(fake::FakeSource::default());
        source.searchable.set(Some("alpha"));
        rig.app.virtual_sources.push(source);
        assert!(rig.app.virtual_card_dimmed(&entry), "text that does not match dims");
        rig.app.graph_filter = "ALP".to_string();
        assert!(!rig.app.virtual_card_dimmed(&entry), "text that matches does not");
    }
}

#[cfg(test)]
mod drag_tests {
    use super::fake::{Dropped, Rig, FAKE};
    use crate::card::DropEffect;

    const DRAG: &str = "application/vnd.manifold.virtual.fake-drag";
    const ALL: &str = "application/vnd.manifold.virtual.fake-all";

    /// A rig with a stored group `g` right of the cards, the camera framing everything.
    fn rig_with_group(ty: &'static str) -> (Rig, String) {
        let mut rig = Rig::with_kind(ty);
        let placement = crate::fetched::Placement { group: None, pos: egui::vec2(700.0, 300.0) };
        rig.app.place_group(Some("g".into()), "G", placement);
        rig.app.camera.jump_to(egui::Rect::from_min_max(egui::pos2(-100.0, -100.0), egui::pos2(1100.0, 600.0)));
        for _ in 0..3 {
            rig.frame(vec![], egui::Modifiers::NONE);
        }
        (rig, "g".to_string())
    }

    fn world_to_screen(rig: &Rig, world: egui::Pos2) -> egui::Pos2 {
        rig.app.graph_world_rect_to_screen(egui::Rect::from_min_size(world, egui::Vec2::ZERO)).min
    }

    fn rects(rig: &Rig) -> Vec<(String, egui::Rect)> {
        rig.app.level_items().into_iter().map(|e| (e.id, e.rect)).collect()
    }

    fn drops(rig: &Rig) -> Vec<Dropped> {
        rig.source.drops.borrow().clone()
    }

    #[test]
    fn dragging_a_virtual_card_moves_nothing_and_the_drag_stays_live_while_held() {
        let (mut rig, _) = rig_with_group(DRAG);
        let before = rects(&rig);
        let from = rig.screen("virtual:fake:a").center();
        rig.hold_drag(from, from + egui::vec2(40.0, 70.0));
        assert_eq!(rig.app.interaction.dragging.as_deref(), Some("virtual:fake:a"));
        assert!(rig.app.interaction.drag_moved.is_empty());
        assert_eq!(rects(&rig), before, "no rect moved");
        assert!(!rig.app.layout.contains_key("virtual:fake:a"), "no layout entry");
        rig.frame(vec![], egui::Modifiers::NONE);
        assert_eq!(rig.app.interaction.dragging.as_deref(), Some("virtual:fake:a"), "end_stale_drag keeps it");
        rig.release(from);
        assert!(rig.app.interaction.dragging.is_none());
        assert_eq!(rects(&rig), before);
        assert!(!rig.app.layout.contains_key("virtual:fake:a") && !rig.app.mirror.has("virtual:fake:a"));
    }

    #[test]
    fn a_kind_without_drag_source_is_not_dragged() {
        let (mut rig, _) = rig_with_group(FAKE);
        let from = rig.screen("virtual:fake:a").center();
        rig.hold_drag(from, from + egui::vec2(40.0, 70.0));
        assert!(rig.app.interaction.dragging.is_none());
    }

    #[test]
    fn a_release_over_empty_canvas_calls_on_drop_once_with_no_target() {
        let (mut rig, _) = rig_with_group(DRAG);
        let empty = world_to_screen(&rig, egui::pos2(500.0, 150.0));
        assert!(rig
            .app
            .level_items()
            .iter()
            .all(|e| !e.rect.contains(rig.app.graph_screen_point_to_world(empty).to_pos2())));
        let from = rig.screen("virtual:fake:a").center();
        rig.drag(from, empty);
        assert_eq!(drops(&rig), [("virtual:fake:a".to_string(), None)]);
        assert_eq!(rig.source.calls.borrow().iter().filter(|c| *c == "on_drop").count(), 1);
        assert!(rig.app.interaction.dragging.is_none());
    }

    #[test]
    fn a_release_over_a_stored_group_calls_on_drop_with_the_effect_and_the_group() {
        let (mut rig, group) = rig_with_group(DRAG);
        let over = rig.app.graph_world_rect_to_screen(rig.app.card_rect(&group).unwrap()).center();
        let from = rig.screen("virtual:fake:a").center();
        rig.drag(from, over);
        assert_eq!(drops(&rig), [("virtual:fake:a".to_string(), Some((DropEffect::Join, Some(group.clone()))))]);
        assert_eq!(rig.app.members_of(&group).count(), 0, "nothing joined the group");
    }

    #[test]
    fn a_virtual_card_is_a_drop_target_only_with_the_ability() {
        for (ty, expect) in [(ALL, true), (DRAG, false)] {
            let mut rig = Rig::with_kind(ty);
            let (a, b) = (rig.screen("virtual:fake:a").center(), rig.screen("virtual:fake:b").center());
            rig.drag(a, b);
            let want = expect.then(|| (DropEffect::Join, Some("virtual:fake:b".to_string())));
            assert_eq!(drops(&rig), [("virtual:fake:a".to_string(), want)], "{ty}");
        }
    }

    #[test]
    fn a_virtual_drop_never_takes_the_stored_selection_with_it() {
        let (mut rig, group) = rig_with_group(ALL);
        let tab = rig.tab();
        rig.app.interaction.selected.extend([tab.clone(), "virtual:fake:a".to_string()]);
        let over = rig.app.graph_world_rect_to_screen(rig.app.card_rect(&group).unwrap()).center();
        let from = rig.screen("virtual:fake:a").center();
        rig.drag(from, over);
        assert_eq!(drops(&rig).len(), 1, "{:?}", drops(&rig));
        assert!(rig.app.mirror.group(&tab).is_none(), "the selected stored tab did not join the group");
        assert!(rig.app.interaction.selected.contains(&tab), "the selection is untouched");
    }

    #[test]
    fn release_drag_for_a_virtual_id_writes_nothing() {
        let (mut rig, _) = rig_with_group(DRAG);
        rig.app.interaction.dragging = Some("virtual:fake:a".to_string());
        rig.app.interaction.drag_moved = vec![];
        rig.app.release_drag();
        assert!(rig.app.interaction.dragging.is_none());
        assert!(!rig.app.mirror.has("virtual:fake:a") && !rig.app.layout.contains_key("virtual:fake:a"));
    }

    #[test]
    fn a_stored_tab_drag_is_unchanged_and_never_reaches_a_source() {
        let (mut rig, group) = rig_with_group(DRAG);
        let tab = rig.tab();
        let over = rig.app.graph_world_rect_to_screen(rig.app.card_rect(&group).unwrap()).center();
        let from = rig.app.graph_world_rect_to_screen(rig.app.card_rect(&tab).unwrap()).center();
        rig.drag(from, over);
        assert_eq!(rig.app.mirror.group(&tab), Some(group.as_str()), "the tab joined the group");
        assert!(drops(&rig).is_empty());
    }
}
