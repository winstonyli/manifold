//! The Graph view: the types its children share and the clicks `build_graph_ui` collects. The scene body (`scene`),
//! cards (`cards`), toolbar, camera, keyboard and hints are the children below.

use super::*;
use crate::store::model::{NodeId, TYPE_DOWNLOAD, TYPE_IMAGE, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB};

mod camera;
mod cards;
mod hints;
mod keys;
mod label_menu;
mod overlays;
mod scene;
mod selection_bar;
mod suggestions;
mod toolbar;

pub(crate) use camera::*;
use hints::*;
pub(crate) use keys::*;
pub(crate) use overlays::*;
pub(crate) use scene::*;
pub(crate) use toolbar::*;

/// Graph view's chrome is just the "+ New Tab" toolbar — no address bar,
/// no per-tab back/forward, since there's no single focused tab there.
/// Equal to `CHROME_HEIGHT_POINTS`, not independently tuned — a shorter
/// value here used to fit the old bundled Ubuntu-Light font's line height
/// but clipped button labels once `apply_fonts` switched to IBM Plex
/// Serif's taller metrics (confirmed live: "+ New Group"/"Reset View"
/// visibly cut off at the old 26.0).
pub(crate) const GRAPH_TOOLBAR_HEIGHT_POINTS: f32 = CHROME_HEIGHT_POINTS;

pub(crate) const GRAPH_PEEK_SIZE: egui::Vec2 = egui::vec2(360.0, 240.0);

/// World-space room `bounds_for_level` leaves around a level's cards, so
/// a fit doesn't leave one flush against the viewport edge.
pub(crate) const GRAPH_FIT_MARGIN: f32 = 60.0;

pub(crate) const GRAPH_ZOOM_MIN: f32 = 0.15;
pub(crate) const GRAPH_ZOOM_MAX: f32 = 3.0;

/// Visible-node count (post-cull, current level, see `build_graph_ui`'s
/// LOD pre-pass) above which Graph view switches to simplified rendering
/// -- skipping thumbnail/favicon textures and always-drawn title/label
/// text across every kind, the two genuinely expensive parts of painting
/// a card (GPU texture sampling and glyph rasterization), while leaving
/// backgrounds, borders, color swatches, and hover-gated buttons alone.
/// Counts visible nodes rather than thresholding on zoom directly, since
/// a dense cluster of many small-but-individually-zoomed-in cards is the
/// actual cost driver this exists to bound, not zoom level itself.
pub(crate) const LOD_SIMPLIFY_COUNT: usize = 60;

/// Below this count, LOD returns to full detail. Deliberately lower than
/// `LOD_SIMPLIFY_COUNT` (a hysteresis gap, not one shared threshold) so a
/// count hovering right at a single boundary doesn't flip every frame.
pub(crate) const LOD_RESTORE_COUNT: usize = 45;

/// How much a card draws, one value per frame set by `Detail::next` from the count of visible cards. Painters ask
/// what to draw (`content`, `link_handles`, `glow_layers`) and never compare the level, so a new level or a new
/// thing that depends on it is one edit here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Detail {
    #[default]
    Full,
    Simple,
}

impl Detail {
    /// The level for a frame that sees `visible` framed cards in the viewport, given the last frame's level: `Simple`
    /// above `LOD_SIMPLIFY_COUNT`, `Full` again only below the lower `LOD_RESTORE_COUNT`, and unchanged in between.
    pub(crate) fn next(self, visible: usize) -> Self {
        match self {
            Self::Simple if visible < LOD_RESTORE_COUNT => Self::Full,
            Self::Full if visible > LOD_SIMPLIFY_COUNT => Self::Simple,
            level => level,
        }
    }

    /// Text, thumbnails, rendered markdown and the other per-kind content of a card body.
    pub(crate) fn content(self) -> bool {
        self == Self::Full
    }

    /// The connect-drag handles on a hovered card.
    pub(crate) fn link_handles(self) -> bool {
        self == Self::Full
    }

