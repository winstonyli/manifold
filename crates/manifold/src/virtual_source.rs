//! Virtual cards (spec `2026-10-16-virtual-cards-design.md`): cards that no stored node backs. A `VirtualSource`
//! supplies a level's extra `CardEntry`s, which draw after the stored ones, so on top. The trait grows with the plan:
//! each handler is added by the task that first calls it (the pre-commit clippy rejects dead code).

use crate::level::{CardEntry, LevelEdge};
use crate::App;

/// Something that puts virtual cards on the current level. Registered in `App::virtual_sources`.
pub(crate) trait VirtualSource {
    /// The source's name: the `Origin::Virtual` of every entry it yields.
    #[allow(dead_code, reason = "read by later virtual-card tasks, which key edges and handlers on it")]
    fn name(&self) -> &'static str;
    /// The cards this source shows on the level being viewed, in draw order.
    fn entries(&self, app: &App) -> Vec<CardEntry>;
    /// The edges this source adds to the level; `rect_of` gives any entry's rect on the level (`None` off it).
    fn edges(&self, app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge>;
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
            Self { calls: RefCell::new(Vec::new()), ty }
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
    }

    /// Headless `build_graph_ui` frames over one sleeping tab and a `FakeSource`, fitted in view.
    pub(crate) struct Rig {
        ctx: egui::Context,
        pub(crate) app: App,
    }

    impl Rig {
        pub(crate) fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::apply_fonts(&ctx);
            let mut app = App::default();
            app.virtual_sources.push(Box::new(FakeSource::default()));
            let id = app.alloc_node_id();
            app.insert_placeholder_node(id, egui::vec2(0.0, 300.0), Kind::Tab, sleeping_tab("https://a.test/", "t"));
            let mut rig = Self { ctx, app };
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
        app.virtual_sources.push(Box::new(fake::FakeSource::default()));
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
