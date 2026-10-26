//! The Graph view's scene body: the frame build, its popups, the edges and the dot grid.

use super::*;
use crate::store::model::NodeId;

/// Edges first so node cards paint on top of the lines feeding them: every edge `level_edges` lists once, each
/// parent->child edge solid and each Link node dashed in the accent colour, both as the same bowed curve between
/// facing card sides (`paint_bow`). Both endpoints must share the currently-viewed level. A hovered link line
/// (`highlight`) is drawn twice as thick. Edges outside the world rect `viewport` are skipped; the trail popup passes
/// its own camera's.
pub(crate) fn paint_edges(ui: &egui::Ui, viewport: egui::Rect, edges: &[LevelEdge], inv_zoom: f32) {
    for edge in edges {
        // Viewport culling: unlike a node's own rect, an edge
        // can have one endpoint off-screen and the other on --
        // or pass through the viewport with both endpoints
        // outside it -- so this checks the bounding box of the
        // whole curve (its control point included) against the
        // viewport, not either endpoint alone. Not pixel-exact
        // for a steep diagonal that grazes past a corner of its
        // own bounding box without actually crossing the
        // viewport, but that's a rare, cheap-to-tolerate false
        // negative, not a visible edge going missing.
        let bounds = match edge.curve {
            EdgeCurve::Bow => crate::link_gestures::bow_bounds(edge.from, edge.to),
            EdgeCurve::Horizontal => egui::Rect::from_two_pos(edge.from, edge.to),
        };
        if !bounds.intersects(viewport) {
            continue;
        }
        let stroke = match edge.kind {
            EdgeKind::Link => {
                let width = if edge.highlight {
                    crate::link_gestures::LINK_HOVER_STROKE_PX
                } else {
                    crate::link_gestures::LINK_STROKE_PX
                };
                egui::Stroke::new(width * inv_zoom, accent(ui.visuals()))
            }
            EdgeKind::Jar => egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color().gamma_multiply(0.6)),
            EdgeKind::Opened => crate::level::opened_edge_stroke(
                edge.highlight,
                inv_zoom,
                accent(ui.visuals()),
                ui.visuals().weak_text_color().gamma_multiply(0.6),
            ),
        };
        match edge.curve {
            EdgeCurve::Bow => {
                let dash = matches!(edge.kind, EdgeKind::Link | EdgeKind::Jar).then_some(inv_zoom);
                crate::link_gestures::paint_bow(ui.painter(), edge.from, edge.to, stroke, dash);
            }
            EdgeCurve::Horizontal => crate::level::paint_horizontal_edge(ui.painter(), edge.from, edge.to, stroke),
        }
    }
}

/// The graph's dot grid over the world rect `visible` (the camera's, or the trail popup's own), in `painter`'s own
/// coordinates. Drawn first, under everything: a fixed-world-spacing reference so panning and zooming read as moving
/// through a space, as in every node-graph editor. Spacing is in world units (farther apart on screen when zoomed in)
/// while the dot's radius is scaled by `inv_zoom` to stay a constant ~1.5 screen px. It is adaptive (doubled or halved,
/// never below a 20 screen-px floor) rather than fixed at 40 world units, which at `GRAPH_ZOOM_MIN` would be tens of
/// thousands of `circle_filled` calls a frame.
pub(crate) fn paint_dot_grid(
    painter: &egui::Painter,
    visible: egui::Rect,
    zoom_scale: f32,
    inv_zoom: f32,
    weak: egui::Color32,
) {
    let mut grid_spacing = 40.0_f32;
    while grid_spacing * zoom_scale < 20.0 {
        grid_spacing *= 2.0;
    }
    while grid_spacing * zoom_scale > 80.0 {
        grid_spacing /= 2.0;
    }
    let dot_color = weak.gamma_multiply(0.55);
    let start_x = (visible.min.x / grid_spacing).floor() as i32;
    let end_x = (visible.max.x / grid_spacing).ceil() as i32;
    let start_y = (visible.min.y / grid_spacing).floor() as i32;
    let end_y = (visible.max.y / grid_spacing).ceil() as i32;
    for gx in start_x..=end_x {
        for gy in start_y..=end_y {
            painter.circle_filled(
                egui::pos2(gx as f32 * grid_spacing, gy as f32 * grid_spacing),
                1.5 * inv_zoom,
                dot_color,
            );
        }
    }
}

