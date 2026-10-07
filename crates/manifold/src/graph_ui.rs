//! The Graph view: its toolbar, camera (pan momentum, zoom, minimap),
//! keyboard and rect selection, the dot grid, edges and cards, the new-node
//! menu and the hover peek.

use super::*;
use crate::store::model::{TYPE_DOWNLOAD, TYPE_JSON, TYPE_JSON_LEAF, TYPE_NOTE, TYPE_TAB};

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

/// A click in Graph view, collected while `build_graph_ui` draws and
/// applied in order once it's done (`App::apply_graph_action`), when
/// nothing is borrowing `self` for drawing any more.
#[rustfmt::skip]
pub(crate) enum GraphAction {
    Node(String, NodeAction),
    /// A new tab under this parent (the toolbar's "+ New Tab", or the
    /// empty-canvas card's New Tab, which can't place it where clicked:
    /// see `spawn_new_tab_with_kind`).
    NewTab(Option<String>),
    /// The empty-canvas card's New Note/New Group/Add File, placed at the
    /// right-clicked world position.
    NewNote(egui::Vec2),
    NewGroup(egui::Vec2),
    NewFile(egui::Vec2),
    EnterGroup(String),
    LeaveGroup,
    /// A card released after a drag, with where it landed
    /// (`App::drop_target`), if anywhere.
    Drop { id: String, target: Option<(DropEffect, Option<String>)> },
    /// Pick-target mode: a click on this valid target (`App::finish_connect`).
    FinishConnect(String),
    GroupSelected,
    CloseSelected,
    UngroupAll,
    ImportGroup,
}

/// One row of the empty-canvas "New Tab/Note/Group/File" menu -- see
/// `App::pending_new_node`'s own doc comment for the menu itself. Paints its
/// own hover-highlighted background in world-space (scales with zoom like
/// the rest of the card, via `ui.painter()`), but its label through
/// `overlay_painter` at a fixed screen font size instead of as a real
/// `egui::Button`'s text -- a real widget's text would scale down with zoom
/// the same way a note's own body text does, which is exactly what made
/// this specific menu illegible/overflowing at low zoom (every other
/// right-click menu in this app is a plain screen-space `Area`, so this was
/// the only one with the problem at all). Same world-space-hit-rect-plus-
/// fixed-screen-text split `paint_card_control` above already uses, for
/// the same reason. The label paint itself is deferred -- see
/// `DeferredOverlays`'s own doc comment -- so `overlay_painter`/`project`
/// are no longer parameters here at all. Returns whether it was clicked.
pub(crate) fn draw_new_node_menu_row(
    ui: &egui::Ui,
    deferred_overlays: &mut DeferredOverlays,
    row_rect: egui::Rect,
    id_salt: &str,
    inv_zoom: f32,
    label: &str,
) -> bool {
    let response = ui
        .interact(row_rect, egui::Id::new(id_salt), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().rect_filled(row_rect, 3.0 * inv_zoom, ACCENT.gamma_multiply(0.35));
    }
    let label = label.to_string();
    let text_pos = row_rect.left_center();
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        overlay_painter.text(
            project(text_pos) + egui::vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &label,
            egui::FontId::proportional(13.0),
            TEXT_PRIMARY,
        );
    }));
    response.clicked()
}

/// What the Graph toolbar (`App::render_graph_toolbar`) hands back to
/// `build_graph_ui`: the actions its buttons pushed, the zoom-chip
/// requests, and the button rects that popups and drops anchor to.
pub(crate) struct GraphToolbar {
    pub(crate) actions: Vec<GraphAction>,
    pub(crate) reset_view: bool,
    pub(crate) reset_zoom: bool,
    pub(crate) zoom_step: Option<f32>,
    pub(crate) shortcuts_help_button_rect: Option<egui::Rect>,
    pub(crate) recently_closed_button_rect: Option<egui::Rect>,
    /// The breadcrumb's back button, also a drop target (see `drop_target`).
    pub(crate) leave_group_button_rect: Option<egui::Rect>,
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
    /// readout -- `100.0` means `graph_world_to_screen_transform`'s scale
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
}

impl GraphCamera {
    pub(crate) fn new() -> Self {
        Self {
            rect: default_graph_scene_rect(),
            zoom_pct: 100.0,
            tween: None,
            momentum: None,
            drag_history: std::collections::VecDeque::new(),
            frame_restored: false,
        }
    }

    /// Moves straight to `rect`.
    pub(crate) fn jump_to(&mut self, rect: egui::Rect) {
        self.rect = rect;
        self.tween = None;
        self.momentum = None;
    }

    /// Eases from the current rect to `to` over `TRANSITION_DURATION`.
    pub(crate) fn ease_to(&mut self, to: egui::Rect) {
        self.tween = Some(GraphCameraTween { from: self.rect, to, start: std::time::Instant::now() });
        self.momentum = None;
    }