    /// How many of the two glow rects behind a card to draw: a simple card shows none until hover starts (the glow
    /// eases in and out with it, so `hovering` stays true while it fades).
    pub(crate) fn glow_layers(self, hovering: bool) -> usize {
        if self == Self::Full || hovering {
            2
        } else {
            0
        }
    }
}

/// A click in Graph view, collected while `build_graph_ui` draws and
/// applied in order once it's done (`App::apply_graph_action`), when
/// nothing is borrowing `self` for drawing any more.
#[rustfmt::skip]
pub(crate) enum GraphAction {
    Node(NodeId, NodeAction),
    /// A new tab under this parent (the toolbar's "+ New Tab", or the
    /// empty-canvas card's New Tab, which can't place it where clicked:
    /// see `spawn_new_tab_with_kind`).
    NewTab(Option<NodeId>),
    /// The empty-canvas card's New Note/New Group/Add File, placed at the
    /// right-clicked world position.
    NewNote(egui::Vec2),
    NewGroup(egui::Vec2),
    NewJson(egui::Vec2),
    NewFile(egui::Vec2),
    EnterGroup(NodeId),
    LeaveGroup,
    /// A card released after a drag, with where it landed
    /// (`App::drop_target`), if anywhere.
    Drop { id: CardId, target: Option<(DropEffect, Option<CardId>)> },
    /// Pick-target mode: a click on this valid target (`App::finish_connect`).
    FinishConnect(NodeId),
    PinSelected,
    /// The selection bar's Label button: opens or closes its menu.
    LabelSelected,
    /// The menu's rows: put the label on the selection (`on`) or take it off, as one undo step.
    ToggleLabel { label: NodeId, on: bool },
    /// The menu's name field: a new label, put on the selection.
    NewLabel(String),
    RenameLabel { label: NodeId, name: String },
    RecolorLabel { label: NodeId, color: Option<egui::Color32> },
    DeleteLabel(NodeId),
    GroupSelected,
    CloseSelected,
    UngroupAll,
    ImportGroup,
}

/// Graph view's camera: the visible world rect and what moves it on its
/// own. At most one animation owns `rect` at a time, so starting one or
/// jumping cancels the others; a leftover one would carry on from where it
/// was and undo the move.
pub(crate) struct GraphCamera {
    /// The graph view's current pan/zoom state (visible world-space rect),
    /// persisted across frames per `egui::Scene::show`'s own contract —
    /// mutated by the user dragging (pan) or scrolling (zoom).
    pub(crate) rect: egui::Rect,
    /// Graph view's own camera zoom, as a percentage for the toolbar's
    /// readout -- `100.0` means `GraphCamera::transform`'s scale
    /// is `1.0` (one world unit per screen point), same "100% = no
    /// scaling" meaning Tab view's own zoom already uses. Updated once
    /// `build_graph_ui` recomputes the real transform later in the same
    /// frame the toolbar (which needs it before that point) is built, so
    /// the toolbar always shows the *previous* frame's value -- one frame
    /// of lag, imperceptible at any real frame rate, and far simpler than
    /// restructuring draw order to compute the transform before the
    /// toolbar panel has even reserved its own screen space.
    pub(crate) zoom_pct: f32,
    /// `Some` while entering/leaving a group is easing the camera toward
    /// the new level's framing instead of having already snapped there --
    /// see `GraphCameraTween`'s own doc comment.
    pub(crate) tween: Option<GraphCameraTween>,
    /// `Some` while a released background-pan drag is still gliding --
    /// see `PanMomentum`'s own doc comment.
    pub(crate) momentum: Option<PanMomentum>,
    /// Recent `(when, camera-center)` samples, recorded only while a
    /// background pan drag is actually being held down (not a node drag,
    /// not a `GraphCameraTween` in progress, not this same momentum
    /// re-applying itself) -- deliberately the *camera's* own tracked
    /// world-space position, not the raw pointer's screen-space velocity,
    /// since that's the value `PanMomentum` actually needs, in the units
    /// it already needs it in, with no zoom-scale conversion or sign
    /// reasoning to get right. Cleared the instant the drag isn't a plain
    /// background pan for any reason, so a stale sample from an unrelated
    /// earlier gesture can never leak into a later release's velocity
    /// estimate.
    pub(crate) drag_history: std::collections::VecDeque<(std::time::Instant, egui::Pos2)>,
    /// Set by `restore_session` and `replace_running_session`, and
    /// cleared by `apply_local` when personal state brings back a camera
    /// for the level. Still set, there's no framing to keep (a first
    /// launch, an imported file without one), so once the restored tabs
    /// have spawned `build_graph_ui` settles and fits the level rather
    /// than showing whatever part of it sits around the world origin.
    pub(crate) frame_restored: bool,
    /// The tab whose trail strip the camera last framed (`App::frame_new_strip`), `None` while no strip shows.
    pub(crate) strip_tab: Option<NodeId>,
    /// A card to bring into view once it has a rect (`App::reveal_card`): a reopened node appears a frame after the
    /// write that reopens it, wherever the layout put it.
    pub(crate) reveal: Option<NodeId>,
}

