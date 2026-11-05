//! The Graph view: the types its children share and the clicks `build_graph_ui` collects. The scene body (`scene`),
//! cards (`cards`), toolbar, camera, keyboard and hints are the children below.

use super::*;
use crate::caps::Click;
use crate::store::model::{NodeId, TYPE_DOWNLOAD, TYPE_JSON, TYPE_NOTE, TYPE_TAB};

mod camera;
mod card_pane;
mod cards;
mod hints;
mod keys;
mod marquee;
mod overlays;
mod right_panel;
mod scene;
mod selection_bar;
mod suggestions;
mod toolbar;

pub(crate) use camera::*;
use hints::*;
pub(crate) use keys::*;
pub(crate) use overlays::*;
pub(crate) use right_panel::RightPanel;
pub(crate) use scene::*;
pub(crate) use toolbar::*;

/// Graph view's chrome is just the "+ New Tab" toolbar — no address bar,
/// no per-tab back/forward, since there's no single focused tab there.
/// Equal to `CHROME_HEIGHT_POINTS`, not independently tuned: a shorter
/// value clips button labels at IBM Plex Serif's line height.
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

/// The Graph view's per-frame state on `App` (`App::graph`).
#[derive(Default)]
pub(crate) struct GraphUi {
    /// A graph card or one of its controls had keyboard focus in the last card pass: the cards
    /// show their Tab-order numbers (`CardPass::show_order`).
    pub(crate) card_nav_active: bool,
    /// Ctrl has been held on its own long enough to reveal key hints (`ctrl_reveal_step`).
    pub(crate) hints_revealed: bool,
    /// The card holding keyboard focus this frame: its kind and screen rect, for the key-hint strip
    /// (`build_card_hint`).
    pub(crate) card_hint: Option<(CardKind, egui::Rect)>,
    /// Level-of-detail: `Simple` once a frame's own count of viewport-visible
    /// nodes (post-cull, see `build_graph_ui`'s own pre-pass) crossed
    /// `LOD_SIMPLIFY_COUNT`, gating the per-kind render loops' thumbnail/
    /// title-text painting to keep dense views cheap. Flips back to
    /// `Full` only once the count drops under the lower `LOD_RESTORE_
    /// COUNT` -- a deliberate hysteresis gap, not a single threshold, so
    /// a count sitting right at one boundary doesn't flicker every frame.
    pub(crate) detail: Detail,
    /// See `truncate_to_width`'s own doc comment.
    pub(crate) truncate_cache: HashMap<TruncateCacheKey, (String, String)>,
    /// A zoom scale (1.0 = 100%) the next Graph frame sets the camera to (`GRAPHZOOM`).
    pub(crate) pending_zoom: Option<f32>,
    /// `(level, level::SettleKey)` for the graph level `graph_settle_current_level`
    /// last ran a settle burst for -- `build_graph_ui` compares this
    /// against the current level's key every frame and only re-settles
    /// (and updates this) when they differ, instead of the physics sim
    /// running continuously. See `graph_settle_current_level`'s own doc
    /// comment for why continuous physics was replaced with this at all.
    pub(crate) settled_key: Option<(Option<NodeId>, crate::level::SettleKey)>,
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
    /// The card pane's label rows: put the label on the selection (`on`) or take it off, as one undo step.
    ToggleLabel { label: NodeId, on: bool },
    /// The card pane's name field: a new label, put on the selection.
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
    /// The scene area `fill` last fitted `rect` to. When the next area differs only in width (the right panel
    /// opening, closing or resized, or the window resized sideways), `fill` keeps the scale and left edge.
    pub(crate) area: Option<egui::Rect>,
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
                let home_page = self.settings.value.home_page.clone();
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
            GraphAction::ToggleLabel { label, on } => self.label_selection(label, on),
            GraphAction::NewLabel(name) => self.new_label_on_selection(&name),
            GraphAction::RenameLabel { label, name } => {
                self.rename_label_or_toast(label, &name);
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
            // Entering/leaving a group saves the view of the level left and eases
            // the camera to the one the new level was left at, or to the target
            // "Reset View" would jump to instantly when it has none (see
            // `App::switch_group_level`) -- keeping whatever pan/zoom the old
            // level happened to be at would likely frame empty space rather than
            // the new level's own nodes. `current_group` is updated first so
            // `fit_all_nodes_scene_rect` bounds the level actually being
            // switched *to*, not the one just left. Settling *before*
            // fitting matters: the new level's members may never have been
            // settled before (every group created via drag-and-drop starts with
            // its members wherever the drop happened, often nearly overlapping)
            // -- fitting first would target their old, pre-settle positions,
            // which then jump to their real spacing mid-animation once the
            // regular per-frame settle-check caught up one frame later.
            // Updating `GraphUi::settled_key` here too avoids that same settle
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
mod tests;
