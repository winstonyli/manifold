//! The Graph view: the scene body (cards, edges, the dot grid, the new-node menu and the hover peek) and the
//! types it shares. The toolbar, camera, keyboard and hints are the children below.

use super::*;
use crate::store::model::{TYPE_DOWNLOAD, TYPE_IMAGE, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB};

mod camera;
mod hints;
mod keys;
mod toolbar;

pub(crate) use camera::*;
use hints::*;
pub(crate) use keys::*;
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
    pub(crate) strip_tab: Option<String>,
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
                // Only the selected cards still shown: a tab mid-crash-respawn or mid-engine-swap has no card, so it
                // is neither counted in the centroid nor moved (its selection is consumed with the rest).
                let live: Vec<String> =
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
                let pos = live.iter().fold(egui::Vec2::ZERO, |acc, t| acc + self.layout_pos(t)) / live.len() as f32;
                let name = self.default_group_name();
                let at = self.placement(&id, Some(pos));
                self.place_group(Some(id.clone()), &name, at);
                for tab_id in live {
                    self.edit_group(&tab_id, Some(id.clone()));
                }
            }
            GraphAction::CloseSelected => {
                let ids = self.take_selected_stored();
                self.begin_tabs_shutdown(&ids);
                // Outermost first (then by id, so the order is the same on every run, not the set's): a closed group
                // hides its whole subtree, so a selected member inside it is already gone at its turn and is
                // skipped, rather than given a closed root of its own beside the group's.
                let mut ids: Vec<String> = ids.into_iter().collect();
                ids.sort_by_cached_key(|id| (self.group_depth(id), id.clone()));
                for id in ids {
                    if self.is_linked(&id) {
                        self.close_node(&id);
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
        let group = self.nodes.get(id).and(self.doc.mirror.group(id)).map(str::to_owned);
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
        self.link.line_press = None;
        self.sync_json_level();
        self.layout_current_level();
        self.graph_settled_key = Some(self.current_level_settle_key());
        self.camera.ease_to(self.fit_all_nodes_scene_rect());
        self.interaction.selected.clear();
        self.interaction.rect_select_start = None;
        self.interaction.pending_new_node = None;
        self.menus.node_menu = None;
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
        self.build_json_outline(ui);
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
            let mut deferred_overlays: DeferredOverlays = Vec::new();
            paint_dot_grid(ui.painter(), self.camera.rect, zoom_scale, inv_zoom, ui.visuals().weak_text_color());

            // The link line under the pointer (link-gestures spec B) is drawn highlighted; a click on it is
            // applied after the Scene, from the Scene's own response.
            let mut edges = self.level_edges();
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
                    connect_target.as_deref(),
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
        for overlay in deferred_overlays {
            overlay(&overlay_painter, &project);
        }
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
            self.click_link_line(&link);
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
        self.build_trail_menu(ui);
    }
}

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
                egui::Stroke::new(width * inv_zoom, ACCENT)
            }
            EdgeKind::Jar => egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color().gamma_multiply(0.6)),
            EdgeKind::Opened => {
                crate::level::opened_edge_stroke(edge.highlight, inv_zoom, ui.visuals().weak_text_color())
            }
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

