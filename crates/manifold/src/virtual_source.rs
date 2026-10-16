//! Virtual cards (spec `2026-10-16-virtual-cards-design.md`): cards that no stored node backs. A `VirtualSource`
//! supplies a level's extra `CardEntry`s, which draw after the stored ones, so on top. The trait grows with the plan:
//! each handler is added by the task that first calls it (the pre-commit clippy rejects dead code).

use crate::card::CardBody;
use crate::level::{CardEntry, LevelEdge};
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
}

impl App {
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

    /// Three cards `virtual:fake:a|b|c`, side by side, that records every handler call in `calls`.
    pub(crate) struct FakeSource {
        pub(crate) calls: RefCell<Vec<String>>,
        /// Where `on_menu` was last asked to open.
        pub(crate) menu_at: std::cell::Cell<Option<egui::Pos2>>,
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
            Self { calls: RefCell::new(Vec::new()), menu_at: std::cell::Cell::new(None), ty }
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
    }

    /// Headless `build_graph_ui` frames over one sleeping tab and a `FakeSource`, fitted in view.
    pub(crate) struct Rig {
        ctx: egui::Context,
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

        /// A secondary click at `at`.
        pub(crate) fn right_click(&mut self, at: egui::Pos2) {
            let secondary = egui::PointerButton::Secondary;
            self.frame(vec![egui::Event::PointerMoved(at), Self::press(at, secondary, true)], egui::Modifiers::NONE);
            self.frame(vec![Self::press(at, secondary, false)], egui::Modifiers::NONE);
            self.frame(vec![], egui::Modifiers::NONE);
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