impl App {
    /// Switches to `id`'s level, as `switch_group_level` does, unless it is
    /// already the one shown. For jumps straight to a tab (quick switcher,
    /// the view toggle): the camera would otherwise keep the old level's
    /// framing and show the new one's nodes off screen.
    pub(crate) fn show_level_of(&mut self, id: NodeId) {
        let group = self.nodes.get(&id).and(self.doc.mirror.group(id));
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
    pub(crate) fn switch_group_level(&mut self, group: Option<NodeId>) {
        self.end_note_edits();
        self.current_group = group;
        self.camera.reveal = None;
        self.end_stale_drag(true);
        self.cancel_connect();
        self.link.line_press = None;
        self.sync_json_level();
        self.layout_current_level();
        self.graph_settled_key = Some(self.current_level_settle_key());
        self.camera.ease_to(self.fit_all_nodes_scene_rect());
        self.interaction.selected.clear();
        self.interaction.rect_select_start = None;
        self.interaction.pending_new_node = None;
        self.menus.node_menu.close();
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
        self.settle_level_if_changed(crate::level::settle_count(&items));

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
        // same pair `GraphCamera::transform` already derives for
        // the Tab<->Graph transition animation (`screen = translation +
        // zoom_scale * world`). The node card itself (`GRAPH_NODE_SIZE`)
        // and the hover peek preview (`GRAPH_PEEK_SIZE`) are deliberately
        // excluded from all of this — those represent actual zoomable
        // content, not chrome, so they're meant to scale (and blur like
        // any other magnified image would, which is normal/expected).
        self.build_right_panel(ui, &mut actions);
        let scene_outer_rect = ui.available_rect_before_wrap();
        self.apply_zoom_requests(scene_outer_rect, reset_zoom, graph_zoom_step);
        // Advance first: everything below (strokes, `zoom_pct`, the new-node
        // menu) must use the rect the Scene draws this frame.
        let (zoom_scale, translation) = self.camera.begin_frame(ui.ctx(), scene_outer_rect);
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
        // The background's primary drag pans, except while it retargets a link (the drag began on a curve).
        let scene_response = show_scene(ui, &mut scene_rect, self.link.retarget.is_none(), |ui| {
            // Every `overlay_painter` decoration painted inside this
            // closure is deferred rather than painted immediately -- see
            // `DeferredOverlays`'s own doc comment for why (this frame's
            // real pan/zoom isn't reflected in `project`, captured
            // above, until after this whole closure returns).
            let mut deferred_overlays: DeferredOverlays = DeferredOverlays::default();
            paint_dot_grid(ui.painter(), self.camera.rect, zoom_scale, inv_zoom, ui.visuals().weak_text_color());

            // The link line under the pointer (link-gestures spec B) is drawn highlighted; a click on it is
            // applied after the Scene, from the Scene's own response.
            // Rects read afresh (a drag or settle since `items` moved cards), the layer scan reused.
            let mut edges = self.level_edges_from(&self.level_items_from(&at_level));
            let minimap_rect = minimap.map(|(rect, _, _)| rect);
            let line_hover = self.hovered_link_line(ui, &edges, inv_zoom, minimap_rect, typing);
            for edge in &mut edges {
                edge.highlight |= edge.link.is_some() && edge.link == line_hover;
            }
            // The link being retargeted is not drawn: its rubber band stands for it.
            crate::link_gestures::without_retargeted(&mut edges, self.link.retarget.as_ref());
            paint_edges(ui, self.camera.rect, &edges, inv_zoom);

            if self.json_level.is_some() {
                self.draw_json_level(ui, &mut deferred_overlays, &mut json_edits, inv_zoom, zoom_scale);
            } else {
                // The card a release now would link to, ringed bright: the release below reads the same function.
                let connect_end = self.connect_drag_end(
                    ui.ctx(),
                    ui.input(|i| i.pointer.interact_pos()),
                    scene_outer_rect,
                    minimap.map(|(rect, _, _)| rect),
                    (zoom_scale, translation),
                );
                // For a dragged link end, what the release there would do: the band and ring show it.
                self.link.retarget_preview = self.retarget_preview(&connect_end);
                let connect_target = match connect_end {
                    crate::link_gestures::DragEnd::Card(target) => Some(target),
                    _ => None,
                };
                self.draw_graph_cards(
                    ui,
                    &mut deferred_overlays,
                    &mut actions,
                    &at_level,
                    inv_zoom,
                    zoom_scale,
                    leave_group_button_rect,
                    connect_target,
                );
                self.draw_pending_new_node_menu(ui, &mut actions, &project);
                self.draw_peek_preview(ui, &mut deferred_overlays);
            }
            (deferred_overlays, line_hover)
        });
        let egui::InnerResponse { inner: (deferred_overlays, line_hover), response: scene_background } = scene_response;
        self.camera.rect = scene_rect;
        // A press on a link curve that has become a drag retargets the link instead of panning (undoing the pan
        // this frame's drag delta applied).
        if scene_background.drag_started() && self.start_line_retarget(ui, scene_outer_rect) {
            scene_rect = self.camera.rect;
        }
        // Now that `self.camera.rect` reflects this frame's actual
        // pan/zoom (not the stale value `project`/`overlay_painter` calls
        // inside the closure above had to work with -- see
        // `DeferredOverlays`'s own doc comment), recompute the transform
        // fresh and paint every decoration queued during that closure with
        // it, closing the one-frame gap between a card and its own
        // overlay-painted label/icons that was otherwise most visible
        // during a fast pan or zoom.
        let (zoom_scale, translation) = self.camera.transform(scene_outer_rect);
        let project = move |world: egui::Pos2| -> egui::Pos2 { (translation + zoom_scale * world.to_vec2()).to_pos2() };
        deferred_overlays.play(&overlay_painter, &project);
        self.draw_selection_bar(ui, &mut actions, scene_outer_rect, &project);
        self.draw_suggestions(ui, scene_outer_rect, &project);
        if scene_rect != scene_rect_before_pan_zoom {
            self.interaction.pending_new_node = None;
        }

        // A click on a link line deletes that link (link-gestures spec B). The Scene's background took the click, so
        // no card, control or popup was on top, and egui never reports a pan (a drag) as a click. Read before
        // `track_pan_momentum`, which clears a glide on the press that `line_click` must still see.
        // The same click on empty canvas, with no line under it, deselects (`canvas_click_deselects`). Read before
        // `line_click` and `track_pan_momentum`, which end a glide.
        let deselect = self.canvas_click_deselects(
            scene_background.clicked(),
            line_hover.is_some(),
            ui.input(|i| i.pointer.interact_pos()),
            minimap.map(|(rect, _, _)| rect),
            typing,
        );
        let line_click = self.line_click(ui, line_hover, scene_background.clicked());
        if deselect {
            self.interaction.selected.clear();
        }

        self.track_pan_momentum(ui, scene_rect);

        // A connect-drag ends when the button comes up (link-gestures spec A). Its target is the card under the
        // pointer, unlike a card drop's (the dragged card's centre), in the scene and off the minimap: the one the
        // ring showed, through this frame's final transform. It cancels instead when the release is a click (no
        // movement: the release is on the pressed handle, whose outer half may lie over a neighbour), or when the
        // button is up with no release this frame (its button-up was lost), so a later ordinary click never links.
        let (down, released, clicked) =
            ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_released(), i.pointer.primary_clicked()));
        if self.link.connect_drag.is_some() && !down {
            let end = if released && !clicked {
                self.connect_drag_end(
                    ui.ctx(),
                    ui.input(|i| i.pointer.interact_pos()),
                    scene_outer_rect,
                    minimap.map(|(rect, _, _)| rect),
                    (zoom_scale, translation),
                )
            } else {
                crate::link_gestures::DragEnd::Abandoned
            };
            self.end_connect_drag(end);
        }

        if let Some((minimap_rect, world_bounds, minimap_scale)) = minimap {
            self.handle_minimap_click(ui, minimap_rect, world_bounds, minimap_scale);
        }

        if let Some(link) = line_click {
            self.click_link_line(link);
        }

        // Before the actions, so a note they make has its full frames.
        if let Some((_, frames)) = &mut self.interaction.focus_new_note {
            *frames -= 1;
            if *frames == 0 {
                self.interaction.focus_new_note = None;
            }
        }
        for action in actions {
            self.apply_graph_action(ui.ctx(), action);
        }
        // Pick-target mode: a click in the graph that didn't finish it above (empty canvas, a card that isn't a
        // target, the minimap) ends it. A pan is a drag, not a click, so the user can still go looking for a target.
        // The node menu that starts the mode is built after this, so the click choosing its verb doesn't count.
        if self.link.connecting.is_some()
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

    /// Popups and menus built last, after the node-decoration layer exists this frame.
    pub(super) fn build_graph_popups(
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
        self.build_trail_menu(ui);
    }
}
