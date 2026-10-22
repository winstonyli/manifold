//! The Graph view's camera: its transform, easing, pan momentum, zoom requests and minimap, and the level-of-detail count.

use super::*;

impl GraphCamera {
    pub(crate) fn new() -> Self {
        Self {
            rect: default_graph_scene_rect(),
            zoom_pct: 100.0,
            tween: None,
            momentum: None,
            drag_history: std::collections::VecDeque::new(),
            frame_restored: false,
            strip_tab: None,
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

    /// Replicates `egui::Scene`'s own internal `fit_to_rect_in_scene`
    /// (private to egui, not part of its public API) — needed to compute
    /// where a graph node actually lands on screen *outside* the `Scene`
    /// widget's own closure, for the shrink/expand animation's target
    /// rect. `(scale, translation)` such that
    /// `screen = translation + scale * world`, matching `Scene`'s own
    /// "fit the visible scene rect into the available screen area,
    /// preserving aspect ratio, clamped to the configured zoom range"
    /// behavior exactly (using the *same* `rect` `Scene` itself
    /// reads/writes) — verified live by screenshotting a
    /// mid-animation frame and confirming the content quad visually lines
    /// up with the target node's real on-screen position.
    pub(crate) fn transform(&self, outer: egui::Rect) -> (f32, egui::Vec2) {
        let scale = (outer.size() / self.rect.size()).min_elem().clamp(GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        (scale, outer.center().to_vec2() - scale * self.rect.center().to_vec2())
    }

    /// A frame's camera work before its `Scene`: advance any tween or glide, keep repainting while one runs, fill
    /// `rect` to `outer`'s shape (`fill`) and set the zoom readout. Returns `transform(outer)`.
    pub(crate) fn begin_frame(&mut self, ctx: &egui::Context, outer: egui::Rect) -> (f32, egui::Vec2) {
        self.advance();
        if self.tween.is_some() || self.momentum.is_some() {
            ctx.request_repaint();
        }
        self.fill(outer);
        let transform = self.transform(outer);
        self.zoom_pct = transform.0 * 100.0;
        transform
    }

    /// Eases the camera so `card` is in view, centred on it when it isn't already fully visible.
    pub(crate) fn pan_to(&mut self, card: egui::Rect) {
        if !self.rect.contains_rect(card) {
            self.ease_to(egui::Rect::from_center_size(card.center(), self.rect.size()));
        }
    }
}

/// The graph's `Scene`: its zoom range, and the primary button pans unless `drag_pan` is off (while a link retargets).
pub(crate) fn show_scene<R>(
    ui: &mut egui::Ui,
    rect: &mut egui::Rect,
    drag_pan: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Scene::new()
        .zoom_range(GRAPH_ZOOM_MIN..=GRAPH_ZOOM_MAX)
        .drag_pan_buttons(if drag_pan { egui::DragPanButtons::PRIMARY } else { egui::DragPanButtons::empty() })
        .show(ui, rect, body)
}

/// Initial visible world-space rect for the graph's `egui::Scene`, before
/// any user panning/zooming — generous enough to show a handful of settled
/// nodes around `GRAPH_WORLD_CENTER` at 1:1 zoom.
const GRAPH_SCENE_DEFAULT_SIZE: egui::Vec2 = egui::vec2(900.0, 620.0);

pub(crate) fn default_graph_scene_rect() -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(0.0, 0.0), GRAPH_SCENE_DEFAULT_SIZE)
}

impl App {
    /// Where a world-space rect lands on screen in Graph view right now --
    /// the same projection `Scene` applies, computed outside it (see
    /// `GraphCamera::transform`).
    pub(crate) fn graph_world_rect_to_screen(&self, world: egui::Rect) -> egui::Rect {
        let (scale, translation) = self.camera.transform(self.graph_scene_outer_rect());
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
        let (scale, translation) = self.camera.transform(self.graph_scene_outer_rect());
        (point.to_vec2() - translation) / scale
    }

    /// The Graph view's scene area in window points: below the toolbar,
    /// minus a JSON level's outline on the right (plan ruling 12). The one
    /// definition both screen<->world conversions use; `build_graph_ui`
    /// reads the live layout rect instead, which this mirrors for code
    /// outside the frame.
    pub(crate) fn graph_scene_outer_rect(&self) -> egui::Rect {
        let screen = self.screen_size_points();
        let outline = if self.json_level.is_some() && self.json_ui.json_outline_visible {
            self.json_ui.json_outline_width
        } else {
            0.0
        };
        egui::Rect::from_min_size(
            egui::pos2(0.0, GRAPH_TOOLBAR_HEIGHT_POINTS),
            egui::vec2((screen.x - outline).max(0.0), (screen.y - GRAPH_TOOLBAR_HEIGHT_POINTS).max(0.0)),
        )
    }

    /// Flips `graph_detail` with hysteresis from how many of `items` are in view.
    pub(super) fn update_graph_lod(&mut self, items: &[CardEntry]) {
        // Level-of-detail: count how many nodes at this level actually
        // intersect the viewport, then step `graph_detail` with `Detail::next`'s
        // hysteresis gap (see `LOD_SIMPLIFY_COUNT`'s doc comment). Uses this frame's not-yet-updated `camera.rect`, the
        // same one-frame-stale viewport the render loops' own cull check
        // already accepts.
        let lod_visible_count: usize =
            items.iter().filter(|item| item.caps().framed && item.rect.intersects(self.camera.rect)).count();
        self.graph_detail = self.graph_detail.next(lod_visible_count);
    }

