use super::*;
use crate::caps::TagSet;
use crate::level::{EdgeCurve, EdgeKind, Origin};
use crate::session_sync::tests::sleeping_tab;
use crate::store::model::{Kind, NodeId};
use crate::CardKind;
use std::cell::RefCell;

/// The kind `FakeSource` entries carry unless `with_kind` says otherwise: the prefix row, no abilities.
pub(crate) const FAKE: &str = "application/vnd.manifold.virtual.fake";

/// One `on_drop`: the entry's id and the target it received.
pub(crate) type Dropped = (CardId, Option<(DropEffect, Option<CardId>)>);

/// The id of the fake card named `letter`.
pub(crate) fn id(letter: char) -> CardId {
    CardId::virt("fake", u64::from(letter))
}

/// Three cards `id('a')`, `id('b')`, `id('c')`, side by side, that records every handler call in `calls`.
pub(crate) struct FakeSource {
    pub(crate) calls: RefCell<Vec<String>>,
    /// Where `on_menu` was last asked to open.
    pub(crate) menu_at: std::cell::Cell<Option<egui::Pos2>>,
    /// Every `on_drop`: the entry's id and the target it received.
    pub(crate) drops: RefCell<Vec<Dropped>>,
    /// The world point each `on_drop` was given, in the same order as `drops`.
    pub(crate) drop_at: RefCell<Vec<egui::Pos2>>,
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
            drop_at: RefCell::default(),
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
        [('a', 0.0), ('b', 200.0), ('c', 400.0)]
            .map(|(letter, x)| CardEntry {
                id: id(letter),
                kind: CardKind { ty: self.ty, tags: TagSet::EMPTY },
                origin: Origin::Virtual("fake"),
                rect: egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(100.0, 60.0)),
            })
            .to_vec()
    }

    /// One edge, `a` to `b`, between the rects' midpoints.
    fn edges(&self, _app: &App, rect_of: &dyn Fn(CardId) -> Option<egui::Rect>) -> Vec<LevelEdge> {
        let (Some(a), Some(b)) = (rect_of(id('a')), rect_of(id('b'))) else {
            return Vec::new();
        };
        vec![LevelEdge {
            from: a.center(),
            to: b.center(),
            from_id: id('a'),
            to_id: id('b'),
            curve: EdgeCurve::Bow,
            highlight: false,
            kind: EdgeKind::Opened,
            link: None,
        }]
    }

    fn paint_body(&self, _app: &App, _ui: &mut egui::Ui, _pass: &mut CardPass, entry: &CardEntry) -> CardBody {
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
        target: Option<(DropEffect, Option<CardId>)>,
        at: egui::Pos2,
    ) -> Option<GraphAction> {
        self.calls.borrow_mut().push("on_drop".to_string());
        self.drops.borrow_mut().push((entry.id, target));
        self.drop_at.borrow_mut().push(at);
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
        let ctx = crate::theme::testutil::ctx(true);
        // The context that runs the frames, so text measuring has fonts.
        let mut app = App { egui_ctx: ctx.clone(), ..App::default() };
        let source = Rc::new(FakeSource::with_kind(ty));
        app.register_virtual_source(source.clone());
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
    pub(crate) fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        crate::theme::testutil::graph_frame(&self.ctx, &mut self.app, events, modifiers);
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
        self.frame(vec![egui::Event::PointerMoved(from), Self::press(from, secondary, true)], egui::Modifiers::NONE);
        self.frame(vec![egui::Event::PointerMoved(to)], egui::Modifiers::NONE);
        self.frame(vec![Self::press(to, secondary, false)], egui::Modifiers::NONE);
        self.frame(vec![], egui::Modifiers::NONE);
    }

    /// The screen rect around all three fake cards, with a margin.
    pub(crate) fn around_virtuals(&self) -> egui::Rect {
        self.screen(id('a')).union(self.screen(id('c'))).expand(10.0)
    }

    /// The stored tab's id.
    pub(crate) fn tab(&self) -> NodeId {
        // `node_order`, not `nodes.keys()`: a HashMap's order is random per map, and a test may add a group beside the tab.
        self.app.node_order.first().copied().expect("the rig's tab")
    }

    /// Entry `id`'s card on screen.
    pub(crate) fn screen(&self, id: CardId) -> egui::Rect {
        let entry = self.app.level_items().into_iter().find(|i| i.id == id).expect("an entry with that id");
        self.app.graph_world_rect_to_screen(entry.rect)
    }
}
