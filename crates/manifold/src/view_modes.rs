//! View modes: Graph/Item/Split, the item transition and swap, and the graph camera's tween and pan momentum.

use super::*;
use crate::store::model::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ViewMode {
    Graph,
    /// One node shown full-screen ("maximized"): a Tab's live page, or --
    /// while `App::viewed_image` is `Some` -- an Image. Was `Tab` before a
    /// second kind could enter it; personal state's own `LocalState::
    /// view_item` keeps the generic name on disk.
    #[default]
    Item,
    /// The gear-icon Settings page -- a real `ViewMode`, not a popup
    /// overlaid on Tab/Graph, since the popup's fixed 200pt width
    /// overflowed on a small window once the saved-sessions list grew (no
    /// scroll area, unbounded height in a fixed-position `Area`). See
    /// `App::open_settings_page`/`build_settings_page`.
    Settings,
}

/// Which way a shrink/expand transition (see `App::transition`) is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionKind {
    /// Item view's full-screen content quad shrinks down into `node_id`'s
    /// graph-node thumbnail spot.
    ToGraph,
    /// `node_id`'s graph-node spot grows to fill the screen as Item view's content.
    ToItem,
}

/// An in-progress shrink/expand animation between Item view and Graph view
/// for one specific node. While this is `Some`, `build_ui` renders Graph
/// view's node layout as the backdrop (for *either* direction — it's the
/// one screen both endpoints have a coherent position in) with `node_id`'s
/// live content quad drawn on top at an interpolated rect, rather than
/// switching `view_mode` instantly. See `App::transition_content_rects_points`
/// (Tab) and `App::image_transition_frame` (Image).
pub(crate) struct Transition {
    pub(crate) node_id: NodeId,
    pub(crate) kind: TransitionKind,
    pub(crate) start: std::time::Instant,
}

pub(crate) const TRANSITION_DURATION: std::time::Duration = std::time::Duration::from_millis(220);

