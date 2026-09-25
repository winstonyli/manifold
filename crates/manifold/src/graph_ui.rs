//! The Graph view: its toolbar, camera (pan momentum, zoom, minimap),
//! keyboard and rect selection, the dot grid, edges and cards, the new-node
//! menu and the hover peek.

use super::*;

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

/// How many of `ids` (one kind's own node-id list at the current level)
/// have a card (`node_card_rect`) intersecting `viewport` -- the exact
/// same per-node rect the render loops build for their own cull check.
/// Shared by `build_graph_ui`'s LOD pre-pass (see `LOD_SIMPLIFY_COUNT`)
/// so it counts "visible" precisely the way culling already does, rather
/// than a second, slightly different notion of visibility.
pub(crate) fn count_visible_nodes(nodes: &NodeMap, ids: &[String], viewport: egui::Rect) -> usize {
    ids.iter()
        .filter(|id| nodes.get(id.as_str()).is_some_and(|n| node_card_rect(n).intersects(viewport)))
        .count()
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

impl App {
    /// Replicates `egui::Scene`'s own internal `fit_to_rect_in_scene`
    /// (private to egui, not part of its public API) — needed to compute
    /// where a graph node actually lands on screen *outside* the `Scene`
    /// widget's own closure, for the shrink/expand animation's target
    /// rect. `(scale, translation)` such that
    /// `screen = translation + scale * world`, matching `Scene`'s own
    /// "fit the visible scene rect into the available screen area,
    /// preserving aspect ratio, clamped to the configured zoom range"
    /// behavior exactly (using the *same* `self.graph_scene_rect` `Scene`
    /// itself reads/writes) — verified live by screenshotting a
    /// mid-animation frame and confirming the content quad visually lines
    /// up with the target node's real on-screen position.
    pub(crate) fn graph_world_to_screen_transform(&self, scene_outer_rect: egui::Rect) -> (f32, egui::Vec2) {
        let scale = (scene_outer_rect.size() / self.graph_scene_rect.size())
            .min_elem()
            .clamp(GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        let translation =
            scene_outer_rect.center().to_vec2() - scale * self.graph_scene_rect.center().to_vec2();
        (scale, translation)
    }

    /// Where a world-space rect lands on screen in Graph view right now --
    /// the same projection `Scene` applies, computed outside it (see
    /// `graph_world_to_screen_transform`).
    pub(crate) fn graph_world_rect_to_screen(&self, world: egui::Rect) -> egui::Rect {
        let screen = self.screen_size_points();
        let scene_outer_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, GRAPH_TOOLBAR_HEIGHT_POINTS),
            egui::vec2(screen.x, (screen.y - GRAPH_TOOLBAR_HEIGHT_POINTS).max(0.0)),
        );
        let (scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
        egui::Rect::from_min_max(
            (translation + scale * world.min.to_vec2()).to_pos2(),
            (translation + scale * world.max.to_vec2()).to_pos2(),
        )
    }

    /// Applies one of `build_graph_ui`'s collected clicks. Called in click
    /// order; no two of these depend on each other's order (a group rename
    /// carries its own name, see `NodeAction::CommitRename`).
    pub(crate) fn apply_graph_action(&mut self, ctx: &egui::Context, action: GraphAction) {
        match action {
            GraphAction::Node(id, action) => self.apply_node_action(ctx, &id, action),
            GraphAction::NewTab(parent) => {
                let home_page = self.settings.home_page.clone();
                let group = self.current_group.clone();
                self.spawn_new_tab(parent, group, &home_page);
            }
            GraphAction::NewNote(pos) => {
                let id = self.alloc_note_id();
                self.insert_note_at(id, pos);
            }
            GraphAction::NewGroup(pos) => {
                let id = self.alloc_group_id();
                self.insert_group_at(id, pos);
            }
            GraphAction::NewFile(pos) => self.add_file_node(Some(pos)),
            GraphAction::GroupSelected => {
                if self.selected_tabs.is_empty() {
                    return;
                }
                let id = self.alloc_group_id();
                // Centroid of the selected cards, rather than the usual
                // world-center-plus-jitter spawn point: these tabs already have
                // real positions the user just drew a box around, so the new
                // group card should appear where they actually are, not at the
                // origin of a possibly-distant part of the graph.
                let count = self.selected_tabs.len() as f32;
                let sum = self
                    .selected_tabs
                    .iter()
                    .filter_map(|id| self.nodes.get(id))
                    .fold(egui::Vec2::ZERO, |acc, node| acc + node.pos);
                let pos = sum / count;
                let name = Self::default_group_name(&id);
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        parent: None,
                        group: self.current_group.clone(),
                        pos,
                        vel: egui::Vec2::ZERO,
                        color: None,
                        payload: NodePayload::Group(GroupPayload { name }),
                    },
                );
                self.node_order.push(id.clone());
                for tab_id in self.selected_tabs.drain() {
                    if let Some(node) = self.nodes.get_mut(&tab_id) {
                        node.group = Some(id.clone());
                    }
                }
            }
            GraphAction::CloseSelected => {
                for id in std::mem::take(&mut self.selected_tabs) {
                    self.close_node(&id);
                }
            }
            GraphAction::UngroupAll => {
                for id in self.nodes_at_current_level(ItemNodeKind::Group) {
                    self.dissolve_group(&id);
                }
            }
            GraphAction::ImportGroup => {
                if let Some(path) = rfd::FileDialog::new().add_filter("Group", &["json"]).pick_file() {
                    match group_export::load_from(&path) {
                        Ok(export) => self.import_group_export(export),
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
                let parent =
                    self.current_group.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.group.clone());
                self.switch_group_level(parent);
                self.renaming_group = None;
            }
            GraphAction::Drop { id, target } => self.apply_drop(&id, target),
        }
    }

    /// Shows `group`'s level (`None` for the top), easing the camera to fit
    /// it, and drops everything tied to the old level: the rectangle
    /// selection (the toolbar's "Group Selected"/"Close Selected" would
    /// otherwise act on tabs no longer visible), the empty-canvas card and
    /// any open node menu.
    pub(crate) fn switch_group_level(&mut self, group: Option<String>) {
        self.current_group = group;
        self.settle_current_level();
        self.graph_settled_key = Some(self.current_level_settle_key());
        self.graph_camera_tween = Some(GraphCameraTween {
            from: self.graph_scene_rect,
            to: self.fit_all_nodes_scene_rect(),
            start: std::time::Instant::now(),
        });
        self.selected_tabs.clear();
        self.rect_select_start = None;
        self.pending_new_node = None;
        self.node_menu = None;
    }

    pub(crate) fn build_graph_ui(&mut self, ui: &mut egui::Ui) {

        // Computed once per frame and reused everywhere below that only
        // needs "every node of kind X at this level" unconditionally --
        // **found during a performance audit**: each of these seven
        // `*_at_current_level` accessors used to get called at least three
        // separate times per frame from this one function alone (the
        // toolbar's own group-count gates, the settle-key check, the
        // minimap's dot-drawing pass, and the main card-rendering loop),
        // each call redoing the identical `O(n)` `self.node_order` scan
        // and allocating a fresh `Vec<String>` -- real, entirely avoidable
        // cost that ran unconditionally every frame regardless of whether
        // anything actually changed, since this app already redraws every
        // frame. `self.current_group` can't change until `EnterGroup`/
        // `LeaveGroup` actions are applied at the very end of this same
        // function, so every unconditional per-frame use below sees the
        // exact same, correct-for-this-frame level throughout. Left as
        // fresh calls: the two `current_level_settle_key()` call sites
        // that run *after* a group change is applied further down (they
        // need a genuinely new key for the new level, not this frame's
        // stale one) and the handful of conditional sites deeper in this
        // function (drag-and-drop-into-group checks, the right-click
        // hit-test chain) that don't run every idle frame, so caching them
        // here would have added surface area without the same payoff.
        let at_level = GRAPH_DRAW_ORDER.map(|kind| self.nodes_at_current_level(kind));
        let [_, _, _, _, _, _, _, groups_at_level] = &at_level;

        // Level-of-detail: count how many nodes at this level actually
        // intersect the viewport, then flip `graph_lod_simplified` with a
        // hysteresis gap -- see `LOD_SIMPLIFY_COUNT`'s own doc comment for
        // why. Uses this frame's not-yet-updated `graph_scene_rect`, the
        // same one-frame-stale viewport the render loops' own cull check
        // already accepts.
        let lod_visible_count: usize = at_level
        .iter()
        .map(|ids| count_visible_nodes(&self.nodes, ids, self.graph_scene_rect))
        .sum();
        if self.graph_lod_simplified {
            if lod_visible_count < LOD_RESTORE_COUNT {
                self.graph_lod_simplified = false;
            }
        } else if lod_visible_count > LOD_SIMPLIFY_COUNT {
            self.graph_lod_simplified = true;
        }

        let GraphToolbar {
            mut actions,
            reset_view,
            reset_zoom,
            zoom_step: graph_zoom_step,
            shortcuts_help_button_rect,
            recently_closed_button_rect,
            leave_group_button_rect,
        } = self.render_graph_toolbar(ui, groups_at_level);
        // `build_shortcuts_help`/`build_recently_closed_popup`/the three
        // context menus below are all deferred to the very end of this
        // function (past the Scene/`overlay_painter` node-rendering block),
        // not called here where their button rects are computed. That
        // order doesn't decide what paints on top, though: see
        // `overlay_painter`'s own comment for why its layer is
        // `Order::Middle`.
        if reset_view {
            self.graph_scene_rect = self.fit_all_nodes_scene_rect();
            self.pending_new_node = None;
        }

        if self.nodes.is_empty() {
            ui.centered_and_justified(|ui| {
                match &self.tab_spawn_error {
                    Some(err) => ui.colored_label(
                        DANGER_COLOR,
                        format!("{err}\nClick \"+ New Tab\" to try again."),
                    ),
                    None => ui.label("No tabs. Click \"+ New Tab\" to start browsing."),
                };
            });
            for action in actions {
                self.apply_graph_action(ui.ctx(), action);
            }
            return;
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
        if self.dragging.is_none() {
            // Same formula `current_level_settle_key()` computes, just
            // built from this frame's already-cached lists instead of
            // redoing all seven scans a second time.
            let current_key = (
                self.current_group.clone(),
                at_level.iter().map(Vec::len).sum(),
            );
            if self.graph_settled_key.as_ref() != Some(&current_key) {
                self.settle_current_level();
                self.graph_settled_key = Some(current_key);
            }
        }

        self.handle_graph_keys(ui);

        // Pan (drag empty background) + zoom (scroll wheel, centered on the
        // cursor) via egui's built-in `Scene` container — everything drawn
        // inside `add_contents` below is in the same world-space coordinates
        // `node.pos` already uses; egui's transform layer handles converting
        // to/from screen space (including for nested widgets' own
        // hover/click/drag responses, e.g. a node's own drag below), so none
        // of that logic needs to know pan/zoom exists at all. `scene_rect`
        // must live outside `self` while the closure below also borrows
        // `self` mutably, so it's round-tripped through a local instead of
        // borrowed from `self.graph_scene_rect` directly.
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
        let scene_outer_rect = ui.available_rect_before_wrap();
        if reset_zoom {
            // Keeps the current pan (unlike "Reset View", which re-fits
            // everything) -- `graph_world_to_screen_transform` derives
            // `zoom_scale` as `screen_size / graph_scene_rect.size()`
            // (`.min_elem()` of the two axis ratios), so matching the world
            // rect's size to the screen's own size on both axes makes that
            // ratio exactly 1.0, the definition of "100%" this readout uses.
            let center = self.graph_scene_rect.center();
            self.graph_scene_rect = egui::Rect::from_center_size(center, scene_outer_rect.size());
        }
        if let Some(factor) = graph_zoom_step {
            self.graph_scene_rect = step_scene_rect(
                self.graph_scene_rect,
                factor,
                scene_outer_rect.size(),
                GRAPH_ZOOM_MIN,
                GRAPH_ZOOM_MAX,
            );
        }
        let (zoom_scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
        self.graph_zoom_pct = zoom_scale * 100.0;
        let inv_zoom = 1.0 / zoom_scale;
        let project = move |world: egui::Pos2| -> egui::Pos2 {
            (translation + zoom_scale * world.to_vec2()).to_pos2()
        };
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

        self.advance_graph_camera();

        let (minimap_rect, world_bounds, minimap_scale) =
            self.draw_graph_minimap(ui, &overlay_painter, scene_outer_rect, &at_level);
        // Compared against after `Scene`'s own closure runs, below --
        // an actual pan drag or scroll-wheel zoom this frame changes this,
        // and dismisses `pending_new_node` the same as every other camera
        // move already does (see that field's own doc comment).
        let scene_rect_before_pan_zoom = self.graph_scene_rect;
        let mut scene_rect = self.graph_scene_rect;
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

                self.draw_graph_edges(ui, inv_zoom);

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

                self.draw_peek_preview(ui);
                deferred_overlays
            });
        self.graph_scene_rect = scene_rect;
        // Now that `self.graph_scene_rect` reflects this frame's actual
        // pan/zoom (not the stale value `project`/`overlay_painter` calls
        // inside the closure above had to work with -- see
        // `DeferredOverlays`'s own doc comment), recompute the transform
        // fresh and paint every decoration queued during that closure with
        // it, closing the one-frame gap between a card and its own
        // overlay-painted label/icons that was otherwise most visible
        // during a fast pan or zoom.
        let (zoom_scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
        let project = move |world: egui::Pos2| -> egui::Pos2 {
            (translation + zoom_scale * world.to_vec2()).to_pos2()
        };
        for overlay in scene_response.inner {
            overlay(&overlay_painter, &project);
        }
        if scene_rect != scene_rect_before_pan_zoom {
            self.pending_new_node = None;
        }

        self.track_pan_momentum(ui, scene_rect);

        self.handle_minimap_click(ui, minimap_rect, world_bounds, minimap_scale);

        for action in actions {
            self.apply_graph_action(ui.ctx(), action);
        }

        // Built last, now that `overlay_painter`'s node-decoration layer
        // above already exists this frame -- see this function's own note
        // by `shortcuts_help_button_rect`/`recently_closed_button_rect` for
        // why these can't be built back where those rects are computed.
        self.build_shortcuts_help(ui, shortcuts_help_button_rect);
        let recently_closed_fade = ui
            .ctx()
            .animate_bool(egui::Id::new("recently_closed_popup_fade"), self.recently_closed_open);
        if recently_closed_fade > 0.0 {
            if let Some(rect) = recently_closed_button_rect {
                self.build_recently_closed_popup(ui, rect, recently_closed_fade);
            }
        }
        self.build_node_menu(ui);
    }

    /// The Graph view's toolbar row: the group breadcrumb, "+ New Tab",
    /// the group and zoom chips, the filter, Recently Closed, the
    /// selection's actions, a spawn error and the universal chip. What its
    /// buttons ask for comes back as a `GraphToolbar`, which
    /// `build_graph_ui` applies at the same points it always did.
    pub(crate) fn render_graph_toolbar(&mut self, ui: &mut egui::Ui, groups_at_level: &[String]) -> GraphToolbar {
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
                    // Empty at the true top level; one entry per ancestor
                    // group otherwise (root-most first, current group
                    // last) -- groups nest now, so this is a real
                    // breadcrumb trail, not just "are we inside the one
                    // possible group or not." See `GroupPayload`'s own doc
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
                        let leave_group_response = ui
                            .button(egui_phosphor::light::CARET_LEFT)
                            .on_hover_text("Back one level");
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
                            // file). Truncated the same char-count way
                            // `json_summary` already bounds a JSON card's
                            // own label, for the same "small fixed chrome,
                            // not a place for arbitrarily long text" reason.
                            let display_name = if name.chars().count() > 20 {
                                format!("{}…", name.chars().take(19).collect::<String>())
                            } else {
                                name.clone()
                            };
                            if i + 1 == crumb_count {
                                let is_renaming =
                                    self.renaming_group.as_ref().is_some_and(|(rid, _)| rid == id);
                                if is_renaming {
                                    if let Some((_, draft)) = &mut self.renaming_group {
                                        let response = ui.add(
                                            egui::TextEdit::singleline(draft).desired_width(140.0),
                                        );
                                        if std::mem::take(&mut self.pending_focus_rename) {
                                            response.request_focus();
                                        }
                                        if response.lost_focus() {
                                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                                actions.push(GraphAction::Node(id.clone(), NodeAction::CancelRename));
                                            } else {
                                                actions.push(GraphAction::Node(id.clone(), NodeAction::CommitRename(draft.clone())));
                                            }
                                        }
                                    }
                                } else if ui
                                    .button(&display_name)
                                    .on_hover_text(if display_name == *name {
                                        "Click to rename".to_string()
                                    } else {
                                        format!("Click to rename ({name})")
                                    })
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
                                    .button(&display_name)
                                    .on_hover_text(if display_name == *name {
                                        "Jump to this group".to_string()
                                    } else {
                                        format!("Jump to this group ({name})")
                                    })
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
                    }
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
                    if ui.button("+ New Tab").clicked() {
                        actions.push(GraphAction::NewTab(self.focused.clone()));
                    }
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
                    // level, not just the top -- see `GroupPayload`'s own
                    // doc comment on nesting; each already acts on
                    // `self.current_group`/`nodes_at_current_level(Group)`
                    // (its own level), never hardcoding the top level.
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
                    if !self.headless || !groups_at_level.is_empty() {
                        toolbar_chip(ui, |ui| {
                            let mut bounds: Option<egui::Rect> = None;
                            let mut grow = |r: egui::Rect| {
                                bounds = Some(bounds.map_or(r, |b| b.union(r)));
                            };
                            if !self.headless {
                                let response = chip_icon_button(
                                    ui,
                                    egui::Id::new("icon_btn_import_group"),
                                    egui_phosphor::light::UPLOAD_SIMPLE,
                                )
                                .on_hover_text("Import Group...");
                                grow(response.rect);
                                if response.clicked() {
                                    actions.push(GraphAction::ImportGroup);
                                }
                            }
                            if !groups_at_level.is_empty() {
                                let response = chip_icon_button(
                                    ui,
                                    egui::Id::new("icon_btn_ungroup_all"),
                                    egui_phosphor::light::FOLDER_MINUS,
                                )
                                .on_hover_text("Ungroup All");
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
                                let response = chip_icon_button(ui, egui::Id::new("icon_btn_collapse_groups"), icon)
                                    .on_hover_text(hover);
                                grow(response.rect);
                                if response.clicked() {
                                    self.groups_collapsed = !self.groups_collapsed;
                                }
                            }
                            bounds.unwrap_or(ui.cursor())
                        });
                    }
                    // −/+/100%/Fit in one chip (`render_zoom_chip`, shared
                    // with Image view). Fit is the old "Reset View"
                    // (re-fit every node at this level); the `%` readout
                    // keeps the current pan and only resets the scale.
                    let zoom = self.graph_zoom_pct / 100.0;
                    if let Some(action) = render_zoom_chip(
                        ui,
                        self.graph_zoom_pct,
                        zoom > GRAPH_ZOOM_MIN + 1e-3,
                        zoom < GRAPH_ZOOM_MAX - 1e-3,
                        "Reset View (fit all)",
                    ) {
                        match action {
                            ZoomChipAction::Fit => reset_view = true,
                            ZoomChipAction::Reset100 => reset_zoom = true,
                            ZoomChipAction::In => graph_zoom_step = Some(ZOOM_STEP),
                            ZoomChipAction::Out => graph_zoom_step = Some(1.0 / ZOOM_STEP),
                        }
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.graph_filter)
                            .desired_width(120.0)
                            .hint_text("Filter..."),
                    );
                    // Only shown once there's actually something to clear --
                    // same `egui_phosphor::light::X` glyph every other close/
                    // dismiss affordance in this app already uses, so this
                    // reads as "clear" at a glance rather than a new symbol.
                    if !self.graph_filter.is_empty()
                        && ui.button(egui_phosphor::light::X).on_hover_text("Clear filter").clicked()
                    {
                        self.graph_filter.clear();
                    }
                    if !self.closed.is_empty() {
                        let label = format!("Recently Closed ({})", self.closed.len());
                        let response = ui.button(label);
                        if response.clicked() {
                            self.recently_closed_open = !self.recently_closed_open;
                        }
                        recently_closed_button_rect = Some(response.rect);
                    }
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
                        if ui
                            .button(format!("Close Selected ({})", self.selected_tabs.len()))
                            .clicked()
                        {
                            actions.push(GraphAction::CloseSelected);
                        }
                    }
                    // Cloned rather than borrowed -- `self.tab_spawn_error =
                    // None` below needs `&mut self`, which a live `&String`
                    // borrow from `self.tab_spawn_error` would conflict
                    // with; this only ever clones a short error string, and
                    // only while one is actually being shown.
                    if let Some(err) = self.tab_spawn_error.clone() {
                        ui.colored_label(DANGER_COLOR, &err);
                        // No dismiss before this -- an error already easy to
                        // miss in a packed row (now scrollable, see the
                        // `ScrollArea` above) had no way to clear itself
                        // short of a new spawn attempt overwriting it.
                        if ui.button(egui_phosphor::light::X).on_hover_text("Dismiss").clicked() {
                            self.tab_spawn_error = None;
                        }
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
    pub(crate) fn handle_graph_keys(&mut self, ui: &egui::Ui) {
        if self.renaming_group.is_none() && self.quick_switcher.is_none() {
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
            if let Some(dir) = dir {
                self.move_graph_focus(dir);
            }
            // Enter opens whatever arrow-key navigation landed on, the same
            // way clicking the node's card does.
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
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
            // no-op the rest of the time.
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.selected_tabs.clear();
                self.node_menu = None;
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
                // Reverse draw order, so the card on top takes the
                // right-click, the same card a left-click there would
                // reach. Within one kind, later in `node_order` draws
                // later, hence the second `.rev()`.
                let hit = GRAPH_DRAW_ORDER.into_iter().rev().find_map(|kind| {
                    self.nodes_at_current_level(kind)
                        .into_iter()
                        .rev()
                        .find(|id| self.nodes.get(id).is_some_and(|n| node_card_rect(n).contains(world_pos)))
                });
                if let Some(id) = hit {
                    self.node_menu = Some((id, pos));
                    self.rect_select_start = None;
                } else {
                    self.node_menu = None;
                    self.rect_select_start = Some(pos);
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
            overlay_painter.rect_stroke(
                screen_rect,
                0.0,
                egui::Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Inside,
            );
            if !secondary_down {
                self.rect_select_start = None;
                // Barely moved -- a plain right-click, not a real
                // rectangle-select drag. `nodes_at_current_level`
                // hit-testing above already ruled out landing on any real
                // node, so this is genuinely empty background: show the
                // "New Tab/Note/Group/File" card here instead of computing
                // a select-nothing zero-area marquee.
                if (current - start).length() < 4.0 {
                    let world_pos = ((start.to_vec2() - translation) / zoom_scale).to_pos2();
                    self.pending_new_node = Some(egui::Vec2::new(world_pos.x, world_pos.y));
                } else {
                    let world_rect = egui::Rect::from_two_pos(
                        ((screen_rect.min.to_vec2() - translation) / zoom_scale).to_pos2(),
                        ((screen_rect.max.to_vec2() - translation) / zoom_scale).to_pos2(),
                    );
                    self.selected_tabs = self
                        .nodes_at_current_level(ItemNodeKind::Tab)
                        .into_iter()
                        .filter(|id| {
                            self.nodes.get(id).is_some_and(|node| {
                                let card = egui::Rect::from_center_size(
                                    egui::Pos2::new(node.pos.x, node.pos.y),
                                    GRAPH_NODE_SIZE,
                                );
                                world_rect.intersects(card)
                            })
                        })
                        .collect();
                }
            }
        }
    }

    /// Eased camera move into/out of a group -- see `GraphCameraTween`'s
    /// doc comment for why this is a separate mechanism from
    /// `Transition`. Recomputed fresh from the tween's fixed `from`/`to`
    /// every frame (not derived from `self.graph_scene_rect`, which
    /// `build_graph_ui`'s `Scene::show` may also nudge from user pan/zoom input this
    /// same frame) so an in-progress user pan can't throw off where the
    /// animation is actually headed.
    pub(crate) fn advance_graph_camera(&mut self) {
        if let Some(tween) = self.graph_camera_tween {
            let elapsed = tween.start.elapsed();
            let t = (elapsed.as_secs_f32() / TRANSITION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
            let eased = t * t * (3.0 - 2.0 * t);
            self.graph_scene_rect = egui::Rect::from_min_max(
                tween.from.min + (tween.to.min - tween.from.min) * eased,
                tween.from.max + (tween.to.max - tween.from.max) * eased,
            );
            if elapsed >= TRANSITION_DURATION {
                self.graph_camera_tween = None;
            }
        } else if let Some(momentum) = self.pan_momentum {
            // Pan momentum -- see `PanMomentum`'s own doc comment.
            // Mutually exclusive with the tween above (both would
            // otherwise fight over `graph_scene_rect` the same frame).
            let elapsed = momentum.start.elapsed();
            let t = elapsed.as_secs_f32();
            let decay = (-PAN_MOMENTUM_DECAY * t).exp();
            let displacement =
                momentum.initial_velocity * ((1.0 - decay) / PAN_MOMENTUM_DECAY);
            let size = self.graph_scene_rect.size();
            self.graph_scene_rect =
                egui::Rect::from_center_size(momentum.start_center + displacement, size);
            if elapsed >= PAN_MOMENTUM_MAX_DURATION {
                self.pan_momentum = None;
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
    /// its world-to-minimap scale, for `handle_minimap_click`.
    pub(crate) fn draw_graph_minimap(
        &self,
        ui: &egui::Ui,
        overlay_painter: &egui::Painter,
        scene_outer_rect: egui::Rect,
        at_level: &[Vec<String>],
    ) -> (egui::Rect, egui::Rect, f32) {
        let minimap_size = egui::vec2(160.0, 110.0);
        let minimap_rect = egui::Rect::from_min_size(
            scene_outer_rect.right_bottom() - minimap_size - egui::Vec2::splat(12.0),
            minimap_size,
        );
        let world_bounds = self.bounds_for_level(
            at_level.iter().flatten(),
        );
        let minimap_scale = (minimap_rect.width() / world_bounds.width().max(1.0))
            .min(minimap_rect.height() / world_bounds.height().max(1.0));
        let world_to_minimap = |p: egui::Pos2| -> egui::Pos2 {
            minimap_rect.center() + (p - world_bounds.center()) * minimap_scale
        };
        overlay_painter.rect_filled(minimap_rect, 4.0, egui::Color32::from_black_alpha(160));
        for (kind, ids) in GRAPH_DRAW_ORDER.into_iter().zip(at_level) {
            let (radius, color) = match kind {
                ItemNodeKind::Tab => (2.0, ui.visuals().text_color()),
                ItemNodeKind::FailedTab => (2.0, DANGER_COLOR),
                ItemNodeKind::Download => (2.0, ui.visuals().weak_text_color()),
                ItemNodeKind::Note => (2.0, NOTE_CARD_COLOR),
                ItemNodeKind::Image | ItemNodeKind::Pdf | ItemNodeKind::Json => (2.0, ACCENT),
                ItemNodeKind::Group => (3.0, ACCENT),
            };
            for id in ids {
                if let Some(node) = self.nodes.get(id) {
                    overlay_painter.circle_filled(world_to_minimap(node.pos.to_pos2()), radius, color);
                }
            }
        }
        let viewport_rect = egui::Rect::from_two_pos(
            world_to_minimap(self.graph_scene_rect.min),
            world_to_minimap(self.graph_scene_rect.max),
        )
        .intersect(minimap_rect);
        overlay_painter.rect_stroke(
            viewport_rect,
            0.0,
            egui::Stroke::new(1.5, ACCENT),
            egui::StrokeKind::Inside,
        );
        overlay_painter.rect_stroke(
            minimap_rect,
            4.0,
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
            egui::StrokeKind::Inside,
        );
        (minimap_rect, world_bounds, minimap_scale)
    }

    /// Pan momentum bookkeeping -- see `PanMomentum`'s own doc
    /// comment. `actively_panning` requires the pointer to still be
    /// held down (ruling out a single-frame minimap-click jump, which
    /// never has "held down across multiple frames" to sample a
    /// velocity from), no node currently grabbed (a node drag moves
    /// the node, not the camera, so it'd contribute a bogus near-zero
    /// sample anyway, but this is the explicit, intended guard), and
    /// no `GraphCameraTween` in progress (that already owns
    /// `graph_scene_rect` this frame).
    pub(crate) fn track_pan_momentum(&mut self, ui: &egui::Ui, scene_rect: egui::Rect) {
        let pointer_down = ui.input(|i| i.pointer.primary_down());
        let actively_panning =
            pointer_down && self.dragging.is_none() && self.graph_camera_tween.is_none();
        if actively_panning {
            // A fresh grab of the background cancels any momentum still
            // gliding from an earlier release -- "grab it to stop it".
            self.pan_momentum = None;
            self.pan_drag_history.push_back((std::time::Instant::now(), scene_rect.center()));
            while self.pan_drag_history.len() > PAN_HISTORY_LEN {
                self.pan_drag_history.pop_front();
            }
        } else {
            if self.pan_momentum.is_none() && ui.input(|i| i.pointer.primary_released()) {
                if let (Some(&(t0, p0)), Some(&(t1, p1))) =
                    (self.pan_drag_history.front(), self.pan_drag_history.back())
                {
                    let dt = (t1 - t0).as_secs_f32();
                    if dt > 0.01 {
                        let velocity = (p1 - p0) / dt;
                        if velocity.length() >= PAN_MOMENTUM_MIN_SPEED {
                            self.pan_momentum = Some(PanMomentum {
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
            self.pan_drag_history.clear();
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
                let world_click =
                    world_bounds.center() + (click_pos - minimap_rect.center()) / minimap_scale;
                self.graph_scene_rect =
                    egui::Rect::from_center_size(world_click, self.graph_scene_rect.size());
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
    /// regardless of zoom. Reads `self.graph_scene_rect` (the
    /// previous frame's visible world rect) rather than the
    /// `build_graph_ui`'s local `scene_rect` -- that one is already mutably
    /// borrowed for the whole `show` call, including this
    /// closure, so it can't also be read from in here. One frame
    /// of lag, same as `graph_zoom_pct`'s own toolbar readout --
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
        let visible = self.graph_scene_rect;
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

    /// Edges first so node cards paint on top of the lines feeding them.
    /// Both endpoints must share the currently-viewed level —
    /// see the physics block's own edge filter above for why.
    /// One pass over every node kind (tabs, downloads, ...), so
    /// each parent->child edge is drawn exactly once.
    pub(crate) fn draw_graph_edges(&self, ui: &egui::Ui, inv_zoom: f32) {
        for node in self.nodes.values() {
            if node.group != self.current_group {
                continue;
            }
            let Some(parent_id) = &node.parent else {
                continue;
            };
            let Some(parent) = self.nodes.get(parent_id).filter(|p| p.group == self.current_group) else {
                continue;
            };
            let parent_pos = egui::Pos2::new(parent.pos.x, parent.pos.y);
            let node_pos = egui::Pos2::new(node.pos.x, node.pos.y);
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
            if egui::Rect::from_two_pos(parent_pos, node_pos).intersects(self.graph_scene_rect) {
                draw_graph_edge(
                    ui.painter(),
                    parent_pos,
                    node_pos,
                    egui::Stroke::new(2.0 * inv_zoom, ui.visuals().weak_text_color()),
                );
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
        // kind by kind in `GRAPH_DRAW_ORDER` (Group last), which is
        // also their stacking order: a later kind draws, and takes
        // the pointer, on top of an earlier one. Each kind's own
        // click action is applied here.
        let mut pass = CardPass {
            deferred_overlays,
            actions,
            inv_zoom,
            zoom_scale,
            drop_preview: &drop_preview,
        };
        for (&kind, ids) in GRAPH_DRAW_ORDER.iter().zip(at_level.iter()) {
            for id in ids.iter() {
                // Read before the frame: a Note's `TextEdit` can lose
                // focus to a click on its own header this very frame,
                // clearing `editing`, and that click must not
                // immediately re-enter editing.
                let note_was_editing = kind == ItemNodeKind::Note
                    && self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);
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
                // A placeholder (see `ImagePayload::error`) has nothing
                // to open; a click tries its file again instead.
                let missing_file = self.nodes.get(id).is_some_and(|n| {
                    n.as_image().is_some_and(|i| i.error.is_some())
                        || n.as_failed_tab().is_some()
                        || n.as_json().is_some_and(|j| j.error.is_some())
                        || n.as_pdf().is_some_and(|p| matches!(p.outcome, Some(Err(_))))
                });
                match kind {
                    _ if missing_file => {
                        if card.response.clicked() {
                            pass.actions.push(GraphAction::Node(id.clone(), NodeAction::RetryMissingFile));
                        }
                    }
                    // Opens the tab into Tab view.
                    ItemNodeKind::Tab if card.response.clicked() => {
                        pass.actions.push(GraphAction::Node(id.clone(), NodeAction::Open))
                    }
                    // Click (not drag -- egui doesn't report
                    // `clicked()` for a drag) reveals the file.
                    ItemNodeKind::Download if card.response.clicked() => pass.actions.push(GraphAction::Node(id.clone(), NodeAction::Reveal)),
                    // Click on the rendered note enters editing in place.
                    ItemNodeKind::Note if card.response.clicked() && !note_was_editing => {
                        if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                            note.editing = true;
                        }
                    }
                    // Maximizes the image into Item view, the same
                    // gesture a Tab card uses.
                    ItemNodeKind::Image if card.response.clicked() => pass.actions.push(GraphAction::Node(id.clone(), NodeAction::Open)),
                    // Opens the full tree in `build_json_viewer`.
                    ItemNodeKind::Json if card.response.double_clicked() => {
                        self.json_viewer_node = Some(id.clone())
                    }
                    // Drills into the group's own subgraph.
                    ItemNodeKind::Group if card.response.double_clicked() => {
                        pass.actions.push(GraphAction::EnterGroup(id.clone()))
                    }
                    _ => {}
                }
            }
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
    /// thumbnail, floating near it, without changing focus. Painted
    /// directly in this same world-space layer (not a separate
    /// `egui::Area`, which would paint in un-transformed screen
    /// space) so it pans/zooms together with its node and simply
    /// ends up on top by paint order, drawn after every card
    /// (`draw_graph_cards`). Skipped while `peek_suppressed` matches -- an
    /// Escape press dismissing this exact node's peek early,
    /// see that field's own doc comment.
    pub(crate) fn draw_peek_preview(&mut self, ui: &mut egui::Ui) {
        if self.peek_suppressed.is_some() && self.peek_suppressed != self.hovered {
            self.peek_suppressed = None;
        }
        let peek_target =
            self.hovered.clone().filter(|id| self.peek_suppressed.as_deref() != Some(id.as_str()));
        // Polled every frame regardless of `peek_target`, same
        // "continuous polling, not just while shown" reasoning the
        // dropdown popups' own fade needs -- a plain `if let Some`
        // gate here would only ever call `animate_bool` while a
        // peek is actually showing, and the real-elapsed-time gap
        // since the last poll (however long the pointer sat
        // elsewhere) would make it jump straight to fully visible
        // the instant it reappears instead of visibly fading in.
        let peek_fade =
            ui.ctx().animate_bool(egui::Id::new("peek_preview_fade"), peek_target.is_some());
        if peek_fade > 0.0 {
            if let Some(hovered_id) = peek_target {
                let node_pos = self.nodes.get(&hovered_id).map(|n| n.pos);
                let thumb_id = self.gpu.as_ref().and_then(|g| g.thumbnail_id(&hovered_id));
                if let (Some(node_pos), Some(tex_id)) = (node_pos, thumb_id) {
                    let rect = egui::Rect::from_min_size(
                        egui::Pos2::new(
                            node_pos.x + GRAPH_NODE_SIZE.x / 2.0 + 12.0,
                            node_pos.y - GRAPH_PEEK_SIZE.y / 2.0,
                        ),
                        GRAPH_PEEK_SIZE,
                    );
                    ui.set_opacity(peek_fade);
                    ui.painter()
                        .rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
                    ui.painter().image(
                        tex_id,
                        rect,
                        egui::Rect::from_min_max(
                            egui::Pos2::new(0.0, 0.0),
                            egui::Pos2::new(1.0, 1.0),
                        ),
                        egui::Color32::WHITE,
                    );
                    ui.painter().rect_stroke(
                        rect,
                        6.0,
                        egui::Stroke::new(1.5, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
    }
}