impl App {
    /// Every card at this level, through the shared frame
    /// (`draw_graph_card`), plus each kind's own click action. Runs inside
    /// `build_graph_ui`'s `Scene`, after the grid and edges, so cards paint
    /// on top of them.
    #[allow(
        clippy::too_many_arguments,
        reason = "one call site, in `build_graph_ui`'s Scene closure: these are that frame's locals, and a struct would only rename them"
    )]
    pub(crate) fn draw_graph_cards(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        actions: &mut Vec<GraphAction>,
        at_level: &[Vec<String>],
        inv_zoom: f32,
        zoom_scale: f32,
        leave_group_button_rect: Option<egui::Rect>,
        connect_target: Option<&str>,
    ) {
        // One drop target per frame, shared by the ring and the
        // release (see `drop_target`).
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let drop_preview = self
            .interaction
            .dragging
            .clone()
            .and_then(|dragged| self.drop_target(&dragged, pointer, leave_group_button_rect));
        // The ring says where a drop would land; this says what it would do, beside the pointer.
        if let (Some((effect, target)), Some(at)) = (&drop_preview, pointer) {
            let name = target.as_deref().map(|id| self.drop_target_name(id)).unwrap_or_default();
            paint_drop_hint(ui.ctx(), at, &effect.hint(&name));
        }

        // Every card: one shared frame each (`draw_graph_card`),
        // layer by layer (`level_scan`, groups last), which is
        // also their stacking order: a later layer draws, and takes
        // the pointer, on top of an earlier one. Each kind's own
        // click action is applied here.
        let tab_order = self.card_tab_order();
        self.card_hint = None;
        if let Some(rect) = card_tab_step(ui, &tab_order, |id| self.level_card_rect(id)) {
            self.camera.pan_to(rect);
        }
        self.handle_card_digits(ui, &tab_order);
        self.handle_undo_keys(ui);
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
        let pick_targets = self.link.connecting.as_ref().map(|(from, edge)| self.valid_targets(edge, from));
        let mut pass = CardPass {
            deferred_overlays,
            actions,
            inv_zoom,
            zoom_scale,
            drop_preview: &drop_preview,
            pick_targets: pick_targets.as_deref().unwrap_or_default(),
            connect_target,
            connect_ring: self.link.retarget_preview.map(|outcome| crate::link_gestures::retarget_look(outcome).ring),
            show_order: self.card_nav_active || self.hints_revealed,
            order: &tab_order,
            any_focused: false,
            members: &members,
            viewport: self.camera.rect,
            pan_target: None,
            carry: true,
        };
        for entry in self.level_items_from(at_level) {
            let id = &entry.id;
            let kind = entry.kind;
            if let Origin::Virtual(name) = entry.origin {
                self.virtual_card_click(ui, &mut pass, &entry, name, pick_targets.is_some());
                continue;
            }
            // Read before the frame: a Note's `TextEdit` can lose
            // focus to a click on its own header this very frame,
            // clearing `editing`, and that click must not
            // immediately re-enter editing.
            let note_was_editing = self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);
            let Some(card) = self.draw_graph_card(ui, &mut pass, &entry) else {
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
            // Ctrl-click on a card toggles it in the selection and opens nothing.
            if card.response.clicked() && ui.input(|i| i.modifiers.ctrl) {
                self.toggle_selected(id);
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
            // A double-click, or Enter on the keyboard-focused card: what opens a card. A plain click only selects it
            // (below); a double-click's first click selects it too, so nothing waits on the second.
            let activated = card.response.double_clicked()
                || (card.response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            // Click (not drag -- egui doesn't report `clicked()` for a drag).
            if card.response.clicked() {
                self.select_only(id);
            }
            let click = activated.then(|| click_action(kind)).flatten();
            match (kind.ty, click) {
                _ if missing_file => {
                    if card.response.clicked() {
                        pass.actions.push(GraphAction::Node(id.clone(), NodeAction::RetryMissingFile));
                    }
                }
                (_, Some(action)) => pass.actions.push(GraphAction::Node(id.clone(), action)),
                // Click on the rendered note enters editing in place.
                (TYPE_NOTE, _) if activated && !note_was_editing => {
                    if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                        note.editing = true;
                    }
                    // The editor appears next frame; focus it so typing can start at once.
                    ui.memory_mut(|m| m.request_focus(egui::Id::new(("note_edit", id))));
                }
                // Enters the node's JSON level, unless its file failed to
                // load (plan ruling 10) or it's still being refetched (`JsonPayload::enterable`).
                (TYPE_JSON, _)
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
        if let Some(rect) = pass.pan_target {
            self.camera.pan_to(rect);
        }
        self.card_nav_active = pass.any_focused;
        self.draw_connect_rubber_band(ui, inv_zoom, connect_target);
    }

    /// A virtual card's frame, then its click: a click in pick-target mode only ends it (as every click does,
    /// `build_graph_ui`); a Ctrl-click toggles a `selectable` kind in the selection and opens nothing; any other
    /// click goes to the source's `on_click`, whose action is applied like a stored card's.
    /// Returns the card's response, `None` when it was culled.
    pub(crate) fn virtual_card_click(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        entry: &CardEntry,
        name: &str,
        picking: bool,
    ) -> Option<egui::Response> {
        let card = self.draw_graph_card(ui, pass, entry)?;
        if picking || !card.response.clicked() {
            return Some(card.response);
        }
        if ui.input(|i| i.modifiers.ctrl) && entry.caps().selectable {
            self.toggle_selected(&entry.id);
        } else if let Some(source) = self.virtual_source(name) {
            pass.actions.extend(source.on_click(self, entry));
        }
        Some(card.response)
    }

    /// World rect of Tab-order card `id`: a stored node's, or a virtual entry's.
    pub(crate) fn level_card_rect(&self, id: &str) -> Option<egui::Rect> {
        self.card_rect(id).or_else(|| {
            self.virtual_sources.iter().flat_map(|source| source.entries(self)).find(|e| e.id == id).map(|e| e.rect)
        })
    }

    /// The empty-canvas "New Tab/Note/Group/File" menu -- see `pending_new_node`'s own doc comment for what
    /// spawns/dismisses it. A screen-space `Foreground` `Area` at the clicked point, like every other context menu:
    /// drawn inside the Scene it sat under the cards' overlay text and chips, and its label size followed the zoom.
    pub(crate) fn draw_pending_new_node_menu(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<GraphAction>,
        project: &impl Fn(egui::Pos2) -> egui::Pos2,
    ) {
        let Some(pos) = self.interaction.pending_new_node else { return };
        let screen_pos = project(egui::Pos2::new(pos.x, pos.y));
        let (mut tab, mut note, mut group, mut file) = (false, false, false, false);
        let id = egui::Id::new("pending_new_node_menu");
        // The right-click that opens the menu is still this frame's click: it must not dismiss it.
        let opened_now = ui.ctx().memory(|m| m.area_rect(id)).is_none();
        let area = menu_popup(ui.ctx(), egui::Area::new(id).fixed_pos(screen_pos), 1.0, |ui| {
            ui.set_min_width(150.0);
            tab = ui.button(format!("{} New Tab", egui_phosphor::light::PLUS)).clicked();
            note = ui.button(format!("{} New Note", egui_phosphor::light::NOTE_PENCIL)).clicked();
            group = ui.button(format!("{} New Group", egui_phosphor::light::FOLDER_PLUS)).clicked();
            // A real picker blocks on OS/human input, which headless mode has neither of.
            if !self.headless {
                file = ui.button(format!("{} Add File...", egui_phosphor::light::FILE_PLUS)).clicked();
            }
        });
        if tab {
            actions.push(GraphAction::NewTab(self.focused.clone()));
        } else if note {
            actions.push(GraphAction::NewNote(pos));
        } else if group {
            actions.push(GraphAction::NewGroup(pos));
        } else if file {
            actions.push(GraphAction::NewFile(pos));
        }
        if tab || note || group || file || !opened_now && clicked_outside_popup(ui, area.rect, None) {
            self.interaction.pending_new_node = None;
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
        if self.interaction.peek_suppressed.is_some() && self.interaction.peek_suppressed != self.interaction.hovered {
            self.interaction.peek_suppressed = None;
        }
        let peek_target = self
            .interaction
            .hovered
            .clone()
            .filter(|id| self.interaction.peek_suppressed.as_deref() != Some(id.as_str()));
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
                        // Slid back inside the graph panel when the node is near its edge.
                        let view = painter.clip_rect();
                        let shift = (view.max - max).min(egui::Vec2::ZERO) + (view.min - min).max(egui::Vec2::ZERO);
                        let rect = egui::Rect::from_min_max(min + shift, max + shift);
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
    let dot_color = weak.gamma_multiply(0.35);
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

/// One deferred `overlay_painter` paint call, queued during `Scene::show`'s
/// content closure and actually painted only after it returns -- see
/// `DeferredOverlays`'s own doc comment for why this exists at all. Takes
/// the *real* `overlay_painter`/`project` (recomputed fresh post-`Scene`)
/// as arguments at call time, not at queue time.
type DeferredOverlay = Box<dyn FnOnce(&egui::Painter, &dyn Fn(egui::Pos2) -> egui::Pos2)>;

/// A frame's worth of `overlay_painter` decorations (node labels, pin/audio/
/// close/eject icons, the empty-canvas menu's own row labels, ...), queued
/// during `Scene::show`'s content closure instead of painted immediately.
///
/// Root cause this works around: `overlay_painter` draws on a screen-space
/// layer deliberately untransformed by `Scene` (see that field's own doc
/// comment for why -- crisp text at any zoom, not blurry rescaled glyphs),
/// so anything painted on it has to convert its own world position to
/// screen space manually via `project`. But `project` is built from
/// `self.camera.rect`, which -- confirmed by reading `egui::Scene::
/// show`'s actual source (0.36.2) -- is only updated *after* the whole
/// content closure returns for the frame; a live pan/zoom drag is applied
/// to `Scene`'s own *internal* transform before the closure runs, which is
/// why `ui.painter()`-drawn card content (rendered on `Scene`'s own
/// transformed layer, where egui applies that internal transform
/// automatically) is always correctly positioned, while anything computed
/// from `project` during the same closure is exactly one frame stale --
/// invisible at rest, but a growing pixel gap between a card and its own
/// label/icons the faster the camera pans, since a fixed one-frame timing
/// lag becomes a bigger on-screen offset the more distance the camera
/// covers in that frame.
///
/// Fix: instead of painting through `overlay_painter` immediately (using
/// the stale `project` captured before `Scene::show`), every such call
/// queues a closure capturing its own world position and paint parameters.
/// Once `Scene::show` returns and `self.camera.rect` has been updated
/// to reflect this frame's actual pan/zoom, `project` is recomputed fresh
/// and every queued closure is invoked with it -- so overlay decorations
/// are painted using the *same* up-to-date transform the card content
/// itself already reflects, closing the gap entirely rather than just
/// making it smaller.
pub(crate) type DeferredOverlays = Vec<DeferredOverlay>;

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
        app.insert_placeholder_node(a.clone(), egui::vec2(100.0, 0.0), Kind::Tab, sleeping_tab("https://a.test/", "a"));
        app.insert_placeholder_node(b.clone(), egui::vec2(300.0, 0.0), Kind::Tab, sleeping_tab("https://b.test/", "b"));
        let expected = (app.layout_pos(&a) + app.layout_pos(&b)) / 2.0;
        app.interaction.selected.extend([a.clone(), b.clone(), "999".to_string()]);
        app.apply_graph_action(&egui::Context::default(), GraphAction::GroupSelected);
        let group = app.doc.mirror.group(&a).expect("a was grouped").to_string();
        assert_eq!(app.doc.mirror.group(&b), Some(group.as_str()));
        assert_eq!(app.doc.mirror.pos(&group), Some(expected), "the mean of the two cards, not divided by three");
        assert!(app.interaction.selected.is_empty());
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
        app.link.connecting = Some((a.clone(), LINK));
        let on_b = screen_rect(&app, &b).center();
        assert!(app.graph_scene_outer_rect().contains(on_b), "{on_b:?} in {:?}", app.graph_scene_outer_rect());
        click(&mut app, on_b);
        assert!(app.link.connecting.is_none());
        assert_eq!(app.links_naming(&a).len(), 1, "the click linked b");
        assert!(!app.nodes[&b].as_note().unwrap().editing, "the click was pick mode's, not the note's");
        app.link.connecting = Some((a.clone(), OPENED_FROM));
        // Below both cards, clear of the minimap in the bottom-right corner.
        let below = egui::pos2(screen_rect(&app, &a).center().x, screen_rect(&app, &a).bottom() + 60.0);
        assert!(![&a, &b].iter().any(|id| screen_rect(&app, id).contains(below)), "empty canvas");
        click(&mut app, below);
        assert!(app.link.connecting.is_none(), "a click on empty canvas ends the mode");
        assert_eq!(app.doc.mirror.parent(&a), None, "and draws nothing");
    }

    /// A right-click on a card opens its menu; on the swatch it opens on the Color page. Driven through
    /// `build_graph_ui` with real egui input.
    #[test]
    fn a_right_click_on_the_swatch_opens_the_menu_on_its_color_page() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        let note_id = app.alloc_node_id();
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(note_id.clone(), egui::vec2(0.0, 0.0), crate::store::model::Kind::Note, note);
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
            let item = app.level_items().into_iter().find(|i| i.id == note_id).expect("on the level");
            app.graph_world_rect_to_screen(item.rect)
        };
        right_click(&mut app, card.center());
        assert_eq!(app.menus.node_menu.as_ref().map(|(id, _)| id), Some(&note_id), "the card's menu opens");
        assert!(app.menus.node_menu_page.is_none(), "on its items");
        app.menus.node_menu = None;
        right_click(&mut app, card.right_bottom() - egui::vec2(1.0, 1.0));
        let page = app.menus.node_menu_page.as_ref().map(|(id, _, page)| (id.clone(), *page));
        assert_eq!(page, Some((note_id, crate::node_menu::MenuPage::Color)), "the swatch opens the Color page");
    }

    /// Wave 2 (H2 / P2): whole `build_graph_ui` frames over the 2000-node fixture spread on a grid: idle at the top
    /// level, hovering a card, in pick-target mode, with the node menu open, filtered, and idle in the first group.
    /// Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins graph_frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn graph_frame_cost() {
        use crate::bench::{level_fixture, report, spread, timed};
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
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
            .find(|id| app.doc.mirror.group(id).is_none() && app.nodes[*id].as_note().is_some())
            .expect("a top-level note")
            .clone();
        app.camera.jump_to(app.card_rect(&note).unwrap().expand(300.0));
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        run(&mut app, "idle, top level", vec![]);
        let on_note = app.graph_world_rect_to_screen(app.card_rect(&note).unwrap()).center();
        run(&mut app, "hovering a card", vec![egui::Event::PointerMoved(on_note)]);
        app.link.connecting = Some((note.clone(), crate::caps::LINK));
        run(&mut app, "pick-target mode (LINK)", vec![]);
        assert!(app.link.connecting.is_some(), "the mode stayed on");
        app.link.connecting = None;
        app.menus.node_menu = Some((note.clone(), egui::pos2(500.0, 400.0)));
        run(&mut app, "node menu open", vec![]);
        assert!(app.menus.node_menu.is_some(), "the menu stayed open");
        app.menus.node_menu = None;
        app.graph_filter = "abc".into();
        run(&mut app, "filtered", vec![]);
        app.graph_filter.clear();
        app.switch_group_level(Some(f.groups[0].clone()));
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        run(&mut app, "idle, first group", vec![]);
    }

    /// Wave 2 (H4): keyboard navigation shows each card's Tab slot (`card_tab_order`'s index plus one) at its
    /// top-left corner. Pins the slot lookup before it moves to a per-pass map.
    #[test]
    fn each_card_shows_its_tab_slot_badge() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
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
        app.insert_group_at(g.clone(), None);
        app.doc.mirror.set(
            &g,
            crate::store::model::field::NAME,
            crate::store::model::FieldValue::Str("Reading".into()),
        );
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
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, app.accessible_name(&g).unwrap_or_default())
            });
        });
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update.expect("accesskit update");
        let named = update.nodes.iter().find(|(_, n)| n.label() == Some("Group: Reading"));
        assert_eq!(named.map(|(_, n)| n.role()), Some(egui::accesskit::Role::Button));
    }
}