    /// Eased camera move into/out of a group -- see `GraphCameraTween`'s
    /// doc comment for why this is a separate mechanism from
    /// `Transition`. Recomputed fresh from the tween's fixed `from`/`to`
    /// every frame (not derived from `rect`, which
    /// `build_graph_ui`'s `Scene::show` may also nudge from user pan/zoom input this
    /// same frame) so an in-progress user pan can't throw off where the
    /// animation is actually headed.
    pub(crate) fn advance(&mut self) {
        if let Some(tween) = self.tween {
            let elapsed = tween.start.elapsed();
            let t = (elapsed.as_secs_f32() / TRANSITION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
            let eased = t * t * (3.0 - 2.0 * t);
            self.rect = egui::Rect::from_min_max(
                tween.from.min + (tween.to.min - tween.from.min) * eased,
                tween.from.max + (tween.to.max - tween.from.max) * eased,
            );
            if elapsed >= TRANSITION_DURATION {
                self.tween = None;
            }
        } else if let Some(momentum) = self.momentum {
            // Pan momentum -- see `PanMomentum`'s own doc comment.
            // Mutually exclusive with the tween above (both would
            // otherwise fight over `camera.rect` the same frame).
            let elapsed = momentum.start.elapsed();
            let t = elapsed.as_secs_f32();
            let decay = (-PAN_MOMENTUM_DECAY * t).exp();
            let displacement = momentum.initial_velocity * ((1.0 - decay) / PAN_MOMENTUM_DECAY);
            let size = self.rect.size();
            self.rect = egui::Rect::from_center_size(momentum.start_center + displacement, size);
            if elapsed >= PAN_MOMENTUM_MAX_DURATION {
                self.momentum = None;
            }
        }
    }

    /// Grows `rect` to everything `Scene` shows of it in `outer`, keeping
    /// its center and scale. `Scene` fits `rect` into `outer` by aspect,
    /// so a rect of another shape (a fit to the cards' bounds, a tween
    /// toward one) is shown with more world beside it than it covers,
    /// and `Scene` only writes the full visible rect back after a user pan
    /// or zoom. Until then the grid, card culling, the LOD count and the
    /// minimap, which all read `rect`, skipped that margin: a card dragged
    /// into it vanished.
    pub(crate) fn fill(&mut self, outer: egui::Rect) {
        if !self.rect.is_finite() || !self.rect.is_positive() || !outer.is_positive() {
            return; // `Scene` resets a bad rect itself.
        }
        let scale = (outer.size() / self.rect.size()).min_elem().clamp(GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        self.rect = egui::Rect::from_center_size(self.rect.center(), outer.size() / scale);
    }
}

impl App {
    /// Replicates `egui::Scene`'s own internal `fit_to_rect_in_scene`
    /// (private to egui, not part of its public API) — needed to compute
    /// where a graph node actually lands on screen *outside* the `Scene`
    /// widget's own closure, for the shrink/expand animation's target
    /// rect. `(scale, translation)` such that
    /// `screen = translation + scale * world`, matching `Scene`'s own
    /// "fit the visible scene rect into the available screen area,
    /// preserving aspect ratio, clamped to the configured zoom range"
    /// behavior exactly (using the *same* `self.camera.rect` `Scene`
    /// itself reads/writes) — verified live by screenshotting a
    /// mid-animation frame and confirming the content quad visually lines
    /// up with the target node's real on-screen position.
    pub(crate) fn graph_world_to_screen_transform(&self, scene_outer_rect: egui::Rect) -> (f32, egui::Vec2) {
        let scale =
            (scene_outer_rect.size() / self.camera.rect.size()).min_elem().clamp(GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        let translation = scene_outer_rect.center().to_vec2() - scale * self.camera.rect.center().to_vec2();
        (scale, translation)
    }

    /// Where a world-space rect lands on screen in Graph view right now --
    /// the same projection `Scene` applies, computed outside it (see
    /// `graph_world_to_screen_transform`).
    pub(crate) fn graph_world_rect_to_screen(&self, world: egui::Rect) -> egui::Rect {
        let (scale, translation) = self.graph_world_to_screen_transform(self.graph_scene_outer_rect());
        egui::Rect::from_min_max(
            (translation + scale * world.min.to_vec2()).to_pos2(),
            (translation + scale * world.max.to_vec2()).to_pos2(),
        )
    }

    /// Inverse of `graph_world_rect_to_screen`: where a screen point
    /// (window points, Graph view) lands in world space right now -- the
    /// same projection a click there is hit-tested against (see
    /// `handle_rect_select`'s own `world_pos`). Used to place a file
    /// dropped on the window (`drop_file`).
    pub(crate) fn graph_screen_point_to_world(&self, point: egui::Pos2) -> egui::Vec2 {
        let (scale, translation) = self.graph_world_to_screen_transform(self.graph_scene_outer_rect());
        (point.to_vec2() - translation) / scale
    }

    /// The Graph view's scene area in window points: below the toolbar,
    /// minus a JSON level's outline on the right (plan ruling 12). The one
    /// definition both screen<->world conversions use; `build_graph_ui`
    /// reads the live layout rect instead, which this mirrors for code
    /// outside the frame.
    pub(crate) fn graph_scene_outer_rect(&self) -> egui::Rect {
        let screen = self.screen_size_points();
        let outline =
            if self.json_level.is_some() && self.json_outline_visible { self.json_outline_width } else { 0.0 };
        egui::Rect::from_min_size(
            egui::pos2(0.0, GRAPH_TOOLBAR_HEIGHT_POINTS),
            egui::vec2((screen.x - outline).max(0.0), (screen.y - GRAPH_TOOLBAR_HEIGHT_POINTS).max(0.0)),
        )
    }

    /// Applies one of `build_graph_ui`'s collected clicks. Called in click
    /// order; no two of these depend on each other's order (a group rename
    /// carries its own name, see `NodeAction::CommitRename`).
    pub(crate) fn apply_graph_action(&mut self, ctx: &egui::Context, action: GraphAction) {
        // Any other action ends pick-target mode (a node's own, in `apply_node_action`).
        if !matches!(action, GraphAction::Node(..) | GraphAction::FinishConnect(_)) {
            self.cancel_connect();
        }
        match action {
            GraphAction::Node(id, action) => self.apply_node_action(ctx, &id, action),
            GraphAction::FinishConnect(target) => self.finish_connect(&target),
            GraphAction::NewTab(parent) => {
                let home_page = self.settings.home_page.clone();
                self.leave_json_level();
                let group = self.current_group.clone();
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
            GraphAction::NewFile(pos) => self.add_file_node(Some(pos)),
            GraphAction::GroupSelected => {
                if self.selected_tabs.is_empty() {
                    return;
                }
                let id = self.alloc_node_id();
                // Centroid of the selected cards, rather than the usual
                // world-center-plus-jitter spawn point: these tabs already have
                // real positions the user just drew a box around, so the new
                // group card should appear where they actually are, not at the
                // origin of a possibly-distant part of the graph.
                let count = self.selected_tabs.len() as f32;
                let sum = self
                    .selected_tabs
                    .iter()
                    .filter(|id| self.nodes.contains_key(*id))
                    .fold(egui::Vec2::ZERO, |acc, id| acc + self.layout_pos(id));
                let pos = sum / count;
                let name = self.default_group_name();
                let at = self.placement(&id, Some(pos));
                self.place_group(Some(id.clone()), &name, at);
                for tab_id in std::mem::take(&mut self.selected_tabs) {
                    if self.nodes.contains_key(&tab_id) {
                        self.edit_group(&tab_id, Some(id.clone()));
                    }
                }
            }
            GraphAction::CloseSelected => {
                for id in std::mem::take(&mut self.selected_tabs) {
                    self.close_node(&id);
                }
            }
            GraphAction::UngroupAll => {
                for id in self.level_scan().groups {
                    self.dissolve_group(&id);
                }
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
            // group's own parent" rule `dissolve_group`/the drag-to-eject
            // handler both use, now that groups nest. Also ends any rename
            // of the group being left.
            GraphAction::LeaveGroup => {
                let parent = self.group_of(&self.current_group);
                self.switch_group_level(parent);
                self.renaming_group = None;
            }
            GraphAction::Drop { id, target } => self.apply_drop(&id, target),
        }
        // Group edits, drops and moves change nothing else that saves; the
        // node menu's actions save the same way.
        self.save_session();
    }

    /// Switches to `id`'s level, as `switch_group_level` does, unless it is
    /// already the one shown. For jumps straight to a tab (quick switcher,
    /// the view toggle): the camera would otherwise keep the old level's
    /// framing and show the new one's nodes off screen.
    pub(crate) fn show_level_of(&mut self, id: &str) {
        let group = self.nodes.get(id).and(self.mirror.group(id)).map(str::to_owned);
        if group != self.current_group {
            self.switch_group_level(group);
        }
    }

    /// Shows `group`'s level (`None` for the top), easing the camera to fit
    /// it, and drops everything tied to the old level: the rectangle
    /// selection (the toolbar's "Group Selected"/"Close Selected" would
    /// otherwise act on tabs no longer visible), the empty-canvas card,
    /// any open node menu, a drag whose card is no longer shown, and pick-target mode (its targets were the old
    /// level's).
    pub(crate) fn switch_group_level(&mut self, group: Option<String>) {
        self.end_note_edits();
        self.current_group = group;
        self.end_stale_drag(true);
        self.cancel_connect();
        self.sync_json_level();
        self.layout_current_level();
        self.graph_settled_key = Some(self.current_level_settle_key());
        self.camera.ease_to(self.fit_all_nodes_scene_rect());
        self.selected_tabs.clear();
        self.rect_select_start = None;
        self.pending_new_node = None;
        self.node_menu = None;
    }

    pub(crate) fn build_graph_ui(&mut self, ui: &mut egui::Ui) {
        // Read before any widget runs: a singleline TextEdit gives up focus
        // while handling Enter, so later this frame it would read false.
        let typing = crate::shortcuts::text_field_has_keys(ui.ctx());
        self.sync_json_level();
        // One level scan per frame, reused by the toolbar, settle key, minimap, items and cards;
        // `current_group` only changes at the end of this function.
        let LevelScan { layers: at_level, groups: groups_at_level } = self.level_scan();
        let items = self.level_items_from(&at_level);

        self.update_graph_lod(&items);

        let GraphToolbar {
            mut actions,
            reset_view,
            reset_zoom,
            zoom_step: graph_zoom_step,
            shortcuts_help_button_rect,
            recently_closed_button_rect,
            leave_group_button_rect,
        } = self.render_graph_toolbar(ui, &groups_at_level);
        // `build_shortcuts_help`/`build_recently_closed_popup`/the three
        // context menus below are all deferred to the very end of this
        // function (past the Scene/`overlay_painter` node-rendering block),
        // not called here where their button rects are computed. That
        // order doesn't decide what paints on top, though: see
        // `overlay_painter`'s own comment for why its layer is
        // `Order::Middle`.
        self.apply_camera_requests(reset_view);

        // The level drawn, not the whole graph: an empty group shows it too.
        if items.is_empty() {
            let rect = ui.max_rect();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                EMPTY_HINT,
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().weak_text_color(),
            );
        }

        // Static layout: the force-directed sim only runs as a one-shot
        // settle burst (`settle_current_level`) when the visible node set
        // at this level actually changes size, not continuously every
        // frame -- see that method's own doc comment for why continuous
        // physics was dropped (a drop target could flee the very drag
        // aimed at it). While a drag is in progress, skip the check
        // entirely: the dragged node's position already updates directly
        // from `response.drag_delta()` in the per-node loop below,
        // independent of this, and everything else should stay exactly
        // where it is for the whole gesture.
        self.settle_level_if_changed(items.len());

        self.handle_graph_keys(ui, typing);

        // Pan (drag empty background) + zoom (scroll wheel, centered on the
        // cursor) via egui's built-in `Scene` container — everything drawn
        // inside `add_contents` below is in the same world-space coordinates
        // `App::layout` positions use; egui's transform layer handles converting
        // to/from screen space (including for nested widgets' own
        // hover/click/drag responses, e.g. a node's own drag below), so none
        // of that logic needs to know pan/zoom exists at all. `scene_rect`
        // must live outside `self` while the closure below also borrows
        // `self` mutably, so it's round-tripped through a local instead of
        // borrowed from `self.camera.rect` directly.
        // Everything painted inside `Scene`'s closure lives in world-space
        // coordinates that `Scene` itself then scales uniformly by the
        // current zoom factor — a *shape* (rect/circle/stroke/image) sized
        // in world units would shrink to illegible at `GRAPH_ZOOM_MIN` and
        // balloon at `GRAPH_ZOOM_MAX` right along with the card, so those
        // are sized as `desired_screen_px * inv_zoom` below (vector
        // geometry re-tessellates at any final scale with no quality
        // loss, so this alone is enough for them). Text is different:
        // egui rasterizes each glyph into its font atlas *at the
        // requested world-space size*, and `Scene`'s transform then just
        // scales that already-rasterized quad — so text sized the same
        // `* inv_zoom` way rasterizes tiny (and blurs badly once
        // magnified back up) at high zoom, since a small `inv_zoom` asks
        // for a tiny atlas glyph. `overlay_painter`/`project` below paint
        // text directly onto an *untransformed* foreground layer instead,
        // at real fixed screen coordinates and a real fixed font size —
        // this is the actual "achieve constant size directly" fix, not
        // just another scale factor. `zoom_scale`/`translation` are the
        // same pair `graph_world_to_screen_transform` already derives for
        // the Tab<->Graph transition animation (`screen = translation +
        // zoom_scale * world`). The node card itself (`GRAPH_NODE_SIZE`)
        // and the hover peek preview (`GRAPH_PEEK_SIZE`) are deliberately
        // excluded from all of this — those represent actual zoomable
        // content, not chrome, so they're meant to scale (and blur like
        // any other magnified image would, which is normal/expected).
        self.build_json_outline(ui);
        let scene_outer_rect = ui.available_rect_before_wrap();
        self.apply_zoom_requests(scene_outer_rect, reset_zoom, graph_zoom_step);
        // Advance first: everything below (strokes, `zoom_pct`, the new-node
        // menu) must use the rect the Scene draws this frame.
        self.camera.advance();
        if self.camera.tween.is_some() || self.camera.momentum.is_some() {
            ui.ctx().request_repaint();
        }
        self.camera.fill(scene_outer_rect);
        let (zoom_scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
        self.camera.zoom_pct = zoom_scale * 100.0;
        let inv_zoom = 1.0 / zoom_scale;
        let project = move |world: egui::Pos2| -> egui::Pos2 { (translation + zoom_scale * world.to_vec2()).to_pos2() };
        // Clipped to the graph panel's own rect (not `layer_painter`'s
        // default full-content-rect) so a node near the top edge can't
        // paint its label over the toolbar above. `Order::Middle`: above
        // the Scene (Background), below every popup, menu and toast (all
        // Foreground Areas). Within one `Order`, egui paints Area layers in
        // their stacking order and a bare layer like this one after all of
        // them (`GraphicsLayers::drain`), so at Foreground it covered the
        // cards' own context menus.
        let overlay_painter = egui::Painter::new(
            ui.ctx().clone(),
            egui::LayerId::new(egui::Order::Middle, egui::Id::new("graph_decoration_text")),
            scene_outer_rect,
        );

        self.handle_rect_select(ui, &overlay_painter, scene_outer_rect, zoom_scale, translation);

        let minimap = self.draw_graph_minimap(ui, &overlay_painter, scene_outer_rect, &items);
        // Compared against after `Scene`'s own closure runs, below --
        // an actual pan drag or scroll-wheel zoom this frame changes this,
        // and dismisses `pending_new_node` the same as every other camera
        // move already does (see that field's own doc comment).
        let scene_rect_before_pan_zoom = self.camera.rect;
        let mut scene_rect = self.camera.rect;
        let mut json_edits = Vec::new();
        let scene_response = egui::Scene::new()
            .zoom_range(GRAPH_ZOOM_MIN..=GRAPH_ZOOM_MAX)
            .drag_pan_buttons(egui::DragPanButtons::PRIMARY)
            .show(ui, &mut scene_rect, |ui| {
                // Every `overlay_painter` decoration painted inside this
                // closure is deferred rather than painted immediately -- see
                // `DeferredOverlays`'s own doc comment for why (this frame's
                // real pan/zoom isn't reflected in `project`, captured
                // above, until after this whole closure returns).
                let mut deferred_overlays: DeferredOverlays = Vec::new();
                self.draw_graph_dot_grid(ui, zoom_scale, inv_zoom);

                // The link line under the pointer (link-gestures spec B) is drawn highlighted; a click on it is
                // applied after the Scene, from the Scene's own response.
                let mut edges = self.level_edges();
                let line_hover = self.hovered_link_line(ui, &edges, inv_zoom, minimap.map(|(rect, _, _)| rect));
                for edge in &mut edges {
                    edge.highlight |= edge.link.is_some() && edge.link == line_hover;
                }
                self.draw_graph_edges(ui, &edges, inv_zoom);

                if self.json_level.is_some() {
                    self.draw_json_level(ui, &mut deferred_overlays, &mut json_edits, inv_zoom, zoom_scale);
                } else {
                    self.draw_graph_cards(
                        ui,
                        &mut deferred_overlays,
                        &mut actions,
                        &at_level,
                        inv_zoom,
                        zoom_scale,
                        leave_group_button_rect,
                    );
                    self.draw_pending_new_node_menu(ui, &mut deferred_overlays, &mut actions, inv_zoom, &project);
                    self.draw_peek_preview(ui, &mut deferred_overlays);
                }
                (deferred_overlays, line_hover)
            });
        let egui::InnerResponse { inner: (deferred_overlays, line_hover), response: scene_background } = scene_response;
        self.camera.rect = scene_rect;
        // Now that `self.camera.rect` reflects this frame's actual
        // pan/zoom (not the stale value `project`/`overlay_painter` calls
        // inside the closure above had to work with -- see
        // `DeferredOverlays`'s own doc comment), recompute the transform
        // fresh and paint every decoration queued during that closure with
        // it, closing the one-frame gap between a card and its own
        // overlay-painted label/icons that was otherwise most visible
        // during a fast pan or zoom.
        let (zoom_scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
        let project = move |world: egui::Pos2| -> egui::Pos2 { (translation + zoom_scale * world.to_vec2()).to_pos2() };
        for overlay in deferred_overlays {
            overlay(&overlay_painter, &project);
        }
        if scene_rect != scene_rect_before_pan_zoom {
            self.pending_new_node = None;
        }

        // A click on a link line deletes that link (link-gestures spec B). The Scene's background took the click, so
        // no card, control or popup was on top, and egui never reports a pan (a drag) as a click. Read before
        // `track_pan_momentum`, which clears a glide on the press that `line_click` must still see.
        let line_click = self.line_click(ui, line_hover, scene_background.clicked());

        self.track_pan_momentum(ui, scene_rect);

        if let Some((minimap_rect, world_bounds, minimap_scale)) = minimap {
            self.handle_minimap_click(ui, minimap_rect, world_bounds, minimap_scale);
        }

        if let Some(link) = line_click {
            self.click_link_line(&link);
        }

        for action in actions {
            self.apply_graph_action(ui.ctx(), action);
        }
        // Pick-target mode: a click in the graph that didn't finish it above (empty canvas, a card that isn't a
        // target, the minimap) ends it. A pan is a drag, not a click, so the user can still go looking for a target.
        // The node menu that starts the mode is built after this, so the click choosing its verb doesn't count.
        if self.connecting.is_some()
            && ui.input(|i| {
                i.pointer.primary_clicked() && i.pointer.interact_pos().is_some_and(|p| scene_outer_rect.contains(p))
            })
        {
            self.cancel_connect();
        }
        for edit in json_edits {
            self.apply_json_edit(edit);
        }

        // Built last, now that `overlay_painter`'s node-decoration layer
        // above already exists this frame -- see this function's own note
        // by `shortcuts_help_button_rect`/`recently_closed_button_rect` for
        // why these can't be built back where those rects are computed.
        self.build_graph_popups(ui, shortcuts_help_button_rect, recently_closed_button_rect);
    }

    /// Flips `graph_lod_simplified` with hysteresis from how many of `items` are in view.
    fn update_graph_lod(&mut self, items: &[LevelItem]) {
        // Level-of-detail: count how many nodes at this level actually
        // intersect the viewport, then flip `graph_lod_simplified` with a
        // hysteresis gap -- see `LOD_SIMPLIFY_COUNT`'s own doc comment for
        // why. Uses this frame's not-yet-updated `camera.rect`, the
        // same one-frame-stale viewport the render loops' own cull check
        // already accepts.
        let lod_visible_count: usize = items.iter().filter(|item| item.rect.intersects(self.camera.rect)).count();
        if self.graph_lod_simplified {
            if lod_visible_count < LOD_RESTORE_COUNT {
                self.graph_lod_simplified = false;
            }
        } else if lod_visible_count > LOD_SIMPLIFY_COUNT {
            self.graph_lod_simplified = true;
        }
    }

    /// Applies a pending frame-restore and the toolbar's Fit request to the camera.
    fn apply_camera_requests(&mut self, reset_view: bool) {
        // See `GraphCamera::frame_restored`.
        if self.camera.frame_restored && self.tab_spawns_in_flight == 0 {
            self.camera.frame_restored = false;
            // Settled first, as in `switch_group_level`: fitting the
            // pre-settle positions would leave spread-out nodes off screen.
            self.layout_current_level();
            self.graph_settled_key = Some(self.current_level_settle_key());
            self.camera.jump_to(self.fit_all_nodes_scene_rect());
        }
        if reset_view {
            self.camera.jump_to(self.fit_all_nodes_scene_rect());
            self.pending_new_node = None;
        }
    }

    /// Re-lays-out the level when its node set changed size, unless a drag is in progress.
    fn settle_level_if_changed(&mut self, item_count: usize) {
        if self.dragging.is_none() {
            // Same formula `current_level_settle_key()` computes, just
            // built from this frame's already-cached `items` instead of
            // redoing the level scan a second time.
            let current_key = (self.current_group.clone(), item_count);
            if self.graph_settled_key.as_ref() != Some(&current_key) {
                self.layout_current_level();
                self.graph_settled_key = Some(current_key);
            }
        }
    }

    /// Applies the zoom chip's 100% and step requests to the camera rect.
    fn apply_zoom_requests(&mut self, scene_outer_rect: egui::Rect, reset_zoom: bool, graph_zoom_step: Option<f32>) {
        if reset_zoom {
            // Keeps the current pan (unlike "Reset View", which re-fits
            // everything) -- `graph_world_to_screen_transform` derives
            // `zoom_scale` as `screen_size / camera.rect.size()`
            // (`.min_elem()` of the two axis ratios), so matching the world
            // rect's size to the screen's own size on both axes makes that
            // ratio exactly 1.0, the definition of "100%" this readout uses.
            let center = self.camera.rect.center();
            self.camera.rect = egui::Rect::from_center_size(center, scene_outer_rect.size());
        }
        if let Some(factor) = graph_zoom_step {
            self.camera.rect =
                step_scene_rect(self.camera.rect, factor, scene_outer_rect.size(), GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        }
    }

    /// Popups and menus built last, after the node-decoration layer exists this frame.
    fn build_graph_popups(
        &mut self,
        ui: &mut egui::Ui,
        shortcuts_help_button_rect: Option<egui::Rect>,
        recently_closed_button_rect: Option<egui::Rect>,
    ) {
        self.build_shortcuts_help(ui, shortcuts_help_button_rect);
        let ctx = ui.ctx().clone();
        if let Some((rect, fade)) =
            self.anchored_popup_fade(&ctx, AnchoredPopup::RecentlyClosed, recently_closed_button_rect)
        {
            self.build_recently_closed_popup(ui, rect, fade);
        }
        self.build_card_hint(ui);
        self.build_ctrl_hint(ui);
        self.build_json_hint(ui);
        self.build_node_menu(ui);
        self.build_json_menu(ui);
    }

    /// While a graph card has keyboard focus, a strip of keycaps under it (above, near the bottom
    /// edge) lists the keys that act on it. Not while its menu is open.
    fn build_card_hint(&self, ui: &egui::Ui) {
        let Some((kind, rect)) = self.card_hint.filter(|_| self.node_menu.is_none() && self.json_level.is_none())
        else {
            return;
        };
        let screen = ui.ctx().content_rect();
        let extra: &[(&str, &str)] = match kind.ty {
            TYPE_TAB => &[("Enter", "open"), ("Space", "select")],
            TYPE_JSON | TYPE_JSON_LEAF => &[("Enter", "enter")],
            ty if is_container(ty) => &[("Enter", "enter")],
            TYPE_NOTE => &[("Enter", "edit")],
            _ => &[],
        };
        let tail = [("Del", del_verb(kind)), ("Shift+F10", "menu")];
        let hints = [("Tab", "next")].iter().chain(extra).chain(&tail);
        let below = rect.bottom() + 40.0 < screen.bottom();
        let (anchor_y, pivot) = if below {
            (rect.bottom() + 8.0, egui::Align2::CENTER_TOP)
        } else {
            (rect.top() - 8.0, egui::Align2::CENTER_BOTTOM)
        };
        let x = rect.center().x.clamp(screen.left() + 190.0, (screen.right() - 190.0).max(screen.left() + 190.0));
        let area = egui::Area::new(egui::Id::new("card_key_hint")).pivot(pivot).fixed_pos(egui::pos2(x, anchor_y));
        key_hint_strip(ui.ctx(), area, hints.copied());
    }

    /// While Ctrl is held on its own (`ctrl_reveal_step`), a strip of the Ctrl chords at the bottom of
    /// the Graph view; the cards show their number badges too.
    fn build_ctrl_hint(&self, ui: &egui::Ui) {
        if !self.hints_revealed || self.json_level.is_some() || self.node_menu.is_some() {
            return;
        }
        let area = egui::Area::new(egui::Id::new("ctrl_key_hint")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0]);
        let hints = [
            ("Ctrl+T", "new tab"),
            ("Ctrl+Shift+T", "reopen"),
            ("Ctrl+K", "switch"),
            ("Ctrl+G", "view"),
            ("Ctrl+L", "address"),
            ("Ctrl+W", "close"),
            ("0 - 9", "jump to card"),
        ];
        key_hint_strip(ui.ctx(), area, hints.into_iter());
    }

    /// In a JSON level with a row or card selected, a strip of keycaps at the bottom of the window
    /// lists the keys the level takes.
    fn build_json_hint(&self, ui: &egui::Ui) {
        if self.json_level.as_ref().is_none_or(|l| l.selected.is_none() || l.menu.is_some()) {
            return;
        }
        let area = egui::Area::new(egui::Id::new("json_key_hint")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0]);
        let hints =
            [("Up / Down", "select"), ("Enter", "expand"), ("Right / Left", "open / close row"), ("Ctrl+C", "copy")];
        key_hint_strip(ui.ctx(), area, hints.into_iter());
    }

    /// The Graph view's toolbar row: the group breadcrumb, "+ New Tab",
    /// the group and zoom chips, the filter, Recently Closed, the
    /// selection's actions, a spawn error and the universal chip. What its
    /// buttons ask for comes back as a `GraphToolbar`, which
    /// `build_graph_ui` applies at the same points it always did.
    pub(crate) fn render_graph_toolbar(&mut self, ui: &mut egui::Ui, groups_at_level: &[String]) -> GraphToolbar {
        // A JSON level is read-only: no creation, grouping or rename here.
        let on_json_level = self.json_level.is_some();
        let mut actions: Vec<GraphAction> = Vec::new();
        let mut reset_view = false;
        let mut reset_zoom = false;
        let mut graph_zoom_step: Option<f32> = None;
        let mut shortcuts_help_button_rect: Option<egui::Rect> = None;
        let mut recently_closed_button_rect: Option<egui::Rect> = None;
        let mut leave_group_button_rect: Option<egui::Rect> = None;
        // `toolbar_panel` already provides the compact spacing and the
        // horizontal-scroll overflow safety this row specifically needed
        // (a breadcrumb of arbitrary depth, a selection-actions cluster, a
        // spawn error, and "Recently Closed" all able to appear at once
        // had already hit real overflow at this fixed row height more than
        // once -- see the icon-only button migration and breadcrumb-name
        // truncation elsewhere in this file, both direct responses to
        // that). Every other toolbar in this app gets the same safety net
        // now too, not just this one.
        toolbar_panel("graph_toolbar", ui, |ui| {
            ui.horizontal_centered(|ui| {
                leave_group_button_rect = self.toolbar_breadcrumb(ui, on_json_level, &mut actions);
                // "+ New Tab" is the only toolbar spawn action kept --
                // it's also the sole recovery path when `self.nodes` is
                // completely empty (see `build_graph_ui`'s early return),
                // where the right-click empty-canvas card can't appear
                // yet. New Note/Add File/New Group used to sit here too,
                // but that card already offers all three identically
                // once any node exists, and while the graph is fully
                // empty those buttons' own flags were silently dropped
                // anyway (their sole consumption site is unreachable
                // from the early-return branch) -- removing them is a
                // pure simplification, not a capability loss.
                if !on_json_level && primary_button(ui, "+ New Tab").on_hover_text("New tab (Ctrl+T)").clicked() {
                    actions.push(GraphAction::NewTab(self.focused.clone()));
                }
                if !on_json_level && (!self.headless || !groups_at_level.is_empty()) {
                    self.toolbar_group_chip(ui, groups_at_level, &mut actions);
                }
                match self.toolbar_zoom(ui) {
                    Some(ZoomChipAction::Fit) => reset_view = true,
                    Some(ZoomChipAction::Reset100) => reset_zoom = true,
                    Some(ZoomChipAction::In) => graph_zoom_step = Some(ZOOM_STEP),
                    Some(ZoomChipAction::Out) => graph_zoom_step = Some(1.0 / ZOOM_STEP),
                    None => {}
                }
                self.toolbar_filter(ui, on_json_level);
                recently_closed_button_rect = self.toolbar_recently_closed(ui);
                if !on_json_level {
                    self.toolbar_selection(ui, &mut actions);
                }
                shortcuts_help_button_rect = Some(self.render_universal_chip(ui));
            });
        });
        GraphToolbar {
            actions,
            reset_view,
            reset_zoom,
            zoom_step: graph_zoom_step,
            shortcuts_help_button_rect,
            recently_closed_button_rect,
            leave_group_button_rect,
        }
    }

    /// The breadcrumb trail with its back button, the rename box on the current group and, on a JSON level, the
    /// JSON bar. Returns the back button's rect.
    fn toolbar_breadcrumb(
        &mut self,
        ui: &mut egui::Ui,
        on_json_level: bool,
        actions: &mut Vec<GraphAction>,
    ) -> Option<egui::Rect> {
        let mut leave_group_button_rect: Option<egui::Rect> = None;
        // Empty at the true top level; one entry per ancestor
        // group otherwise (root-most first, current group
        // last) -- groups nest now, so this is a real
        // breadcrumb trail, not just "are we inside the one
        // possible group or not." See `App::current_group`'s own doc
        // comment on nesting and `group_breadcrumb`'s own.
        let breadcrumb = self.group_breadcrumb();
        if !breadcrumb.is_empty() {
            // Goes up exactly one level (to the current group's
            // own parent, possibly another group, not straight
            // to the top) -- the breadcrumb trail below takes
            // the place "+ New Group" would otherwise fill in
            // the empty space "+ New Tab"/"Reset View" already
            // occupy at the top level. `CARET_LEFT`, not a raw
            // "◀" glyph -- the Tab toolbar's identical back
            // chevron already migrated to `egui_phosphor` for
            // this exact font's tofu-box risk with symbol
            // glyphs; this one had been missed.
            let leave_group_response = ui.button(egui_phosphor::light::CARET_LEFT).on_hover_text("Back one level");
            if leave_group_response.clicked() {
                actions.push(GraphAction::LeaveGroup);
            }
            leave_group_button_rect = Some(leave_group_response.rect);
            let crumb_count = breadcrumb.len();
            for (i, (id, name)) in breadcrumb.iter().enumerate() {
                // Each crumb's own name comes straight from a
                // Group's name -- unbounded, and now often an
                // object key or `[index]` from "Expand as
                // Graph" (see that feature's own doc comment),
                // which tend to run longer than a hand-typed
                // group name. A plain `ui.button(name)` has no
                // width limit of its own in this horizontal,
                // non-wrapping row, so a deeply-nested chain of
                // long names could push clean off the toolbar's
                // own visible edge -- the exact overflow this
                // toolbar's other buttons were already
                // confirmed live to hit once before (see the
                // icon-only button conversion elsewhere in this
                // file). So it is drawn by the display rule and
                // cut to `CRUMB_UNITS` by `names::shorten` (small
                // fixed chrome, not a place for arbitrarily long
                // text), where a placeholder counts as one unit
                // and cannot dodge the bound; the hover gives the
                // whole name.
                let segs = crate::names::display_name(name, crate::names::UNTITLED);
                let shown = crate::names::shorten(&segs, CRUMB_UNITS);
                let text = crate::names::name_job(ui, "", shown.as_deref().unwrap_or(&segs), "");
                if i + 1 == crumb_count && on_json_level {
                    // The Json node itself: its name is its file's,
                    // so there is nothing to rename.
                    let label = ui.label(text);
                    let hover = crate::names::name_hover(name, CRUMB_UNITS, "", str::to_string);
                    if !hover.is_empty() {
                        label.on_hover_text(hover);
                    }
                } else if i + 1 == crumb_count {
                    let is_renaming = self.renaming_group.as_ref().is_some_and(|(rid, _)| rid == id);
                    if is_renaming {
                        if let Some((_, draft)) = &mut self.renaming_group {
                            let response = ui.add(egui::TextEdit::singleline(draft).desired_width(140.0));
                            if take_focus(&mut self.pending_focus, FocusTarget::GroupRename) {
                                response.request_focus();
                            }
                            if response.lost_focus() {
                                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                    actions.push(GraphAction::Node(id.clone(), NodeAction::CancelRename));
                                } else {
                                    actions
                                        .push(GraphAction::Node(id.clone(), NodeAction::CommitRename(draft.clone())));
                                }
                            }
                        }
                    } else if ui
                        .button(text)
                        .on_hover_text(crate::names::name_hover(name, CRUMB_UNITS, "Click to rename (F2)", |f| {
                            format!("Click to rename, F2 ({f})")
                        }))
                        .clicked()
                    {
                        actions.push(GraphAction::Node(id.clone(), NodeAction::Rename));
                    }
                } else {
                    // An ancestor, not the current group itself
                    // -- click to jump straight there (reuses
                    // `GraphAction::EnterGroup`, the exact same "switch
                    // current_group and ease the camera" effect
                    // double-clicking a group card triggers;
                    // it doesn't care which direction that is).
                    if ui
                        .button(text)
                        .on_hover_text(crate::names::name_hover(name, CRUMB_UNITS, "Jump to this group", |f| {
                            format!("Jump to this group ({f})")
                        }))
                        .clicked()
                    {
                        actions.push(GraphAction::EnterGroup(id.clone()));
                    }
                    // `CARET_RIGHT`, not a raw "›" glyph -- same
                    // tofu-box reasoning as the back-one-level
                    // button above.
                    ui.label(egui_phosphor::light::CARET_RIGHT);
                }
            }
            if on_json_level {
                self.render_json_bar(ui);
            }
        }
        leave_group_button_rect
    }

    /// Import / Ungroup All / Collapse chip for the groups at this level.
    fn toolbar_group_chip(&mut self, ui: &mut egui::Ui, groups_at_level: &[String], actions: &mut Vec<GraphAction>) {
        // These three buttons used to carry full text labels
        // ("Import Group...", "Ungroup All", "Collapse Groups"/
        // "Expand Groups") and, together with "+ New Group"/
        // "+ New Tab"/"Reset View", made this toolbar cramped
        // enough to confirm live (text touching the window
        // edge at 1300pt wide). Switched to `egui_phosphor`
        // icons instead of folding into a dropdown -- each
        // stays a one-click action, just narrower, with the
        // old label preserved as a hover tooltip so the action
        // is still discoverable. All three now work at any
        // level, not just the top -- see `App::current_group`'s own
        // doc comment on nesting; each already acts on
        // `self.current_group`/the level's groups (`level_scan`,
        // its own level), never hardcoding the top level.
        //
        // The rule this toolbar actually follows, for any
        // future addition: the identity/entry-point action that
        // defines what this toolbar *is* ("+ New Tab") stays
        // full text; everything else added later to relieve
        // crowding goes icon-only with a hover tooltip carrying
        // the old label, per this paragraph's own history.
        // "Reset View" was the other text holdout until it
        // became the zoom chip's Fit button (hover text keeps
        // the name).
        //
        // Same "real file dialog blocks on OS/human input
        // neither exists in headless mode" reasoning as every
        // other Export/Import button -- `IMPORTGROUP <path>`
        // calls the same underlying import directly instead.
        toolbar_chip(ui, |ui| {
            let mut bounds: Option<egui::Rect> = None;
            let mut grow = |r: egui::Rect| {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            };
            if !self.headless {
                let response =
                    chip_icon_button(ui, egui::Id::new("icon_btn_import_group"), egui_phosphor::light::UPLOAD_SIMPLE)
                        .named_tip("Import Group...");
                grow(response.rect);
                if response.clicked() {
                    actions.push(GraphAction::ImportGroup);
                }
            }
            if !groups_at_level.is_empty() {
                let response =
                    chip_icon_button(ui, egui::Id::new("icon_btn_ungroup_all"), egui_phosphor::light::FOLDER_MINUS)
                        .named_tip("Ungroup All");
                grow(response.rect);
                if response.clicked() {
                    actions.push(GraphAction::UngroupAll);
                }
            }
            if !groups_at_level.is_empty() {
                let (icon, hover) = if self.groups_collapsed {
                    (egui_phosphor::light::CORNERS_OUT, "Expand Groups")
                } else {
                    (egui_phosphor::light::CORNERS_IN, "Collapse Groups")
                };
                let response = chip_icon_button(ui, egui::Id::new("icon_btn_collapse_groups"), icon).named_tip(hover);
                grow(response.rect);
                if response.clicked() {
                    self.groups_collapsed = !self.groups_collapsed;
                }
            }
            bounds.unwrap_or(ui.cursor())
        });
    }

    /// The zoom chip; what it asks for, if anything.
    fn toolbar_zoom(&self, ui: &mut egui::Ui) -> Option<ZoomChipAction> {
        // −/+/100%/Fit in one chip (`render_zoom_chip`, shared
        // with Image view). Fit is the old "Reset View"
        // (re-fit every node at this level); the `%` readout
        // keeps the current pan and only resets the scale.
        let zoom = self.camera.zoom_pct / 100.0;
        render_zoom_chip(
            ui,
            self.camera.zoom_pct,
            zoom > GRAPH_ZOOM_MIN + 1e-3,
            zoom < GRAPH_ZOOM_MAX - 1e-3,
            "Reset View (fit all)",
        )
    }

    /// The filter box and its clear button (not on a JSON level).
    fn toolbar_filter(&mut self, ui: &mut egui::Ui, on_json_level: bool) {
        // Not on a JSON level: it filters no cards there, and the
        // outline has the level's own search.
        if !on_json_level {
            toolbar_field(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.graph_filter)
                        .desired_width(150.0)
                        .min_size(egui::vec2(0.0, 28.0))
                        .margin(egui::Margin::symmetric(8, 4))
                        .hint_text("Filter..."),
                )
            });
        }
        // Only shown once there's actually something to clear --
        // same `egui_phosphor::light::X` glyph every other close/
        // dismiss affordance in this app already uses, so this
        // reads as "clear" at a glance rather than a new symbol.
        if !on_json_level
            && !self.graph_filter.is_empty()
            && ui.button(egui_phosphor::light::X).on_hover_text("Clear filter").clicked()
        {
            self.graph_filter.clear();
        }
    }

    /// "Recently Closed (n)"; returns its rect for the popup anchor.
    fn toolbar_recently_closed(&mut self, ui: &mut egui::Ui) -> Option<egui::Rect> {
        let mut recently_closed_button_rect: Option<egui::Rect> = None;
        if !self.closed.is_empty() {
            let label = format!("Recently Closed ({})", self.closed.len());
            let response = ui.button(label).on_hover_text("Reopen the last closed tab (Ctrl+Shift+T)");
            self.popup_opener_clicked(ui, AnchoredPopup::RecentlyClosed, &response);
            recently_closed_button_rect = Some(response.rect);
        }
        recently_closed_button_rect
    }

    /// Group/Close Selected, shown once a rectangle select has picked something.
    fn toolbar_selection(&self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        // Rectangle-select's own action bar: only shown once a
        // right-drag has actually selected something (see
        // `handle_rect_select`).
        if !self.selected_tabs.is_empty() {
            // Matches "+ New Group" above: groups the selection
            // into a new subgroup of whichever level is
            // currently being viewed, not just the top.
            if ui.button(format!("Group Selected ({})", self.selected_tabs.len())).clicked() {
                actions.push(GraphAction::GroupSelected);
            }
            if ui.button(format!("Close Selected ({})", self.selected_tabs.len())).clicked() {
                actions.push(GraphAction::CloseSelected);
            }
        }
    }

    /// Arrow-key graph navigation: moves `self.focused` to the nearest
    /// tab node in the pressed direction, reusing the same field the
    /// ACCENT card border already highlights and every other
    /// "act on the focused tab" path (Ctrl+W, "+ New Tab"'s parent,
    /// Enter/click-to-open) already reads -- so a keyboard-selected node
    /// behaves exactly like a clicked one, just without the view
    /// transition. Gated on the group-rename text field and quick-
    /// switcher both being closed so arrow keys still do their normal
    /// job there instead of hijacking focus underneath an open widget --
    /// the Settings page itself needs no such check here, since it's now
    /// a separate `ViewMode` `build_graph_ui` never runs under at all.
    /// `typing`: a text field had focus at frame start; its keys are its own
    /// (`shortcuts::text_field_has_keys`).
    pub(crate) fn handle_graph_keys(&mut self, ui: &egui::Ui, typing: bool) {
        if self.json_level.is_some() {
            self.handle_json_keys(ui, typing);
            return;
        }
        if !typing && self.renaming_group.is_none() && self.quick_switcher.is_none() {
            let dir = ui.input(|i| {
                if i.key_pressed(egui::Key::ArrowRight) {
                    Some(egui::vec2(1.0, 0.0))
                } else if i.key_pressed(egui::Key::ArrowLeft) {
                    Some(egui::vec2(-1.0, 0.0))
                } else if i.key_pressed(egui::Key::ArrowDown) {
                    Some(egui::vec2(0.0, 1.0))
                } else if i.key_pressed(egui::Key::ArrowUp) {
                    Some(egui::vec2(0.0, -1.0))
                } else {
                    None
                }
            });
            // An open node menu keeps the arrows and Enter for itself.
            let menu_open = self.node_menu.is_some();
            if let Some(dir) = dir.filter(|_| !menu_open) {
                self.move_graph_focus(dir);
            }
            // Enter opens whatever arrow-key navigation landed on, the same
            // way clicking the node's card does. A Tab-focused card handles its own
            // Enter in the card pass, so this skips it (else it would open the
            // app-focused tab instead of the card).
            if !self.card_nav_active && !menu_open && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.open_focused_graph_node();
            }
            // Escape clears a rectangle-select selection, dismisses any
            // node's context menu, and dismisses a hover-peek preview
            // early -- `self.hovered` itself is left alone (clearing it
            // here would just get overwritten later this same frame by the
            // card loop (`draw_graph_cards`) re-detecting the pointer still sitting over
            // it) and `peek_suppressed` is set instead, checked by the peek
            // preview (`draw_peek_preview`) so it skips rendering for this
            // specific id without needing the pointer to actually move
            // away first. Graph view has no other use for Escape (unlike
            // Tab view, where it steps back to Graph), so this is a plain
            // no-op the rest of the time. Pick-target mode comes first: that
            // Escape ends it and does nothing else.
            if self.connecting.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.cancel_connect();
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.selected_tabs.clear();
                if let Some((id, _)) = self.node_menu.take() {
                    ui.memory_mut(|m| m.request_focus(egui::Id::new(("graph_card", id.as_str()))));
                }
                self.pending_new_node = None;
                self.dismiss_hover_peek();
            }
        }
    }

    /// Rectangle-select: a right-button drag over the graph panel
    /// selects every tab node its screen-space rectangle overlaps.
    /// Deliberately the secondary button, not primary -- primary is
    /// Scene's own pan gesture, and `.drag_pan_buttons(PRIMARY)` in `build_graph_ui`
    /// restricts Scene to just that button so the two never compete for
    /// the same drag. Read directly off `ui.input`'s raw pointer state
    /// rather than through a `Response`: this needs to work anywhere
    /// over the panel, including empty space no widget claims, and
    /// egui's own hit-testing/consumption (which a `Response` reflects)
    /// is irrelevant to a plain "where is the mouse" query.
    pub(crate) fn handle_rect_select(
        &mut self,
        ui: &egui::Ui,
        overlay_painter: &egui::Painter,
        scene_outer_rect: egui::Rect,
        zoom_scale: f32,
        translation: egui::Vec2,
    ) {
        let secondary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
        let pointer_pos = ui.input(|i| i.pointer.interact_pos());
        let secondary_pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        if secondary_pressed {
            // A fresh right-click press always cancels any still-open
            // "New Tab/Note/Group/File" card from a previous one -- same
            // "starting a new gesture dismisses the old one" rule every
            // other popup here already follows.
            self.pending_new_node = None;
            if let Some(pos) = pointer_pos.filter(|p| scene_outer_rect.contains(*p)) {
                // A right-press landing on any card opens that card's
                // context menu instead of starting a marquee -- groups
                // specifically aren't marquee-selectable (only tabs are,
                // see `nearest_tab_in_direction`'s own doc comment on why
                // `self.focused` stays tab-only), so a right-click on one
                // would otherwise just compute an empty selection.
                let world_pos = ((pos.to_vec2() - translation) / zoom_scale).to_pos2();
                // Reverse draw order, `level_items`, so the card on top
                // takes the right-click, the same card a left-click there
                // would reach.
                let hit = self.level_items().into_iter().rev().find(|item| item.rect.contains(world_pos));
                match hit {
                    Some(LevelItem { id, kind: LevelItemKind::Node(_), .. }) => {
                        self.node_menu = Some((id, pos));
                        self.rect_select_start = None;
                    }
                    Some(LevelItem { kind: LevelItemKind::JsonCard, .. }) => {
                        self.rect_select_start = None;
                        if let Some(level) = self.json_level.as_mut() {
                            if let Some(i) = level.card_at(world_pos) {
                                let target =
                                    level.row_at(i, world_pos).unwrap_or_else(|| level.cards[i].pointer.clone());
                                level.selected = Some(target.clone());
                                level.menu = Some((target, pos));
                            }
                        }
                    }
                    None => {
                        self.node_menu = None;
                        if let Some(level) = self.json_level.as_mut() {
                            level.menu = None;
                        }
                        self.rect_select_start = Some(pos);
                    }
                }
            }
        }
        if let Some(start) = self.rect_select_start {
            let current = pointer_pos.unwrap_or(start);
            let screen_rect = egui::Rect::from_two_pos(start, current);
            overlay_painter.rect_filled(
                screen_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 35),
            );
            overlay_painter.rect_stroke(screen_rect, 0.0, egui::Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
            if !secondary_down {
                self.rect_select_start = None;
                // Barely moved -- a plain right-click, not a real
                // rectangle-select drag. The level items'
                // hit-testing above already ruled out landing on any real
                // node, so this is genuinely empty background: show the
                // "New Tab/Note/Group/File" card here instead of computing
                // a select-nothing zero-area marquee.
                if (current - start).length() < 4.0 {
                    // A JSON level is read-only: no creation card there.
                    if self.json_level.is_none() {
                        let world_pos = ((start.to_vec2() - translation) / zoom_scale).to_pos2();
                        self.pending_new_node = Some(egui::Vec2::new(world_pos.x, world_pos.y));
                    }
                } else {
                    let world_rect = egui::Rect::from_two_pos(
                        ((screen_rect.min.to_vec2() - translation) / zoom_scale).to_pos2(),
                        ((screen_rect.max.to_vec2() - translation) / zoom_scale).to_pos2(),
                    );
                    self.selected_tabs = self
                        .tabs_at_current_level()
                        .into_iter()
                        .filter(|id| {
                            self.nodes.contains_key(id) && {
                                let card = egui::Rect::from_center_size(self.layout_pos(id).to_pos2(), GRAPH_NODE_SIZE);
                                world_rect.intersects(card)
                            }
                        })
                        .collect();
                }
            }
        }
    }

    /// Minimap: a small fixed overview in the graph panel's bottom-right
    /// corner, screen-space (drawn via `overlay_painter`, so it's
    /// unaffected by Scene's own pan/zoom the same way node labels are)
    /// showing every current-level node as a dot and the main view's
    /// current viewport as an outlined rect. World bounds reuse
    /// `fit_all_nodes_scene_rect()` -- the same extent "Reset View" and
    /// entering/leaving a group already frame to, so the minimap and the
    /// main view always agree on "the whole graph". Clicking anywhere in
    /// it recenters the main viewport there without changing zoom.
    /// Returns the minimap's screen rect, the world bounds it shows and
    /// its world-to-minimap scale, for `handle_minimap_click`. Not drawn
    /// (`None`) while every card at this level is in view: there's nowhere
    /// off screen to navigate to, and it would only cover the cards in
    /// its corner.
    pub(crate) fn draw_graph_minimap(
        &self,
        ui: &egui::Ui,
        overlay_painter: &egui::Painter,
        scene_outer_rect: egui::Rect,
        items: &[LevelItem],
    ) -> Option<(egui::Rect, egui::Rect, f32)> {
        if items.is_empty() {
            return None;
        }
        let world_bounds = self.bounds_for_rects(items.iter().map(|item| item.rect));
        if self.camera.rect.contains_rect(world_bounds.shrink(GRAPH_FIT_MARGIN)) {
            return None;
        }
        let minimap_size = egui::vec2(160.0, 110.0);
        let minimap_rect = egui::Rect::from_min_size(
            scene_outer_rect.right_bottom() - minimap_size - egui::Vec2::splat(12.0),
            minimap_size,
        );
        let minimap_scale = (minimap_rect.width() / world_bounds.width().max(1.0))
            .min(minimap_rect.height() / world_bounds.height().max(1.0));
        let world_to_minimap =
            |p: egui::Pos2| -> egui::Pos2 { minimap_rect.center() + (p - world_bounds.center()) * minimap_scale };
        overlay_painter.rect_filled(minimap_rect, 4.0, egui::Color32::from_black_alpha(160));
        for item in items {
            let (radius, color) = match item.kind {
                LevelItemKind::JsonCard => (2.0, ACCENT),
                LevelItemKind::Node(kind) => match kind.ty {
                    ty if is_container(ty) => (3.0, ACCENT),
                    TYPE_TAB => (2.0, ui.visuals().text_color()),
                    TYPE_DOWNLOAD => (2.0, ui.visuals().weak_text_color()),
                    TYPE_NOTE => (2.0, NOTE_CARD_COLOR),
                    // Image, PDF and JSON cards; a type this build doesn't know draws as one of them.
                    _ => (2.0, ACCENT),
                },
            };
            let failed = self
                .nodes
                .get(&item.id)
                .and_then(|n| n.as_unloaded())
                .is_some_and(|t| matches!(t.state, UnloadedState::Failed(_)));
            let color = if failed { DANGER_COLOR } else { color };
            overlay_painter.circle_filled(world_to_minimap(item.rect.center()), radius, color);
        }
        let viewport_rect =
            egui::Rect::from_two_pos(world_to_minimap(self.camera.rect.min), world_to_minimap(self.camera.rect.max))
                .intersect(minimap_rect);
        overlay_painter.rect_stroke(viewport_rect, 0.0, egui::Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
        overlay_painter.rect_stroke(
            minimap_rect,
            4.0,
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
            egui::StrokeKind::Inside,
        );
        Some((minimap_rect, world_bounds, minimap_scale))
    }

    /// Pan momentum bookkeeping -- see `PanMomentum`'s own doc
    /// comment. `actively_panning` requires the pointer to still be
    /// held down (ruling out a single-frame minimap-click jump, which
    /// never has "held down across multiple frames" to sample a
    /// velocity from), no node currently grabbed (a node drag moves
    /// the node, not the camera, so it'd contribute a bogus near-zero
    /// sample anyway, but this is the explicit, intended guard), and
    /// no `GraphCameraTween` in progress (that already owns
    /// `camera.rect` this frame).
    pub(crate) fn track_pan_momentum(&mut self, ui: &egui::Ui, scene_rect: egui::Rect) {
        let pointer_down = ui.input(|i| i.pointer.primary_down());
        let actively_panning = pointer_down && self.dragging.is_none() && self.camera.tween.is_none();
        if actively_panning {
            // A fresh grab of the background cancels any momentum still
            // gliding from an earlier release -- "grab it to stop it".
            self.camera.momentum = None;
            self.camera.drag_history.push_back((std::time::Instant::now(), scene_rect.center()));
            while self.camera.drag_history.len() > PAN_HISTORY_LEN {
                self.camera.drag_history.pop_front();
            }
        } else {
            if self.camera.momentum.is_none() && ui.input(|i| i.pointer.primary_released()) {
                if let (Some(&(t0, p0)), Some(&(t1, p1))) =
                    (self.camera.drag_history.front(), self.camera.drag_history.back())
                {
                    let dt = (t1 - t0).as_secs_f32();
                    if dt > 0.01 {
                        let velocity = (p1 - p0) / dt;
                        if velocity.length() >= PAN_MOMENTUM_MIN_SPEED {
                            self.camera.momentum = Some(PanMomentum {
                                initial_velocity: velocity,
                                start: std::time::Instant::now(),
                                start_center: scene_rect.center(),
                            });
                        }
                    }
                }
            }
            // Not actively panning this frame for any reason (released,
            // never pressed, grabbed a node, mid-tween) -- clear so a
            // stale sample from an unrelated earlier gesture can never
            // leak into a later release's velocity estimate.
            self.camera.drag_history.clear();
        }
    }

    /// Minimap click-to-recenter: deliberately registered *after*
    /// `build_graph_ui`'s `Scene::show` (not alongside the minimap's own drawing,
    /// which runs before it) -- egui resolves overlapping hit-testing in
    /// the same layer by allocation order, last-registered on top, so an
    /// `interact()` added before Scene's own background pan sense would
    /// lose the minimap's screen area to it. `world_bounds`/
    /// `minimap_rect`/`minimap_scale` are what `draw_graph_minimap`
    /// returned.
    /// Read raw pointer state rather than `ui.interact()`: Scene's own
    /// background pan sense already covers this whole panel (it's
    /// allocated over `scene_outer_rect`, which the minimap sits inside)
    /// and wins egui's hit-testing for any widget added here, so a
    /// normal `interact()`-based click never actually saw the pointer --
    /// confirmed live via temporary logging showing zero hover events
    /// even with the cursor sitting directly over the minimap. The same
    /// "read `ui.input` directly, ignore widget hit-testing entirely"
    /// fix the rectangle-select marquee already needed for an identical
    /// reason (see that code's own doc comment).
    pub(crate) fn handle_minimap_click(
        &mut self,
        ui: &egui::Ui,
        minimap_rect: egui::Rect,
        world_bounds: egui::Rect,
        minimap_scale: f32,
    ) {
        let minimap_clicked = ui.input(|i| {
            i.pointer.primary_clicked() && i.pointer.interact_pos().is_some_and(|p| minimap_rect.contains(p))
        });
        if minimap_clicked {
            if let Some(click_pos) = ui.input(|i| i.pointer.interact_pos()) {
                let world_click = world_bounds.center() + (click_pos - minimap_rect.center()) / minimap_scale;
                self.camera.jump_to(egui::Rect::from_center_size(world_click, self.camera.rect.size()));
                self.pending_new_node = None;
            }
        }
    }

    /// Dot grid first, under everything -- a fixed-world-spacing
    /// reference so panning/zooming reads as moving through a
    /// real space rather than an empty void, the same idea every
    /// node-graph editor (Blender, Figma, ComfyUI) uses. Spacing
    /// is in *world* units (dots get farther apart on screen
    /// when zoomed in, closer together zoomed out -- the
    /// intended feel), while the dot's own radius is scaled by
    /// `inv_zoom` so it stays a constant ~1.5 screen px
    /// regardless of zoom. Reads `self.camera.rect` (the
    /// previous frame's visible world rect) rather than the
    /// `build_graph_ui`'s local `scene_rect` -- that one is already mutably
    /// borrowed for the whole `show` call, including this
    /// closure, so it can't also be read from in here. One frame
    /// of lag, same as `camera.zoom_pct`'s own toolbar readout --
    /// imperceptible at any real frame rate.
    ///
    /// Spacing is adaptive (doubled/halved in world units, never
    /// below a 20 screen-px floor) rather than a fixed 40 world
    /// units -- a fixed spacing looks right near 100% zoom but
    /// turns into tens of thousands of `circle_filled` calls a
    /// frame at `GRAPH_ZOOM_MIN` (0.15x: a full-HD window's
    /// visible world rect alone would need ~57,000 dots at 40
    /// world units apart), a real per-frame cost this avoids by
    /// keeping on-screen density roughly constant regardless of
    /// zoom level instead.
    pub(crate) fn draw_graph_dot_grid(&self, ui: &egui::Ui, zoom_scale: f32, inv_zoom: f32) {
        let mut grid_spacing = 40.0_f32;
        while grid_spacing * zoom_scale < 20.0 {
            grid_spacing *= 2.0;
        }
        while grid_spacing * zoom_scale > 80.0 {
            grid_spacing /= 2.0;
        }
        let dot_color = ui.visuals().weak_text_color().gamma_multiply(0.35);
        let visible = self.camera.rect;
        let start_x = (visible.min.x / grid_spacing).floor() as i32;
        let end_x = (visible.max.x / grid_spacing).ceil() as i32;
        let start_y = (visible.min.y / grid_spacing).floor() as i32;
        let end_y = (visible.max.y / grid_spacing).ceil() as i32;
        for gx in start_x..=end_x {
            for gy in start_y..=end_y {
                ui.painter().circle_filled(
                    egui::pos2(gx as f32 * grid_spacing, gy as f32 * grid_spacing),
                    1.5 * inv_zoom,
                    dot_color,
                );
            }
        }
    }

    /// Edges first so node cards paint on top of the lines feeding them: every edge `level_edges` lists once, each
    /// parent->child edge solid and each Link node dashed and straight in the accent colour. Both endpoints must share
    /// the currently-viewed level. A hovered link line (`highlight`) is drawn twice as thick.
    pub(crate) fn draw_graph_edges(&self, ui: &egui::Ui, edges: &[LevelEdge], inv_zoom: f32) {
        for edge in edges {
            // Viewport culling: unlike a node's own rect, an edge
            // can have one endpoint off-screen and the other on --
            // or pass through the viewport with both endpoints
            // outside it -- so this checks the bounding box of the
            // whole segment against the viewport, not either
            // endpoint alone. Not pixel-exact for a steep diagonal
            // that grazes past a corner of its own bounding box
            // without actually crossing the viewport, but that's a
            // rare, cheap-to-tolerate false negative, not a
            // visible edge going missing.
            if !egui::Rect::from_two_pos(edge.from, edge.to).intersects(self.camera.rect) {
                continue;
            }
            let stroke = match edge.kind {
                EdgeKind::Link => {
                    let width = if edge.highlight {
                        crate::link_gestures::LINK_HOVER_STROKE_PX
                    } else {
                        crate::link_gestures::LINK_STROKE_PX
                    };
                    egui::Stroke::new(width * inv_zoom, ACCENT)
                }
                EdgeKind::Opened => egui::Stroke::new(
                    2.0 * inv_zoom,
                    if edge.highlight { ACCENT } else { ui.visuals().weak_text_color() },
                ),
            };
            match edge.curve {
                EdgeCurve::Straight => {
                    ui.painter().add(egui::Shape::dashed_line(
                        &[edge.from, edge.to],
                        stroke,
                        6.0 * inv_zoom,
                        4.0 * inv_zoom,
                    ));
                }
                EdgeCurve::Bow => draw_graph_edge(ui.painter(), edge.from, edge.to, stroke),
                EdgeCurve::Horizontal => {
                    let bend = ((edge.to.x - edge.from.x) / 2.0).max(20.0);
                    ui.painter().add(egui::epaint::CubicBezierShape::from_points_stroke(
                        [edge.from, edge.from + egui::vec2(bend, 0.0), edge.to - egui::vec2(bend, 0.0), edge.to],
                        false,
                        egui::Color32::TRANSPARENT,
                        stroke,
                    ));
                }
            }
        }
    }

    /// Every card at this level, through the shared frame
    /// (`draw_graph_card`), plus each kind's own click action. Runs inside
    /// `build_graph_ui`'s `Scene`, after the grid and edges, so cards paint
    /// on top of them.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_graph_cards(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        actions: &mut Vec<GraphAction>,
        at_level: &[Vec<String>],
        inv_zoom: f32,
        zoom_scale: f32,
        leave_group_button_rect: Option<egui::Rect>,
    ) {
        // One drop target per frame, shared by the ring and the
        // release (see `drop_target`).
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let drop_preview =
            self.dragging.clone().and_then(|dragged| self.drop_target(&dragged, pointer, leave_group_button_rect));

        // Every card: one shared frame each (`draw_graph_card`),
        // layer by layer (`level_scan`, groups last), which is
        // also their stacking order: a later layer draws, and takes
        // the pointer, on top of an earlier one. Each kind's own
        // click action is applied here.
        let tab_order = self.card_tab_order();
        self.card_hint = None;
        self.handle_card_tab(ui, &tab_order);
        self.handle_card_digits(ui, &tab_order);
        self.hints_revealed = {
            let (held, other_key) = ui.input(|i| {
                (
                    i.modifiers.ctrl && !i.modifiers.alt && !i.modifiers.shift,
                    i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. })),
                )
            });
            let now = ui.input(|i| i.time);
            let state: CtrlReveal = ui.data(|d| d.get_temp(egui::Id::new("ctrl_reveal"))).unwrap_or_default();
            let (state, shown) = ctrl_reveal_step(state, held, other_key, now);
            ui.data_mut(|d| d.insert_temp(egui::Id::new("ctrl_reveal"), state));
            if state.since.is_some() && !shown {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(CTRL_REVEAL_AFTER));
            }
            shown
        };
        let members = self.group_members();
        // Pick-target mode: the cards a click would connect to, ringed.
        let pick_targets = self.connecting.as_ref().map(|(from, edge)| self.valid_targets(edge, from));
        let mut pass = CardPass {
            deferred_overlays,
            actions,
            inv_zoom,
            zoom_scale,
            drop_preview: &drop_preview,
            pick_targets: pick_targets.as_deref().unwrap_or_default(),
            show_order: self.card_nav_active || self.hints_revealed,
            order: &tab_order,
            any_focused: false,
            members: &members,
        };
        for id in at_level.iter().flatten() {
            let Some(kind) = self.card_kind(id) else {
                continue;
            };
            // Read before the frame: a Note's `TextEdit` can lose
            // focus to a click on its own header this very frame,
            // clearing `editing`, and that click must not
            // immediately re-enter editing.
            let note_was_editing = self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);
            let Some(card) = self.draw_graph_card(ui, &mut pass, id) else {
                continue;
            };
            if let Some(action) = card.control {
                // A control consumed this frame's click, so
                // the kind's own click action below (e.g.
                // Image maximize) is skipped.
                pass.actions.push(GraphAction::Node(id.clone(), action));
                continue;
            }
            // Pick-target mode takes the card's click: a valid target finishes it, any other card ends it (as every
            // click does, `build_graph_ui`), and neither does the card's own click action.
            if pick_targets.is_some() {
                if card.response.clicked() && pass.pick_targets.contains(id) {
                    pass.actions.push(GraphAction::FinishConnect(id.clone()));
                }
                continue;
            }
            // A placeholder (see `ImagePayload::error`) has nothing
            // to open; a click tries its file again instead.
            let missing_file = self.nodes.get(id).is_some_and(|n| {
                n.as_image().is_some_and(|i| i.error.is_some())
                    || n.as_unloaded().is_some_and(|t| matches!(t.state, UnloadedState::Failed(_)))
                    || n.as_json().is_some_and(|j| j.error.is_some())
                    || n.as_pdf().is_some_and(|p| matches!(p.outcome, Some(Err(_))))
            });
            // A double-click, or Enter on the keyboard-focused card (a single Enter already
            // counts as `clicked()`, which is what the other kinds act on).
            let activated = card.response.double_clicked()
                || (card.response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            // Click (not drag -- egui doesn't report `clicked()` for a drag).
            let click = card.response.clicked().then(|| click_action(kind)).flatten();
            match (kind.ty, click) {
                _ if missing_file => {
                    if card.response.clicked() {
                        pass.actions.push(GraphAction::Node(id.clone(), NodeAction::RetryMissingFile));
                    }
                }
                (_, Some(action)) => pass.actions.push(GraphAction::Node(id.clone(), action)),
                // Click on the rendered note enters editing in place.
                (TYPE_NOTE, _) if card.response.clicked() && !note_was_editing => {
                    if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                        note.editing = true;
                    }
                    // The editor appears next frame; focus it so typing can start at once.
                    ui.memory_mut(|m| m.request_focus(egui::Id::new(("note_edit", id))));
                }
                // Enters the node's JSON level, unless it's a leaf, its
                // file failed to load (plan ruling 10) or it's still
                // being refetched (`JsonPayload::enterable`).
                (TYPE_JSON | TYPE_JSON_LEAF, _)
                    if activated && self.nodes.get(id).and_then(|n| n.as_json()).is_some_and(|j| j.enterable()) =>
                {
                    // Entered from the keyboard: the level's root card takes focus
                    // (`draw_json_level`), sparing the toolbar's ~17 Tab stops.
                    if !card.response.double_clicked() {
                        ui.data_mut(|d| d.insert_temp(crate::json_level_ui::json_focus_root_key(), true));
                    }
                    pass.actions.push(GraphAction::EnterGroup(id.clone()))
                }
                // Drills into the group's own subgraph.
                (ty, _) if is_container(ty) && activated => pass.actions.push(GraphAction::EnterGroup(id.clone())),
                _ => {}
            }
        }
        self.card_nav_active = pass.any_focused;
    }

    /// The current level's cards in creation order: the Tab order.
    fn card_tab_order(&self) -> Vec<String> {
        self.node_order.iter().filter(|id| self.nodes.contains_key(*id) && self.in_level(id)).cloned().collect()
    }

    /// Tab / Shift+Tab from a focused card moves focus to the next / previous card in `order`
    /// (and pans to it, since it may be culled). Past either end Tab is left to egui, which moves
    /// on to the next widget outside the cards.
    fn handle_card_tab(&mut self, ui: &egui::Ui, order: &[String]) {
        let card_id = |id: &String| egui::Id::new(("graph_card", id.as_str()));
        let Some(at) = order.iter().position(|id| ui.memory(|m| m.has_focus(card_id(id)))) else { return };
        // The shift state comes from the key event itself, which is also how the debug `TABKEY shift` sends it.
        let (tab, shift) = ui.input(|i| {
            let shifted = i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } if modifiers.shift
                )
            });
            (i.key_pressed(egui::Key::Tab), shifted)
        });
        let target = if shift { at.checked_sub(1) } else { Some(at + 1).filter(|&n| n < order.len()) };
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                card_id(&order[at]),
                egui::EventFilter { tab: target.is_some(), ..Default::default() },
            )
        });
        if let (true, Some(target)) = (tab, target) {
            ui.memory_mut(|m| m.request_focus(card_id(&order[target])));
            if let Some(rect) = self.card_rect(&order[target]) {
                self.pan_to_card(rect);
            }
        }
    }

    /// Plain digit keys jump to the card with that Tab-order number (the badge's number): focus and
    /// pan, never open. Two digits within `DIGIT_WINDOW` make a longer number; a lone digit that no
    /// longer number could extend jumps at once. Ignored while a text field, popup, menu or JSON
    /// level has the keyboard.
    fn handle_card_digits(&mut self, ui: &egui::Ui, order: &[String]) {
        let key = egui::Id::new("card_digits");
        let now = ui.input(|i| i.time);
        let (mut buf, at): (String, f64) = ui.data(|d| d.get_temp(key)).unwrap_or_default();
        let free = self.json_level.is_none()
            && self.node_menu.is_none()
            && !self.popup_open()
            && !self.showing_tab_view()
            && !ui.ctx().egui_wants_keyboard_input();
        let digit = free.then(|| ui.input(pressed_digit)).flatten();
        if let Some(d) = digit {
            if now - at > DIGIT_WINDOW {
                buf.clear();
            }
            buf.push(d);
            ui.data_mut(|d| d.insert_temp(key, (buf.clone(), now)));
        }
        if !free && !buf.is_empty() {
            ui.data_mut(|d| d.remove_temp::<(String, f64)>(key));
            return;
        }
        let Some(number) = digits_ready(&buf, order.len(), now - at) else {
            if !buf.is_empty() {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(DIGIT_WINDOW));
            }
            return;
        };
        ui.data_mut(|d| d.remove_temp::<(String, f64)>(key));
        if let Some(id) = number.checked_sub(1).and_then(|n| order.get(n)) {
            ui.memory_mut(|m| m.request_focus(egui::Id::new(("graph_card", id.as_str()))));
            if let Some(rect) = self.card_rect(id) {
                self.pan_to_card(rect);
            }
        }
    }

    /// Eases the camera so a card that just took keyboard focus is in view, centered on it when it
    /// isn't already fully visible.
    pub(crate) fn pan_to_card(&mut self, card: egui::Rect) {
        let view = self.camera.rect;
        if !view.contains_rect(card) {
            self.camera.ease_to(egui::Rect::from_center_size(card.center(), view.size()));
        }
    }

    /// The empty-canvas "New Tab/Note/Group/File" card -- see
    /// `pending_new_node`'s own doc comment for what spawns/
    /// dismisses it. A vertical text-label list, same visual
    /// language every other right-click menu in this app
    /// already uses (tab/group/note/image/pdf/json), rather
    /// than the icon-only horizontal row this started as --
    /// this was the one context menu here that didn't match.
    /// Drawn the same "real egui widgets embedded in the
    /// Scene's own transformed `Ui`" way a note's `TextEdit`
    /// already is, so it scales/repositions with zoom exactly
    /// like a real card's own contents would, without this
    /// needing to be a real node itself.
    pub(crate) fn draw_pending_new_node_menu(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        actions: &mut Vec<GraphAction>,
        inv_zoom: f32,
        project: &impl Fn(egui::Pos2) -> egui::Pos2,
    ) {
        if let Some(pos) = self.pending_new_node {
            let center_pos = egui::Pos2::new(pos.x, pos.y);
            let row_count = if self.headless { 3.0 } else { 4.0 };
            // `* inv_zoom`: the background box's own world-space
            // size must grow as zoom shrinks, the same trick
            // `paint_card_control`'s own control rect already
            // uses, so it still renders at a constant on-screen size
            // after Scene's transform scales it back down -- without
            // this, the box shrinks with zoom exactly like ordinary
            // card content does, while the now-fixed-size row text
            // below stays constant, and would visibly overflow a
            // background that's shrunk out from under it.
            let card_size = egui::vec2(150.0, 12.0 + row_count * 26.0) * inv_zoom;
            let rect = egui::Rect::from_center_size(center_pos, card_size);
            ui.painter().rect_filled(rect, 2.0 * inv_zoom, TOOLBAR_FILL);
            ui.painter().rect_stroke(
                rect,
                2.0 * inv_zoom,
                egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color()),
                egui::StrokeKind::Inside,
            );
            let row_height = 26.0 * inv_zoom;
            let mut row_top = rect.min.y + 6.0 * inv_zoom;
            let mut next_row_rect = || {
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + 6.0 * inv_zoom, row_top),
                    egui::vec2(rect.width() - 12.0 * inv_zoom, row_height - 4.0 * inv_zoom),
                );
                row_top += row_height;
                row_rect
            };
            let clicked_tab = draw_new_node_menu_row(
                ui,
                deferred_overlays,
                next_row_rect(),
                "pending_new_node_tab",
                inv_zoom,
                &format!("{} New Tab", egui_phosphor::light::PLUS),
            );
            let clicked_note = draw_new_node_menu_row(
                ui,
                deferred_overlays,
                next_row_rect(),
                "pending_new_node_note",
                inv_zoom,
                &format!("{} New Note", egui_phosphor::light::NOTE_PENCIL),
            );
            let clicked_group = draw_new_node_menu_row(
                ui,
                deferred_overlays,
                next_row_rect(),
                "pending_new_node_group",
                inv_zoom,
                &format!("{} New Group", egui_phosphor::light::FOLDER_PLUS),
            );
            // Same reasoning as every other file-dialog button in
            // this app -- a real picker blocks on OS/human input
            // neither exists in headless mode.
            let clicked_file = !self.headless
                && draw_new_node_menu_row(
                    ui,
                    deferred_overlays,
                    next_row_rect(),
                    "pending_new_node_file",
                    inv_zoom,
                    &format!("{} Add File...", egui_phosphor::light::FILE_PLUS),
                );
            if clicked_tab {
                actions.push(GraphAction::NewTab(self.focused.clone()));
                self.pending_new_node = None;
            }
            if clicked_note {
                actions.push(GraphAction::NewNote(pos));
                self.pending_new_node = None;
            }
            if clicked_group {
                actions.push(GraphAction::NewGroup(pos));
                self.pending_new_node = None;
            }
            if clicked_file {
                actions.push(GraphAction::NewFile(pos));
                self.pending_new_node = None;
            }
            // Same dismiss-on-click-elsewhere mechanism every other
            // popup here uses, just projected to screen space first
            // -- this card's own rect is in world-space (Scene's
            // transformed coordinates), but `clicked_outside_popup`
            // reads the real screen-space pointer position.
            let screen_rect = egui::Rect::from_min_max(project(rect.min), project(rect.max));
            if clicked_outside_popup(ui, screen_rect, None) {
                self.pending_new_node = None;
            }
        }
    }

    /// Peek preview: an enlarged view of the hovered node's cached
    /// thumbnail, floating near it, without changing focus. Queued as a
    /// deferred overlay (not a separate `egui::Area`) so it is projected
    /// with the frame's final pan/zoom and paints after every card's
    /// overlay text and chips (`draw_graph_cards`), which would
    /// otherwise cover it. Skipped while `peek_suppressed` matches -- an
    /// Escape press dismissing this exact node's peek early,
    /// see that field's own doc comment.
    pub(crate) fn draw_peek_preview(&mut self, ui: &mut egui::Ui, deferred_overlays: &mut DeferredOverlays) {
        if self.peek_suppressed.is_some() && self.peek_suppressed != self.hovered {
            self.peek_suppressed = None;
        }
        let peek_target = self.hovered.clone().filter(|id| self.peek_suppressed.as_deref() != Some(id.as_str()));
        // Polled every frame regardless of `peek_target`, same
        // "continuous polling, not just while shown" reasoning the
        // dropdown popups' own fade needs -- a plain `if let Some`
        // gate here would only ever call `animate_bool` while a
        // peek is actually showing, and the real-elapsed-time gap
        // since the last poll (however long the pointer sat
        // elsewhere) would make it jump straight to fully visible
        // the instant it reappears instead of visibly fading in.
        let peek_fade = ui.ctx().animate_bool(egui::Id::new("peek_preview_fade"), peek_target.is_some());
        if peek_fade > 0.0 {
            if let Some(hovered_id) = peek_target {
                let node_pos = self.nodes.contains_key(&hovered_id).then(|| self.layout_pos(&hovered_id));
                let thumb_id = self.gpu.as_ref().and_then(|g| g.thumbnail_id(&hovered_id));
                if let (Some(node_pos), Some(tex_id)) = (node_pos, thumb_id) {
                    let rect = egui::Rect::from_min_size(
                        egui::Pos2::new(
                            node_pos.x + GRAPH_NODE_SIZE.x / 2.0 + 12.0,
                            node_pos.y - GRAPH_PEEK_SIZE.y / 2.0,
                        ),
                        GRAPH_PEEK_SIZE,
                    );
                    // Queued, not painted here: card labels and chips paint on the overlay layer after the
                    // scene, so a peek drawn in the scene would sit under a neighbour's text.
                    let bg = ui.visuals().extreme_bg_color.gamma_multiply(peek_fade);
                    deferred_overlays.push(Box::new(move |painter, project| {
                        let (min, max) = (project(rect.min), project(rect.max));
                        let rounding = 6.0 * (max.x - min.x) / rect.width();
                        let rect = egui::Rect::from_min_max(min, max);
                        painter.rect_filled(rect, rounding, bg);
                        painter.image(
                            tex_id,
                            rect,
                            crate::card::FULL_UV,
                            egui::Color32::WHITE.gamma_multiply(peek_fade),
                        );
                        painter.rect_stroke(
                            rect,
                            rounding,
                            egui::Stroke::new(1.5, ACCENT.gamma_multiply(peek_fade)),
                            egui::StrokeKind::Inside,
                        );
                    }));
                }
            }
        }
    }
}