/// A `Transition`'s current rect: smoothstep-eased between the Item-view
/// end (`item_rect`) and the graph-card end (`graph_rect`), in whichever
/// direction `transition.kind` runs.
pub(crate) fn transition_eased_rect(
    transition: &Transition,
    item_rect: egui::Rect,
    graph_rect: egui::Rect,
) -> egui::Rect {
    let (from, to) = match transition.kind {
        TransitionKind::ToGraph => (item_rect, graph_rect),
        TransitionKind::ToItem => (graph_rect, item_rect),
    };
    let t = (transition.start.elapsed().as_secs_f32() / TRANSITION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    // Smoothstep: eases in and out, rather than the constant-velocity
    // (and slightly mechanical-looking) feel of plain linear `lerp`.
    let eased = t * t * (3.0 - 2.0 * t);
    egui::Rect::from_min_max(from.min + (to.min - from.min) * eased, from.max + (to.max - from.max) * eased)
}

/// An in-progress eased pan/zoom from one `camera.rect` to another --
/// entering/leaving a group moves the camera to the new level's
/// `fit_all_nodes_scene_rect()` over
/// `TRANSITION_DURATION` rather than instantly, with the same smoothstep easing as `Transition`
/// above (see `GraphCamera::advance`). Deliberately a separate,
/// simpler mechanism rather than reusing `Transition` itself: `Transition`
/// morphs *one tab's* rendered content quad between two rects, but entering
/// a group swaps the whole visible node set, not one tab's content -- there
/// is nothing to morph, only the camera to move. Because a group's members
/// already live in the same shared world-space coordinates the group's own
/// card does (not a separate per-group origin), easing the camera toward
/// them doubles as the "zoom into the subgraph" reveal effect for free: the
/// new level's real nodes render (tiny, correctly positioned) from the very
/// first frame, and the camera zooming in is what grows them into view --
/// no separate rendered "thumbnail" of the subgraph is needed.
#[derive(Clone, Copy)]
pub(crate) struct GraphCameraTween {
    pub(crate) from: egui::Rect,
    pub(crate) to: egui::Rect,
    pub(crate) start: std::time::Instant,
}

/// A background pan drag released with enough speed keeps the camera
/// gliding and decelerating instead of stopping dead the instant the
/// mouse button lifts -- the same "flick to scroll" feel most pan/zoom
/// canvases (Figma, Google Maps, Miro) already have. `initial_velocity`
/// is world-space units/second, sampled from `GraphCamera::drag_history`'s
/// own recent camera-center movement (not the raw pointer's own screen
/// velocity) -- see that field's own doc comment for why. Recomputed
/// fresh from `start`/`start_center`/`initial_velocity` every frame
/// (never accumulated frame-to-frame), same "not derived from a value
/// that can drift with frame-time variance" reasoning `GraphCameraTween`
/// above already has: exponential decay (`v(t) = v0 * e^-kt`) has a
/// closed-form displacement (the integral of `v(t)` from 0 to `t`), so
/// each frame is one direct formula evaluation from elapsed time, not an
/// integration loop.
#[derive(Clone, Copy)]
pub(crate) struct PanMomentum {
    pub(crate) initial_velocity: egui::Vec2,
    pub(crate) start: std::time::Instant,
    pub(crate) start_center: egui::Pos2,
}

/// Exponential decay rate (`k` in `v(t) = v0 * e^-kt`, per second) for
/// `PanMomentum` -- tuned for a glide that's clearly perceptible but
/// settles well under a second, not a floaty drift.
pub(crate) const PAN_MOMENTUM_DECAY: f32 = 3.5;

/// Minimum release speed (world-space units/second, from `GraphCamera::
/// drag_history`) for a released pan drag to start gliding at all -- below
/// this, an ordinary slow drag-and-let-go just stops, matching what a
/// user who wasn't trying to "flick" the canvas would expect.
pub(crate) const PAN_MOMENTUM_MIN_SPEED: f32 = 500.0;

/// Hard cap on how long a `PanMomentum` stays active, regardless of
/// `PAN_MOMENTUM_DECAY` -- at that rate, decay is already below 1% of
/// the initial speed well before this, so this is just a bound on the
/// `Option` field's own lifetime, not a value that shapes the motion.
pub(crate) const PAN_MOMENTUM_MAX_DURATION: std::time::Duration = std::time::Duration::from_millis(2000);

/// How many recent camera-center samples `GraphCamera::drag_history` keeps
/// while a background pan drag is held down -- spans roughly 100ms at a
/// steady frame rate, matching the same window egui's own `PointerState::
/// velocity()` uses internally for its analogous smoothing.
pub(crate) const PAN_HISTORY_LEN: usize = 6;

impl App {
    /// Graph view's node-click equivalent of `focus_item` — starts an
    /// expand animation instead of switching instantly. Deliberately a
    /// separate method rather than folding this into `focus_tab` itself:
    /// `focus_tab` is also used for a same-view tab switch (accepting an
    /// open-tab autocomplete suggestion, already in Tab view), where an
    /// expand-from-a-graph-node animation wouldn't make sense.
    ///
    /// Ignored while a `Transition` is already running: `build_ui` keeps
    /// rendering Graph view (with all its normal node-click handling) as
    /// the backdrop for a transition's whole duration, so a second click
    /// during the first click's 220ms animation would otherwise retarget
    /// `self.transition` mid-flight — the animation would jump to the new
    /// node/tab partway through, and `focus_tab` would hide the first
    /// node's still-animating tab out from under it. Found live: clicking a
    /// tab node multiple times in quick succession visibly glitched the
    /// expand animation instead of queuing or ignoring the extra clicks.
    pub(crate) fn start_transition_to_item(&mut self, id: NodeId) {
        if self.transition.is_some() {
            return;
        }
        self.focus_item(id);
        // `focus_item` already set `view_mode = Item`, but `build_ui` renders
        // Graph view as the backdrop for as long as `self.transition` is
        // `Some` regardless of `view_mode` (see its own doc comment) —
        // `finish_transition_if_done` is what actually matters once the
        // animation completes.
        self.transition =
            Some(Transition { node_id: id, kind: TransitionKind::ToItem, start: std::time::Instant::now() });
    }

    /// Checked once per frame, before rendering: once a `Transition`'s
    /// duration has elapsed, applies whatever `focus_tab`/`leave_to_graph`
    /// would have done instantly (the view-mode switch, and — for
    /// `ToGraph` specifically — finally suspending the tab now that its
    /// last real frame has been captured into the thumbnail it just
    /// shrank into).
    pub(crate) fn finish_transition_if_done(&mut self) {
        let Some(transition) = &self.transition else {
            return;
        };
        if transition.start.elapsed() < TRANSITION_DURATION {
            return;
        }
        let node_id = transition.node_id;
        let kind = transition.kind;
        self.transition = None;
        match kind {
            TransitionKind::ToGraph => {
                self.view_mode = ViewMode::Graph;
                self.viewed_image = None;
                if let Some(node) = self.nodes.get_mut(&node_id).and_then(|n| n.as_tab_mut()) {
                    node.tab.set_visible(false);
                }
            }
            TransitionKind::ToItem => {
                self.view_mode = ViewMode::Item;
            }
        }
        self.save_session();
    }

    /// Where a `Transition` draws its tab's live content *this frame* (egui points, screen space),
    /// as `(viewport, clip)`: the page is stretched over `viewport` and only `clip` shows. It eases
    /// between Tab view's full content area and the tab's graph-card thumbnail, direction per
    /// `transition.kind`. At the card end the page is scaled to cover the thumbnail area
    /// (`cover_uv`, as `paint_tab_body` crops it), so the quad lands on what the card shows.
    pub(crate) fn transition_content_rects_points(&self, transition: &Transition) -> (egui::Rect, egui::Rect) {
        let screen = self.screen_size_points();
        let full_content_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, CHROME_HEIGHT_POINTS),
            egui::vec2(screen.x, (screen.y - CHROME_HEIGHT_POINTS).max(0.0)),
        );
        if !self.nodes.contains_key(&transition.node_id) {
            let rect = transition_eased_rect(transition, full_content_rect, full_content_rect);
            return (rect, rect);
        }
        // Same rect `paint_tab_body` blits the thumbnail at, so the transition starts and ends on it.
        let world = egui::Rect::from_center_size(self.layout_pos(transition.node_id).to_pos2(), GRAPH_NODE_SIZE);
        let zoom = self.graph_world_rect_to_screen(world).width() / GRAPH_NODE_SIZE.x;
        let clip_end = self.graph_world_rect_to_screen(tab_thumb_rect(world, 1.0 / zoom));
        let viewport_end = match self.gpu.as_ref().and_then(|g| g.thumbnail_size(transition.node_id)) {
            Some(size) => {
                let uv = cover_uv(size, clip_end.size());
                let size = egui::vec2(clip_end.width() / uv.width(), clip_end.height() / uv.height());
                egui::Rect::from_min_size(clip_end.min - uv.min.to_vec2() * size, size)
            }
            None => clip_end,
        };
        (
            transition_eased_rect(transition, full_content_rect, viewport_end),
            transition_eased_rect(transition, full_content_rect, clip_end),
        )
    }

    /// Image counterpart of `transition_content_rects_points`: the image's
    /// texture and where to paint it this frame, between its graph card
    /// and its current Image-view rect (Fit on entry; whatever zoom/pan
    /// the user left on exit), so the aspect ratio is right throughout.
    /// `None` for a non-Image transition or a missing node/texture.
    pub(crate) fn image_transition_frame(&self, transition: &Transition) -> Option<(egui::TextureId, egui::Rect)> {
        let node = self.nodes.get(&transition.node_id)?;
        let image = node.as_image()?;
        let tex_id = self.gpu.as_ref()?.image_id(transition.node_id)?;
        let size = image_pixel_size(image);
        let card = self.graph_world_rect_to_screen(egui::Rect::from_center_size(
            self.layout_pos(transition.node_id).to_pos2(),
            image_node_card_size(image.width, image.height),
        ));
        let body = self.item_body_rect();
        let (scale, pan) = self.image_view.effective(size, body.size());
        let view = image_display_rect(body, size, scale, pan);
        Some((tex_id, transition_eased_rect(transition, view, card)))
    }

    /// M2's swap sequence, now per-tab and driven by the engine-picker
    /// dropdown: `GetState` -> spawn new engine at the same tab_id -> new
    /// engine's first `FrameReady` becomes the displayed texture (once
    /// focused) -> old view-host dropped when its graph node is replaced.
    pub(crate) fn start_swap_to(&mut self, tab_id: NodeId, target: ViewKind) {
        if self.swap_in_flight(tab_id).is_some() {
            return; // one swap at a time for this tab
        }
        let Some(node) = self.nodes.get_mut(&tab_id).and_then(|n| n.as_tab_mut()) else {
            return;
        };
        if node.tab.kind == target {
            return; // no-op swap
        }
        node.swap_target = Some(target);
        // Saved as the swap starts: a save before the snapshot writes the target, and a failure
        // that falls back writes the old engine back (`handle_tab_spawn_result`).
        self.edit_view_kind(tab_id, target);
        // Cookies first: the old host is dropped once its snapshot arrives (`hold_for_cookies` waits for them).
        self.pull_cookies(tab_id);
        if let Some(node) = self.nodes.get_mut(&tab_id).and_then(|n| n.as_tab_mut()) {
            node.tab.send(&HostToView::GetState);
        }
    }

    /// The engine a host swap in flight for tab `id` lands on: its `swap_target` until the old host's snapshot
    /// arrives, then the snapshot parked for the host's cookies (`hold_for_cookies`; "Switch Jar Now" parks one on
    /// the running engine). The one lock: `start_swap_to` starts no second swap, whose snapshot would never come once
    /// the first drops the host, and a remote engine meanwhile is written over (`update_live_node`).
    pub(crate) fn swap_in_flight(&self, id: NodeId) -> Option<ViewKind> {
        let tab = self.nodes.get(&id)?.as_tab()?;
        tab.swap_target.or(match self.cookies.cookie_holds.get(&id) {
            Some((_, crate::tabs::Held::Snapshot(target, _))) => Some(*target),
            _ => None,
        })
    }

    pub(crate) fn finish_swap(&mut self, tab_id: NodeId, target: ViewKind, url: String) {
        let Some(spawn) = self.nodes.get(&tab_id).and_then(|n| {
            let old = n.as_tab()?.tab.kind;
            Some(TabSpawn {
                fallback_kind: Some(old),
                ..TabSpawn::respawn(tab_id, n, self.doc.mirror.carried(tab_id), target, url)?
            })
        }) else {
            return;
        };
        // Must drop (and thus Shutdown + kill) the old tab before spawning
        // the new one at the same tab_id — the new view-host's named-pipe
        // listeners would collide with the old ones otherwise.
        self.nodes.remove(&tab_id);
        // Same stale-state guard `destroy_tab` already applies: without it, a
        // drag in progress on this exact tab when its engine swap/crash
        // fires would leave `self.interaction.dragging` pointing at an id invisible for
        // the whole respawn window, then get misread as "just dropped" the
        // instant the respawned node starts rendering again.
        let card = CardId::from(tab_id);
        if self.interaction.dragging == Some(card) {
            self.interaction.dragging = None;
        }
        if self.interaction.hovered == Some(card) {
            self.interaction.hovered = None;
        }
        self.respawn_tab(spawn);
    }
}