    /// Applies a pending frame-restore and the toolbar's Fit request to the camera.
    pub(super) fn apply_camera_requests(&mut self, reset_view: bool) {
        // See `GraphCamera::frame_restored`.
        if self.camera.frame_restored && self.spawns.tab_spawns_in_flight == 0 {
            self.camera.frame_restored = false;
            // Settled first, as in `switch_group_level`: fitting the
            // pre-settle positions would leave spread-out nodes off screen.
            self.layout_current_level();
            self.graph_settled_key = Some(self.current_level_settle_key());
            self.camera.jump_to(self.fit_all_nodes_scene_rect());
        }
        if reset_view {
            self.camera.jump_to(self.fit_all_nodes_scene_rect());
            self.interaction.pending_new_node = None;
        }
        self.frame_new_strip();
    }

    /// Re-lays-out the level when its node set changed size, unless a drag is in progress.
    pub(super) fn settle_level_if_changed(&mut self, item_count: usize) {
        if self.interaction.dragging.is_none() {
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
    pub(super) fn apply_zoom_requests(
        &mut self,
        scene_outer_rect: egui::Rect,
        reset_zoom: bool,
        graph_zoom_step: Option<f32>,
    ) {
        if reset_zoom {
            // Keeps the current pan (unlike "Reset View", which re-fits
            // everything) -- `GraphCamera::transform` derives
            // `zoom_scale` as `screen_size / camera.rect.size()`
            // (`.min_elem()` of the two axis ratios), so matching the world
            // rect's size to the screen's own size on both axes makes that
            // ratio exactly 1.0, the definition of "100%" this readout uses.
            let center = self.camera.rect.center();
            self.camera.rect = egui::Rect::from_center_size(center, scene_outer_rect.size());
        }
        if let Some(scale) = self.pending_graph_zoom.take() {
            let scale = scale.clamp(GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
            self.camera.rect = egui::Rect::from_center_size(self.camera.rect.center(), scene_outer_rect.size() / scale);
        }
        if let Some(factor) = graph_zoom_step {
            self.camera.rect =
                step_scene_rect(self.camera.rect, factor, scene_outer_rect.size(), GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
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
        items: &[CardEntry],
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
            let (radius, color) = match item.kind.ty {
                ty if is_container(ty) => (3.0, ACCENT),
                TYPE_TAB => (2.0, ui.visuals().text_color()),
                TYPE_DOWNLOAD => (2.0, ui.visuals().weak_text_color()),
                TYPE_NOTE => (2.0, NOTE_CARD_COLOR),
                // Image, PDF and JSON cards, a JSON level's cards and the virtual ones; a type this build doesn't
                // know draws as one of them.
                _ => (2.0, ACCENT),
            };
            let failed = self
                .nodes
                .get(&item.id.to_string())
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
    /// sample anyway, but this is the explicit, intended guard), no
    /// connect-drag (the camera stays put, but a wheel zoom mid-drag moves
    /// its centre, which would start a glide on release; the press's
    /// "grab it to stop it" is `start_connect_drag`'s), and
    /// no `GraphCameraTween` in progress (that already owns
    /// `camera.rect` this frame).
    pub(crate) fn track_pan_momentum(&mut self, ui: &egui::Ui, scene_rect: egui::Rect) {
        let pointer_down = ui.input(|i| i.pointer.primary_down());
        let actively_panning = pointer_down
            && self.interaction.dragging.is_none()
            && self.link.connect_drag.is_none()
            && self.camera.tween.is_none();
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
                self.interaction.pending_new_node = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_camera_transform_maps_its_rect_onto_the_outer_rect() {
        let mut camera = GraphCamera::new();
        camera.rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 200.0));
        let outer = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(800.0, 400.0));
        let (scale, translation) = camera.transform(outer);
        assert_eq!(scale, 2.0);
        let screen = translation + scale * camera.rect.center().to_vec2();
        assert_eq!(screen, outer.center().to_vec2());
        camera.rect = egui::Rect::from_center_size(egui::Pos2::ZERO, egui::vec2(8.0, 4.0));
        assert_eq!(camera.transform(outer).0, GRAPH_ZOOM_MAX, "the zoom range clamps");
    }

    #[test]
    fn begin_frame_fills_the_rect_to_the_outer_shape_and_sets_the_readout() {
        let ctx = egui::Context::default();
        let mut camera = GraphCamera::new();
        camera.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        let outer = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
        let (scale, _) = camera.begin_frame(&ctx, outer);
        assert_eq!(scale, 2.0);
        assert_eq!(camera.rect.size(), egui::vec2(200.0, 100.0));
        assert_eq!(camera.zoom_pct, 200.0);
    }

    #[test]
    fn pan_to_eases_only_when_the_card_is_not_fully_in_view() {
        let mut camera = GraphCamera::new();
        camera.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        camera.pan_to(egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 20.0)));
        assert!(camera.tween.is_none(), "already visible");
        let far = egui::Rect::from_min_size(egui::pos2(500.0, 0.0), egui::vec2(20.0, 20.0));
        camera.pan_to(far);
        let tween = camera.tween.expect("off screen: eases");
        assert_eq!(tween.to, egui::Rect::from_center_size(far.center(), camera.rect.size()));
    }
}