/// Painted on the canvas when the level has no nodes (the bindings are in `shortcuts.rs`).
const EMPTY_HINT: &str = "Ctrl+T for a new tab \u{b7} drop a file or URL \u{b7} F1 for shortcuts";

/// The most units a breadcrumb crumb shows (`names::shorten`; a placeholder counts one).
const CRUMB_UNITS: usize = 20;

/// Ctrl held alone this long reveals the key hints.
const CTRL_REVEAL_AFTER: f64 = 0.35;

/// The hold-Ctrl-to-reveal state: when a solo Ctrl hold began, and whether another key was pressed
/// during it (Ctrl+C and the like), which cancels the reveal until Ctrl is released.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct CtrlReveal {
    since: Option<f64>,
    cancelled: bool,
}

/// One frame of the reveal: `held` is Ctrl alone down, `other_key` a key press this frame. Returns the
/// new state and whether the hints show.
fn ctrl_reveal_step(mut state: CtrlReveal, held: bool, other_key: bool, now: f64) -> (CtrlReveal, bool) {
    if !held {
        return (CtrlReveal::default(), false);
    }
    let since = *state.since.get_or_insert(now);
    state.cancelled |= other_key;
    (state, !state.cancelled && now - since >= CTRL_REVEAL_AFTER)
}