impl App {
    /// Applies one of `build_graph_ui`'s collected clicks. Called in click
    /// order; no two of these depend on each other's order (a group rename
    /// carries its own name, see `NodeAction::CommitRename`).
    pub(crate) fn apply_graph_action(&mut self, ctx: &egui::Context, action: GraphAction) {
        // Any other action ends pick-target mode (a node's own, in `apply_node_action`).
        if !matches!(action, GraphAction::Node(..) | GraphAction::FinishConnect(_)) {
            self.cancel_connect();
        }
        match action {
            GraphAction::Node(id, action) => self.apply_node_action(ctx, id, action),
            GraphAction::FinishConnect(target) => self.finish_connect(target),
            GraphAction::NewTab(parent) => {
                let home_page = self.settings.home_page.clone();
                self.leave_json_level();
                let group = self.current_group;
                self.spawn_new_tab(parent, group, &home_page);
            }
            GraphAction::NewNote(pos) => {
                let id = self.alloc_node_id();
                self.insert_note_at(id, Some(pos));
            }
            GraphAction::NewGroup(pos) => {
                let id = self.alloc_node_id();
                self.insert_group_at(id, Some(pos));
            }
            GraphAction::NewJson(pos) => {
                let id = self.alloc_node_id();
                self.insert_inline_json_at(id, Some(pos));
            }
            GraphAction::NewFile(pos) => self.add_file_node(Some(pos)),
            GraphAction::PinSelected => self.pin_shortcut(),
            GraphAction::LabelSelected => self.toggle_label_menu(),
            GraphAction::ToggleLabel { label, on } => self.label_selection(label, on),
            GraphAction::NewLabel(name) => self.new_label_on_selection(&name),
            GraphAction::RenameLabel { label, name } => {
                self.rename_label(label, &name);
            }
            GraphAction::RecolorLabel { label, color } => self.recolor_label(label, color),
            GraphAction::DeleteLabel(label) => self.delete_label(label),
            GraphAction::GroupSelected => {
                // Only the selected cards still shown: a tab mid-crash-respawn or mid-engine-swap has no card, so it
                // is neither counted in the centroid nor moved (its selection is consumed with the rest).
                let live: Vec<NodeId> =
                    self.take_selected_stored().into_iter().filter(|id| self.nodes.contains_key(id)).collect();
                if live.is_empty() {
                    return;
                }
                let id = self.alloc_node_id();
                // Centroid of the selected cards, rather than the usual
                // world-center-plus-jitter spawn point: these tabs already have
                // real positions the user just drew a box around, so the new
                // group card should appear where they actually are, not at the
                // origin of a possibly-distant part of the graph.
                let pos = live.iter().fold(egui::Vec2::ZERO, |acc, t| acc + self.layout_pos(*t)) / live.len() as f32;
                let name = self.default_group_name();
                let at = self.placement(id, Some(pos));
                self.place_group(Some(id), &name, at);
                for tab_id in live {
                    self.edit_group(tab_id, Some(id));
                }
            }
            GraphAction::CloseSelected => {
                let ids = self.take_selected_stored();
                self.begin_tabs_shutdown(&ids);
                // Outermost first (then by id, so the order is the same on every run, not the set's): a closed group
                // hides its whole subtree, so a selected member inside it is already gone at its turn and is
                // skipped, rather than given a closed root of its own beside the group's.
                let mut ids: Vec<NodeId> = ids.into_iter().collect();
                ids.sort_by_cached_key(|id| (self.group_depth(*id), *id));
                for id in ids {
                    if self.is_linked(id) {
                        self.close_node(id);
                    }
                }
            }
            GraphAction::UngroupAll => {
                let groups = self.level_scan().groups;
                self.ungroup(&groups);
            }
            GraphAction::ImportGroup => {
                if let Some(path) = rfd::FileDialog::new().add_filter("Group", &["json"]).pick_file() {
                    match crate::json_file::load::<crate::store::json::JsonDoc>(&path, "group") {
                        Ok(json) => self.import_group_json(&json),
                        Err(err) => self.queue_toast(err),
                    }
                }
            }
            // Entering/leaving a group eases the camera to the same target
            // "Reset View" would jump to instantly -- keeping whatever pan/zoom
            // the old level happened to be at would likely frame empty space
            // rather than the new level's own nodes. `current_group` is updated
            // first so `fit_all_nodes_scene_rect` bounds the level actually
            // being switched *to*, not the one just left. Settling *before*
            // fitting matters: the new level's members may never have been
            // settled before (every group created via drag-and-drop starts with
            // its members wherever the drop happened, often nearly overlapping)
            // -- fitting first would target their old, pre-settle positions,
            // which then jump to their real spacing mid-animation once the
            // regular per-frame settle-check caught up one frame later.
            // Updating `graph_settled_key` here too avoids that same settle
            // running a second time on the very next frame. See
            // `GraphCameraTween`'s doc comment for why the actual camera move
            // eases instead of snapping, and why that's a separate mechanism
            // from the Tab<->Graph `Transition`.
            GraphAction::EnterGroup(id) => self.switch_group_level(Some(id)),
            // One level up, not straight to the top -- same "go to this
            // group's own parent" rule `take_out`/the drag-to-eject
            // handler both use, now that groups nest. Also ends any rename
            // of the group being left.
            GraphAction::LeaveGroup => {
                let parent = self.group_of(self.current_group);
                self.switch_group_level(parent);
                self.renaming_group = None;
            }
            GraphAction::Drop { id, target } => self.apply_drop(id, target),
        }
        // Group edits, drops and moves change nothing else that saves; the
        // node menu's actions save the same way.
        self.save_session();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L1: a selected id with no card (its tab mid-crash-respawn or mid-engine-swap) is left out of the centroid,
    /// not counted in it.
    #[test]
    fn group_selected_centres_on_the_tabs_still_shown() {
        use crate::session_sync::tests::sleeping_tab;
        use crate::store::model::Kind;
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_placeholder_node(a, egui::vec2(100.0, 0.0), Kind::Tab, sleeping_tab("https://a.test/", "a"));
        app.insert_placeholder_node(b, egui::vec2(300.0, 0.0), Kind::Tab, sleeping_tab("https://b.test/", "b"));
        let expected = (app.layout_pos(a) + app.layout_pos(b)) / 2.0;
        app.interaction.selected.extend([CardId::from(a), CardId::from(b), crate::card_id::testutil::card(999)]);
        app.apply_graph_action(&egui::Context::default(), GraphAction::GroupSelected);
        let group = app.doc.mirror.group(a).expect("a was grouped");
        assert_eq!(app.doc.mirror.group(b), Some(group));
        assert_eq!(app.doc.mirror.pos(group), Some(expected), "the mean of the two cards, not divided by three");
        assert!(app.interaction.selected.is_empty());
    }

    /// Links: in pick-target mode a click on a target card draws the edge (and not the card's own click action), and
    /// a click on empty canvas ends the mode. Driven through `build_graph_ui` with real egui input.
    #[test]
    fn a_click_finishes_or_cancels_pick_target_mode() {
        use crate::caps::{LINK, OPENED_FROM};
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        for (id, x) in [(a, 0.0), (b, 600.0)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id, egui::vec2(x, 0.0), crate::store::model::Kind::Note, note);
        }
        // The size `graph_world_rect_to_screen` assumes with no window.
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
        };
        let click = |app: &mut App, at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(app, vec![egui::Event::PointerMoved(at), press(true)]);
            frame(app, vec![press(false)]);
            frame(app, vec![]);
        };
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        app.camera.jump_to(app.fit_all_nodes_scene_rect());
        frame(&mut app, vec![]);
        let screen_rect = |app: &App, id: NodeId| {
            let item = app.level_items().into_iter().find(|i| i.id == CardId::from(id)).expect("on the level");
            app.graph_world_rect_to_screen(item.rect)
        };
        app.link.connecting = Some((a, LINK));
        let on_b = screen_rect(&app, b).center();
        assert!(app.graph_scene_outer_rect().contains(on_b), "{on_b:?} in {:?}", app.graph_scene_outer_rect());
        click(&mut app, on_b);
        assert!(app.link.connecting.is_none());
        assert_eq!(app.links_naming(a).len(), 1, "the click linked b");
        assert!(!app.nodes[&b].as_note().unwrap().editing, "the click was pick mode's, not the note's");
        app.link.connecting = Some((a, OPENED_FROM));
        // Below both cards, clear of the minimap in the bottom-right corner.
        let below = egui::pos2(screen_rect(&app, a).center().x, screen_rect(&app, a).bottom() + 60.0);
        assert!(![a, b].iter().any(|&id| screen_rect(&app, id).contains(below)), "empty canvas");
        click(&mut app, below);
        assert!(app.link.connecting.is_none(), "a click on empty canvas ends the mode");
        assert_eq!(app.doc.mirror.parent(a), None, "and draws nothing");
    }

    /// A right-click on a card opens its menu; on the swatch it opens on the Color page. Driven through
    /// `build_graph_ui` with real egui input.
    #[test]
    fn a_right_click_on_the_swatch_opens_the_menu_on_its_color_page() {
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App::default();
        let note_id = app.alloc_node_id();
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(note_id, egui::vec2(0.0, 0.0), crate::store::model::Kind::Note, note);
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
        };
        let right_click = |app: &mut App, at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(app, vec![egui::Event::PointerMoved(at), press(true)]);
            frame(app, vec![press(false)]);
            frame(app, vec![]);
        };
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        app.camera.jump_to(app.fit_all_nodes_scene_rect());
        frame(&mut app, vec![]);
        let card = {
            let item = app.level_items().into_iter().find(|i| i.id == CardId::from(note_id)).expect("on the level");
            app.graph_world_rect_to_screen(item.rect)
        };
        right_click(&mut app, card.center());
        assert_eq!(app.menus.node_menu.open.as_ref().map(|(id, _)| *id), Some(note_id), "the card's menu opens");
        assert!(app.menus.node_menu.page.is_none(), "on its items");
    }

    /// Wave 2 (H2 / P2): whole `build_graph_ui` frames over the 2000-node fixture spread on a grid: idle at the top
    /// level, hovering a card, in pick-target mode, with the node menu open, filtered, and idle in the first group.
    /// Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins graph_frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn graph_frame_cost() {
        use crate::bench::{level_fixture, report, spread, timed};
        let ctx = crate::theme::testutil::ctx(true);
        let mut f = level_fixture(false);
        spread(&mut f);
        let mut app = f.app;
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
        };
        let run = |app: &mut App, what: &str, events: Vec<egui::Event>| {
            for _ in 0..5 {
                frame(app, events.clone());
            }
            report(what, timed(50, || frame(app, events.clone())));
        };
        let note = f
            .nodes
            .iter()
            .find(|id| app.doc.mirror.group(**id).is_none() && app.nodes[*id].as_note().is_some())
            .copied()
            .expect("a top-level note");
        app.camera.jump_to(app.card_rect(note).unwrap().expand(300.0));
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        run(&mut app, "idle, top level", vec![]);
        let on_note = app.graph_world_rect_to_screen(app.card_rect(note).unwrap()).center();
        run(&mut app, "hovering a card", vec![egui::Event::PointerMoved(on_note)]);
        app.link.connecting = Some((note, crate::caps::LINK));
        run(&mut app, "pick-target mode (LINK)", vec![]);
        assert!(app.link.connecting.is_some(), "the mode stayed on");
        app.link.connecting = None;
        app.menus.node_menu.open(note, egui::pos2(500.0, 400.0));
        run(&mut app, "node menu open", vec![]);
        assert!(app.menus.node_menu.open.is_some(), "the menu stayed open");
        app.menus.node_menu.close();
        app.set_graph_filter("abc");
        run(&mut app, "filtered", vec![]);
        app.set_graph_filter("");
        app.switch_group_level(Some(f.groups[0]));
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        run(&mut app, "idle, first group", vec![]);
    }

    /// Wave 2 (H4): keyboard navigation shows each card's Tab slot (`card_tab_order`'s index plus one) at its
    /// top-left corner. Pins the slot lookup before it moves to a per-pass map.
    #[test]
    fn each_card_shows_its_tab_slot_badge() {
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App::default();
        for k in 0..3 {
            let id = app.alloc_node_id();
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id, egui::vec2(400.0 * k as f32, 0.0), crate::store::model::Kind::Note, note);
        }
        let frame = |app: &mut App| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
            output
        };
        for _ in 0..3 {
            frame(&mut app);
        }
        app.camera.jump_to(app.fit_all_nodes_scene_rect());
        frame(&mut app);
        app.card_nav_active = true; // read when the pass starts; the frame may clear it at its end
        let output = frame(&mut app);
        let badges: Vec<(String, egui::Pos2)> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().parse::<usize>().is_ok() => {
                    Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()).center()))
                }
                _ => None,
            })
            .collect();
        let order = app.card_tab_order();
        assert_eq!(order.len(), 3);
        for (k, id) in order.iter().enumerate() {
            // The rect the card is drawn at, as `a_click_finishes_or_cancels_pick_target_mode` reads it.
            let item = app.level_items().into_iter().find(|i| i.id == *id).expect("on the level");
            let corner = app.graph_world_rect_to_screen(item.rect).left_top();
            let number = (k + 1).to_string();
            assert!(
                badges.iter().any(|(text, at)| *text == number && at.distance(corner) < 15.0),
                "{id}: badge {number} near {corner:?}; badges {badges:?}"
            );
        }
    }

    /// With accesskit on, a card's `widget_info` puts a Button named by `Node::accessible_name`
    /// in the tree egui hands the platform.
    #[test]
    fn card_widget_info_reaches_the_accesskit_tree() {
        let mut app = App::default();
        let g = app.alloc_node_id();
        app.insert_group_at(g, None);
        app.doc.mirror.set(g, crate::store::model::field::NAME, crate::store::model::FieldValue::Str("Reading".into()));
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 200.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 50.0));
            let response = ui.interact(rect, crate::card_id::CardId::from(g).widget(), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, app.accessible_name(g).unwrap_or_default())
            });
        });
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update.expect("accesskit update");
        let named = update.nodes.iter().find(|(_, n)| n.label() == Some("Group: Reading"));
        assert_eq!(named.map(|(_, n)| n.role()), Some(egui::accesskit::Role::Button));
    }
}