/// How long a digit waits for a second one before the number is taken as typed.
const DIGIT_WINDOW: f64 = 0.6;

/// The plain digit key (no modifiers) pressed this frame.
fn pressed_digit(i: &egui::InputState) -> Option<char> {
    use egui::Key::*;
    [
        (Num0, '0'),
        (Num1, '1'),
        (Num2, '2'),
        (Num3, '3'),
        (Num4, '4'),
        (Num5, '5'),
        (Num6, '6'),
        (Num7, '7'),
        (Num8, '8'),
        (Num9, '9'),
    ]
    .into_iter()
    .find(|(k, _)| i.key_pressed(*k) && !i.modifiers.any())
    .map(|(_, c)| c)
}

/// The card number to jump to once the typed `digits` are final: after two digits, once `idle`
/// passes `DIGIT_WINDOW`, or when no longer number (appending a digit) could name one of `count`
/// cards. A leading 0 names nothing and is dropped.
fn digits_ready(digits: &str, count: usize, idle: f64) -> Option<usize> {
    let n: usize = digits.parse().ok()?;
    let final_ = digits.len() >= 2 || idle > DIGIT_WINDOW || n * 10 > count;
    final_.then_some(n)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ctrl_reveals_hints_only_after_a_solo_hold() {
        use super::{ctrl_reveal_step as step, CtrlReveal};
        let (s, shown) = step(CtrlReveal::default(), true, false, 10.0);
        assert!(!shown, "just pressed");
        let (s, shown) = step(s, true, false, 10.4);
        assert!(shown, "held past the delay");
        let (s, shown) = step(s, true, true, 10.5);
        assert!(!shown, "a chord key cancels it");
        let (s, shown) = step(s, true, false, 12.0);
        assert!(!shown, "stays cancelled while Ctrl is held");
        let (s, _) = step(s, false, false, 12.1);
        assert_eq!(s, CtrlReveal::default(), "release resets");
    }

    #[test]
    fn digits_wait_only_while_a_longer_number_could_still_name_a_card() {
        use super::digits_ready;
        assert_eq!(digits_ready("3", 5, 0.0), Some(3), "no card 30..39 exists");
        assert_eq!(digits_ready("1", 15, 0.0), None, "10..15 exist: wait for a second digit");
        assert_eq!(digits_ready("1", 15, 1.0), Some(1), "window elapsed");
        assert_eq!(digits_ready("12", 15, 0.0), Some(12));
        assert_eq!(digits_ready("0", 15, 1.0), Some(0), "0 names no card; the caller finds none");
    }

    use super::*;

    /// Review C-B1: Escape (or Enter, or an arrow) typed into a text field is the field's, not the graph's.
    #[test]
    fn keys_typed_into_a_text_field_stay_out_of_graph_navigation() {
        let mut app = App::default();
        app.selected_tabs.insert("t".into());
        let ctx = egui::Context::default();
        let field = egui::Id::new("test_field");
        let mut text = String::new();
        let mut frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                // Read first, as `build_graph_ui` does.
                let typing = crate::shortcuts::text_field_has_keys(ui.ctx());
                ui.add(egui::TextEdit::singleline(&mut text).id(field));
                app.handle_graph_keys(ui, typing);
            });
            output.textures_delta.clear();
        };
        ctx.memory_mut(|m| m.request_focus(field));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        let key = |key| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![key(egui::Key::Escape)]);
        assert!(app.selected_tabs.contains("t"), "Escape ended the edit only");
        assert!(!ctx.memory(|m| m.has_focus(field)), "the field let go");
        frame(&mut app, vec![key(egui::Key::Escape)]);
        assert!(app.selected_tabs.is_empty(), "with no field focused, Escape is the graph's");
    }

    /// Links: Escape in pick-target mode ends the mode and does nothing else; the next one is the graph's as before.
    #[test]
    fn escape_ends_pick_target_mode_first() {
        let mut app = App::default();
        let note = app.alloc_node_id();
        app.insert_note_at(note.clone(), None);
        app.connecting = Some((note, crate::caps::LINK));
        app.selected_tabs.insert("t".into());
        let ctx = egui::Context::default();
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let frame = |app: &mut App| {
            let input = egui::RawInput { events: vec![escape.clone()], ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| app.handle_graph_keys(ui, false));
            output.textures_delta.clear();
        };
        frame(&mut app);
        assert!(app.connecting.is_none());
        assert!(app.selected_tabs.contains("t"), "that Escape was pick mode's only");
        frame(&mut app);
        assert!(app.selected_tabs.is_empty());
    }

    /// Links: in pick-target mode a click on a target card draws the edge (and not the card's own click action), and
    /// a click on empty canvas ends the mode. Driven through `build_graph_ui` with real egui input.
    #[test]
    fn a_click_finishes_or_cancels_pick_target_mode() {
        use crate::caps::{LINK, OPENED_FROM};
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        for (id, x) in [(&a, 0.0), (&b, 600.0)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id.clone(), egui::vec2(x, 0.0), crate::store::model::Kind::Note, note);
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
        let screen_rect = |app: &App, id: &str| {
            let item = app.level_items().into_iter().find(|i| i.id == id).expect("on the level");
            app.graph_world_rect_to_screen(item.rect)
        };
        app.connecting = Some((a.clone(), LINK));
        let on_b = screen_rect(&app, &b).center();
        assert!(app.graph_scene_outer_rect().contains(on_b), "{on_b:?} in {:?}", app.graph_scene_outer_rect());
        click(&mut app, on_b);
        assert!(app.connecting.is_none());
        assert_eq!(app.links_naming(&a).len(), 1, "the click linked b");
        assert!(!app.nodes[&b].as_note().unwrap().editing, "the click was pick mode's, not the note's");
        app.connecting = Some((a.clone(), OPENED_FROM));
        // Below both cards, clear of the minimap in the bottom-right corner.
        let below = egui::pos2(screen_rect(&app, &a).center().x, screen_rect(&app, &a).bottom() + 60.0);
        assert!(![&a, &b].iter().any(|id| screen_rect(&app, id).contains(below)), "empty canvas");
        click(&mut app, below);
        assert!(app.connecting.is_none(), "a click on empty canvas ends the mode");
        assert_eq!(app.mirror.parent(&a), None, "and draws nothing");
    }

    #[test]
    fn easing_the_camera_stops_a_glide() {
        let mut camera = GraphCamera::new();
        let start = std::time::Instant::now() - std::time::Duration::from_millis(50);
        camera.momentum =
            Some(PanMomentum { initial_velocity: egui::vec2(2000.0, 0.0), start, start_center: camera.rect.center() });
        let to = egui::Rect::from_center_size(egui::pos2(5000.0, 5000.0), egui::vec2(400.0, 300.0));
        camera.ease_to(to);
        // Once the ease ends, the glide it replaced must not take over again.
        camera.tween.as_mut().unwrap().start -= TRANSITION_DURATION;
        camera.advance();
        camera.advance();
        assert_eq!(camera.rect, to);
        assert!(camera.tween.is_none() && camera.momentum.is_none());
    }

    /// A wide fit in a squarer window: `fill` adds the world `Scene` shows
    /// above and below it, and moves nothing on screen.
    #[test]
    fn filling_grows_the_rect_to_the_visible_world_without_moving_it() {
        let outer = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 600.0));
        let wide = egui::Rect::from_center_size(egui::pos2(250.0, 0.0), egui::vec2(1000.0, 300.0));
        let scale = |rect: egui::Rect| (outer.size() / rect.size()).min_elem();
        let mut camera = GraphCamera::new();
        camera.jump_to(wide);
        camera.fill(outer);
        assert_eq!((camera.rect.center(), scale(camera.rect)), (wide.center(), scale(wide)));
        assert_eq!(camera.rect.size(), egui::vec2(1000.0, 750.0));
        let filled = camera.rect;
        camera.fill(outer);
        assert_eq!(camera.rect, filled, "already filling: unchanged");
    }

    /// Tab and Shift+Tab on a focused card step through the level in creation order and stop
    /// handling at either end (egui then moves on to widgets outside the cards).
    #[test]
    fn tab_steps_through_cards_in_creation_order() {
        let mut app = App::default();
        for _ in 0..3 {
            app.new_note_shortcut();
        }
        let order = app.card_tab_order();
        assert_eq!(order.len(), 3);
        let card = |i: usize| egui::Id::new(("graph_card", order[i].as_str()));
        let ctx = egui::Context::default();
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                // Cards the pass would register; egui drops focus from an id nothing registers.
                for (i, id) in order.iter().enumerate() {
                    let rect =
                        egui::Rect::from_min_size(egui::pos2(10.0 + 60.0 * i as f32, 10.0), egui::vec2(50.0, 50.0));
                    ui.interact(rect, egui::Id::new(("graph_card", id.as_str())), egui::Sense::click());
                }
                app.handle_card_tab(ui, &order)
            });
            output.textures_delta.clear();
        };
        let tab = |modifiers| egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: Some(egui::Key::Tab),
            pressed: true,
            repeat: false,
            modifiers,
        };
        ctx.memory_mut(|m| m.request_focus(card(0)));
        frame(&mut app, vec![]);
        frame(&mut app, vec![tab(egui::Modifiers::NONE)]);
        assert!(ctx.memory(|m| m.has_focus(card(1))), "Tab: first -> second");
        frame(&mut app, vec![]);
        frame(&mut app, vec![tab(egui::Modifiers::NONE)]);
        assert!(ctx.memory(|m| m.has_focus(card(2))), "Tab: second -> third");
        frame(&mut app, vec![]);
        frame(&mut app, vec![tab(egui::Modifiers::SHIFT)]);
        assert!(ctx.memory(|m| m.has_focus(card(1))), "Shift+Tab: third -> second");
        frame(&mut app, vec![]);
        frame(&mut app, vec![tab(egui::Modifiers::NONE)]);
        frame(&mut app, vec![]);
        // At the last card the handler does nothing (target is `None`), so focus is not moved back.
        frame(&mut app, vec![tab(egui::Modifiers::NONE)]);
        assert!(!ctx.memory(|m| m.has_focus(card(0)) || m.has_focus(card(1))), "no wrap past the end");
    }

    /// With accesskit on, a card's `widget_info` puts a Button named by `Node::accessible_name`
    /// in the tree egui hands the platform.
    #[test]
    fn card_widget_info_reaches_the_accesskit_tree() {
        let mut app = App::default();
        let g = app.alloc_node_id();
        app.insert_group_at(g.clone(), None);
        app.mirror.set(&g, crate::store::model::field::NAME, crate::store::model::FieldValue::Str("Reading".into()));
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 200.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 50.0));
            let response = ui.interact(rect, egui::Id::new(("graph_card", g.as_str())), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    app.nodes[&g].accessible_name(app.own_text(&g), app.source_url(&g)),
                )
            });
        });
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update.expect("accesskit update");
        let named = update.nodes.iter().find(|(_, n)| n.label() == Some("Group: Reading"));
        assert_eq!(named.map(|(_, n)| n.role()), Some(egui::accesskit::Role::Button));
    }

    /// Name rules: crumbs draw hidden characters as placeholders and shorten to 20 units, a placeholder counting one.
    #[test]
    fn breadcrumb_crumbs_follow_the_display_rule() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        let (outer, inner) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(outer.clone(), None);
        app.insert_group_at(inner.clone(), None);
        app.edit_group(&inner, Some(outer.clone()));
        app.mirror.set(&outer, crate::store::model::field::NAME, crate::store::model::FieldValue::Str("x".repeat(30)));
        app.mirror.set(
            &inner,
            crate::store::model::field::NAME,
            crate::store::model::FieldValue::Str("Re\u{200B}ading".into()),
        );
        app.current_group = Some(inner);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.horizontal(|ui| app.toolbar_breadcrumb(ui, false, &mut Vec::new()));
        });
        output.textures_delta.clear();
        let galleys: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.clone()),
                _ => None,
            })
            .collect();
        let texts: Vec<String> = galleys.iter().map(|g| g.text().to_string()).collect();
        assert!(texts.contains(&format!("{}…", "x".repeat(19))), "{texts:?}");
        let current = galleys.iter().find(|g| g.text() == "Re⟦200B⟧ading").unwrap_or_else(|| panic!("{texts:?}"));
        assert!(current.job.sections.iter().any(|s| s.format.font_id.family == egui::FontFamily::Monospace));
    }

    /// An empty level keeps its canvas: the whole of `build_graph_ui` runs without panicking and paints the hint.
    #[test]
    fn an_empty_level_paints_the_hint_on_its_canvas() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        assert!(app.nodes.is_empty());
        let mut hints = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
            hints = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == EMPTY_HINT => Some(t.pos),
                    _ => None,
                })
                .collect();
        }
        assert_eq!(hints.len(), 1, "the hint is painted once");
    }

    /// The hint is per level: an empty group shows it; a level with a node (a group card counts) does not.
    #[test]
    fn the_hint_shows_on_every_empty_level_not_only_an_empty_graph() {
        use crate::fetched::Placement;
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        let at = Placement { group: None, pos: egui::Vec2::ZERO };
        app.place_group(Some("full".into()), "Full", at);
        app.place_group(Some("empty".into()), "Empty", Placement { group: Some("full".into()), pos: egui::Vec2::ZERO });
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node("n".into(), egui::Vec2::ZERO, crate::store::model::Kind::Note, note);
        app.edit_group("n", Some("full".into()));
        let hints_at = |app: &mut App, level: Option<&str>| {
            app.current_group = level.map(str::to_owned);
            let mut hints = 0;
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
                output.textures_delta.clear();
                hints = output
                    .shapes
                    .iter()
                    .filter(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == EMPTY_HINT))
                    .count();
            }
            hints
        };
        assert_eq!(hints_at(&mut app, Some("empty")), 1, "an empty group shows the hint");
        assert_eq!(hints_at(&mut app, Some("full")), 0, "a level with a node does not");
        assert_eq!(hints_at(&mut app, None), 0, "the top level holds the group card");
    }
}
