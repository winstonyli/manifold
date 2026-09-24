mod bookmarks;
mod crash_report;
mod debug_control;
mod favicon_fetch;
mod gpu;
mod graph_layout;
mod group_export;
mod json_file;
mod redirect_fetch;
mod pdf;
mod pdf_fetch;
mod process_manager;
mod session;
mod settings;
mod suggestions;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use favicon_fetch::FaviconFetcher;
use redirect_fetch::{RedirectFetcher, RedirectKind};
use pdf_fetch::PdfFetcher;
use suggestions::{resolve_navigation_url, Suggestion, SuggestionFetcher};

use debug_control::DebugCommand;
use gpu::GpuState;
use ipc_protocol::{
    frame_path, modifiers, EngineToHost, HostToEngine, InputEvent, KeyCode, MouseButton,
    SharedFrameBuffer,
};
use process_manager::{EngineKind, EngineTab};
use settings::{SearchEngine, Settings};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{DeviceId, ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode as WinitKeyCode, ModifiersState, PhysicalKey};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Icon, Window, WindowId};

/// Tab view's chrome is a single top `Panel` of this exact height, in egui
/// points — kept as a constant rather than read back from egui's layout so
/// the compositor's content viewport (see `gpu::GpuState::render`'s
/// `content_top_px`) always agrees with where egui actually painted,
/// without a frame of lag. Was two stacked bars (a tab-identity strip above
/// a separate navigation toolbar) until they were merged into one row —
/// the identity info (favicon/spinner, engine dot) now lives inline with
/// navigation controls in the same bar, and the separate page-title label
/// was dropped as redundant with the address bar's URL.
const CHROME_HEIGHT_POINTS: f32 = 32.0;
/// Graph view's chrome is just the "+ New Tab" toolbar — no address bar,
/// no per-tab back/forward, since there's no single focused tab there.
/// Equal to `CHROME_HEIGHT_POINTS`, not independently tuned — a shorter
/// value here used to fit the old bundled Ubuntu-Light font's line height
/// but clipped button labels once `apply_fonts` switched to IBM Plex
/// Serif's taller metrics (confirmed live: "+ New Group"/"Reset View"
/// visibly cut off at the old 26.0).
const GRAPH_TOOLBAR_HEIGHT_POINTS: f32 = CHROME_HEIGHT_POINTS;

const GRAPH_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 110.0);
/// Smaller than `GRAPH_NODE_SIZE` — a download node has no live thumbnail,
/// just a filename, so it doesn't need the same footprint a tab's preview
/// does.
const DOWNLOAD_NODE_SIZE: egui::Vec2 = egui::vec2(130.0, 60.0);
/// A group card's footprint — same idea as `DOWNLOAD_NODE_SIZE` (no live
/// thumbnail, just a name/member-count), sized a bit larger since it's a
/// container rather than a single file. Also the drop-target rect a
/// dragged tab is tested against on release — see `GroupPayload`'s own doc
/// comment.
const GROUP_NODE_SIZE: egui::Vec2 = egui::vec2(150.0, 90.0);
/// A note card's footprint -- taller than the other non-thumbnail kinds
/// since its whole body is an editable multi-line text area, not just a
/// title strip.
const NOTE_NODE_SIZE: egui::Vec2 = egui::vec2(180.0, 140.0);
/// The largest an image node's card gets, in either dimension -- unlike
/// every other kind, an image card's actual footprint is derived per-node
/// from `ImagePayload::width`/`height`'s own aspect ratio (see
/// `image_node_card_size`), scaled down to fit inside this box. Never
/// scaled *up* past the image's real pixel size, so a small picture isn't
/// blurrily stretched to fill it.
const IMAGE_NODE_MAX_SIZE: egui::Vec2 = egui::vec2(220.0, 220.0);
/// A PDF node's card size while its background render is still in flight
/// (`PdfPayload::outcome` is `None`) or failed (`Some(Err(_))`) -- the real
/// page's own aspect ratio isn't known yet in either case, so this is a
/// plain fixed box, same idea `DOWNLOAD_NODE_SIZE` already has for "no live
/// content to size around." Once rendered, the card switches to
/// `image_node_card_size` like an `Image` node does, sized from the real
/// page dimensions `PdfOutcome` carries.
const PDF_PENDING_NODE_SIZE: egui::Vec2 = egui::vec2(150.0, 90.0);
/// A JSON node's card size -- always this fixed box, never derived from
/// content the way `Image`/`Pdf`'s own sizing is: there's no natural aspect
/// ratio for a JSON document, and the card only ever shows a compact shape
/// summary (double-click opens the real content in `build_json_viewer`).
const JSON_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 90.0);

/// Visible-node count (post-cull, current level, see `build_graph_ui`'s
/// LOD pre-pass) above which Graph view switches to simplified rendering
/// -- skipping thumbnail/favicon textures and always-drawn title/label
/// text across every kind, the two genuinely expensive parts of painting
/// a card (GPU texture sampling and glyph rasterization), while leaving
/// backgrounds, borders, color swatches, and hover-gated buttons alone.
/// Counts visible nodes rather than thresholding on zoom directly, since
/// a dense cluster of many small-but-individually-zoomed-in cards is the
/// actual cost driver this exists to bound, not zoom level itself.
const LOD_SIMPLIFY_COUNT: usize = 60;
/// Below this count, LOD returns to full detail. Deliberately lower than
/// `LOD_SIMPLIFY_COUNT` (a hysteresis gap, not one shared threshold) so a
/// count hovering right at a single boundary doesn't flip every frame.
const LOD_RESTORE_COUNT: usize = 45;

/// Fixed tag-color choices a group or tab card's color swatch cycles
/// through (see `next_tag_color`) -- vermillion/orange/yellow/bluish
/// green/sky blue/reddish purple, chosen for mutual distinguishability
/// against this app's dark chrome rather than any particular meaning.
/// Used via the now-shared `GraphNode::color`, for both `Tab` and
/// `Group`. These are the Okabe-Ito colorblind-safe palette values
/// (Okabe & Ito, 2008) -- every swatch here is rendered as a plain
/// filled circle with no secondary encoding (shape, letter, pattern) to
/// distinguish them, confirmed live: the original arbitrary red/orange/
/// yellow/green/cyan/purple choice had several genuinely confusable
/// pairs (red/orange, green/cyan) for common colorblindness types.
/// Swapping the six RGB values keeps the same visual role per slot and
/// needs no rendering changes at all.
const TAG_COLOR_PALETTE: [egui::Color32; 6] = [
    egui::Color32::from_rgb(213, 94, 0),
    egui::Color32::from_rgb(230, 159, 0),
    egui::Color32::from_rgb(240, 228, 66),
    egui::Color32::from_rgb(0, 158, 115),
    egui::Color32::from_rgb(86, 180, 233),
    egui::Color32::from_rgb(204, 121, 167),
];

/// One click on a card's color swatch advances to the next palette entry,
/// wrapping back to `None` (the plain default border) after the last --
/// `None` -> red -> orange -> ... -> purple -> `None`, matching how the pin
/// dot's own binary toggle needs no separate popup/menu to operate.
/// Strips a "Save Session As..." name down to characters safe to use
/// verbatim as a filename (`sessions/<name>.json`) -- alphanumeric, space,
/// hyphen, underscore only. No path separators or `.` survive, so there's
/// no path-traversal or extension-confusion risk from typed input, and no
/// separate validation step is needed before treating the result as a bare
/// file stem.
fn sanitize_session_name(raw: &str) -> String {
    raw.trim().chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).collect::<String>()
}

/// Case-insensitive substring match against any of `haystacks` -- shared
/// by the Graph toolbar's own filter (via `graph_filter_dims` below) and
/// the History/Bookmarks popup filter boxes, so the three don't each
/// reimplement the same `to_lowercase().contains()` check slightly
/// differently. An empty `filter` always matches (nothing typed yet, so
/// nothing should be filtered out).
fn filter_matches(filter: &str, haystacks: &[&str]) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    let filter = filter.to_lowercase();
    haystacks.iter().any(|h| h.to_lowercase().contains(&filter))
}

/// Graph toolbar filter box: `true` when the node should be painted
/// dimmed -- exactly the negation of `filter_matches`, kept as its own
/// named function since "dims" (not "doesn't match") is the meaning every
/// call site actually cares about.
fn graph_filter_dims(filter: &str, haystacks: &[&str]) -> bool {
    !filter_matches(filter, haystacks)
}

/// Every dropdown/popup (bookmarks, history, downloads, tab actions, tab
/// context menu, group context menu, recently-closed) closes itself on a
/// click landing outside both its own area and, where one exists, the
/// button that opened it -- this was the same `ui.input(|i| ...)` check
/// reimplemented at each of the ~9 call sites. `anchor_rect` is `None` at
/// the sites that never had one (the anchor button already consumes its
/// own click before this runs, so there's nothing left to exclude).
fn clicked_outside_popup(ui: &egui::Ui, area_rect: egui::Rect, anchor_rect: Option<egui::Rect>) -> bool {
    ui.input(|i| {
        i.pointer.any_click()
            && i.pointer.interact_pos().is_some_and(|pos| {
                !area_rect.contains(pos) && anchor_rect.map_or(true, |r| !r.contains(pos))
            })
    })
}

/// Background, border and padding shared by every menu and dropdown
/// (right-click menus, toolbar lists, the address-bar suggestions).
fn popup_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::default()
        .fill(TOOLBAR_FILL)
        .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
        .corner_radius(4u8)
        .inner_margin(egui::Margin::same(6))
}

/// `popup_frame` for centered dialogs (quick switcher, shortcuts help,
/// import confirm, JSON viewer): rounder, with more padding.
fn dialog_frame(ui: &egui::Ui) -> egui::Frame {
    popup_frame(ui).corner_radius(6u8).inner_margin(egui::Margin::same(14))
}

/// The `Area`/`Frame` shell every toolbar list-dropdown (Recently Closed,
/// Bookmarks, History, Downloads) wraps its own contents in -- pinned below
/// `anchor_rect`, same fill/stroke/corner-radius/margin, same min/max width
/// (`f32::INFINITY` for no maximum).
/// A free function (not a method) so callers can still freely capture
/// `&mut self` in `add_contents` -- an `&mut self` method here would
/// conflict with that same borrow. Returns the drawn area's own rect, for
/// the caller's own `clicked_outside_popup` check. `fade` (0.0-1.0, from the
/// caller's own `ctx.animate_bool` on its own open flag) sets the whole
/// popup's opacity, so it fades in/out instead of popping instantly.
/// `interactable` is the caller's open flag: while fading out the popup is
/// click-through, so the rest of a double-click on an item can't run it
/// twice.
fn build_list_popup_frame(
    ctx: &egui::Context,
    id: &str,
    anchor_rect: egui::Rect,
    min_width: f32,
    max_width: f32,
    fade: f32,
    interactable: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    egui::Area::new(egui::Id::new(id))
        .fixed_pos(anchor_rect.left_bottom())
        .order(egui::Order::Foreground)
        .interactable(interactable)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            popup_frame(ui)
                .show(ui, |ui| {
                    ui.set_min_width(min_width);
                    // An infinite cap makes egui lay out NaN rects; no cap
                    // lets the area size to its contents.
                    if max_width.is_finite() {
                        ui.set_max_width(max_width);
                    }
                    add_contents(ui);
                });
        })
        .response
        .rect
}

/// What clicking one item of a node's right-click menu does, applied by
/// `App::apply_node_menu_action`. `build_node_context_menu` only reports
/// which item was clicked; the caller owns every effect.
#[derive(Clone, Copy, Debug, PartialEq)]
enum NodeMenuAction {
    CopyText,
    Reveal,
    CopyPath,
    ExpandAsGraph,
    TogglePin,
    Duplicate,
    CopyUrl,
    ViewJson,
    FitSubtree,
    /// Swaps the group menu's items for a name field (see
    /// `build_node_context_menu`); the menu stays open.
    Rename,
    /// The rename field lost focus by any means but Escape.
    CommitRename,
    /// Escape in the rename field.
    CancelRename,
    SetColor(Option<egui::Color32>),
    RemoveFromGroup,
    Dissolve,
    ExportGroup,
    /// Delete for a file card, "Remove from List" for a download, Close
    /// for a tab or group.
    Delete,
}

impl NodeMenuAction {
    fn hover_text(self) -> Option<&'static str> {
        match self {
            NodeMenuAction::FitSubtree => Some("Zoom the graph to frame just this tab and its children"),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
enum NodeMenuItem {
    Button(&'static str, NodeMenuAction),
    /// The "Color" label and tag-color swatch row, ending in a "no color"
    /// ring.
    Colors,
    Separator,
}

/// A node's right-click menu: its items, in order. `area_id` is the
/// popup's egui `Area` id, distinct per kind so egui's per-area memory
/// never bleeds between kinds. The file menus have a fixed width; the tab
/// and group menus have no maximum, so they grow to fit the swatch row.
#[derive(Debug, PartialEq)]
struct NodeMenuConfig {
    area_id: &'static str,
    min_width: f32,
    max_width: f32,
    items: Vec<NodeMenuItem>,
}

/// Draws a node's right-click menu and reports what was clicked this
/// frame, plus whether the click landed outside the menu. A free function,
/// like `build_list_popup_frame`, so the caller keeps `&mut self` free to
/// apply the action. `pos` stands in for a zero-size anchor rect, so the
/// menu opens right at the pointer. `rename` is the group-name draft (and
/// whether to focus it this frame) while a group is being renamed: the
/// menu then shows just that field. `color` rings the node's current
/// swatch.
fn build_node_context_menu(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    fade: f32,
    interactable: bool,
    config: &NodeMenuConfig,
    color: Option<egui::Color32>,
    rename: Option<(&mut String, bool)>,
) -> (Option<NodeMenuAction>, bool) {
    let mut action = None;
    let anchor = egui::Rect::from_min_size(pos, egui::Vec2::ZERO);
    let area_rect =
        build_list_popup_frame(ui.ctx(), config.area_id, anchor, config.min_width, config.max_width, fade, interactable, |ui| {
            if let Some((draft, focus)) = rename {
                let response = ui.add(egui::TextEdit::singleline(draft).desired_width(140.0));
                if focus {
                    response.request_focus();
                }
                if response.lost_focus() {
                    action = Some(if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        NodeMenuAction::CancelRename
                    } else {
                        NodeMenuAction::CommitRename
                    });
                }
                return;
            }
            for item in &config.items {
                match *item {
                    NodeMenuItem::Button(label, item_action) => {
                        let mut response = ui.button(label);
                        if let Some(text) = item_action.hover_text() {
                            response = response.on_hover_text(text);
                        }
                        if response.clicked() {
                            action = Some(item_action);
                        }
                    }
                    NodeMenuItem::Colors => {
                        if let Some(picked) = color_swatch_row(ui, color) {
                            action = Some(NodeMenuAction::SetColor(picked));
                        }
                    }
                    NodeMenuItem::Separator => {
                        ui.separator();
                    }
                }
            }
        });
    let clicked_outside = clicked_outside_popup(ui, area_rect, None);
    (action, clicked_outside)
}

/// The "Color" label and one swatch per `TAG_COLOR_PALETTE` entry, then a
/// "no color" ring; `current` gets a ring of its own. Returns the clicked
/// choice (`Some(None)` for no color).
fn color_swatch_row(ui: &mut egui::Ui, current: Option<egui::Color32>) -> Option<Option<egui::Color32>> {
    let mut picked = None;
    ui.add_space(4.0);
    ui.label("Color");
    ui.horizontal(|ui| {
        for choice in TAG_COLOR_PALETTE.iter().copied().map(Some).chain([None]) {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            match choice {
                Some(swatch) => {
                    ui.painter().circle_filled(rect.center(), 8.0, swatch);
                    if current == Some(swatch) {
                        ui.painter().circle_stroke(rect.center(), 8.0, egui::Stroke::new(2.0, ui.visuals().text_color()));
                    }
                }
                None => {
                    ui.painter().circle_stroke(
                        rect.center(),
                        8.0,
                        egui::Stroke::new(1.5, ui.visuals().weak_text_color()),
                    );
                }
            }
            if response.clicked() {
                picked = Some(choice);
            }
        }
    });
    picked
}

/// Which items a node's right-click menu shows, from the state frozen in
/// `view`. Export Group is left out in headless mode, since its file
/// dialog would block on input that never comes (`EXPORTGROUP <path>`
/// saves directly instead).
fn node_menu_config(view: &NodeMenuView, headless: bool) -> NodeMenuConfig {
    use NodeMenuAction as A;
    use NodeMenuItem::{Button, Colors, Separator};
    let file_items = || vec![Button("Open Containing Folder", A::Reveal), Button("Copy Path", A::CopyPath)];
    // Every menu but the group's ends in Remove from Group (when the node
    // is grouped) and the kind's delete item.
    let with_group_and_delete = |area_id, min_width, max_width, mut items: Vec<NodeMenuItem>, delete_label| {
        if view.has_group {
            items.extend([Separator, Button("Remove from Group", A::RemoveFromGroup)]);
        }
        items.extend([Separator, Button(delete_label, A::Delete)]);
        NodeMenuConfig { area_id, min_width, max_width, items }
    };
    let (area_id, width, items, delete_label) = match view.kind {
        ItemNodeKind::Download => ("download_context_menu", 170.0, file_items(), "Remove from List"),
        ItemNodeKind::Note => ("note_context_menu", 150.0, vec![Button("Copy Text", A::CopyText)], "Delete"),
        ItemNodeKind::Image => ("image_context_menu", 170.0, file_items(), "Delete"),
        ItemNodeKind::Pdf => ("pdf_context_menu", 170.0, file_items(), "Delete"),
        ItemNodeKind::Json => {
            let mut items = file_items();
            items.push(Button("Expand as Graph", A::ExpandAsGraph));
            ("json_context_menu", 170.0, items, "Delete")
        }
        ItemNodeKind::Tab => {
            let mut items = vec![
                Button(if view.pinned { "Unpin" } else { "Pin" }, A::TogglePin),
                Button("Duplicate", A::Duplicate),
                Button("Copy URL", A::CopyUrl),
            ];
            if view.has_json {
                items.push(Button("View as JSON", A::ViewJson));
            }
            items.extend([Button("Fit Subtree", A::FitSubtree), Colors]);
            return with_group_and_delete("tab_context_menu", 150.0, f32::INFINITY, items, "Close");
        }
        ItemNodeKind::Group => {
            let mut items =
                vec![Button("Rename", A::Rename), Colors, Separator, Button("Dissolve", A::Dissolve), Button("Close Group", A::Delete)];
            if !headless {
                items.push(Button("Export Group...", A::ExportGroup));
            }
            return NodeMenuConfig { area_id: "group_context_menu", min_width: 150.0, max_width: f32::INFINITY, items };
        }
    };
    with_group_and_delete(area_id, width, width, items, delete_label)
}

/// Everything a node's right-click menu shows that its own actions can
/// change, read each frame while the menu is open and frozen in
/// `node_menu_cache` for the fade-out. The fading menu keeps the look it
/// had when an item was clicked, instead of redrawing the new state (a
/// "Remove from Group" row vanishing, "Pin" turning into "Unpin", ...), and
/// still fades when the action removed the node itself.
#[derive(Clone)]
struct NodeMenuView {
    kind: ItemNodeKind,
    has_group: bool,
    color: Option<egui::Color32>,
    /// Tab only.
    pinned: bool,
    /// Tab only.
    has_json: bool,
    /// Group only: the rename draft, while this group is being renamed.
    renaming: Option<String>,
}

/// Lets a popup driven by `Option<T>` app state (the right-click context
/// menus) fade out instead of vanishing the instant that state goes back
/// to `None` -- the position/content those need to keep drawing during a
/// fade-out would otherwise disappear the same frame the menu "closes."
/// `cache` is a sibling field remembering the last real value; `id` keys
/// the fade animation (must be unique per popup). Polled every frame
/// regardless of `current`'s state, same "continuous polling" reasoning
/// the toolbar dropdowns' own fade needs (see their own call sites) --
/// calling `ctx.animate_bool` only while `current` is `Some` would leave
/// real elapsed time untracked while it's `None`, making the next open
/// jump straight to visible instead of easing in. Returns `None` once the
/// fade has actually finished (nothing left to draw), or `Some((value,
/// fade))` otherwise -- `value` is `current`'s own content while it's
/// `Some`, else the frozen `cache`.
fn fade_popup_state<T: Clone>(
    ctx: &egui::Context,
    id: egui::Id,
    current: &Option<T>,
    cache: &mut Option<T>,
) -> Option<(T, f32)> {
    let fade = ctx.animate_bool(id, current.is_some());
    if let Some(value) = current {
        *cache = Some(value.clone());
    }
    if fade <= 0.0 {
        return None;
    }
    cache.clone().map(|value| (value, fade))
}

/// The app's own window/taskbar icon -- embedded at compile time (`include_
/// bytes!`, same posture `IBMPlexSerif-Regular.ttf` already uses) rather
/// than loaded from disk at runtime, so the binary stays self-contained.
/// `None` on decode failure rather than panicking -- a missing icon just
/// means winit's own default, not a reason to fail to start.
fn load_app_icon() -> Option<Icon> {
    let bytes = include_bytes!("../assets/icon/icon-64.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Icon::from_rgba(img.into_raw(), width, height).ok()
}

/// Shared body for the six `App::alloc_*_id` methods -- each owns a
/// separate monotonic counter field (a distinct id space per node kind, so
/// e.g. a tab and a note can both be "id 3" without colliding) and only
/// ever differed in which counter and which string prefix it used.
fn next_id(counter: &mut u64, prefix: &str) -> String {
    let id = format!("{prefix}-{counter}");
    *counter += 1;
    id
}

/// Raises `counter` past `id` when `id` is one `next_id(counter, prefix)`
/// could produce, so it's never handed out again.
fn bump_past_id(counter: &mut u64, id: &str, prefix: &str) {
    let n = id.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('-'));
    if let Some(n) = n.and_then(|n| n.parse::<u64>().ok()) {
        *counter = (*counter).max(n.saturating_add(1));
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
/// `self.graph_scene_rect`, which -- confirmed by reading `egui::Scene::
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
/// Once `Scene::show` returns and `self.graph_scene_rect` has been updated
/// to reflect this frame's actual pan/zoom, `project` is recomputed fresh
/// and every queued closure is invoked with it -- so overlay decorations
/// are painted using the *same* up-to-date transform the card content
/// itself already reflects, closing the gap entirely rather than just
/// making it smaller.
type DeferredOverlays = Vec<DeferredOverlay>;

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
/// fixed-screen-text split `draw_node_close_button` above already uses, for
/// the same reason. The label paint itself is deferred -- see
/// `DeferredOverlays`'s own doc comment -- so `overlay_painter`/`project`
/// are no longer parameters here at all. Returns whether it was clicked.
fn draw_new_node_menu_row(
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

/// Graph toolbar filter: when `filter` doesn't match any of `haystacks`,
/// dims the card at `rect` with one black scrim, pushed as a deferred
/// overlay so it paints last (on the same untransformed foreground layer
/// card text uses, so it covers that too, not just the thumbnail/borders
/// below it) rather than threading a dim factor through every draw call.
/// `rounding` matches the card's own corner radius (screen space).
fn dim_if_filtered_out(
    deferred_overlays: &mut DeferredOverlays,
    filter: &str,
    haystacks: &[&str],
    rect: egui::Rect,
    rounding: f32,
) {
    if graph_filter_dims(filter, haystacks) {
        deferred_overlays.push(Box::new(move |overlay_painter, project| {
            overlay_painter.rect_filled(
                egui::Rect::from_min_max(project(rect.min), project(rect.max)),
                rounding,
                egui::Color32::from_black_alpha(190),
            );
        }));
    }
}

/// The per-kind colors and filter-dim rounding a graph card's shared
/// frame (`App::draw_graph_card`) paints with. Note is the one kind on a
/// light (yellow) card, so its border and ✕ are dark instead of the
/// theme's text colors.
struct CardStyle {
    border_hover: egui::Color32,
    border_idle: egui::Color32,
    close_fill: egui::Color32,
    close_border: egui::Color32,
    close_icon: egui::Color32,
    dim_rounding: f32,
}

fn card_style(kind: ItemNodeKind, visuals: &egui::Visuals) -> CardStyle {
    let themed = CardStyle {
        border_hover: visuals.text_color(),
        border_idle: visuals.weak_text_color(),
        close_fill: visuals.extreme_bg_color,
        close_border: visuals.weak_text_color(),
        close_icon: visuals.text_color(),
        dim_rounding: 4.0,
    };
    match kind {
        ItemNodeKind::Note => CardStyle {
            border_hover: egui::Color32::from_black_alpha(200),
            border_idle: egui::Color32::from_black_alpha(120),
            close_fill: NOTE_CARD_COLOR,
            close_border: egui::Color32::from_black_alpha(160),
            close_icon: egui::Color32::from_black_alpha(200),
            dim_rounding: 6.0,
        },
        ItemNodeKind::Download => CardStyle { dim_rounding: 6.0, ..themed },
        _ => themed,
    }
}

/// What a card's body (`App::paint_card_body`) tells the shared frame:
/// which part of the card is the click/drag handle (the whole card,
/// except a Note being edited, where only its header is, so clicks in
/// the text go to the `TextEdit`), and an optional hover tooltip (a
/// failed PDF render's error).
struct CardBody {
    interact_rect: egui::Rect,
    tooltip: Option<String>,
}

/// What `App::draw_graph_card` hands back to `build_graph_ui`, which
/// applies each kind's own click action.
struct CardOutput {
    response: egui::Response,
    close_clicked: bool,
}

/// Small hover-revealed "X" close button at a Graph-view card's top-right
/// corner -- called from `App::draw_graph_card` for all five card kinds
/// (Download/Note/Image/Pdf/Json), previously hand-copied once per kind.
/// Colors are parameters rather than hardcoded, coming from `card_style`:
/// Note's card deliberately uses a different (warm, black-alpha-based)
/// palette than the rest's shared themed one, so the button needs to
/// match whichever card it's drawn on. The icon's own paint is deferred
/// -- see `DeferredOverlays`'s own doc comment -- so
/// `overlay_painter`/`project` are no longer parameters here at all.
/// Returns the button's response: the caller decides what a click means
/// for its own kind, and counts the button's hover as the card's.
fn draw_node_close_button(
    ui: &egui::Ui,
    deferred_overlays: &mut DeferredOverlays,
    card_rect: egui::Rect,
    close_id_salt: (&str, &str),
    inv_zoom: f32,
    base_fill: egui::Color32,
    border_color: egui::Color32,
    icon_color: egui::Color32,
) -> egui::Response {
    let close_rect = egui::Rect::from_center_size(card_rect.right_top(), egui::vec2(18.0, 18.0) * inv_zoom);
    let close_response = ui
        .interact(close_rect, egui::Id::new(close_id_salt), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let close_fill = if close_response.hovered() { DANGER_COLOR } else { base_fill };
    ui.painter().rect_filled(close_rect, 2.0 * inv_zoom, close_fill);
    ui.painter().rect_stroke(
        close_rect,
        2.0 * inv_zoom,
        egui::Stroke::new(1.0 * inv_zoom, border_color),
        egui::StrokeKind::Inside,
    );
    let icon_pos = close_rect.center();
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        overlay_painter.text(
            project(icon_pos),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::light::X,
            egui::FontId::proportional(14.0),
            icon_color,
        );
    }));
    close_response
}

/// Shows a native "Save As" dialog defaulting to `default_name`, filtered
/// to `.json`; if a path was picked, returns it along with the exact
/// "Saved <path>" toast text this app shows for every export (Session,
/// Bookmarks, one Group) on success. Doesn't write anything itself, and
/// deliberately isn't a method on `App` -- the caller still does its own
/// serialization (`self.build_session_snapshot().save_to(path)`, etc.)
/// and sets `self.save_toast`, which a `&mut self` helper here couldn't
/// do without a borrow conflict against a closure also wanting to touch
/// `self`. Previously this dialog-plus-toast tail was hand-copied three
/// times, differing only in the default filename and the filter's label.
fn save_dialog_with_toast(
    default_name: impl Into<String>,
    filter_label: &str,
) -> Option<(std::path::PathBuf, (String, std::time::Instant))> {
    let path = rfd::FileDialog::new()
        .set_file_name(default_name.into())
        .add_filter(filter_label, &["json"])
        .save_file()?;
    let toast = (format!("Saved {}", path.display()), std::time::Instant::now());
    Some((path, toast))
}

/// The bare filename portion of a download's full path, for display --
/// shared by the graph card's own label and the "Downloads (N)" toolbar
/// dropdown so the two never show it differently. Falls back to the whole
/// path if it has no separable filename component (shouldn't normally
/// happen for a real downloaded file's path).
fn download_filename(path: &str) -> &str {
    std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path)
}

/// Recursively renders one `serde_json::Value` inside the JSON viewer's
/// tree -- an object/array as a `CollapsingHeader` (its own children
/// indented beneath, count shown in the header so a collapsed node still
/// tells you how much it holds), a scalar as a plain `"key: value"` label.
/// `top_level` only controls whether *this* node starts expanded --
/// `false` for everything but the document root, so a large real-world
/// payload doesn't dump every nested level open at once.
fn draw_json_value(ui: &mut egui::Ui, key: &str, value: &serde_json::Value, top_level: bool) {
    match value {
        serde_json::Value::Object(map) => {
            egui::CollapsingHeader::new(format!("{key}  {{{}}}", map.len()))
                .default_open(top_level)
                .show(ui, |ui| {
                    for (k, v) in map {
                        draw_json_value(ui, k, v, false);
                    }
                });
        }
        serde_json::Value::Array(items) => {
            egui::CollapsingHeader::new(format!("{key}  [{}]", items.len()))
                .default_open(top_level)
                .show(ui, |ui| {
                    for (i, v) in items.iter().enumerate() {
                        draw_json_value(ui, &i.to_string(), v, false);
                    }
                });
        }
        // `Value`'s own `Display` already renders exactly the compact form
        // wanted here -- a quoted string, a bare number, `true`/`false`, or
        // `null` -- no need to match each scalar variant separately.
        other => {
            ui.label(format!("{key}: {other}"));
        }
    }
}

/// A one-line shape summary for a JSON node's own small graph card -- the
/// full tree only ever shows in `build_json_viewer`'s own popup, so the
/// card needs something glanceable instead: `{ N }`/`[ N ]` for a container,
/// or the scalar's own compact `Display` form (truncated if it would
/// otherwise overflow a card this small).
fn json_summary(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => format!("{{ {} }}", map.len()),
        serde_json::Value::Array(items) => format!("[ {} ]", items.len()),
        other => {
            let text = other.to_string();
            if text.chars().count() > 24 {
                format!("{}…", text.chars().take(23).collect::<String>())
            } else {
                text
            }
        }
    }
}

/// An image node's own card size: `width`x`height` scaled down (never up)
/// to fit inside `IMAGE_NODE_MAX_SIZE` while preserving aspect ratio --
/// shared by the graph card's own layout and `fit_all_nodes_scene_rect`'s
/// bounds union so the two never disagree about how big a given image's
/// card actually is.
/// A restored image/JSON card whose file couldn't be loaded: the same
/// `FILE_X` glyph a failed PDF card shows (see `ImagePayload::error`).
fn paint_missing_file_card(
    ui: &egui::Ui,
    deferred_overlays: &mut DeferredOverlays,
    rect: egui::Rect,
    inv_zoom: f32,
    lod_simplified: bool,
) {
    ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
    if lod_simplified {
        return;
    }
    let icon_pos = rect.center();
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        overlay_painter.text(
            project(icon_pos),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::light::FILE_X,
            egui::FontId::proportional(24.0),
            DANGER_COLOR,
        );
    }));
}

fn image_node_card_size(width: u32, height: u32) -> egui::Vec2 {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (IMAGE_NODE_MAX_SIZE.x / width).min(IMAGE_NODE_MAX_SIZE.y / height).min(1.0);
    egui::vec2(width * scale, height * scale)
}

/// A PDF node's own card size: `PDF_PENDING_NODE_SIZE` while still loading
/// or on a failed render (the real page dimensions aren't known in either
/// case), otherwise the rendered page's own size via `image_node_card_size`
/// -- shared by `fit_all_nodes_scene_rect`'s bounds union, the right-click
/// hit-test, and the graph card's own layout, so all three never disagree.
fn pdf_card_size(pdf: &PdfPayload) -> egui::Vec2 {
    match &pdf.outcome {
        Some(Ok(outcome)) => image_node_card_size(outcome.width, outcome.height),
        _ => PDF_PENDING_NODE_SIZE,
    }
}

/// Any node's own card size, every kind -- the one source the card
/// frame (`App::draw_graph_card`), `bounds_for_level`, the LOD visible
/// count and the right-click hit test all read, so none of them can
/// disagree about how big a given card is (they used to each carry
/// their own per-kind copy, and the LOD count approximated Image/Pdf).
fn node_card_size(payload: &NodePayload) -> egui::Vec2 {
    match payload {
        NodePayload::Tab(_) => GRAPH_NODE_SIZE,
        NodePayload::Download(_) => DOWNLOAD_NODE_SIZE,
        NodePayload::Note(_) => NOTE_NODE_SIZE,
        // A placeholder has no real size: the same box a failed PDF gets.
        NodePayload::Image(image) if image.error.is_some() => PDF_PENDING_NODE_SIZE,
        NodePayload::Image(image) => image_node_card_size(image.width, image.height),
        NodePayload::Pdf(pdf) => pdf_card_size(pdf),
        NodePayload::Json(_) => JSON_NODE_SIZE,
        NodePayload::Group(_) => GROUP_NODE_SIZE,
    }
}

/// `node`'s card in world space: `node_card_size` centered on its position.
fn node_card_rect(node: &GraphNode) -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(node.pos.x, node.pos.y), node_card_size(&node.payload))
}

/// What the Graph filter box matches a Download/Note/Image/Pdf/Json card
/// against: a download's full path, a note's text, a file node's
/// filename. Tab and Group cards match their own fields (title + URL,
/// name) in their own loops and never call this; they get `""`.
fn card_filter_text(payload: &NodePayload) -> &str {
    match payload {
        NodePayload::Download(download) => &download.path,
        NodePayload::Note(note) => &note.text,
        NodePayload::Image(image) => download_filename(&image.path),
        NodePayload::Pdf(pdf) => download_filename(&pdf.path),
        NodePayload::Json(json) => download_filename(&json.path),
        NodePayload::Tab(_) | NodePayload::Group(_) => "",
    }
}

/// A key press as egui receives it from a keyboard, for headless key
/// commands (see `App::debug_input_events`).
fn debug_key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// "Remove from Group" (the tab card's eject button, dragging a card out
/// through the way back, and the tab and item menus): moves `id` up one
/// level, into the parent of the group it's in, since groups nest. `None`
/// (the top level) when that group isn't itself in one.
fn remove_from_group(nodes: &mut NodeMap, id: &str) {
    let grandparent = nodes
        .get(id)
        .and_then(|n| n.group.as_ref())
        .and_then(|group| nodes.get(group))
        .and_then(|g| g.group.clone());
    if let Some(node) = nodes.get_mut(id) {
        node.group = grandparent;
    }
}

/// How many of `ids` (one kind's own node-id list at the current level)
/// have a card (`node_card_rect`) intersecting `viewport` -- the exact
/// same per-node rect the render loops build for their own cull check.
/// Shared by `build_graph_ui`'s LOD pre-pass (see `LOD_SIMPLIFY_COUNT`)
/// so it counts "visible" precisely the way culling already does, rather
/// than a second, slightly different notion of visibility.
fn count_visible_nodes(nodes: &NodeMap, ids: &[String], viewport: egui::Rect) -> usize {
    ids.iter()
        .filter(|id| nodes.get(id.as_str()).is_some_and(|n| node_card_rect(n).intersects(viewport)))
        .count()
}

/// `1234` -> `"1.2 KB"`, binary (1024-based) units -- matches Windows
/// Explorer's own convention for a "how big is this file" readout, which is
/// the nearest existing reference point a download progress display has in
/// this app.
fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn next_tag_color(current: Option<egui::Color32>) -> Option<egui::Color32> {
    match current.and_then(|c| TAG_COLOR_PALETTE.iter().position(|&p| p == c)) {
        None => Some(TAG_COLOR_PALETTE[0]),
        Some(i) if i + 1 < TAG_COLOR_PALETTE.len() => Some(TAG_COLOR_PALETTE[i + 1]),
        Some(_) => None,
    }
}
/// Below this on-screen card width (in points), a node's label is hidden
/// entirely rather than truncated — a card this small can't fit more than
/// one or two letters plus "…" anyway (see `truncate_to_width`), and a
/// near-illegible sliver of text reads as noise, not information, at the
/// zoom levels this app's `GRAPH_ZOOM_MIN` allows.
const MIN_LABEL_CARD_SCREEN_WIDTH: f32 = 40.0;
const GRAPH_THUMB_MARGIN: f32 = 6.0;
const GRAPH_PEEK_SIZE: egui::Vec2 = egui::vec2(360.0, 240.0);
const FAVICON_SIZE: egui::Vec2 = egui::vec2(16.0, 16.0);
/// The graph's world-space center that the physics centering force pulls
/// toward — independent of screen/window coordinates now that the graph is
/// pannable/zoomable (`egui::Scene`), unlike pre-pan/zoom where node
/// positions doubled as screen coordinates directly.
const GRAPH_WORLD_CENTER: egui::Vec2 = egui::Vec2::ZERO;
/// Initial visible world-space rect for the graph's `egui::Scene`, before
/// any user panning/zooming — generous enough to show a handful of settled
/// nodes around `GRAPH_WORLD_CENTER` at 1:1 zoom.
const GRAPH_SCENE_DEFAULT_SIZE: egui::Vec2 = egui::vec2(900.0, 620.0);
const GRAPH_ZOOM_MIN: f32 = 0.15;
const GRAPH_ZOOM_MAX: f32 = 3.0;

/// Fill for the toolbar/chrome bars (Tab view's merged single bar, Graph
/// view's "+ New Tab" bar, the settings popup) — distinct from the
/// graph/content area below.
const TOOLBAR_FILL: egui::Color32 = egui::Color32::from_rgb(29, 26, 20);

fn default_graph_scene_rect() -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(0.0, 0.0), GRAPH_SCENE_DEFAULT_SIZE)
}

/// `%USERPROFILE%\Pictures\manifold-<unix-seconds>.png` — same
/// "pick the obvious default, no save-as dialog" posture `downloads_dir()`
/// (in each engine-host's own download-handling code) already takes, just
/// on the manifold side for the toolbar's "Screenshot" button. The
/// timestamp is the simplest collision-free name that needs no extra
/// state; a real "Save As" dialog would be the alternative, but this
/// project doesn't build custom save UI anywhere else either.
fn screenshot_default_path() -> std::path::PathBuf {
    let dir = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join("Pictures");
    let _ = std::fs::create_dir_all(&dir);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(format!("manifold-{timestamp}.png"))
}

/// A single accent color used throughout — selection highlights, hyperlinks,
/// the focused-tab-strip fill, the focused graph node's border — so the
/// whole chrome reads as one coherent theme rather than egui's flat default
/// dark gray-on-gray. A muted gold rather than the cooler blue this used to
/// be, to sit with the rest of the warm/editorial palette below instead of
/// against it.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(215, 162, 74);

/// The one severity-red used everywhere this app needs to say "warning" or
/// "destructive": a group card's overloaded border, every close/eject
/// button's hover fill, error text, and the muted-audio icon all reference
/// this single constant rather than each re-deriving the same RGB triple --
/// so it reads unambiguously as one meaning across the whole chrome, and a
/// future shade change only has one place to edit. A warm rust rather than a
/// pure red, so it still reads unambiguously as "danger" without clashing
/// with the rest of this palette the way a stop-sign red would.
const DANGER_COLOR: egui::Color32 = egui::Color32::from_rgb(196, 87, 58);
/// A group card's border tints `DANGER_COLOR` once its member count exceeds
/// this threshold -- a cheap glanceable "this one's gotten big" cue. Only
/// applies while the group has no explicit tag color of its own (see its use
/// below) -- a deliberate user choice always wins over this automatic one,
/// same precedent tag colors already have over the plain default `ACCENT`
/// border.
const GROUP_OVERLOAD_THRESHOLD: usize = 8;

/// A sticky note's card fill -- a warm, light color deliberately unlike
/// every other card's dark `extreme_bg_color`/`TOOLBAR_FILL` tone, so a note
/// reads as "a physical note" at a glance rather than another chrome-toned
/// panel. The text drawn on it (`NotePayload`'s editable body) is dark to
/// stay legible against this light fill, the one card in this app that
/// isn't light-text-on-dark.
const NOTE_CARD_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 236, 148);

/// The window/content-area base — behind every panel, the darkest layer.
/// Distinct from `TOOLBAR_FILL` on purpose: egui's own default has exactly
/// one dark gray for everything (panels, buttons, the app background all
/// read as the same flat plane), which is the single biggest reason this
/// app used to look like an unstyled egui demo rather than real chrome.
/// This + `TOOLBAR_FILL` + `SURFACE_RAISED` form a three-step depth ladder
/// (background < panel < raised widget) that egui's stock palette doesn't
/// have at all.
const BG: egui::Color32 = egui::Color32::from_rgb(23, 20, 15);
/// A button/field's own resting fill, one step up from `TOOLBAR_FILL` — the
/// panel it sits on. Also used for the graph card's engine-picker `Frame`.
/// Deliberately a wide jump from `TOOLBAR_FILL`, not a subtle one — the
/// point is a button reads as its own raised chip at a glance, not just
/// under close pixel inspection.
const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(41, 36, 27);
/// Hover state, one step up again — every interactive widget in this app's
/// toolbar/popups visibly lightens on hover, which stock egui's near-
/// identical inactive/hovered grays don't make obvious at a glance.
const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(58, 50, 37);
const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(233, 226, 211);
const TEXT_WEAK: egui::Color32 = egui::Color32::from_rgb(168, 154, 120);

/// Applied once at startup (see `Default for App`). Bumps corner rounding
/// and spacing up from egui's fairly tight defaults (which read as "plain
/// tool window" rather than "app chrome"), replaces egui's stock flat-gray
/// widget palette with the `BG`/`TOOLBAR_FILL`/`SURFACE_RAISED`/
/// `SURFACE_HOVER` depth ladder above, and threads `ACCENT` through
/// selection/hyperlink/focus/active-widget colors so the whole chrome reads
/// as one deliberately designed theme rather than an unstyled egui app.
fn apply_style(ctx: &egui::Context) {
    // `all_styles_mut` (not `style`/`set_style`, egui 0.36 keeps separate
    // dark/light styles internally) applies the same tweaks regardless of
    // which theme the OS/window ends up reporting — this app's chrome is
    // dark-only in practice, but there's no "just style whatever's active"
    // entry point, so set both identically. **Found live**: this used to
    // only tweak specific widget-state fields on top of whichever preset
    // (`Visuals::dark()`/`Visuals::light()`) egui already had loaded for
    // that style slot — under a light OS theme, every field this function
    // doesn't explicitly touch (heading/`.strong()` text color among them)
    // kept the light preset's own near-black default, invisible against
    // this app's always-dark backgrounds (`BG`/`TOOLBAR_FILL`/etc. are
    // forced dark regardless of OS theme). Resetting to `Visuals::dark()`
    // first, for both style slots, means every untouched field starts from
    // a coherent dark-appropriate default before the cosmetic overrides
    // below apply on top.
    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        // Airier than egui's own defaults on purpose -- the editorial
        // direction this app's chrome now takes wants real breathing room in
        // dialogs/menus/settings. The one place this is deliberately *not*
        // wanted -- the graph view's own toolbar, already packed with icon
        // buttons and genuinely screen-space-constrained -- pushes narrower
        // values back onto its own `Ui` locally (see wherever that toolbar
        // is drawn) rather than fighting this global default.
        style.spacing.item_spacing = egui::vec2(10.0, 7.0);
        style.spacing.button_padding = egui::vec2(13.0, 7.0);

        let corner_radius: egui::CornerRadius = 2u8.into();
        style.visuals.widgets.noninteractive.corner_radius = corner_radius;
        style.visuals.widgets.noninteractive.bg_fill = TOOLBAR_FILL;
        style.visuals.widgets.noninteractive.weak_bg_fill = TOOLBAR_FILL;
        style.visuals.widgets.noninteractive.fg_stroke.color = TEXT_WEAK;

        style.visuals.widgets.inactive.corner_radius = corner_radius;
        style.visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
        style.visuals.widgets.inactive.weak_bg_fill = SURFACE_RAISED;
        style.visuals.widgets.inactive.fg_stroke.color = TEXT_PRIMARY;
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, SURFACE_HOVER);

        style.visuals.widgets.hovered.corner_radius = corner_radius;
        style.visuals.widgets.hovered.bg_fill = SURFACE_HOVER;
        style.visuals.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
        style.visuals.widgets.hovered.fg_stroke.color = TEXT_PRIMARY;
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);

        style.visuals.widgets.active.corner_radius = corner_radius;
        style.visuals.widgets.active.bg_fill = ACCENT;
        style.visuals.widgets.active.weak_bg_fill = ACCENT;
        style.visuals.widgets.active.fg_stroke.color = egui::Color32::BLACK;
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);

        style.visuals.widgets.open.corner_radius = corner_radius;
        style.visuals.widgets.open.bg_fill = SURFACE_RAISED;
        style.visuals.widgets.open.weak_bg_fill = SURFACE_RAISED;
        style.visuals.widgets.open.fg_stroke.color = TEXT_PRIMARY;
        style.visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, ACCENT);

        style.visuals.window_corner_radius = 2u8.into();
        style.visuals.menu_corner_radius = 2u8.into();
        style.visuals.window_fill = TOOLBAR_FILL;
        style.visuals.panel_fill = BG;
        style.visuals.extreme_bg_color = BG;
        style.visuals.faint_bg_color = SURFACE_RAISED;
        style.visuals.window_stroke = egui::Stroke::new(1.0, SURFACE_RAISED);

        style.visuals.selection.bg_fill = ACCENT;
        // Warm cream (`TEXT_PRIMARY`), not pure white -- white read cold and
        // slightly out of place against a gold selection fill once the rest
        // of this palette warmed up.
        style.visuals.selection.stroke.color = TEXT_PRIMARY;
        style.visuals.hyperlink_color = ACCENT;
        style.visuals.override_text_color = None;
    });
}

/// The named font family `heading_text` requests for dialog titles and
/// settings-section headers -- kept separate from `FontFamily::Proportional`
/// (body text) so the two can be genuinely different typefaces rather than
/// the same face at a bigger size. Both are static single-weight instances
/// generated from Google's variable-only source files via `fonttools
/// varLib.instancer` (neither Literata nor Newsreader ships a static build
/// upstream any more, and egui/ab_glyph has no variable-axis support of its
/// own -- it would otherwise just rasterize whatever the font's default
/// `fvar` instance happens to be, ignoring any weight/opsz this app wants).
fn heading_family() -> egui::FontFamily {
    egui::FontFamily::Name("heading".into())
}

/// Replaces egui's bundled default (`Ubuntu-Light`) with Literata for body
/// text, inserted ahead of it in the proportional family rather than
/// instead of it — a glyph outside Literata's own coverage still falls back
/// to the bundled default (and from there to egui's bundled emoji/symbol
/// fonts) instead of tofu-boxing. Picked over the OS's own Segoe UI for one
/// concrete reason: `egui`/`ab_glyph` needs the actual font bytes at build
/// time, and there's no equivalent of a browser's "ask the OS for this
/// family" for a native GPU-rendered glyph atlas — bundling a real file was
/// required either way, so the choice came down to which file, not whether
/// to bundle one. Literata is Google's own "built for reading apps" text
/// face (tuned for legibility at small sizes on low-DPI screens), instanced
/// here at a text-appropriate `opsz`, not IBM Plex Serif's old plain
/// Regular-only cut, which read a little soft for UI chrome once compared
/// side by side. Newsreader (also Google's, from the same reading-app
/// design effort, so the pairing is deliberate) supplies just the separate
/// `heading_family()` above, instanced Bold and at a display-sized `opsz`,
/// for the handful of dialog titles/section headers that want more
/// contrast than body text at any weight gives for free.
fn apply_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Literata".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Literata-Regular.ttf"
        ))),
    );
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "Literata".to_owned());
    }
    fonts.font_data.insert(
        "Newsreader".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Newsreader-Bold.ttf"
        ))),
    );
    // Falls back to the same Literata/default chain as body text for any
    // glyph Newsreader's own Latin-focused coverage doesn't have (a heading
    // is still just text -- it shouldn't tofu-box on a character body text
    // would have rendered fine).
    let mut heading_fallback = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    heading_fallback.insert(0, "Newsreader".to_owned());
    fonts.families.insert(heading_family(), heading_fallback);
    // A real icon font, not a symbol glyph gamble -- Literata's own doc
    // comment above notes a theoretical fallback chain to egui's bundled
    // emoji/symbol font, but this project has repeatedly confirmed live
    // that several such glyphs still render as empty tofu boxes regardless
    // (see the pin/mute/audio-indicator dots, all painter-drawn shapes
    // specifically to dodge this). Phosphor's own codepoints live in a
    // private-use range no other font in this chain defines, so there's no
    // coverage ambiguity: every `egui_phosphor::light::*` constant used
    // in this app's chrome is guaranteed to resolve here. `Light` (not
    // `Regular`) to sit better against the now-serif-forward, less heavy
    // typography around it.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Light);
    ctx.set_fonts(fonts);
}

/// A dialog title or settings-section header — `heading_family()` (Newsreader
/// Bold) at a size that reads as a real step up from body text, in
/// `TEXT_PRIMARY`. Every such title in this app's chrome goes through this
/// one helper rather than each popup separately picking a size/weight, so
/// they stay visually identical to each other by construction.
fn heading_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text)
        .font(egui::FontId::new(17.0, heading_family()))
        .color(TEXT_PRIMARY)
}

/// Every fixed-height toolbar-style row in this app (Tab view's own
/// address-bar row, Graph view's own row, the Settings page's back-button
/// row, and the tab-respawn placeholder) goes through this one function
/// now, instead of each hand-rolling the same `Panel::top(id).exact_size
/// (CHROME_HEIGHT_POINTS).frame(...)` shell separately. That duplication
/// was never just repetition -- three of these four had already silently
/// drifted out of sync with each other at least once: the Graph toolbar
/// got a compact-spacing override during a later palette pass (`apply_
/// style` bumped the app-wide default spacing airier, which this fixed
/// row height was never resized for), but the Tab toolbar and the
/// Settings page's own back-button row didn't get the same override --
/// both later overflowed the fixed row height for it, found live on two
/// separate occasions, each requiring its own investigation to notice.
/// A single shared constructor makes that whole *class* of bug
/// structurally impossible to reintroduce one toolbar at a time again:
/// every future toolbar automatically gets the same compact spacing this
/// row height needs, and the same horizontal-scroll overflow safety (see
/// the Graph toolbar's own former comment on this, now here instead) --
/// without a fresh author needing to already know why, or remembering to
/// copy it from whichever toolbar they happened to look at first.
///
/// `content` still owns its own internal layout (`horizontal_centered`
/// vs. `with_layout(.., Align::Min)`, etc.) -- callers differ there for
/// reasons specific to each row (see the Tab toolbar's own `Align::Min`
/// doc comment), so this only ever provides the shared *shell* around it.
fn toolbar_panel(id: &'static str, ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Panel::top(id)
        .exact_size(CHROME_HEIGHT_POINTS)
        .frame(egui::Frame::default().fill(TOOLBAR_FILL).inner_margin(egui::Margin::symmetric(8, 3)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, content);
        });
}

/// The "back to Graph" control every non-Graph item view shares (today,
/// only Tab view; see the design spec's "item-view shared chip" layer).
/// Deliberately not chip-wrapped, unlike the icon clusters elsewhere in
/// this file -- it sits immediately before those clusters in the Tab
/// toolbar's own row, and merging two different composition layers (this
/// shared control and Tab's own per-kind nav icons) into one visual chip
/// would require either layer to know about the other's rendering, which
/// the composition model deliberately keeps separate. A plain button
/// here reads as a clearly distinct action from the icon cluster next to
/// it, which is an acceptable, honest reading of the approved mockup
/// rather than a literal pixel reproduction of it.
fn render_return_to_graph(ui: &mut egui::Ui) -> bool {
    ui.button("Graph").clicked()
}

/// Everything the Tab toolbar's own content (see `render_tab_toolbar_
/// content`) needs to report back to `build_tab_view_ui`, which applies these
/// exactly where the equivalent bare `let mut` locals were applied
/// before this struct existed -- this is a pure "package the same flags
/// as a return value" move, not a behavior change. `navigate` is
/// deliberately NOT a field here: today it's computed by `build_tab_view_ui`
/// itself, after this content returns, from `enter_pressed` plus the
/// suggestion list -- only `enter_pressed` needs to cross the function
/// boundary.
#[derive(Default)]
struct ItemToolbarActions {
    go_back: bool,
    go_forward: bool,
    reload_requested: bool,
    home_requested: bool,
    enter_pressed: bool,
    toggle_bookmark_requested: bool,
    close_split_requested: bool,
    tab_actions_button_rect: Option<egui::Rect>,
    split_engine_button_rect: Option<egui::Rect>,
    address_bar_rect: Option<egui::Rect>,
}

/// What `render_item_toolbar` (the Item toolbar's shared frame) hands back:
/// the frame's own two results plus whatever the per-kind content reported.
struct ItemToolbarFrame {
    /// Layer 2's "Graph" button was clicked.
    back_to_graph: bool,
    /// Layer 1's Help button -- the shortcuts-help popup anchors to it.
    shortcuts_help_button_rect: Option<egui::Rect>,
    actions: ItemToolbarActions,
}

/// Forces the native (OS-drawn, undecorated-in-headless-mode-only -- see
/// `resumed`'s own window setup) titlebar dark, independent of the current
/// Windows theme setting. Without this, a system in light mode draws a
/// bright titlebar directly above this app's now warm-dark chrome -- a
/// visible seam undermining the rest of this theming pass for the one strip
/// of the window this app doesn't control. A full custom-drawn titlebar
/// (matching the palette exactly, not just light/dark) was considered and
/// deliberately deferred -- it would mean reimplementing drag/minimize/
/// maximize/close hit-testing, a real feature addition, not a theme tweak.
/// `DWMWA_USE_IMMERSIVE_DARK_MODE` (20) is undocumented pre-Windows 11 but
/// has shipped stably since a Windows 10 20H1 update; failure is silently
/// ignored (`DwmSetWindowAttribute`'s return value discarded) the same way
/// this app already treats titlebar theming as cosmetic, never fatal.
#[cfg(target_os = "windows")]
fn apply_dark_titlebar(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else { return };
    let hwnd: windows_sys::Win32::Foundation::HWND = win32.hwnd.get() as *mut core::ffi::c_void;
    let enabled: windows_sys::Win32::Foundation::BOOL = 1;
    unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows_sys::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &enabled as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<windows_sys::Win32::Foundation::BOOL>() as u32,
        );
    }
}
#[cfg(not(target_os = "windows"))]
fn apply_dark_titlebar(_window: &Window) {}

/// Shortens `text` to the longest prefix (plus an ellipsis) that lays out
/// no wider than `max_width` at `font_id`, or returns it unchanged if it
/// already fits. Needed because graph-view node labels are now painted at
/// a fixed screen-space font size regardless of zoom (see
/// `build_graph_ui`'s `overlay_painter`/`project`) — at low zoom a node's
/// card shrinks well below the label's natural width, and unlike a normal
/// egui `Label` widget (which can `.truncate()` itself during layout),
/// text painted directly via `Painter::text` has no such built-in limit.
/// A binary search over char-boundary indices rather than a linear
/// shrink-by-one-char loop, since `layout_no_wrap` (one text-shaping pass)
/// is the expensive part and a title could in principle be long.
/// `truncate_to_width`'s cache key -- the exact text and font size (as a
/// bit pattern, since `f32` isn't `Hash`/`Eq`) plus available width
/// *bucketed* to the nearest `TRUNCATE_WIDTH_BUCKET` pixels, not the
/// exact width. Every call site in this app builds its `FontId` via
/// `FontId::proportional`, so font family never needs to be part of the
/// key. See `truncate_to_width`'s own doc comment for why bucketing (not
/// the exact width) is the key.
type TruncateCacheKey = (String, u32, i32);

/// Width granularity `truncate_to_width` bucket its cache key to -- see
/// that function's own doc comment. A truncation result only actually
/// changes at the specific width where one more character's glyph would
/// fit, so nearby widths overwhelmingly share the same correct answer;
/// this just needs to be small enough that a bucket rarely straddles one
/// of those boundaries. Flooring (not rounding) to this bucket size
/// means the width actually used for the search is always <= the real
/// available width, so the visible failure mode of a stale/coarse bucket
/// is "very occasionally one character more truncated than strictly
/// necessary," never overflow past the card's own edge.
const TRUNCATE_WIDTH_BUCKET: f32 = 4.0;

/// Cap on `App::truncate_cache`'s size -- past this it's cleared outright
/// rather than evicted incrementally. No LRU bookkeeping for what should,
/// in steady state, only ever hold a small multiple of "how many labeled
/// cards are visible at once"; a full clear is cheap insurance against
/// unbounded growth if that assumption is ever wrong (e.g. many distinct
/// titles cycling through over a long session).
const TRUNCATE_CACHE_CAP: usize = 2000;

/// Same truncation behavior as before (binary search for the longest
/// prefix, plus "…", that fits `max_width`), but memoized on `cache` --
/// each binary-search step needs a real text-shaping call
/// (`ctx.fonts_mut(...).layout_no_wrap`), and the overwhelming common
/// case frame to frame is "this exact label, this exact width, again",
/// since neither a card's title nor its on-screen width usually change
/// between two consecutive frames. **Found on a general (not just
/// large-graph) performance audit**: this ran unconditionally, every
/// frame, for every visible labeled card, regardless of graph size.
///
/// **`max_width` is bucketed, not used exactly, for both the cache key
/// and the search bound below.** Every call site derives it from
/// `zoom_scale`, which changes continuously (as a raw float) on every
/// frame during an active pan/zoom gesture -- keying on the exact width
/// meant this cache hit approximately never while the user was actually
/// zooming, the one time avoiding repeated font-shaping calls matters
/// most. Found live, following up on the general audit above.
fn truncate_to_width(
    cache: &mut HashMap<TruncateCacheKey, String>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
) -> String {
    let bucketed_width = (max_width / TRUNCATE_WIDTH_BUCKET).floor() * TRUNCATE_WIDTH_BUCKET;
    let width_bucket = (max_width / TRUNCATE_WIDTH_BUCKET).floor() as i32;
    let key: TruncateCacheKey = (text.to_owned(), font_id.size.to_bits(), width_bucket);
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }
    let width_of = |s: &str| -> f32 {
        ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font_id.clone(), egui::Color32::WHITE))
            .size()
            .x
    };
    let result = if bucketed_width <= 0.0 {
        String::new()
    } else if width_of(text) <= bucketed_width {
        text.to_owned()
    } else {
        let chars: Vec<char> = text.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let candidate: String = chars[..mid].iter().collect::<String>() + "…";
            if width_of(&candidate) <= bucketed_width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        if lo == 0 {
            "…".to_owned()
        } else {
            chars[..lo].iter().collect::<String>() + "…"
        }
    };
    if cache.len() >= TRUNCATE_CACHE_CAP {
        cache.clear();
    }
    cache.insert(key, result.clone());
    result
}

fn engine_label(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::Servo => "Servo",
        EngineKind::Cef => "Blink (CEF)",
        EngineKind::Webkit => "WebKit",
        EngineKind::Webview2 => "WebView2",
    }
}

/// A distinct color per engine (loosely nodding to each project's own
/// branding — Servo orange, Chromium/Blink blue, Safari/WebKit teal) used
/// as a small identifying dot/badge wherever a tab's engine matters at a
/// glance: the tab strip, the graph node's engine badge. Deliberately
/// separate from `ACCENT` (which means "focused"/"selected", a different
/// axis of meaning) so the two never get confused for one another.
fn engine_color(kind: EngineKind) -> egui::Color32 {
    match kind {
        EngineKind::Servo => egui::Color32::from_rgb(255, 138, 80),
        EngineKind::Cef => egui::Color32::from_rgb(66, 133, 244),
        EngineKind::Webkit => egui::Color32::from_rgb(26, 188, 156),
        // Windows/Edge blue — distinct enough from CEF's Chromium blue to
        // tell the two apart at a glance despite both being Blink under
        // the hood.
        EngineKind::Webview2 => egui::Color32::from_rgb(0, 120, 212),
    }
}

/// A small filled circle identifying a tab's engine at a glance — used
/// inline in a `ui.horizontal` layout (the tab strip, the engine picker's
/// own dropdown rows), so it allocates its own space rather than being a
/// raw `painter()` call positioned by hand.
fn engine_dot(ui: &mut egui::Ui, kind: EngineKind, diameter: f32) {
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), diameter / 2.0, engine_color(kind));
}

/// Where each engine actually stands in this specific app, not a general
/// verdict on the upstream project -- "Experimental" for Servo is about
/// *this app's own* software-rendering-only integration and Servo's
/// still-incomplete standards coverage, not a claim about Servo itself.
/// Ordered `Tier::One` first for `engine_picker_order` below.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EngineTier {
    Recommended,
    Experimental,
    Limited,
}

impl EngineTier {
    fn label(self) -> &'static str {
        match self {
            EngineTier::Recommended => "Recommended",
            EngineTier::Experimental => "Experimental",
            EngineTier::Limited => "Limited support",
        }
    }

    /// A muted, tier-coded badge color -- deliberately not reusing
    /// `ACCENT`/`DANGER_COLOR` (both already mean something else
    /// elsewhere: "focused" and "destructive") or any `engine_color`
    /// (that axis is "which engine," this one is "how ready is it").
    fn color(self) -> egui::Color32 {
        match self {
            EngineTier::Recommended => egui::Color32::from_rgb(150, 190, 90),
            EngineTier::Experimental => egui::Color32::from_rgb(215, 162, 74),
            EngineTier::Limited => egui::Color32::from_rgb(196, 87, 58),
        }
    }
}

fn engine_tier(kind: EngineKind) -> EngineTier {
    match kind {
        EngineKind::Cef | EngineKind::Webview2 => EngineTier::Recommended,
        EngineKind::Servo => EngineTier::Experimental,
        EngineKind::Webkit => EngineTier::Limited,
    }
}

/// Short, specific claims only -- see `engine_cons` for the matching
/// tradeoff each one of these doesn't mention.
fn engine_pros(kind: EngineKind) -> &'static [&'static str] {
    match kind {
        EngineKind::Cef => &[
            "Full Chromium/Blink compatibility -- broadest real-world site support of the four",
            "Mature, widely embedded elsewhere -- not this app's own experiment",
        ],
        EngineKind::Webview2 => &[
            "Also Blink-based -- same site compatibility as CEF",
            "Uses the OS's own Edge runtime -- no separate browser binary for this app to ship or update",
        ],
        EngineKind::Servo => &[
            "Modern, from-scratch Rust engine -- same language as this app itself",
            "No legacy multi-decade codebase to work around, unlike the other three",
        ],
        EngineKind::Webkit => &[
            "The only non-Blink, non-Servo engine here -- real engine diversity for cross-engine testing",
            "Catches Safari/WebKit-specific site behavior the other three can't show you",
        ],
    }
}

/// Matching tradeoff for each `engine_pros` claim -- specific to how this
/// app integrates each engine today, not a permanent verdict (Servo's own
/// software-rendering limitation here, for instance, is this app's current
/// `SoftwareRenderingContext` choice, not something inherent to Servo).
fn engine_cons(kind: EngineKind) -> &'static [&'static str] {
    match kind {
        EngineKind::Cef => &["Heaviest process footprint of the four"],
        EngineKind::Webview2 => &[
            "Requires the WebView2 Runtime to be installed (usually already is, on Windows 10 1803+/11)",
            "Tied to whatever Edge version is installed system-wide, not pinned by this app",
        ],
        EngineKind::Servo => &[
            "Runs via software (CPU) rendering only in this app today -- slower than the other three",
            "Younger engine -- some sites may render incorrectly or not at all yet",
        ],
        EngineKind::Webkit => &[
            "No native Windows WebKit exists -- runs inside a WSL2 Ubuntu VM, relayed over TCP (no shared memory, higher latency)",
            "Slowest to start (a cold WSL2 boot can take up to 45s); needs WSL2 + Ubuntu + several Linux packages preinstalled",
        ],
    }
}

/// Display order for the engine picker: best-supported first, so a first-
/// time chooser sees the safe defaults before the tradeoffs.
const ENGINE_PICKER_ORDER: [EngineKind; 4] =
    [EngineKind::Cef, EngineKind::Webview2, EngineKind::Servo, EngineKind::Webkit];

/// One engine's own card inside `build_engine_picker_popup` -- name, color
/// dot, tier badge, then its pros/cons side by side. Returns whether this
/// card was clicked this frame (the whole card is the hit target, not just
/// a button within it, so picking an engine doesn't require precision-
/// clicking a small label).
fn draw_engine_card(ui: &mut egui::Ui, kind: EngineKind, is_selected: bool) -> bool {
    let frame_response = egui::Frame::default()
        .fill(if is_selected { SURFACE_HOVER } else { SURFACE_RAISED })
        .corner_radius(2u8)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                engine_dot(ui, kind, 10.0);
                ui.label(egui::RichText::new(engine_label(kind)).strong());
                if is_selected {
                    ui.label(egui_phosphor::light::CHECK_CIRCLE);
                }
                let tier = engine_tier(kind);
                ui.label(egui::RichText::new(tier.label()).small().color(tier.color()));
            });
            ui.add_space(4.0);
            for pro in engine_pros(kind) {
                ui.label(egui::RichText::new(format!("+ {pro}")).small());
            }
            for con in engine_cons(kind) {
                ui.label(egui::RichText::new(format!("\u{2212} {con}")).small().color(TEXT_WEAK));
            }
        });
    // Interacted on *after* the frame's content is laid out, using its own
    // resulting rect -- interacting on `ui.max_rect()` before any content
    // exists gives a hit region sized to whatever was allocated so far
    // (near-zero at that point), not the card's real, final size. Found
    // live: clicking squarely on a card's own visible pros/cons text did
    // nothing, since only a sliver at the very top was ever actually
    // clickable.
    let response = ui
        .interact(frame_response.response.rect, ui.id().with(("engine_card", kind as u8)), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.clicked()
}

/// The shared engine-picker popup -- see `draw_engine_card`'s own doc
/// comment for each card's content. Used by all three places this app lets
/// you choose an engine (Settings' "Default starting engine," Settings'
/// "Current Tab" swap, and the Tab toolbar's "Split" picker), replacing
/// what used to be three separate plain `egui::ComboBox`es with no tier/
/// tradeoff information at all. Returns the popup's own drawn rect (for
/// the caller's `clicked_outside_popup` check) and whichever engine was
/// clicked this frame, if any. `interactable` is the caller's open flag,
/// same as `build_list_popup_frame`'s.
fn build_engine_picker_popup(
    ctx: &egui::Context,
    id: &str,
    anchor_rect: egui::Rect,
    selected: Option<EngineKind>,
    fade: f32,
    interactable: bool,
) -> (egui::Rect, Option<EngineKind>) {
    let mut picked = None;
    let area_rect = egui::Area::new(egui::Id::new(id))
        .fixed_pos(anchor_rect.left_bottom())
        .order(egui::Order::Foreground)
        .interactable(interactable)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            popup_frame(ui)
                .show(ui, |ui| {
                    ui.set_width(340.0);
                    // Same "cap height, let it scroll" shape the other
                    // toolbar list popups (Bookmarks/History/Downloads)
                    // already use -- four cards this rich run well past a
                    // short window's own height otherwise, with the last
                    // one (WebKit, usually) silently unreachable past the
                    // window's bottom edge (confirmed live).
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for (i, kind) in ENGINE_PICKER_ORDER.into_iter().enumerate() {
                            if i > 0 {
                                ui.add_space(6.0);
                            }
                            if draw_engine_card(ui, kind, selected == Some(kind)) {
                                picked = Some(kind);
                            }
                            if i > 0 {
                                ui.add_space(2.0);
                            }
                        }
                    });
                });
        })
        .response
        .rect;
    (area_rect, picked)
}

/// Per-channel linear interpolation between two colors -- `egui`/`epaint`
/// has no built-in `Color32::lerp`. Used to ease a card's border color
/// between its resting and hovered tint instead of snapping instantly,
/// matching the same eased feel `animate_bool_responsive` already gives
/// the hover glow.
fn lerp_color32(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp_u8 = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgba_unmultiplied(
        lerp_u8(a.r(), b.r()),
        lerp_u8(a.g(), b.g()),
        lerp_u8(a.b(), b.b()),
        lerp_u8(a.a(), b.a()),
    )
}

/// A toolbar icon button whose hover fill/text color eases in/out instead
/// of snapping -- egui's stock `Button` picks its resting/hovered colors
/// fresh every frame with no built-in transition of its own (same reason
/// the graph cards needed their own `hover_t` easing earlier this
/// session). `id` must be unique per button *concept*, not per call site --
/// the same id reused across, say, the Tab and Graph toolbars' own gear
/// buttons is fine, since only one of those toolbars is ever showing in a
/// given frame to begin with. One frame of hover-state lag: this frame's
/// color comes from *last* frame's hover (stashed in `ctx` memory), since
/// a button's fill has to be picked before `ui.add` reveals whether the
/// pointer is actually over its not-yet-laid-out rect -- imperceptible at
/// any real frame rate, same "acceptable one-frame lag" the zoom-%
/// toolbar readout and the dot-grid background already accept elsewhere.
/// Deliberately scoped to every *icon-only* toolbar button (Reload/Home/
/// Screenshot/Print-to-PDF/Settings/Keyboard-Shortcuts/Settings-page-Back/
/// Close-Split/Import-Group/Ungroup-All/Collapse-Expand-Groups) rather than
/// every `ui.button` call in the app -- see README.md's own "Toolbar
/// icon-button hover easing" paragraph for why a full sweep
/// wasn't attempted.
fn icon_button_eased(ui: &mut egui::Ui, id: egui::Id, label: impl Into<String>) -> egui::Response {
    let was_hovered: bool = ui.ctx().memory(|m| m.data.get_temp(id).unwrap_or(false));
    let hover_t = ui.ctx().animate_bool_responsive(id, was_hovered);
    let bg = lerp_color32(
        ui.visuals().widgets.inactive.weak_bg_fill,
        ui.visuals().widgets.hovered.weak_bg_fill,
        hover_t,
    );
    let fg = lerp_color32(
        ui.visuals().widgets.inactive.fg_stroke.color,
        ui.visuals().widgets.hovered.fg_stroke.color,
        hover_t,
    );
    let response = ui.add(egui::Button::new(egui::RichText::new(label.into()).color(fg)).fill(bg));
    ui.ctx().memory_mut(|m| m.data.insert_temp(id, response.hovered()));
    response
}

fn chip_icon_button(ui: &mut egui::Ui, id: egui::Id, icon: impl Into<String>) -> egui::Response {
    chip_icon_button_enabled(ui, id, icon, true)
}

/// Same as `chip_icon_button`, but for icons that need a disabled state
/// inside a chip (e.g. nav back/forward, dimmed when there's no history) --
/// `chip_icon_button` alone can't express `ui.add_enabled`'s disabled
/// styling since it always calls `ui.add`.
///
/// Same hover-eased feedback as `icon_button_eased`, but with no visible
/// border ever (idle or hovered) and a transparent idle fill -- for icons
/// already sitting inside a shared background chip (see `toolbar_chip`),
/// where `icon_button_eased`'s own always-drawn box reads as a border
/// nested inside the chip's own border. Deliberately still a real,
/// fully-framed `egui::Button` (not `.frame(false)`, which shrinks the
/// widget's own allocated size well below a normal button's and was
/// found live to make a chip noticeably shorter than its "+ New Tab"/
/// "Reset View" neighbors) -- keeping the frame but making it invisible
/// at rest matches every sibling button's sizing exactly while still
/// hiding the box until the icon is hovered, at which point a soft fill
/// (no stroke) confirms the hit target.
fn chip_icon_button_enabled(ui: &mut egui::Ui, id: egui::Id, icon: impl Into<String>, enabled: bool) -> egui::Response {
    let was_hovered: bool = ui.ctx().memory(|m| m.data.get_temp(id).unwrap_or(false));
    let hover_t = ui.ctx().animate_bool_responsive(id, was_hovered);
    let bg = lerp_color32(egui::Color32::TRANSPARENT, ui.visuals().widgets.hovered.weak_bg_fill, hover_t);
    let fg = lerp_color32(
        ui.visuals().widgets.inactive.fg_stroke.color,
        ui.visuals().widgets.hovered.fg_stroke.color,
        hover_t,
    );
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(icon.into()).color(fg))
            .fill(bg)
            .stroke(egui::Stroke::NONE),
    );
    ui.ctx().memory_mut(|m| m.data.insert_temp(id, response.hovered()));
    response
}

/// Paints a raised chip background behind `content` -- e.g. a cluster of
/// `chip_icon_button`s that should read as one grouped control instead of
/// a flat row of buttons. Uses egui's "reserve then backfill" idiom
/// (`Shape::Noop` now, `Painter::set` once the real size is known) since
/// the chip's own size depends on content not yet laid out.
///
/// `content` MUST run directly against the `ui` it's given -- never
/// inside its own nested `ui.horizontal`/`ui.new_child`. A nested Ui
/// becomes an independently-sized layout block that the OUTER row then
/// centers (or top-aligns) using its own possibly-smaller reported
/// height, which is exactly what caused a confirmed-live ~5px vertical
/// misalignment in the `toolbar-chip-wip` prototype: the outer row
/// centered the nested block within its own taller row height, and no
/// amount of forcing the nested block's *height* fixed it, because the
/// bug was the block's *existence*, not its size. `content` must instead
/// call the SAME `ui` for every widget (making them direct siblings of
/// whatever sits before/after the chip in the row) and return the union
/// of its own widgets' rects -- exact by construction, since each
/// widget's own returned `Response::rect` is real.
fn toolbar_chip(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> egui::Rect) {
    let chip_bg_slot = ui.painter().add(egui::Shape::Noop);
    let (lead_rect, _) =
        ui.allocate_exact_size(egui::Vec2::new(5.0, 0.0), egui::Sense::hover());
    let content_rect = content(ui);
    let (trail_rect, _) =
        ui.allocate_exact_size(egui::Vec2::new(5.0, 0.0), egui::Sense::hover());
    let rect = lead_rect.union(content_rect).union(trail_rect);
    // Content that renders nothing can leave `ui.cursor()`'s fallback
    // infinite in one axis (`.max.x` for left-to-right layouts, `.min.x`
    // for right-to-left) -- skip painting rather than filling an
    // unbounded rect.
    if rect.is_finite() {
        ui.painter().set(chip_bg_slot, egui::Shape::rect_filled(rect, 6.0, TOOLBAR_FILL.gamma_multiply(1.6)));
    }
}

/// Multiplicative step for one click of a zoom chip's −/+ (and, in Image
/// view, one `Ctrl+=`/`Ctrl+-`) -- shared so Graph and Image zoom feel the
/// same.
const ZOOM_STEP: f32 = 1.25;

/// What a click on `render_zoom_chip` asked for. The chip holds no zoom
/// state of its own -- each caller applies this to whatever "zoom" means
/// in its own view (Graph: the camera's `graph_scene_rect`; Image view:
/// `ImageViewState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ZoomChipAction {
    Out,
    In,
    Reset100,
    Fit,
}

/// `rect` (a Graph camera's world-space `graph_scene_rect`) resized about
/// its own center so the zoom scale `graph_world_to_screen_transform`
/// derives from it (`screen / rect.size()`, min axis) is multiplied by
/// `factor`, clamped to `min..=max`. The ratio is computed from the raw,
/// unclamped scale so the result lands exactly on the clamped target.
fn step_scene_rect(rect: egui::Rect, factor: f32, screen: egui::Vec2, min: f32, max: f32) -> egui::Rect {
    let raw = (screen / rect.size()).min_elem();
    let new_scale = (raw.clamp(min, max) * factor).clamp(min, max);
    egui::Rect::from_center_size(rect.center(), rect.size() * (raw / new_scale))
}

/// −, a clickable `NN%` readout (back to 100%), +, and Fit, as one chip.
/// Left-to-right rows only (both callers are): in a right-to-left `Ui`
/// its buttons would come out reversed -- see `in_reading_order` for the
/// pattern if a right-to-left caller ever needs it. The readout is a
/// frameless `Button` rather than a `Label` so it has the same height as
/// its icon-button neighbors (a `Label` sits high in the Item toolbar's
/// top-aligned row), with a fixed minimum width so the chip doesn't
/// jitter as the digit count changes.
fn render_zoom_chip(
    ui: &mut egui::Ui,
    pct: f32,
    can_zoom_out: bool,
    can_zoom_in: bool,
    fit_hover: &str,
) -> Option<ZoomChipAction> {
    debug_assert_eq!(
        ui.layout().main_dir(),
        egui::Direction::LeftToRight,
        "render_zoom_chip lays out left to right only"
    );
    let mut action = None;
    toolbar_chip(ui, |ui| {
        let out = chip_icon_button_enabled(
            ui,
            egui::Id::new("icon_btn_zoom_out"),
            egui_phosphor::light::MAGNIFYING_GLASS_MINUS,
            can_zoom_out,
        )
        .on_hover_text("Zoom out");
        if out.clicked() {
            action = Some(ZoomChipAction::Out);
        }
        let readout = ui
            .add(
                egui::Button::new(format!("{}%", pct.round() as i32))
                    .frame(false)
                    .min_size(egui::vec2(44.0, 0.0)),
            )
            .on_hover_text("Reset zoom to 100%")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if readout.clicked() {
            action = Some(ZoomChipAction::Reset100);
        }
        let zoom_in = chip_icon_button_enabled(
            ui,
            egui::Id::new("icon_btn_zoom_in"),
            egui_phosphor::light::MAGNIFYING_GLASS_PLUS,
            can_zoom_in,
        )
        .on_hover_text("Zoom in");
        if zoom_in.clicked() {
            action = Some(ZoomChipAction::In);
        }
        let fit = chip_icon_button(ui, egui::Id::new("icon_btn_zoom_fit"), egui_phosphor::light::FRAME_CORNERS)
            .on_hover_text(fit_hover);
        if fit.clicked() {
            action = Some(ZoomChipAction::Fit);
        }
        out.rect.union(readout.rect).union(zoom_in.rect).union(fit.rect)
    });
    action
}

const IMAGE_ZOOM_MIN: f32 = 0.1;
const IMAGE_ZOOM_MAX: f32 = 16.0;
/// Image view mouse-wheel sensitivity: zoom factor per point of vertical
/// scroll, applied as `exp(delta * rate)` so equal scroll amounts in and
/// out cancel exactly.
const IMAGE_WHEEL_ZOOM_RATE: f32 = 0.002;

/// `render_zoom_chip`'s own on-screen width in the Image toolbar row,
/// including the gap `render_image_toolbar_content`'s identity label
/// leaves before it -- measured live (cursor-position delta across the
/// `render_zoom_chip` call, 1300×800 window) since the chip is drawn
/// after the label in the same row and so isn't known yet when the label
/// picks its own max width.
const IMAGE_ZOOM_CHIP_RESERVED_WIDTH: f32 = 171.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ImageZoom {
    /// Contain within the body, never above 100% (`image_fit_scale`).
    Fit,
    Scale(f32),
}

/// Image view's zoom/pan. `pan` is the image center's offset from the
/// body center, in points; ignored while `zoom` is `Fit`. Reset to the
/// default (Fit) every time an Image enters Item view; not persisted.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ImageViewState {
    zoom: ImageZoom,
    pan: egui::Vec2,
}

impl Default for ImageViewState {
    fn default() -> Self {
        Self { zoom: ImageZoom::Fit, pan: egui::Vec2::ZERO }
    }
}

impl ImageViewState {
    /// The concrete `(scale, pan)` to draw with for an `image`-sized
    /// picture in a `body`-sized area -- Fit resolved, pan re-clamped (the
    /// window may have been resized since it was stored).
    fn effective(&self, image: egui::Vec2, body: egui::Vec2) -> (f32, egui::Vec2) {
        match self.zoom {
            ImageZoom::Fit => (image_fit_scale(image, body), egui::Vec2::ZERO),
            ImageZoom::Scale(scale) => (scale, clamp_image_pan(self.pan, image * scale, body)),
        }
    }
}

/// An Image's pixel size as a `Vec2` (at least 1×1).
fn image_pixel_size(image: &ImagePayload) -> egui::Vec2 {
    egui::vec2(image.width.max(1) as f32, image.height.max(1) as f32)
}

/// Contain `image` within `body`, never above 1.0 -- same "scale down,
/// never up" rule `image_node_card_size` uses, so a small image isn't
/// blurred by magnification it didn't ask for.
fn image_fit_scale(image: egui::Vec2, body: egui::Vec2) -> f32 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    (body.x / image.x).min(body.y / image.y).clamp(IMAGE_ZOOM_MIN, 1.0)
}

/// Whether a just-(re)spawned tab's engine view should be shown, used by
/// `handle_tab_spawn_result`. The focused tab stays hidden while Image view
/// is up front of it (relaunching straight into Image view, or a crash
/// respawn of the tab hidden behind an open image) -- `focused` keeps
/// pointing at it per the Item-view design (see the design doc's Plan-time
/// corrections on `App::focused`), but nothing should render or receive
/// input for it until the image is closed. A split's secondary pane is
/// always visible regardless, same as before this fn existed.
fn spawned_tab_visible(is_focused: bool, is_split: bool, image_view_showing: bool) -> bool {
    (is_focused && !image_view_showing) || is_split
}

fn image_display_rect(body: egui::Rect, image: egui::Vec2, scale: f32, pan: egui::Vec2) -> egui::Rect {
    egui::Rect::from_center_size(body.center() + pan, image * scale)
}

/// Per axis: when the scaled image is larger than the body, no gap may
/// open past its edges; when it fits, it stays centered on that axis.
fn clamp_image_pan(pan: egui::Vec2, scaled: egui::Vec2, body: egui::Vec2) -> egui::Vec2 {
    let axis = |pan: f32, scaled: f32, body: f32| {
        let slack = (scaled - body) / 2.0;
        if slack <= 0.0 { 0.0 } else { pan.clamp(-slack, slack) }
    };
    egui::vec2(axis(pan.x, scaled.x, body.x), axis(pan.y, scaled.y, body.y))
}

/// Zooms by `factor` (clamped to `IMAGE_ZOOM_MIN..=IMAGE_ZOOM_MAX`) so the
/// image point under `anchor` (relative to the body center) stays put.
fn zoom_about(scale: f32, pan: egui::Vec2, factor: f32, anchor: egui::Vec2) -> (f32, egui::Vec2) {
    let new_scale = (scale * factor).clamp(IMAGE_ZOOM_MIN, IMAGE_ZOOM_MAX);
    let ratio = new_scale / scale;
    (new_scale, anchor - (anchor - pan) * ratio)
}

/// Applies a zoom-chip click (or its keyboard equivalent) to Image view's
/// state. −/+ zoom about the body center.
fn image_view_after_action(
    state: ImageViewState,
    action: ZoomChipAction,
    image: egui::Vec2,
    body: egui::Vec2,
) -> ImageViewState {
    match action {
        ZoomChipAction::Fit => ImageViewState::default(),
        ZoomChipAction::Reset100 => ImageViewState { zoom: ImageZoom::Scale(1.0), pan: egui::Vec2::ZERO },
        ZoomChipAction::In | ZoomChipAction::Out => {
            let factor = if action == ZoomChipAction::In { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
            let (scale, pan) = state.effective(image, body);
            let (scale, pan) = zoom_about(scale, pan, factor, egui::Vec2::ZERO);
            ImageViewState { zoom: ImageZoom::Scale(scale), pan: clamp_image_pan(pan, image * scale, body) }
        }
    }
}

/// Calls `first` then `second` so that their widgets read first-then-second
/// left to right in either layout direction. A right-to-left `ui` places
/// each new widget to the *left* of the previous one, so there the calls
/// are made in reverse. `cx` is threaded through rather than captured so
/// both callbacks can take the same `&mut` state (e.g. `&mut App`).
fn in_reading_order<C: ?Sized, A, B>(
    ui: &mut egui::Ui,
    cx: &mut C,
    first: impl FnOnce(&mut C, &mut egui::Ui) -> A,
    second: impl FnOnce(&mut C, &mut egui::Ui) -> B,
) -> (A, B) {
    if ui.layout().main_dir() == egui::Direction::RightToLeft {
        let b = second(cx, ui);
        let a = first(cx, ui);
        (a, b)
    } else {
        let a = first(cx, ui);
        let b = second(cx, ui);
        (a, b)
    }
}

/// The actual shapes `draw_card_glow` paints -- split out so a caller whose
/// rect isn't known until *after* its content is laid out (the save toast:
/// an `egui::Frame` that auto-sizes to its own label) can still put the
/// glow visually *behind* that content. `Painter::add` only ever appends
/// (there's no "insert behind" for shapes already painted), so that caller
/// pre-reserves placeholder slots with `Shape::Noop` before drawing its
/// frame, then retroactively fills them in with these via `Painter::set`
/// once the frame's real rect is known -- the standard egui pattern for
/// this, also how e.g. selection highlights behind not-yet-laid-out text
/// are done. A graph card's rect is fixed upfront (`GRAPH_NODE_SIZE`/
/// `GROUP_NODE_SIZE`), so `draw_card_glow` itself just paints these
/// directly, immediately, without needing this indirection.
fn card_glow_shapes(
    rect: egui::Rect,
    base_corner_radius: f32,
    color: egui::Color32,
    inv_zoom: f32,
    intensity: f32,
) -> [egui::Shape; 2] {
    [(85u8, 3.5_f32), (38u8, 8.5)].map(|(alpha, grow)| {
        let alpha = ((alpha as f32) * intensity).min(255.0) as u8;
        egui::Shape::rect_filled(
            rect.expand(grow * inv_zoom),
            (base_corner_radius + grow) * inv_zoom,
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha),
        )
    })
}

/// A soft colored glow behind a graph card (tab or group), grown outward
/// from its own rect with decreasing alpha -- used instead of a plain black
/// drop-shadow, which was tried first and found live to be invisible
/// against this app's near-black `BG` (nothing darker to contrast against).
/// Tinted the card's own border/tag color rather than a neutral shade, so
/// it doubles as an echo of that color-coding; no offset (radiates evenly
/// outward) since it isn't meant to imply a directional light source.
/// `intensity` (1.0 = normal) scales the alpha only, not the spread --
/// a hover "lift" cue, brightening the existing glow rather than resizing
/// the card itself, which would misalign it against its own thumbnail/
/// label content (drawn at the card's un-hovered rect).
fn draw_card_glow(
    painter: &egui::Painter,
    rect: egui::Rect,
    base_corner_radius: f32,
    color: egui::Color32,
    inv_zoom: f32,
    intensity: f32,
) {
    for shape in card_glow_shapes(rect, base_corner_radius, color, inv_zoom, intensity) {
        painter.add(shape);
    }
}

/// A parent-child graph edge, bowed into a gentle quadratic-bezier curve
/// instead of a straight `line_segment` -- purely cosmetic softening, not
/// port-based routing (this graph's nodes are free-positioned by physics,
/// not laid out left-to-right with fixed input/output sides, so a "smart"
/// horizontal S-curve like most node editors use would look wrong here;
/// a curve that just bows perpendicular to the straight line works for an
/// edge in any direction). Bow amount is proportional to edge length
/// (capped at 30 world units) so short edges between adjacent nodes don't
/// over-bow into an ugly loop.
fn draw_graph_edge(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, stroke: egui::Stroke) {
    let delta = to - from;
    let dist = delta.length();
    let bow = (dist * 0.15).min(30.0);
    let perp = egui::vec2(-delta.y, delta.x) / dist.max(0.0001) * bow;
    let control = from + delta * 0.5 + perp;
    painter.add(egui::epaint::QuadraticBezierShape::from_points_stroke(
        [from, control, to],
        false,
        egui::Color32::TRANSPARENT,
        stroke,
    ));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Graph,
    /// One node shown full-screen ("maximized"): a Tab's live page, or --
    /// while `App::viewed_image` is `Some` -- an Image. Was `Tab` before a
    /// second kind could enter it; `session::Session::view_mode_tab` keeps
    /// the old name on disk.
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
enum TransitionKind {
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
/// switching `view_mode` instantly. See `App::transition_content_rect_points`
/// (Tab) and `App::image_transition_frame` (Image).
struct Transition {
    node_id: String,
    kind: TransitionKind,
    start: std::time::Instant,
}

const TRANSITION_DURATION: std::time::Duration = std::time::Duration::from_millis(220);

/// A `Transition`'s current rect: smoothstep-eased between the Item-view
/// end (`item_rect`) and the graph-card end (`graph_rect`), in whichever
/// direction `transition.kind` runs.
fn transition_eased_rect(transition: &Transition, item_rect: egui::Rect, graph_rect: egui::Rect) -> egui::Rect {
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

/// An in-progress eased pan/zoom from one `graph_scene_rect` to another --
/// entering/leaving a group used to snap the camera to the new level's
/// `fit_all_nodes_scene_rect()` instantly; this animates the same jump over
/// `TRANSITION_DURATION` instead, same smoothstep easing as `Transition`
/// above (see `App::eased_graph_scene_rect`). Deliberately a separate,
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
struct GraphCameraTween {
    from: egui::Rect,
    to: egui::Rect,
    start: std::time::Instant,
}

/// A background pan drag released with enough speed keeps the camera
/// gliding and decelerating instead of stopping dead the instant the
/// mouse button lifts -- the same "flick to scroll" feel most pan/zoom
/// canvases (Figma, Google Maps, Miro) already have. `initial_velocity`
/// is world-space units/second, sampled from `App::pan_drag_history`'s
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
struct PanMomentum {
    initial_velocity: egui::Vec2,
    start: std::time::Instant,
    start_center: egui::Pos2,
}

/// Exponential decay rate (`k` in `v(t) = v0 * e^-kt`, per second) for
/// `PanMomentum` -- tuned for a glide that's clearly perceptible but
/// settles well under a second, not a floaty drift.
const PAN_MOMENTUM_DECAY: f32 = 3.5;
/// Minimum release speed (world-space units/second, from `pan_drag_
/// history`) for a released pan drag to start gliding at all -- below
/// this, an ordinary slow drag-and-let-go just stops, matching what a
/// user who wasn't trying to "flick" the canvas would expect.
const PAN_MOMENTUM_MIN_SPEED: f32 = 500.0;
/// Hard cap on how long a `PanMomentum` stays active, regardless of
/// `PAN_MOMENTUM_DECAY` -- at that rate, decay is already below 1% of
/// the initial speed well before this, so this is just a bound on the
/// `Option` field's own lifetime, not a value that shapes the motion.
const PAN_MOMENTUM_MAX_DURATION: std::time::Duration = std::time::Duration::from_millis(2000);
/// How many recent camera-center samples `App::pan_drag_history` keeps
/// while a background pan drag is held down -- spans roughly 100ms at a
/// steady frame rate, matching the same window egui's own `PointerState::
/// velocity()` uses internally for its analogous smoothing.
const PAN_HISTORY_LEN: usize = 6;

/// How long a "Saved <path>" toast stays on screen — see `App::save_toast`'s
/// doc comment.
const SAVE_TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(4);

/// Kind-specific data for one node in the unified graph-node model --
/// `GraphNode` holds the fields every kind shares (position, graph
/// membership); this holds only what varies per kind. Tab/Download/Group
/// were the migration off three separate hardcoded node types (the old
/// `TabNode`/`DownloadNode`/`GroupNode`); `Note`/`Image`/`Pdf`/`Json` are
/// genuinely new kinds added on top of that unified model -- see each one's
/// own `*Payload` doc comment.
enum NodePayload {
    Tab(TabPayload),
    Download(DownloadPayload),
    Group(GroupPayload),
    Note(NotePayload),
    Image(ImagePayload),
    Pdf(PdfPayload),
    Json(JsonPayload),
}

impl NodePayload {
    fn as_tab(&self) -> Option<&TabPayload> {
        match self {
            NodePayload::Tab(t) => Some(t),
            _ => None,
        }
    }

    fn as_tab_mut(&mut self) -> Option<&mut TabPayload> {
        match self {
            NodePayload::Tab(t) => Some(t),
            _ => None,
        }
    }

    fn as_download(&self) -> Option<&DownloadPayload> {
        match self {
            NodePayload::Download(d) => Some(d),
            _ => None,
        }
    }

    fn as_download_mut(&mut self) -> Option<&mut DownloadPayload> {
        match self {
            NodePayload::Download(d) => Some(d),
            _ => None,
        }
    }

    fn as_group(&self) -> Option<&GroupPayload> {
        match self {
            NodePayload::Group(g) => Some(g),
            _ => None,
        }
    }

    fn as_group_mut(&mut self) -> Option<&mut GroupPayload> {
        match self {
            NodePayload::Group(g) => Some(g),
            _ => None,
        }
    }

    fn as_note(&self) -> Option<&NotePayload> {
        match self {
            NodePayload::Note(n) => Some(n),
            _ => None,
        }
    }

    fn as_note_mut(&mut self) -> Option<&mut NotePayload> {
        match self {
            NodePayload::Note(n) => Some(n),
            _ => None,
        }
    }

    fn as_image(&self) -> Option<&ImagePayload> {
        match self {
            NodePayload::Image(i) => Some(i),
            _ => None,
        }
    }

    fn as_pdf(&self) -> Option<&PdfPayload> {
        match self {
            NodePayload::Pdf(p) => Some(p),
            _ => None,
        }
    }

    fn as_pdf_mut(&mut self) -> Option<&mut PdfPayload> {
        match self {
            NodePayload::Pdf(p) => Some(p),
            _ => None,
        }
    }

    fn as_json(&self) -> Option<&JsonPayload> {
        match self {
            NodePayload::Json(j) => Some(j),
            _ => None,
        }
    }
}

/// A cheap, owned, `Copy` classifier for `&NodePayload` -- computed once
/// up front so its immutable borrow of `self.nodes` ends immediately,
/// before a dispatcher needs `&mut self` to render that kind's toolbar
/// content. Mirrors `close_node`'s own match for exactly the same reason:
/// matching directly on `&NodePayload` would keep that borrow alive for as
/// long as the match arms run, which conflicts with the `&mut self` those
/// arms need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemNodeKind {
    Tab,
    Download,
    Group,
    Note,
    Image,
    Pdf,
    Json,
}

fn item_node_kind(payload: &NodePayload) -> ItemNodeKind {
    match payload {
        NodePayload::Tab(_) => ItemNodeKind::Tab,
        NodePayload::Download(_) => ItemNodeKind::Download,
        NodePayload::Group(_) => ItemNodeKind::Group,
        NodePayload::Note(_) => ItemNodeKind::Note,
        NodePayload::Image(_) => ItemNodeKind::Image,
        NodePayload::Pdf(_) => ItemNodeKind::Pdf,
        NodePayload::Json(_) => ItemNodeKind::Json,
    }
}

/// A sticky note's own data -- free-form user-typed text, no engine process
/// or IPC of any kind behind it (unlike every other node kind so far). The
/// first new kind added on top of the unified `GraphNode`/`NodePayload`
/// model, rather than another migration of a pre-existing type -- proof the
/// model actually supports "arrange anything" the way it was built to.
/// Persisted in `session.json` (`session::SessionNote`) since a note is
/// user-authored content, not a display preference like `TabPayload::pinned`
/// or a group's own membership -- losing it on restart would defeat the
/// point of writing one down.
struct NotePayload {
    text: String,
    /// `true` shows a real, focused `TextEdit` (today's own behavior);
    /// `false` renders `text` as real Markdown (`egui_commonmark`) instead,
    /// read-only, with the whole card (not just its header strip) clickable
    /// to switch back to editing -- the same "click to edit, blur to
    /// render" split Obsidian/Notion use, since a rendered view has no
    /// natural place to put a text cursor. Session-only, like `TabPayload::
    /// pinned` -- not persisted (`session::SessionNote` still only holds
    /// `text`), so a restored note always starts rendered regardless of
    /// what it was showing when the app last closed. A freshly created
    /// note starts `true` instead, so typing into it works immediately
    /// with no extra click.
    editing: bool,
}

/// A locally-added picture, embedded directly on the graph -- the second
/// new node kind built on the unified model (see `NotePayload`'s own doc
/// comment for the first). Unlike a tab's thumbnail or a favicon, this
/// isn't fetched or re-rendered continuously: `path` is decoded once, at
/// creation (`App::add_image_node`, via a real file picker -- blocks on
/// OS/human input, so unavailable in headless mode, same reasoning every
/// other file-dialog action here has) or at session restore, and the
/// resulting texture (`GpuState::images`, keyed by this node's own id)
/// never changes after that. `width`/`height` are the *decoded* image's
/// real pixel dimensions, kept so the card can size itself to the image's
/// own aspect ratio instead of stretching/squashing it into a fixed box.
/// Persisted in `session.json` (`session::SessionImage`) as just the file
/// path -- same "re-derive, don't duplicate the bytes" posture a restored
/// tab's fresh navigation already has, rather than embedding the picture's
/// own bytes a second time. If the file has since moved or been deleted,
/// restore keeps a placeholder card for it (`error`) rather than failing
/// the whole session load or forgetting the card.
struct ImagePayload {
    path: String,
    width: u32,
    height: u32,
    /// `Some` for a card restored from `session.json` whose file couldn't
    /// be opened: no texture, `width`/`height` 0, drawn like a failed PDF
    /// with this message as its tooltip. It stays in the session, so the
    /// next launch tries the file again.
    error: Option<String>,
}

/// A locally-added PDF, rendered as its own graph node -- the third new
/// kind built on the unified model. Unlike an image, rendering a PDF page
/// is genuinely slow the first time (`pdf_fetch::PdfFetcher` may need to
/// download and cache a real `pdfium.dll` before it can render anything at
/// all), so this can't decode synchronously at creation the way
/// `ImagePayload` does -- `outcome` starts `None` (rendering in the
/// background) and is filled in once `PdfFetcher::tick` reports a result
/// for this node's own id, the same "spawn a thread, `mpsc` channel, main
/// loop `tick()`s it" pattern `SuggestionFetcher`/`FaviconFetcher` already
/// use. Only the first page is ever rendered -- no page-navigation UI yet,
/// same "start with the simplest useful slice" scope Notes' own initial cut
/// had for its context menu/drag-into-group (both added in a later step).
/// Persisted in `session.json` (`session::SessionPdf`) as just the file
/// path, same "re-derive, don't duplicate" posture `ImagePayload` already
/// has -- restoring re-renders the first page from scratch rather than
/// caching the rendered bitmap anywhere.
struct PdfPayload {
    path: String,
    /// `None` while the background render is still in flight. `Some(Ok(_))`
    /// once the first page's texture is ready in `GpuState::images` (keyed
    /// by this node's own id -- the exact same map/texture path `Image`
    /// nodes use, since a rendered PDF page is just another RGBA bitmap by
    /// the time it reaches the GPU). `Some(Err(_))` if the background
    /// render failed (bad file, corrupt PDF, `pdfium.dll` download/bind
    /// failure) -- shown in place of the page image, distinct from "still
    /// loading" so a real failure isn't mistaken for a hang.
    outcome: Option<Result<PdfOutcome, String>>,
}

struct PdfOutcome {
    width: u32,
    height: u32,
    page_count: usize,
}

/// A locally-added JSON file, embedded directly on the graph -- the fourth
/// new kind built on the unified model. Unlike `Image`/`Pdf`, there's no GPU
/// texture at all: the card is drawn with plain egui widgets (a compact
/// shape summary), and the full content reuses the exact same collapsible-
/// tree popup (`draw_json_value`/`build_json_viewer`) already built for a
/// tab's own "View as JSON" -- see `App::json_viewer_node`'s own doc comment
/// for how that popup now serves both sources. `value` is parsed once, at
/// creation (`App::add_json_node`, via a real file picker, same
/// headless-unavailable posture `ImagePayload` already has) or at session
/// restore -- like `ImagePayload`, a bad/no-longer-JSON file just means
/// that one node is skipped at restore rather than failing the whole
/// session load.
struct JsonPayload {
    path: String,
    value: serde_json::Value,
    /// The JSON key (or `[index]` for an array element) this value came
    /// from -- `None` for a genuine top-level file-backed JSON node (its
    /// card just shows the value's own shape), `Some(k)` for a leaf created
    /// by "Expand as Graph" (see `App::expand_json_entries`), whose card
    /// shows `"{k}: {value}"` instead so it's clear which field this is.
    /// Never persisted -- exploded leaves are session-only, matching
    /// `GroupPayload`'s own doc comment for the containers they live in.
    key: Option<String>,
    /// Same as `ImagePayload::error`: a restored card whose file couldn't
    /// be loaded (`value` is `Null`).
    error: Option<String>,
}

/// A tab's own data -- was `TabNode`'s non-shared fields (`parent`/`group`/
/// `pos`/`vel`/`color` moved to the now-shared `GraphNode`). See
/// `NodePayload`'s own doc comment.
struct TabPayload {
    tab: EngineTab,
    url: String,
    title: String,
    can_go_back: bool,
    can_go_forward: bool,
    loading: bool,
    /// Shown as a speaker icon on this tab's graph node — see
    /// `ipc_protocol::EngineToHost::AudioPlayingChanged`'s doc comment for
    /// which engines actually send this (WebView2, WebKit; never CEF or
    /// Servo, so this just stays `false` for those tabs).
    audio_playing: bool,
    /// User-toggled via the toolbar's "Muted" button — host-tracked (not
    /// read back from the engine), see `ipc_protocol::HostToEngine::
    /// SetMuted`'s doc comment. Resets to `false` on a fresh spawn/respawn/
    /// engine swap, same as every other per-tab UI-only flag here (nothing
    /// about mute state survives a swap to a different engine process).
    muted: bool,
    /// This tab's own current zoom, as last sent via `SetZoom` -- was
    /// implicitly just "whatever `Settings::zoom_factor` happens to be
    /// right now" (a single global value with no per-tab tracking at
    /// all), confirmed live to cause a real discrepancy: zooming tab A
    /// then switching to tab B left the Settings page's zoom control
    /// showing tab A's value while tab B's engine was still at its own
    /// (different) real zoom, so pressing "-" from that stale display
    /// jumped tab B's actual zoom by an unexpected amount. Initialized
    /// from `Settings::zoom_factor` at spawn time (a fresh tab still
    /// picks up "whatever the default is right now", unchanged), then
    /// tracked per-tab from there -- carried through a respawn/engine
    /// swap/crash recovery the same way `pinned`/`color` already are, not
    /// persisted to `session.json`, same smaller-scope reasoning
    /// `pinned`'s own doc comment gives.
    zoom_factor: f64,
    /// Graph view's pin toggle — a pinned node is excluded from the
    /// force-directed layout entirely (same `pinned` exclusion
    /// `graph_layout::step` already gives a node currently being dragged),
    /// so it stays exactly where the user put it instead of drifting under
    /// repulsion/centering forces as siblings come and go. Not persisted to
    /// `session.json` (unlike `pos`, which is) — resets to unpinned on
    /// every restart, a smaller scope than plumbing a new field through
    /// `SessionTab` warranted for a graph-view display preference.
    pinned: bool,
    /// Set while a swap is in flight for *this* tab: the engine to swap
    /// *to*, once this tab's `GetState` reply (its `StateSnapshot`) comes
    /// back. Per-tab (not a flat `App` field) so a swap on one tab can't
    /// block or interfere with a concurrent swap/crash on another.
    swap_target: Option<EngineKind>,
    /// The current page's own raw JSON text, if `EngineToHost::JsonBody`
    /// reported it as one -- see that message's own doc comment for
    /// detection/per-engine support. Cleared back to `None` whenever this
    /// tab's url changes for real (same gate `history`/`nav_url_changed`
    /// already use), so navigating away from a JSON response doesn't leave
    /// a stale "View as JSON" affordance pointing at content that's no
    /// longer showing.
    json_body: Option<String>,
}

/// A download's own data -- was `DownloadNode`'s non-shared fields (`path`,
/// byte progress). See `NodePayload`'s own doc comment for why this is
/// split out from the position/graph-membership fields `GraphNode` now
/// holds for every kind.
struct DownloadPayload {
    path: String,
    /// Bytes received so far -- created (node included) at
    /// `EngineToHost::DownloadStarted`, not only once finished, so a
    /// download is visible with live progress for its whole lifetime
    /// instead of only appearing once done. `None` total (see below) still
    /// updates this field; it just can't be shown as a fraction/percentage.
    bytes_received: u64,
    /// The server's reported final size, if any (a `Content-Length`-less
    /// response -- chunked transfer -- means `None`). Shown as a byte count
    /// with no percentage/bar in that case, rather than a misleading guess.
    total_bytes: Option<u64>,
    /// Set once `DownloadCompleted` arrives for this node's own `path`.
    /// Distinguishes an in-progress node (still shows a progress bar) from
    /// a finished one (plain filename, same as before this feature) without
    /// needing a separate enum -- nothing about a completed download's
    /// other fields becomes invalid or needs clearing.
    completed: bool,
}

/// `App::nodes`'s own map type -- `std::collections::HashMap` (same type,
/// same API, at every one of its call sites throughout this file) with
/// `foldhash` plugged in as the hasher instead of the standard library's
/// own SipHash default. SipHash's DoS-resistance guarantee is wasted here:
/// every key is an app-generated id (`"tab-0"`, `"note-5"`, ...), never
/// derived from untrusted input, and this map is looked up many times per
/// frame across the Graph-view render loops. `foldhash` is already fully
/// resolved in this workspace's own dependency tree (see `Cargo.toml`'s
/// own comment on it) -- this is a hasher swap, not a new dependency.
type NodeMap = HashMap<String, GraphNode, foldhash::fast::RandomState>;

/// One node in the graph view -- fields every kind shares (position, graph
/// membership) live here; `payload` holds whatever's specific to the
/// node's actual kind. See `NodePayload`'s own doc comment for the
/// migration this is a step of.
struct GraphNode {
    /// The "opened from" edge for a tab (which tab spawned it, making the
    /// tab collection a forest for Graph view), or, for a download, the tab
    /// it was downloaded from. `None` for a root node, and always `None`
    /// for a group (never "opened from").
    parent: Option<String>,
    /// Which group's subgraph this node is shown in -- `None` at the top
    /// level. Groups nest, so a group node has one too; see
    /// `GroupPayload`'s own doc comment.
    group: Option<String>,
    pos: egui::Vec2,
    vel: egui::Vec2,
    /// User-chosen tag color -- `None` for any kind that doesn't use one
    /// (`Download` doesn't; `Tab`/`Group` do).
    color: Option<egui::Color32>,
    payload: NodePayload,
}

impl GraphNode {
    fn as_tab(&self) -> Option<&TabPayload> {
        self.payload.as_tab()
    }

    fn as_tab_mut(&mut self) -> Option<&mut TabPayload> {
        self.payload.as_tab_mut()
    }

    fn as_download(&self) -> Option<&DownloadPayload> {
        self.payload.as_download()
    }

    fn as_download_mut(&mut self) -> Option<&mut DownloadPayload> {
        self.payload.as_download_mut()
    }

    fn as_group(&self) -> Option<&GroupPayload> {
        self.payload.as_group()
    }

    fn as_group_mut(&mut self) -> Option<&mut GroupPayload> {
        self.payload.as_group_mut()
    }

    fn as_note(&self) -> Option<&NotePayload> {
        self.payload.as_note()
    }

    fn as_note_mut(&mut self) -> Option<&mut NotePayload> {
        self.payload.as_note_mut()
    }

    fn as_image(&self) -> Option<&ImagePayload> {
        self.payload.as_image()
    }

    fn as_pdf(&self) -> Option<&PdfPayload> {
        self.payload.as_pdf()
    }

    fn as_pdf_mut(&mut self) -> Option<&mut PdfPayload> {
        self.payload.as_pdf_mut()
    }

    fn as_json(&self) -> Option<&JsonPayload> {
        self.payload.as_json()
    }
}

/// A tab group's own data -- was `GroupNode`'s non-shared field (`name`;
/// `pos`/`vel`/`color` moved to `GraphNode`, shared with every kind). A
/// group is a container node shown at whichever level of Graph view it
/// itself belongs to that, double-clicked, drills into its own subgraph of
/// members (tabs/downloads/notes/images/PDFs/JSON, and other groups) in
/// place of that level's -- the same "swap which set of nodes this view
/// renders" idea `ViewMode::Item`/`Graph` already is, just recursively, and
/// reusing that same flat `graph_layout::step` simulation rather than a
/// second nested one running inside the group's own node.
///
/// **Groups nest.** A group's own `GraphNode::group` can itself be
/// `Some(parent_group_id)` -- the "no nesting, `GraphNode::group` is a
/// single `Option<String>` not a path" limitation this doc comment used to
/// describe no longer applies: a single `Option<String>` *is* enough to
/// nest, since walking `.group` links upward from any node already forms
/// the whole ancestor chain (`App::group_breadcrumb`), with no separate
/// path/stack type needed. `App::current_group` names whichever group's
/// subgraph is currently showing (`None` at the true top level, same as
/// before); leaving a group (the toolbar's "◀" button) goes to that
/// group's own parent, not straight to the top, so leaving a deeply-nested
/// group takes as many "◀" clicks as it took to get there (or one
/// breadcrumb click to jump straight to any ancestor). Dissolving a nested
/// group reparents its members to *its own* parent (`App::dissolve_group`),
/// not unconditionally to the top level.
///
/// Dragging one group's card onto another's is deliberately left as
/// **merge** (dissolve the source, flatten its members into the target),
/// unchanged from before nesting existed -- it does not nest the source
/// under the target. Nesting is created only through "+ New Group"/Import
/// Group/"Group Selected" while already viewing a group's own subgraph, a
/// deliberate choice (confirmed with the user) to avoid changing what an
/// already-shipped drag gesture does.
///
/// Entering/leaving eases the camera via `GraphCameraTween` rather than
/// `Transition` — see that type's own doc comment for why. Groups (and
/// membership, including nesting) are session-only, same scope decision as
/// `TabPayload::pinned` — not written to `session.json`.
struct GroupPayload {
    name: String,
}

/// Snapshot of a tab taken by `close_tab` just before removal, enough to
/// respawn it via `Ctrl+Shift+T`/headless `REOPENTAB` (the last one) or the
/// "Recently Closed" toolbar dropdown (any specific one) — see `App::
/// closed_tabs`. Not cookies/localStorage/scroll position, same "graph
/// shape only" limitation session restore and an engine swap already both
/// have.
struct ClosedTab {
    url: String,
    kind: EngineKind,
    parent: Option<String>,
    group: Option<String>,
}

/// How many closed tabs `App::closed_tabs` remembers before dropping the
/// oldest -- unbounded would leak slowly for a long-running session that
/// opens/closes a lot of tabs, and a real browser's own "Recently closed"
/// list is bounded too.
const MAX_CLOSED_TABS: usize = 10;

/// Same reasoning as `MAX_CLOSED_TABS`, just a smaller cap -- closing a
/// whole group is a rarer, heavier action than closing one tab.
const MAX_CLOSED_GROUPS: usize = 5;

/// A Note/Image/Pdf/Json node's content, captured just before it's
/// destroyed -- either via its own "Delete" context-menu action, or via
/// `close_node` when its containing group is closed. Unlike a tab
/// or group, deleting one of these four kinds previously had no undo path
/// at all, a real risk specifically for Notes (user-typed content,
/// permanently gone on a single misclick, unlike a tab/group's own
/// already-existing "Recently Closed" recovery). Doesn't preserve the
/// original position, same as `ClosedTab`'s own reopen behavior -- a fresh
/// jittered spawn, not a restored one.
enum ClosedNode {
    Note(String),
    Image(String),
    Pdf(String),
    Json(String),
}

/// Which `ClosedNode` (if any) closing a node with this payload should add
/// to "Recently Closed" -- the one rule behind every single-node close
/// (`App::close_node`: hover ✕, context-menu Delete, closing a group's
/// members). `None` for a Tab (it has its own `closed_tabs` stack via
/// `close_tab`), a Group (`close_group` snapshots it into
/// `closed_groups`), a Download (no "closed" state), and a synthetic
/// "Expand as Graph" JSON leaf -- recording that as a `ClosedNode::Json`
/// would let "undo" re-add the *whole source file* as a fresh top-level
/// card, not the leaf that was actually closed. Only a genuine top-level
/// JSON node's own path is real undo-worthy state. Likewise a blank
/// (empty or whitespace-only) Note: reopening it restores nothing, and
/// recording it could push a real entry out of the `MAX_CLOSED_NODES` cap.
fn closed_node_for(payload: &NodePayload) -> Option<ClosedNode> {
    match payload {
        NodePayload::Note(note) => (!note.text.trim().is_empty()).then(|| ClosedNode::Note(note.text.clone())),
        NodePayload::Image(image) => Some(ClosedNode::Image(image.path.clone())),
        NodePayload::Pdf(pdf) => Some(ClosedNode::Pdf(pdf.path.clone())),
        NodePayload::Json(json) => json.key.is_none().then(|| ClosedNode::Json(json.path.clone())),
        NodePayload::Tab(_) | NodePayload::Group(_) | NodePayload::Download(_) => None,
    }
}

/// Same reasoning as `MAX_CLOSED_TABS`.
const MAX_CLOSED_NODES: usize = 10;

/// One entry in `App::history` -- see that field's own doc comment.
struct HistoryEntry {
    title: String,
    url: String,
}

/// How many `App::history` remembers before dropping the oldest -- same
/// "bounded, not a slow leak in a long-running session" reasoning as
/// `MAX_CLOSED_TABS`.
const MAX_HISTORY_ENTRIES: usize = 200;

/// A minimal right-click context menu: just "Open in new tab" for a link,
/// positioned where the click landed — see `ipc_protocol::EngineToHost::
/// ContextMenuLink`'s doc comment for why this is the one action
/// implemented rather than a full copy/paste/inspect menu. `x`/`y` are
/// already in window (not content-area) point space — see where this is
/// constructed in `drain_tab_events`.
#[derive(Clone)]
struct ContextMenuState {
    x: f32,
    y: f32,
    href: String,
}

/// `Ctrl+K`'s tab quick-switcher: substring-search-by-title-or-url across
/// *every* open tab regardless of level/group (unlike the graph views'
/// own level-scoped listings — jumping to a tab buried in a different
/// group is the whole point of this), then jump straight to whichever one
/// is selected. `selected` indexes into whatever `App::quick_switcher_
/// matches` currently returns for `query` -- clamped on every keystroke
/// since the match list can shrink out from under a previously valid
/// index (e.g. a query that used to match 5 tabs now matches 2).
#[derive(Clone)]
struct QuickSwitcher {
    query: String,
    selected: usize,
}

/// The event loop's real user-event type: headless debug commands, plus
/// `TabSpawned` — see its own doc comment for why engine-host spawning needs
/// to be async in the first place.
enum UserEvent {
    Debug(DebugCommand),
    TabSpawned(TabSpawnOutcome),
}

/// Which call site triggered a tab spawn — `handle_tab_spawn_result` uses
/// this to know what to do once the (now-async) `process_manager::spawn_tab`
/// call finishes, mirroring what `spawn_root_tab`/`spawn_new_tab`/
/// `respawn_tab` used to do inline before the spawn was made async.
enum SpawnPurpose {
    /// The very first tab at startup: becomes focused, switches to Tab view.
    Root,
    /// Graph view's "+ New Tab" (or a page-triggered new tab): spawned
    /// invisible, no focus change.
    New,
    /// Engine swap finishing, or crash recovery: visibility/focus depend on
    /// whether `tab_id` is still the focused tab by the time the spawn
    /// actually completes (checked at handling time, not capture time).
    Respawn,
    /// Startup session restore (see `session::Session`) — one of
    /// potentially many concurrent spawns kicked off together, so (like
    /// `Respawn`) visibility/focus is decided at completion time against
    /// whatever `self.focused` was set to before any of them were issued,
    /// not capture time.
    Restore,
    /// `App::start_split`'s secondary pane -- spawned invisible like `New`,
    /// but once it completes it becomes `self.split_tab` and turns visible
    /// (a split's secondary pane is always live, unlike an ordinary
    /// background tab) rather than gaining focus. See `App::split_tab`'s
    /// own doc comment.
    Split,
}

/// A pending `process_manager::spawn_tab` call's outcome, carried back to
/// the main/event-loop thread via `EventLoopProxy` — see `App::spawn_tab_async`
/// for why this exists: spawning WebKit's engine-host blocks on a cold WSL2
/// boot + Xvfb + WebKitGTK init (confirmed live to take several real seconds
/// on first use, occasionally much longer — see `process_manager::
/// spawn_webkit_tab`'s own doc comment), which would otherwise freeze the
/// whole app (redraw, input, headless stdin) for that entire duration if run
/// on this thread.
struct TabSpawnOutcome {
    id: String,
    parent: Option<String>,
    group: Option<String>,
    /// Carried through a respawn/engine-swap/crash-recovery so a pinned
    /// tab stays pinned — see `TabPayload::pinned`'s own doc comment. Always
    /// `false` for a genuinely new tab (`SpawnPurpose::New`/`Root`).
    pinned: bool,
    /// Same "carried through respawn, `None` for a genuinely new tab"
    /// treatment as `pinned` above, but for `GraphNode::color`.
    color: Option<egui::Color32>,
    /// Same "carried through respawn" treatment as `pinned`/`color`, but
    /// for `TabPayload::zoom_factor` -- see that field's own doc comment.
    /// `Settings::zoom_factor` (the current default) for a genuinely new
    /// tab, since there's no prior per-tab value to inherit.
    zoom_factor: f64,
    url: String,
    pos: egui::Vec2,
    vel: egui::Vec2,
    purpose: SpawnPurpose,
    /// `App::session_generation` when the spawn started -- see that field.
    generation: u64,
    result: std::io::Result<EngineTab>,
}

struct App {
    window: Option<Arc<Window>>,
    /// The OS window title `update_window_title` last actually set --
    /// compared against each frame's freshly computed title so `Window::
    /// set_title` (a real OS call) is only made on an actual change, not
    /// once a frame regardless (this app's winit loop runs `ControlFlow::
    /// Poll`, redrawing continuously).
    last_window_title: String,
    gpu: Option<GpuState>,
    /// Every graph node -- tabs, downloads, and groups alike (see
    /// `NodePayload`'s own doc comment). `node_order` is a stable
    /// iteration/z-order for graph rendering and hit-testing, separate from
    /// this `HashMap`'s own unspecified iteration order (which would make
    /// node click/drag targeting flaky if used directly).
    nodes: NodeMap,
    node_order: Vec<String>,
    /// Toggled by the Graph toolbar's "Collapse Groups"/"Expand Groups"
    /// button -- while `true`, every non-empty group card falls back to
    /// the plain centered name+count layout an empty group already uses,
    /// skipping the 2x2 member-thumbnail grid. A denser overview for a
    /// graph with many/large groups, purely a rendering toggle -- doesn't
    /// touch `groups`/membership at all. Session-only, resets on restart.
    groups_collapsed: bool,
    /// `None` while viewing the top level; `Some(group_id)` while a
    /// group's own subgraph has been drilled into (see `GroupPayload`'s doc
    /// comment) — only meaningful when `view_mode == Graph`, but not reset
    /// on leaving Graph view, so returning to Graph resumes wherever the
    /// user last drilled into, same "graph view remembers its own state"
    /// treatment `graph_scene_rect`'s pan/zoom already gets.
    current_group: Option<String>,
    /// `(group_id, draft_name)` once renaming has started -- either the
    /// toolbar's group-name button (only shown while `current_group` is
    /// `Some`) or the top-level right-click context menu's own "Rename"
    /// (see `node_menu`) sets this the same way. A real
    /// `egui::TextEdit` replaces whichever plain button/menu row started
    /// it until Enter commits or Escape cancels. Transient UI state, not
    /// part of `GroupPayload` itself.
    renaming_group: Option<(String, String)>,
    /// Set for exactly one frame when `renaming_group` is first set, same
    /// one-shot `response.request_focus()` pattern as
    /// `pending_focus_address_bar`.
    pending_focus_rename: bool,
    /// `(node_id, screen_pos)` while a right-click context menu is showing
    /// for that node's card -- one field for every kind, so at most one
    /// menu is ever open by construction. Right-click is otherwise the
    /// rectangle-select gesture over the graph panel (see that gesture's
    /// own doc comment in `build_graph_ui`); a press landing on any card
    /// opens this instead of starting a marquee. Drawn by
    /// `App::build_node_menu`, which dispatches on the node's kind.
    node_menu: Option<(String, egui::Pos2)>,
    /// Last real value of `node_menu`, kept around so `build_node_menu`
    /// can keep drawing (fading out, not vanishing instantly) for a few
    /// frames after `node_menu` itself goes back to `None` -- see
    /// `fade_popup_state`.
    node_menu_cache: Option<(String, egui::Pos2, NodeMenuView)>,
    /// `Some(node_id)` while that node's JSON content is showing full-screen
    /// (`build_json_viewer`) -- a tab's own `TabPayload::json_body` (opened
    /// from Tab view's toolbar button or Graph view's tab context menu's
    /// "View as JSON") or a `JsonPayload::value` (opened by double-clicking
    /// a JSON node's card), closed via its own close button or Escape. Not
    /// `bool`: needs to remember *which* node, since either source can be
    /// opened from Graph view for a node other than whichever tab is
    /// focused.
    json_viewer_node: Option<String>,
    /// `Ctrl+K` quick-switcher, if currently open — see `QuickSwitcher`'s
    /// own doc comment and `App::build_quick_switcher`.
    quick_switcher: Option<QuickSwitcher>,
    /// Same fade-out role as `node_menu_cache`, for `quick_switcher`
    /// -- but rendered read-only (no live `TextEdit`, no clickable rows)
    /// once actually closing, since editing/clicking needs `&mut self.
    /// quick_switcher`, which no longer exists by then. See `build_quick_
    /// switcher`'s own `is_live` branch.
    quick_switcher_cache: Option<QuickSwitcher>,
    /// `F1` / headless `TOGGLESHORTCUTSHELP` -- whether the keyboard-
    /// shortcut reference popup is showing. See `App::build_shortcuts_help`.
    shortcuts_help_open: bool,
    /// A session picked via "Import Session..." (or headless `IMPORTSESSION`),
    /// staged here rather than applied immediately -- `build_import_confirm_
    /// popup` shows its tab count against the currently running session's
    /// own, so replacing a real working session with an accidentally-wrong
    /// file is a deliberate second click, not a silent one-shot dialog pick.
    pending_import: Option<session::Session>,
    /// The settings popup's "Save Session As..." name field -- kept across
    /// popup open/close within the running session (not persisted), same
    /// convenience the group-rename draft gets.
    session_name_draft: String,
    /// Graph view's own toolbar filter box -- dims (doesn't hide) any
    /// tab/download/group whose title/url/path/name doesn't contain this
    /// case-insensitively, keeping edges and spatial layout intact rather
    /// than making the graph reflow the way actually removing non-matches
    /// would. Empty means no filtering. See `App::graph_node_dimmed`.
    graph_filter: String,
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
    graph_zoom_pct: f32,
    /// Set for exactly one frame when `quick_switcher` is first opened,
    /// same one-shot `response.request_focus()` pattern as
    /// `pending_focus_address_bar`/`pending_focus_find_bar`.
    pending_focus_quick_switcher: bool,
    /// The tab shown in Tab view, and — even while in Graph view — the
    /// default parent for a newly created tab (last thing the user
    /// selected). `None` only when there are no tabs left at all.
    focused: Option<String>,
    /// The Image node `ViewMode::Item` is showing, if any. `None` means
    /// Item view (if active) shows `focused`, a Tab. Kept separate from
    /// `focused` because `focused` is tab-only by contract -- see the
    /// Image Item View spec's "Plan-time corrections".
    viewed_image: Option<String>,
    /// Zoom/pan for `viewed_image` -- see `ImageViewState`.
    image_view: ImageViewState,
    /// The secondary pane's tab id while a split (engine-vs-engine compare
    /// view) is showing, alongside `focused` in the primary pane -- `None`
    /// for ordinary single-pane Tab view. Set once the tab `start_split`
    /// spawns actually finishes (`SpawnPurpose::Split`). Unlike `focused`,
    /// this tab never receives keyboard/mouse input in this first cut --
    /// it's a live, real graph node (its own engine-host process, closable
    /// and reachable from Graph view like any other tab), just view-only
    /// while shown here. Cleared (not the tab itself closed) whenever the
    /// split closes, `focused` changes to a different tab, or this tab
    /// closes out from under the split -- see `close_split`.
    split_tab: Option<String>,
    view_mode: ViewMode,
    next_tab_id: u64,
    next_download_id: u64,
    /// Tab spawns started by `spawn_tab_async` whose `TabSpawned` result
    /// hasn't been handled yet. Read by the headless `WAITTABS` command,
    /// whose waiting senders sit in `tabs_spawned_waiters` until this
    /// reaches zero.
    tab_spawns_in_flight: usize,
    tabs_spawned_waiters: Vec<std::sync::mpsc::Sender<()>>,
    /// Bumped by `replace_running_session`. A tab spawn carries the value it
    /// started under, and `handle_tab_spawn_result` drops a result from an
    /// older one: it belongs to the replaced session, and inserting it would
    /// add a stray tab to the new scene or, when its id matches one the new
    /// session uses (both count from `tab-0`), replace that tab's node and
    /// leave two engine hosts fighting over one id.
    session_generation: u64,
    next_group_id: u64,
    next_note_id: u64,
    next_image_id: u64,
    next_pdf_id: u64,
    next_json_id: u64,
    /// Shared across every note's own rendered (non-`editing`) view --
    /// `egui_commonmark`'s own recommended usage (glyph-layout/heading-id
    /// state, not per-note content), not a fresh cache per card per frame.
    commonmark_cache: egui_commonmark::CommonMarkCache,
    cursor_pos: (f64, f64),
    /// The tab a left `InputEvent::MouseDown` was actually forwarded to and
    /// which hasn't been sent the matching `MouseUp` yet — i.e. "a native
    /// drag is in progress on this tab".
    ///
    /// `MouseDown` forwarding is gated on the cursor being below chrome and
    /// in the primary pane; applying that same gate to `MouseUp` means a
    /// press inside the page followed by a release over the app's own
    /// toolbar never delivers a button-up to the page at all. That was
    /// harmless while input was JS-driven, but mouse input is real native
    /// injection now (`SendMouseInput` on WebView2, `SendMouseClickEvent`
    /// on CEF), so Chromium itself never learns the button came up and
    /// keeps extending a text selection under every later hover. Tracking
    /// the in-progress drag here lets the release be forwarded to the same
    /// tab unconditionally, wherever it lands — the `manifold`-side mirror
    /// of the engine-side `DragState` each native-input engine-host keeps.
    native_drag_tab: Option<String>,
    /// Bitflags (see `ipc_protocol::modifiers`), tracked from winit's
    /// `WindowEvent::ModifiersChanged` so every forwarded `KeyDown`/`KeyUp`
    /// carries the modifier state at the time of that key, not just the
    /// currently-pressed key itself.
    key_modifiers: u8,

    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    address_bar_text: String,
    address_bar_focused: bool,
    /// Address-bar autocomplete: open-tab matches are recomputed
    /// synchronously (cheap, from data already in memory) whenever the text
    /// changes; search-engine suggestions arrive later, asynchronously, via
    /// `suggestion_fetcher`'s debounced background fetch. Only shown while
    /// `address_bar_focused`.
    open_tab_suggestions: Vec<Suggestion>,
    search_suggestions: Vec<String>,
    suggestion_fetcher: SuggestionFetcher,
    /// Background fetcher for `favicon_fetch::fetch_favicon` -- see that
    /// module's own doc comment for why favicons are fetched independently
    /// here rather than aggregated from each engine's own (inconsistent)
    /// native favicon API.
    favicon_fetcher: FaviconFetcher,
    /// See `PdfPayload::outcome`'s own doc comment.
    pdf_fetcher: PdfFetcher,
    /// See `redirect_fetch`'s own module doc comment.
    redirect_fetcher: RedirectFetcher,
    /// Origins already resolved (successfully or not) by `favicon_fetcher`
    /// -- checked before requesting again, so a failed origin (no
    /// `/favicon.ico`, a network error, undecodable bytes) isn't refetched
    /// on every navigation back to it. A *successful* fetch's actual pixel
    /// data lives in `GpuState::favicons` (keyed by the same origin
    /// string), not duplicated here -- this only needs to remember whether
    /// resolution was already attempted, not the result itself.
    favicon_resolved: HashSet<String>,
    /// Index into `combined_suggestions()` the user has moved to via
    /// Up/Down — `None` means no row is highlighted (Enter falls through
    /// to a plain navigate of the typed text). Reset whenever the
    /// suggestion list itself changes or the dropdown closes, so a stale
    /// index can never point at an unrelated row — see `build_tab_view_ui`'s
    /// dropdown-handling comments for the full set of reset sites.
    dropdown_highlighted: Option<usize>,

    /// Graph view: the node currently being dragged (pinned in the physics
    /// sim — see `graph_layout::step`'s `pinned` parameter) and the node
    /// currently hovered (drives the peek-preview popup).
    dragging: Option<String>,
    hovered: Option<String>,
    /// Set to `self.hovered` by an Escape press while a hover-peek preview
    /// is showing, so that preview can be dismissed early without needing
    /// the pointer to actually move away -- the peek drawing code skips
    /// rendering while this matches the current `hovered` id. Cleared
    /// whenever `hovered` changes to anything else (including `None`), so
    /// hovering the same or a different node again always shows a fresh
    /// peek.
    peek_suppressed: Option<String>,
    /// Graph view rectangle-select: `Some` (holding the screen-space anchor
    /// point) for exactly the duration of a right-button drag over the
    /// graph panel -- the right button specifically, since the primary
    /// button is already Scene's own pan gesture (`drag_pan_buttons` is
    /// restricted to `PRIMARY` in `build_graph_ui` so the two never fight
    /// over the same drag). `selected_tabs` is the committed result, kept
    /// until the next selection (a fresh right-drag) or an explicit clear
    /// (a plain click on empty background, or Escape).
    rect_select_start: Option<egui::Pos2>,
    selected_tabs: HashSet<String>,
    /// World position of the small "New Tab/Note/Group/File" card shown
    /// after a plain right-click (no real drag) on empty Graph-view
    /// background -- see the marquee-release logic below for where this
    /// gets set (same gesture, disambiguated from a real rectangle-select
    /// drag by movement distance) and `build_graph_ui`'s own rendering of
    /// it. Deliberately *not* a `NodePayload`/`self.nodes` entry -- it's a
    /// pending choice, not real graph content, so it never needs excluding
    /// from `nodes_at_current_level(Group)`/session persistence/drag-into-group/
    /// filters the way a real new node kind would. Cleared on any camera
    /// movement (pan/zoom/Reset View/entering or leaving a group/minimap
    /// click), Escape, clicking elsewhere, or picking one of its own
    /// actions -- deliberately short-lived rather than a droppable/
    /// draggable placeholder (a real node's card lingers regardless of
    /// camera movement; this one is a transient choice, not content, so
    /// letting it silently drift out of sync with a moved camera would be
    /// the wrong feel for it).
    pending_new_node: Option<egui::Vec2>,
    /// The graph view's current pan/zoom state (visible world-space rect),
    /// persisted across frames per `egui::Scene::show`'s own contract —
    /// mutated by the user dragging (pan) or scrolling (zoom).
    graph_scene_rect: egui::Rect,
    /// Level-of-detail: true once a frame's own count of viewport-visible
    /// nodes (post-cull, see `build_graph_ui`'s own pre-pass) crossed
    /// `LOD_SIMPLIFY_COUNT`, gating the per-kind render loops' thumbnail/
    /// title-text painting to keep dense views cheap. Flips back to
    /// `false` only once the count drops under the lower `LOD_RESTORE_
    /// COUNT` -- a deliberate hysteresis gap, not a single threshold, so
    /// a count sitting right at one boundary doesn't flicker every frame.
    graph_lod_simplified: bool,
    /// See `truncate_to_width`'s own doc comment.
    truncate_cache: HashMap<TruncateCacheKey, String>,
    /// See `App::cached_favicon_origin`'s own doc comment.
    favicon_origin_cache: HashMap<String, Option<String>>,
    /// `Some` while entering/leaving a group is easing the camera toward
    /// the new level's framing instead of having already snapped there --
    /// see `GraphCameraTween`'s own doc comment.
    graph_camera_tween: Option<GraphCameraTween>,
    /// `Some` while a released background-pan drag is still gliding --
    /// see `PanMomentum`'s own doc comment.
    pan_momentum: Option<PanMomentum>,
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
    pan_drag_history: std::collections::VecDeque<(std::time::Instant, egui::Pos2)>,
    /// `(level, node_count)` for the graph level `graph_settle_current_level`
    /// last ran a settle burst for -- `build_graph_ui` compares this
    /// against the current level/count every frame and only re-settles
    /// (and updates this) when they differ, instead of the physics sim
    /// running continuously. See `graph_settle_current_level`'s own doc
    /// comment for why continuous physics was replaced with this at all.
    graph_settled_key: Option<(Option<String>, usize)>,

    /// Headless mode (`--headless`): the window is created real but
    /// positioned far off-screen (NOT invisible — a genuinely invisible
    /// window's swapchain never successfully acquires a frame on Windows,
    /// see `resumed`'s doc comment), so it never steals focus/foreground or
    /// visually contends with the user's own screen (project memory's
    /// `SetForegroundWindow`/foreground-lock gotchas), while still
    /// rendering normally. Driven entirely by `DebugCommand`s read from
    /// stdin instead of real OS input. See `debug_control`'s doc comment
    /// for the full rationale and the text-command protocol.
    headless: bool,
    /// Key events from headless `ENTER`/`ESCAPE`/`ARROWDOWN`/`ARROWUP`,
    /// added to the next frame's egui input exactly like real key presses
    /// (see `RedrawRequested`), so they take the same code paths and, like
    /// a real key, last one frame whether or not anything reads them.
    debug_input_events: Vec<egui::Event>,
    /// Text from a headless `ADDRESSBAR`, typed (select-all, then the text)
    /// once the address bar actually has egui focus: the command requests
    /// focus through `pending_focus_address_bar`, which only takes hold in
    /// the frame after, so typing waits for a frame where
    /// `address_bar_focused` is true.
    debug_address_bar_typing: Option<String>,
    /// Set by `Ctrl+L`/headless `FOCUSADDRESS` (see
    /// `App::focus_address_bar_shortcut`), consumed the next time
    /// `build_tab_view_ui` builds the address bar's `TextEdit` — unlike the
    /// headless key commands above, this drives a *real* `request_focus()`
    /// call on the real widget (this is the one shortcut where a real user
    /// genuinely wants real, persistent egui focus; headless `ADDRESSBAR`
    /// uses it too), so it needs a frame to actually exist as a widget to
    /// focus.
    pending_focus_address_bar: bool,
    /// Set by a headless `QUIT` that arrived while debug input above (or
    /// `pending_screenshot`) was still waiting for a frame — e.g. `ENTER`+`QUIT`,
    /// or `SCREENSHOT`+`QUIT`, sent back-to-back with no delay. Exiting
    /// immediately in that case could drop the pending command/screenshot
    /// before any frame ever consumed it (`event_loop.exit()` gives no
    /// guarantee a `RedrawRequested` runs first). Holds how many more frames
    /// to wait: `RedrawRequested` exits once nothing is pending, or when
    /// this runs out (an `ADDRESSBAR` needs two frames, focus then typing;
    /// a screenshot on a stalled surface might never land).
    quit_after_redraws: Option<u8>,
    /// Set by a headless `SCREENSHOT` command; consumed on the next
    /// `RedrawRequested` and passed into `gpu::GpuState::render`.
    pending_screenshot: Option<String>,
    /// The stdin reader waiting on `pending_screenshot` (see
    /// `DebugCommand::Screenshot`); sent `()` once it's been attempted.
    screenshot_waiter: Option<std::sync::mpsc::Sender<()>>,
    /// "Print to PDF": the target `.pdf` path, set alongside
    /// `pending_screenshot` (a temp PNG) so the underlying GPU capture
    /// mechanism doesn't need to know PDF exists at all -- once that
    /// screenshot actually completes (see `RedrawRequested`'s handling of
    /// `RenderOutcome::screenshot_result`), the temp PNG is converted via
    /// `pdf::write_image_as_pdf` and deleted. `None` the rest of the time,
    /// including for an ordinary "Screenshot" button click.
    pending_print_to_pdf: Option<String>,

    /// Find-in-page: tied to the focused tab only, not persisted per-tab —
    /// closes (and sends `FindStop`) on tab switch/close, same "don't try
    /// to be a full session" scope as the address bar's own suggestion
    /// state. `find_text` mirrors what's sent as the last `FindStart`;
    /// `find_result` is `None` until the first `FindResult` for the current
    /// search arrives (so the bar shows nothing rather than a stale count
    /// while a fresh search is still in flight). See `EngineToHost::
    /// FindResult`'s doc comment for why `active_index` is always 0 on a
    /// WebKit tab and Servo tabs never populate this at all.
    /// `Ctrl+Shift+T`/headless `REOPENTAB` respawns and removes the last
    /// entry; the "Recently Closed" toolbar dropdown can also respawn (and
    /// remove) any specific one — see `ClosedTab`'s own doc comment. Capped
    /// at `MAX_CLOSED_TABS`, oldest dropped first.
    closed_tabs: Vec<ClosedTab>,
    /// Whole groups closed via `close_group` (its own "Close Group" context-
    /// menu entry) -- each a full `group_export::GroupExport` snapshot
    /// (name, tag color, member tabs/positions), same shape "Export
    /// Group..." already produces, so reopening one goes through the exact
    /// same `import_group_export` a real file import would. Listed in the
    /// same "Recently Closed" dropdown as `closed_tabs`, in their own
    /// section -- `close_group` closes each member through the normal
    /// `close_tab` path too (for its other bookkeeping: parent-relinking,
    /// thumbnail cleanup, focus/hover reset), then throws away the
    /// individual `ClosedTab` entries that produced, so a closed group
    /// doesn't ALSO leave behind N separate single-tab undo entries a user
    /// could reopen from and get duplicates. Capped at `MAX_CLOSED_GROUPS`,
    /// oldest dropped first. Not reachable from `Ctrl+Shift+T` (that
    /// shortcut only ever looks at `closed_tabs`) -- only the dropdown.
    closed_groups: Vec<group_export::GroupExport>,
    /// Note/Image/Pdf/Json nodes deleted via `close_node` -- hover ✕,
    /// their own context menu's "Delete", or a containing group being
    /// closed -- see `ClosedNode`'s own doc comment for why this exists.
    /// Listed in the same "Recently Closed" dropdown as `closed_tabs`/
    /// `closed_groups`, in their own section. Capped at `MAX_CLOSED_NODES`,
    /// oldest dropped first.
    closed_nodes: Vec<ClosedNode>,

    /// The minimal right-click context menu, if currently showing — see
    /// `ContextMenuState`'s own doc comment.
    context_menu: Option<ContextMenuState>,
    /// Same fade-out role as `node_menu_cache`, for `context_menu`.
    context_menu_cache: Option<ContextMenuState>,

    /// A brief toast message (full text, not just a path -- see `build_
    /// save_toast`'s own doc comment) for the most recent completed
    /// download/save or crash/respawn notice, and when it was first drawn —
    /// cleared once `SAVE_TOAST_DURATION` has passed, at which point the
    /// next queued message (`save_toast_queue`) takes over, if any. Was
    /// "latest wins" with no queue at all until this session, when a
    /// second toast (e.g. a crash notice) arriving while a download's own
    /// "Saved <path>" toast was still showing was confirmed to silently
    /// replace it, unlike `find_result`'s own "latest wins" (correct
    /// there -- a stale search count would be actively wrong, whereas a
    /// dropped one-time notification like "tab crashed" is just gone).
    save_toast: Option<(String, std::time::Instant)>,
    /// See `save_toast`'s own doc comment -- messages waiting to be shown,
    /// one at a time in order, each after the previous one's
    /// `SAVE_TOAST_DURATION` elapses. Pushed to by `queue_toast` (and
    /// directly by `DownloadCompleted`, for borrow reasons).
    save_toast_queue: std::collections::VecDeque<String>,

    /// When the auto-restore `session.json` was last written -- shown in
    /// the settings popup's Sessions section next to a "Save Now" button,
    /// so a save that's actually continuous (every mutation calls
    /// `save_session`, not a timer) has some visible confirmation instead
    /// of being purely invisible. `None` until the first save this run.
    last_session_save: Option<std::time::Instant>,
    /// `session::fingerprint()`/`settings::fingerprint()` as this process
    /// last saw them (set at load, updated after every successful save) --
    /// see `save_session`'s own doc comment for what they're for. `None`
    /// until the corresponding file has actually been loaded or saved once
    /// this run, which also means "skip the conflict check" the first time.
    session_fingerprint: Option<u64>,
    settings_fingerprint: Option<u64>,

    find_bar_open: bool,
    find_text: String,
    /// Resets to `false` on every `close_find_bar` — a fresh find session
    /// (new tab, or reopening after closing) starts case-insensitive by
    /// default, matching a real browser's find bar.
    find_match_case: bool,
    find_result: Option<(u32, u32)>,
    /// Set by `Ctrl+F`/headless `OPENFINDBAR`, consumed the next time
    /// `build_tab_view_ui` builds the find bar's `TextEdit` — same "needs a
    /// frame to exist as a widget to focus" reason as
    /// `pending_focus_address_bar`.
    pending_focus_find_bar: bool,

    /// Set when `spawn_root_tab`/`spawn_new_tab` fails (e.g. a stale
    /// engine-host from a previous crashed run still holding the tab's
    /// named pipe). Without this, a failed spawn left `tabs` empty and the
    /// user saw the ordinary "No tabs" Graph view with no indication
    /// anything had gone wrong — see `build_graph_ui`. Cleared at the start
    /// of the next spawn attempt so a retry doesn't leave a stale message
    /// on screen.
    tab_spawn_error: Option<String>,

    /// User-configurable defaults (search engine, starting engine, zoom,
    /// home page), loaded once at startup and persisted to
    /// `%APPDATA%\manifold\settings.json` on every change — see
    /// `settings.rs`.
    settings: Settings,
    /// Which view `ViewMode::Settings` should return to (its own Back
    /// button, `Escape`, or `Ctrl+G`) -- set once when the settings page is
    /// entered, from whichever of Tab/Graph was showing at that moment.
    settings_return_view: ViewMode,
    /// Flat `(title, url)` list, loaded once at startup and persisted to
    /// `%APPDATA%\manifold\bookmarks.json` on every change — see
    /// `bookmarks.rs`. `bookmarks_open` is the Tab-view toolbar dropdown's
    /// own visibility, see `App::build_bookmarks_popup`.
    bookmarks: Vec<bookmarks::Bookmark>,
    bookmarks_open: bool,
    /// The three places this app lets you choose an engine (Settings'
    /// "Default starting engine," Settings' "Current Tab" swap, and the
    /// Tab toolbar's "Split" picker) each get their own open flag, same
    /// "one bool per popup" shape as `bookmarks_open` -- see
    /// `build_engine_picker_popup`'s own doc comment for the shared popup
    /// itself.
    default_engine_picker_open: bool,
    current_engine_picker_open: bool,
    split_engine_picker_open: bool,
    /// The bookmarks popup's own filter box text -- cleared each time the
    /// popup is opened, same "fresh state each open" reasoning
    /// `find_match_case` resetting on a new find session already has.
    bookmarks_filter: String,
    /// `(bookmark_url, draft)` while a bookmark's folder label in the
    /// popup is being renamed -- keyed by url (a bookmark's own natural
    /// identity, same as `toggle_bookmark`'s own match) rather than index,
    /// so it can't end up pointing at the wrong row if the list reorders.
    /// Same "real `TextEdit` replaces the label, Enter/blur commits,
    /// Escape cancels" shape `renaming_group` already has.
    renaming_bookmark_folder: Option<(String, String)>,
    /// Set for exactly one frame when `renaming_bookmark_folder` is first
    /// set, same one-shot `response.request_focus()` pattern as
    /// `pending_focus_rename`.
    pending_focus_bookmark_folder: bool,
    /// Every distinct `(title, url)` this session has actually navigated
    /// to, most-recent-last, capped at `MAX_HISTORY_ENTRIES` (oldest
    /// dropped first) -- recorded once per real URL change in
    /// `EngineToHost::NavigationStateChanged`'s own handling, so it covers
    /// in-page navigation (clicked links) the same as address-bar entries,
    /// not just the latter. Session-only, cleared on restart -- unlike
    /// bookmarks, nothing here is ever written to disk.
    history: Vec<HistoryEntry>,
    history_open: bool,
    /// The history popup's own filter box text -- same "cleared on each
    /// open" reasoning as `bookmarks_filter`.
    history_filter: String,
    /// Whether the Tab-view toolbar's "Downloads (N)" dropdown is open --
    /// lists `self.downloads` (the same data the graph view's own download
    /// nodes/context menu already show) so a completed download can be
    /// found without first switching to Graph view. See `App::
    /// build_downloads_popup`.
    downloads_open: bool,
    /// Whether the Tab-view toolbar's "Tab actions" dropdown is open --
    /// consolidates "Mute Others"/"Close Others"/the "Pin All"/"Unpin All"
    /// toggle into one popup instead of three always-visible buttons. See
    /// `App::build_tab_actions_popup`.
    tab_actions_open: bool,
    /// Whether the "Recently Closed" toolbar dropdown (Graph view only) is
    /// currently open -- see `App::build_recently_closed_popup`.
    recently_closed_open: bool,

    /// The in-progress Tab↔Graph shrink/expand animation, if any — see
    /// `Transition`'s own doc comment.
    transition: Option<Transition>,

    /// Set right after `App::default()` in `main`, before `run_app` — needed
    /// by `spawn_tab_async` (called from `resumed`'s `spawn_root_tab`, which
    /// runs before any other opportunity to set it) to hand a background
    /// spawn thread a way to report back. `None` only in the brief window
    /// before `main` sets it; every real spawn happens after that.
    event_proxy: Option<EventLoopProxy<UserEvent>>,

    /// The tab id `respawn_tab` was called for *while it was the focused
    /// tab* — i.e. an engine swap or crash recovery is currently respawning
    /// the tab the user is looking at. `build_tab_view_ui`'s existing "focused
    /// tab missing from `self.nodes`" guard (a pre-existing one-frame safety
    /// net, from before spawning was async) would otherwise bounce the user
    /// to Graph view for the tab's *entire* respawn duration — imperceptible
    /// when spawning was synchronous, but a visible multi-second flicker
    /// back to Graph and then back to Tab view now that a WebKit respawn can
    /// take many seconds. Set in `respawn_tab`, cleared in
    /// `handle_tab_spawn_result` once that same id's `Respawn` outcome
    /// (`Ok` or `Err`) arrives.
    pending_respawn_focus: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        let egui_ctx = egui::Context::default();
        apply_style(&egui_ctx);
        apply_fonts(&egui_ctx);
        let (settings, settings_err) = Settings::load();
        // Re-reads the file `settings::fingerprint()` already just read for
        // `load()` -- a second read of a file this small, once at startup,
        // isn't worth threading a fingerprint out through `load()`'s own
        // return type for.
        let settings_fingerprint = settings::fingerprint();
        let (bookmarks, bookmarks_err) = bookmarks::load();
        // Before constructing anything that might make its own first real
        // HTTPS request (search suggestions, favicons) -- see `suggestions::
        // warm_up_https_client`'s own doc comment for why this needs to run
        // exactly once, centrally, rather than each network consumer doing
        // its own redundant warm-up (or, worse, one racing a real request
        // against another's not-yet-finished warm-up).
        suggestions::warm_up_https_client();
        let mut app = Self {
            window: None,
            last_window_title: "manifold".to_string(),
            gpu: None,
            nodes: NodeMap::default(),
            node_order: Vec::new(),
            groups_collapsed: false,
            current_group: None,
            renaming_group: None,
            pending_focus_rename: false,
            node_menu: None,
            node_menu_cache: None,
            json_viewer_node: None,
            quick_switcher: None,
            quick_switcher_cache: None,
            shortcuts_help_open: false,
            pending_import: None,
            session_name_draft: String::new(),
            graph_filter: String::new(),
            graph_zoom_pct: 100.0,
            pending_focus_quick_switcher: false,
            focused: None,
            viewed_image: None,
            image_view: ImageViewState::default(),
            split_tab: None,
            view_mode: ViewMode::Item,
            next_tab_id: 0,
            next_download_id: 0,
            tab_spawns_in_flight: 0,
            tabs_spawned_waiters: Vec::new(),
            session_generation: 0,
            next_group_id: 0,
            next_note_id: 0,
            next_image_id: 0,
            next_pdf_id: 0,
            next_json_id: 0,
            commonmark_cache: egui_commonmark::CommonMarkCache::default(),
            cursor_pos: (0.0, 0.0),
            native_drag_tab: None,
            key_modifiers: 0,
            egui_ctx,
            egui_state: None,
            address_bar_text: settings.home_page.clone(),
            address_bar_focused: false,
            open_tab_suggestions: Vec::new(),
            search_suggestions: Vec::new(),
            suggestion_fetcher: SuggestionFetcher::new(),
            favicon_fetcher: FaviconFetcher::new(),
            pdf_fetcher: PdfFetcher::new(),
            redirect_fetcher: RedirectFetcher::new(),
            favicon_resolved: HashSet::new(),
            dropdown_highlighted: None,
            dragging: None,
            hovered: None,
            peek_suppressed: None,
            rect_select_start: None,
            selected_tabs: HashSet::new(),
            pending_new_node: None,
            graph_scene_rect: default_graph_scene_rect(),
            graph_lod_simplified: false,
            truncate_cache: HashMap::new(),
            favicon_origin_cache: HashMap::new(),
            graph_camera_tween: None,
            pan_momentum: None,
            pan_drag_history: std::collections::VecDeque::new(),
            graph_settled_key: None,
            headless: false,
            debug_input_events: Vec::new(),
            debug_address_bar_typing: None,
            pending_focus_address_bar: false,
            quit_after_redraws: None,
            pending_screenshot: None,
            screenshot_waiter: None,
            pending_print_to_pdf: None,
            closed_tabs: Vec::new(),
            closed_nodes: Vec::new(),
            closed_groups: Vec::new(),
            context_menu: None,
            context_menu_cache: None,
            save_toast: None,
            save_toast_queue: std::collections::VecDeque::new(),
            last_session_save: None,
            session_fingerprint: None,
            settings_fingerprint,
            find_bar_open: false,
            find_text: String::new(),
            find_match_case: false,
            find_result: None,
            pending_focus_find_bar: false,
            tab_spawn_error: None,
            settings,
            settings_return_view: ViewMode::Item,
            bookmarks,
            bookmarks_open: false,
            default_engine_picker_open: false,
            current_engine_picker_open: false,
            split_engine_picker_open: false,
            bookmarks_filter: String::new(),
            renaming_bookmark_folder: None,
            pending_focus_bookmark_folder: false,
            history: Vec::new(),
            history_open: false,
            history_filter: String::new(),
            downloads_open: false,
            tab_actions_open: false,
            recently_closed_open: false,
            transition: None,
            event_proxy: None,
            pending_respawn_focus: None,
        };
        for err in [settings_err, bookmarks_err].into_iter().flatten() {
            app.queue_toast(err);
        }
        app
    }
}

impl App {
    /// winit's own DPI scale factor — physical pixels per logical point.
    ///
    /// **Not applied to outgoing `InputEvent::Mouse*`/`Scroll` coordinates.**
    /// Those are sent in the engine's own *physical-pixel surface* space,
    /// the exact same space as `HostToEngine::Resize`'s `width`/`height`
    /// (see `ipc_protocol::InputEvent`'s doc comment) — every engine-host
    /// renders a surface of precisely that pixel size, so a physical offset
    /// into the on-screen content quad is already the right value to send,
    /// with no conversion at all. Used instead for the two places that
    /// genuinely need logical points: saving window geometry
    /// (`Settings::window_width`) and placing egui UI at a coordinate an
    /// engine reported back (`EngineToHost::ContextMenuLink`).
    fn ppp(&self) -> f64 {
        self.window.as_ref().map_or(1.0, |w| w.scale_factor())
    }

    fn chrome_height_points(&self) -> f32 {
        match self.view_mode {
            ViewMode::Item => CHROME_HEIGHT_POINTS,
            ViewMode::Graph | ViewMode::Settings => GRAPH_TOOLBAR_HEIGHT_POINTS,
        }
    }

    /// Item view showing a Tab -- the case every engine-facing path (input
    /// forwarding, content panes, address bar, find bar) cares about.
    fn showing_tab_view(&self) -> bool {
        self.view_mode == ViewMode::Item && self.viewed_image.is_none()
    }

    fn showing_image_view(&self) -> bool {
        self.view_mode == ViewMode::Item && self.viewed_image.is_some()
    }

    fn screen_size_points(&self) -> egui::Vec2 {
        let screen = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let ppp = self.ppp() as f32;
        egui::vec2(screen.width as f32 / ppp, screen.height as f32 / ppp)
    }

    /// Item view's content area (points, screen space): everything below
    /// the toolbar.
    fn item_body_rect(&self) -> egui::Rect {
        let screen = self.screen_size_points();
        egui::Rect::from_min_max(
            egui::pos2(0.0, CHROME_HEIGHT_POINTS),
            egui::pos2(screen.x, screen.y.max(CHROME_HEIGHT_POINTS)),
        )
    }

    /// Keeps the OS window title in sync with the focused tab's own title
    /// -- previously hardcoded to `"manifold"` regardless of
    /// what was open, unlike every real browser's own "the window
    /// represents the page you're looking at" convention. Reflects
    /// `self.focused` regardless of `view_mode` (it's "the last tab you
    /// were looking at" even from Graph/Settings, same reasoning
    /// `self.focused` already uses as the default parent for a new tab
    /// there). Falls back to the url if the title hasn't arrived yet
    /// (early in a fresh navigation), and to the plain app name if nothing
    /// is focused at all (no tabs open, or a tab that's since vanished).
    /// Called once a frame; only actually calls `Window::set_title` when
    /// the computed title changed from `last_window_title`. In Image view,
    /// the image's filename instead.
    fn update_window_title(&mut self) {
        let Some(window) = &self.window else { return };
        let image_title = self
            .viewed_image
            .as_ref()
            .filter(|_| self.view_mode == ViewMode::Item)
            .and_then(|id| self.nodes.get(id))
            .and_then(|n| n.as_image())
            .map(|i| download_filename(&i.path).to_owned());
        let title = image_title
            .or_else(|| {
                self.focused
                    .as_ref()
                    .and_then(|id| self.nodes.get(id))
                    .and_then(|n| n.as_tab())
                    .map(|n| if n.title.is_empty() { n.url.clone() } else { n.title.clone() })
            })
            .filter(|t| !t.is_empty())
            .map(|t| format!("{t} — manifold"))
            .unwrap_or_else(|| "manifold".to_string());
        if title != self.last_window_title {
            window.set_title(&title);
            self.last_window_title = title;
        }
    }

    fn chrome_height_px(&self) -> u32 {
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        (self.chrome_height_points() * ppp).round() as u32
    }

    /// The engine-host's rendered surface only needs to cover the area
    /// below chrome — telling it to render at the full window size (when
    /// the compositor only ever displays it in the smaller content
    /// viewport, see `gpu::GpuState::render`) would waste work and,
    /// because the viewport stretches whatever the engine sends to fill
    /// it, visibly squash the page. Halved (no divider gap reserved) while
    /// `split_tab` is set -- both the primary and secondary pane are
    /// exactly half the window wide on screen, so each engine should
    /// render at that same real footprint rather than the full width, the
    /// same "match the actual on-screen size" reasoning this already
    /// applied to chrome height. `send_resize_to` is the only caller, and
    /// both `focused` and `split_tab` get resent through it whenever this
    /// value would change for either of them (window resize, or a split
    /// opening/closing) -- see those call sites.
    fn content_size_px(&self) -> (u32, u32) {
        let full = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let height = full.height.saturating_sub(self.chrome_height_px());
        let width = if self.split_tab.is_some() { full.width / 2 } else { full.width };
        (width, height)
    }

    /// The on-screen boundary (physical pixels, x) between the primary and
    /// secondary pane while a split is open -- `None` when there's no
    /// split, meaning the whole content width belongs to the primary pane.
    /// `window_event`'s mouse handlers only ever forward to the primary
    /// pane (`self.focused`) in this first cut -- the secondary pane is
    /// view-only, see `split_tab`'s own doc comment -- so they gate on the
    /// cursor being left of this boundary (or there being no split at all)
    /// on top of the existing below-chrome check.
    fn split_divider_x_px(&self) -> Option<f64> {
        self.split_tab.as_ref()?;
        let width = self.window.as_ref().map_or(1024, |w| w.inner_size().width);
        Some(width as f64 / 2.0)
    }

    fn send_resize_to(&mut self, tab_id: &str) {
        let (width, height) = self.content_size_px();
        let dpi_scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            node.tab.send(&HostToEngine::Resize {
                width,
                height,
                dpi_scale,
            });
        }
    }

    /// Sends `tab_id`'s own `TabPayload::zoom_factor` to its engine — called
    /// right after spawning it (already initialized from `Settings::
    /// zoom_factor` at that point, see `TabPayload::zoom_factor`'s own doc
    /// comment) and again whenever `set_focused_zoom` changes that specific
    /// tab's value. Deliberately reads the *tab's own* field, not `self.
    /// settings.zoom_factor` directly -- see `set_focused_zoom`'s own doc
    /// comment for the bug this replaced.
    fn send_zoom_to(&mut self, tab_id: &str) {
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            let factor = node.zoom_factor;
            node.tab.send(&HostToEngine::SetZoom { factor });
        }
    }

    /// The focused tab's own real zoom, for the Settings page's display and
    /// as the base the `+`/`-` shortcuts adjust from -- falls back to
    /// `Settings::zoom_factor` (the default for new tabs) only when nothing
    /// is focused at all (e.g. Graph view, nothing selected), since there's
    /// no tab-specific value to show in that case.
    fn focused_zoom_factor(&self) -> f64 {
        self.focused
            .as_deref()
            .and_then(|id| self.nodes.get(id))
            .and_then(|n| n.as_tab())
            .map(|t| t.zoom_factor)
            .unwrap_or(self.settings.zoom_factor)
    }

    /// Sets `factor` as both the new default for future tabs (`Settings::
    /// zoom_factor`, persisted) and, immediately, the focused tab's own
    /// zoom -- replaces the old `apply_zoom_change`, which only ever
    /// touched the shared global value and re-sent *that* to the focused
    /// tab. Confirmed live to cause a real bug: zooming tab A then
    /// switching to tab B left the Settings page's zoom control showing
    /// tab A's value (the stale global) while tab B's engine was still at
    /// its own different real zoom -- pressing "-" from that display then
    /// jumped tab B's actual zoom by an unexpected amount, not the 10%
    /// step the button claimed. Now every read/write of "the current zoom"
    /// goes through this pair (`focused_zoom_factor`/`set_focused_zoom`)
    /// instead of `self.settings.zoom_factor` directly, so the display and
    /// the tab it's supposedly controlling can't diverge.
    fn set_focused_zoom(&mut self, factor: f64) {
        self.settings.zoom_factor = factor;
        self.save_settings();
        if let Some(id) = self.focused.clone() {
            if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                node.zoom_factor = factor;
            }
            self.send_zoom_to(&id);
        }
    }

    /// The Settings page (`ViewMode::Settings`): default search engine,
    /// default starting engine, zoom, and home page — see `settings.rs` for
    /// what each actually controls and `App::settings`'s own doc comment
    /// for why this is one shared page rather than per-view state. Every
    /// control saves to disk immediately on change (no separate "Save"
    /// step, consistent with every other instant-apply control in this
    /// chrome, e.g. the engine picker). **A real page, not a popup**: the
    /// original gear-icon dropdown was a fixed-position `Area` with no
    /// scroll area and a fixed 200pt width -- overflowed off the bottom of
    /// a small window once the saved-sessions list grew long enough,
    /// reported live by a user. A full page gets the whole window to
    /// scroll in instead of a hand-picked popup height.
    fn build_settings_page(&mut self, ui: &mut egui::Ui) {
        let mut save_as_requested = false;
        let mut open_session_name: Option<String> = None;
        let mut delete_session_name: Option<String> = None;
        let mut export_requested = false;
        let mut import_requested = false;
        let mut export_bookmarks_requested = false;
        let mut import_bookmarks_requested = false;
        let mut save_now_requested = false;
        // "Current Tab" section, below -- `self.focused` persists across a
        // view switch (entering Settings doesn't clear it), so these act on
        // whichever tab was focused right before Settings was opened, same
        // as the toolbar controls they replaced.
        let mut current_tab_id: Option<String> = None;
        let mut mute_toggle = false;
        let mut screenshot_requested = false;
        let mut print_to_pdf_requested = false;
        // "Lists" section, below -- same popups the old toolbar "Lists"
        // dropdown opened, just anchored to these buttons instead.
        let mut bookmarks_button_rect: Option<egui::Rect> = None;
        let mut history_button_rect: Option<egui::Rect> = None;
        let mut downloads_button_rect: Option<egui::Rect> = None;
        let mut default_engine_button_rect: Option<egui::Rect> = None;
        let mut current_engine_button_rect: Option<egui::Rect> = None;

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_settings_page();
        }

        toolbar_panel("settings_toolbar", ui, |ui| {
            ui.horizontal_centered(|ui| {
                if icon_button_eased(
                    ui,
                    egui::Id::new("icon_btn_settings_back"),
                    format!("{} Back", egui_phosphor::light::CARET_LEFT),
                )
                .clicked()
                {
                    self.close_settings_page();
                }
                ui.label(heading_text("Settings"));
            });
        });

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::default().inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
                ui.set_max_width(480.0);
                ui.add_space(12.0);

                ui.label("Default search engine");
                egui::ComboBox::from_id_salt("settings_search_engine")
                    .selected_text(self.settings.default_search_engine.label())
                    .show_ui(ui, |ui| {
                        for engine in SearchEngine::ALL {
                            let selected = self.settings.default_search_engine == engine;
                            if ui.selectable_label(selected, engine.label()).clicked() && !selected {
                                self.settings.default_search_engine = engine;
                                self.save_settings();
                            }
                        }
                    });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Suggestions always come from DuckDuckGo; this only picks where a search actually goes.",
                        )
                        .small()
                        .weak(),
                    )
                    .wrap(),
                );

                ui.add_space(8.0);
                ui.label("Default starting engine (new tabs)");
                let combo_text =
                    egui::RichText::new(engine_label(self.settings.default_starting_engine))
                        .color(engine_color(self.settings.default_starting_engine));
                let default_engine_response = ui.button(combo_text);
                if default_engine_response.clicked() {
                    self.default_engine_picker_open = !self.default_engine_picker_open;
                }
                default_engine_button_rect = Some(default_engine_response.rect);

                ui.add_space(8.0);
                ui.label("Zoom (new tabs; also applies to the current tab now)");
                ui.horizontal(|ui| {
                    // Reads/writes the *focused tab's own* zoom, not the raw
                    // `Settings::zoom_factor` -- see `set_focused_zoom`'s own
                    // doc comment for the display-vs-reality bug this fixed.
                    if ui.button(egui_phosphor::light::MINUS).clicked() {
                        self.set_focused_zoom((self.focused_zoom_factor() - 0.1).max(0.5));
                    }
                    ui.label(format!("{:.0}%", self.focused_zoom_factor() * 100.0));
                    if ui.button(egui_phosphor::light::PLUS).clicked() {
                        self.set_focused_zoom((self.focused_zoom_factor() + 0.1).min(2.0));
                    }
                    if ui.button("Reset").clicked() {
                        self.set_focused_zoom(1.0);
                    }
                });

                ui.add_space(8.0);
                ui.label("Home page (new tabs)");
                if ui
                    .add(egui::TextEdit::singleline(&mut self.settings.home_page).desired_width(f32::INFINITY))
                    .lost_focus()
                {
                    self.save_settings();
                }

                ui.add_space(10.0);
                ui.separator();
                ui.label(heading_text("Current Tab"));
                // Engine swap, Mute, Screenshot, and Print to PDF used to
                // be live toolbar controls next to the address bar --
                // moved here to keep that toolbar down to nav + address
                // bar + a small handful of page actions, at the cost of a
                // click through Settings instead of an in-place toggle.
                // `focus_kind` is `None` either when no tab is focused at
                // all, or Settings was reached with a non-tab node
                // (group/note/etc.) focused -- either way, nothing here
                // applies to it.
                let focus_kind =
                    self.focused.clone().and_then(|id| {
                        self.nodes.get(&id).and_then(|n| n.as_tab()).map(|n| n.tab.kind).map(|kind| (id, kind))
                    });
                match focus_kind {
                    Some((id, kind)) => {
                        current_tab_id = Some(id.clone());
                        ui.horizontal(|ui| {
                            ui.label("Engine");
                            let combo_text = egui::RichText::new(engine_label(kind)).color(engine_color(kind));
                            let current_engine_response = ui.button(combo_text);
                            if current_engine_response.clicked() {
                                self.current_engine_picker_open = !self.current_engine_picker_open;
                            }
                            current_engine_button_rect = Some(current_engine_response.rect);
                        });
                        let node_muted =
                            self.nodes.get(&id).and_then(|n| n.as_tab()).is_some_and(|n| n.muted);
                        let mute_label = if node_muted {
                            format!("{} Unmute", egui_phosphor::light::SPEAKER_X)
                        } else {
                            format!("{} Mute", egui_phosphor::light::SPEAKER_HIGH)
                        };
                        if ui.selectable_label(node_muted, mute_label).clicked() {
                            mute_toggle = true;
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .button(format!("{} Screenshot", egui_phosphor::light::CAMERA))
                                .clicked()
                            {
                                screenshot_requested = true;
                            }
                            if !self.headless
                                && ui
                                    .button(format!("{} Print to PDF", egui_phosphor::light::PRINTER))
                                    .clicked()
                            {
                                print_to_pdf_requested = true;
                            }
                        });
                    }
                    None => {
                        ui.add(
                            egui::Label::new(egui::RichText::new("No tab focused").small().weak()).wrap(),
                        );
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.label(heading_text("Lists"));
                ui.horizontal(|ui| {
                    let bookmarks_response = ui.button(format!("Bookmarks ({})", self.bookmarks.len()));
                    if bookmarks_response.clicked() {
                        self.bookmarks_open = !self.bookmarks_open;
                        if self.bookmarks_open {
                            self.bookmarks_filter.clear();
                        }
                    }
                    bookmarks_button_rect = Some(bookmarks_response.rect);
                    let history_response = ui.button(format!("History ({})", self.history.len()));
                    if history_response.clicked() {
                        self.history_open = !self.history_open;
                        if self.history_open {
                            self.history_filter.clear();
                        }
                    }
                    history_button_rect = Some(history_response.rect);
                    // Not `self.nodes.len()` (every graph node, not just
                    // downloads) -- a real pre-existing bug found and fixed
                    // when this button was still in the toolbar's own
                    // "Lists" dropdown, carried forward here unchanged.
                    let download_count = self.nodes.values().filter(|n| n.as_download().is_some()).count();
                    let downloads_response = ui.button(format!("Downloads ({download_count})"));
                    if downloads_response.clicked() {
                        self.downloads_open = !self.downloads_open;
                    }
                    downloads_button_rect = Some(downloads_response.rect);
                });

                ui.add_space(10.0);
                ui.separator();
                ui.label(heading_text("Sessions"));
                if ui
                    .button("Save Now")
                    .on_hover_text("Write the auto-restore session.json immediately")
                    .clicked()
                {
                    save_now_requested = true;
                }
                let last_saved = match self.last_session_save {
                    Some(at) => format!("Last saved {}s ago", at.elapsed().as_secs()),
                    None => "Not saved yet this run".to_string(),
                };
                ui.add(egui::Label::new(egui::RichText::new(last_saved).small().weak()).wrap());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.session_name_draft)
                            .desired_width(120.0)
                            .hint_text("Name"),
                    );
                    if ui.button("Save As").clicked() {
                        save_as_requested = true;
                    }
                });
                let names = session::Session::list_named();
                if names.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new("No saved sessions").small().weak())
                            .wrap(),
                    );
                }
                for name in names {
                    ui.horizontal(|ui| {
                        if ui.button(&name).on_hover_text("Open").clicked() {
                            open_session_name = Some(name.clone());
                        }
                        if ui
                            .small_button(egui_phosphor::light::X)
                            .on_hover_text("Delete")
                            .clicked()
                        {
                            delete_session_name = Some(name.clone());
                        }
                    });
                }
                // A real native file dialog blocks on OS/human
                // input -- fine for the real app (that's what a
                // modal dialog is for), but headless mode has
                // neither, so it would just hang forever with no
                // way to test it here. `EXPORTSESSION`/
                // `IMPORTSESSION` debug commands call
                // `build_session_snapshot().save_to`/
                // `session::Session::load_from` directly with an
                // explicit path instead, bypassing the dialog
                // entirely (import still stages into
                // `pending_import` same as a real file pick would,
                // so `CONFIRMIMPORT`/`CANCELIMPORT` can headlessly
                // test the confirm popup too), the same "headless
                // calls the effect directly" pattern every real-
                // keyboard-shortcut debug command uses.
                if !self.headless {
                    ui.horizontal(|ui| {
                        if ui.button("Export...").clicked() {
                            export_requested = true;
                        }
                        if ui.button("Import...").clicked() {
                            import_requested = true;
                        }
                    });
                }

                ui.add_space(10.0);
                ui.separator();
                ui.label(heading_text("Bookmarks"));
                // Same "real file dialog blocks on OS/human input
                // neither exists in headless mode" reasoning as
                // Export/Import Session above -- `EXPORTBOOKMARKS`/
                // `IMPORTBOOKMARKS` debug commands bypass the dialog
                // the same way.
                if !self.headless {
                    ui.horizontal(|ui| {
                        if ui.button("Export...").clicked() {
                            export_bookmarks_requested = true;
                        }
                        if ui.button("Import...").clicked() {
                            import_bookmarks_requested = true;
                        }
                    });
                }

                ui.add_space(12.0);
            });
        });
        if save_now_requested {
            self.save_session();
        }
        if save_as_requested {
            let name = sanitize_session_name(&self.session_name_draft);
            if !name.is_empty() {
                self.build_session_snapshot().save_as(&name);
                self.queue_toast(format!("Saved session '{name}'"));
                self.session_name_draft.clear();
            }
        }
        if let Some(name) = open_session_name {
            self.load_named_session(&name);
            self.close_settings_page();
        }
        if let Some(name) = delete_session_name {
            session::Session::delete_named(&name);
        }
        if export_requested {
            if let Some((path, toast)) = save_dialog_with_toast("session.json", "Session") {
                self.build_session_snapshot().save_to(&path);
                self.queue_toast(toast.0);
            }
        }
        if import_requested {
            if let Some(path) =
                rfd::FileDialog::new().add_filter("Session", &["json"]).pick_file()
            {
                match session::Session::load_from(&path) {
                    Ok(session) => {
                        self.pending_import = Some(session);
                        self.close_settings_page();
                    }
                    Err(err) => self.queue_toast(err),
                }
            }
        }
        if export_bookmarks_requested {
            if let Some((path, toast)) = save_dialog_with_toast("bookmarks.json", "Bookmarks") {
                bookmarks::save_to(&self.bookmarks, &path);
                self.queue_toast(toast.0);
            }
        }
        if import_bookmarks_requested {
            if let Some(path) =
                rfd::FileDialog::new().add_filter("Bookmarks", &["json"]).pick_file()
            {
                match bookmarks::load_from(&path) {
                    Ok(imported) => {
                        self.bookmarks = imported;
                        bookmarks::save(&self.bookmarks);
                    }
                    Err(err) => self.queue_toast(err),
                }
                self.close_settings_page();
            }
        }
        if mute_toggle {
            if let Some(id) = &current_tab_id {
                self.toggle_muted(id);
            }
        }
        if screenshot_requested {
            // No toast here -- the save hasn't happened yet. `save_toast`
            // is only set once `RenderOutcome::screenshot_result` actually
            // confirms success (see `WindowEvent::RedrawRequested`'s
            // handling of `pending_screenshot`).
            let path = screenshot_default_path();
            self.pending_screenshot = Some(path.to_string_lossy().to_string());
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        if print_to_pdf_requested {
            if let Some(path) =
                rfd::FileDialog::new().set_file_name("page.pdf").add_filter("PDF", &["pdf"]).save_file()
            {
                self.start_print_to_pdf(path.to_string_lossy().into_owned());
            }
        }
        // Same fade-in/out-off-`ctx.animate_bool` popup pattern the old
        // toolbar "Lists" dropdown used, just anchored to these buttons
        // instead -- `build_bookmarks_popup`/`build_history_popup`/
        // `build_downloads_popup` themselves are unchanged either way.
        let bookmarks_fade =
            ui.ctx().animate_bool(egui::Id::new("bookmarks_popup_fade"), self.bookmarks_open);
        if bookmarks_fade > 0.0 {
            if let Some(rect) = bookmarks_button_rect {
                self.build_bookmarks_popup(ui, rect, bookmarks_fade);
            }
        }
        let history_fade = ui.ctx().animate_bool(egui::Id::new("history_popup_fade"), self.history_open);
        if history_fade > 0.0 {
            if let Some(rect) = history_button_rect {
                self.build_history_popup(ui, rect, history_fade);
            }
        }
        let downloads_fade =
            ui.ctx().animate_bool(egui::Id::new("downloads_popup_fade"), self.downloads_open);
        if downloads_fade > 0.0 {
            if let Some(rect) = downloads_button_rect {
                self.build_downloads_popup(ui, rect, downloads_fade);
            }
        }
        // Same fade pattern as the three list popups above, but drawing
        // `build_engine_picker_popup` (tier/pros/cons cards) instead of a
        // plain list -- see that function's own doc comment.
        let default_engine_fade = ui
            .ctx()
            .animate_bool(egui::Id::new("default_engine_popup_fade"), self.default_engine_picker_open);
        if default_engine_fade > 0.0 {
            if let Some(rect) = default_engine_button_rect {
                let (area_rect, picked) = build_engine_picker_popup(
                    ui.ctx(),
                    "default_engine_popup",
                    rect,
                    Some(self.settings.default_starting_engine),
                    default_engine_fade,
                    self.default_engine_picker_open,
                );
                if let Some(kind) = picked {
                    self.settings.default_starting_engine = kind;
                    self.save_settings();
                    self.default_engine_picker_open = false;
                } else if clicked_outside_popup(ui, area_rect, Some(rect)) {
                    self.default_engine_picker_open = false;
                }
            }
        }
        let current_engine_fade = ui
            .ctx()
            .animate_bool(egui::Id::new("current_engine_popup_fade"), self.current_engine_picker_open);
        if current_engine_fade > 0.0 {
            if let Some(rect) = current_engine_button_rect {
                let current_kind = current_tab_id
                    .as_ref()
                    .and_then(|id| self.nodes.get(id))
                    .and_then(|n| n.as_tab())
                    .map(|t| t.tab.kind);
                let (area_rect, picked) = build_engine_picker_popup(
                    ui.ctx(),
                    "current_engine_popup",
                    rect,
                    current_kind,
                    current_engine_fade,
                    self.current_engine_picker_open,
                );
                if let Some(kind) = picked {
                    if let Some(id) = &current_tab_id {
                        if Some(kind) != current_kind {
                            self.start_swap_to(id, kind);
                        }
                    }
                    self.current_engine_picker_open = false;
                } else if clicked_outside_popup(ui, area_rect, Some(rect)) {
                    self.current_engine_picker_open = false;
                }
            }
        }
    }

    /// The "Recently Closed" toolbar dropdown (Graph view, top level or
    /// not -- unlike groups, this isn't level-scoped): closed groups first
    /// (`self.closed_groups`), then closed tabs (`self.closed_tabs`), each
    /// section most recently closed first, click one to respawn it and
    /// remove it from its list. `Ctrl+Shift+T`/`REOPENTAB` only ever
    /// reaches `closed_tabs`' *last* entry (see that field's own doc
    /// comment for why groups aren't reachable that way); this dropdown is
    /// the only way to reopen an older tab, or any closed group at all.
    fn build_recently_closed_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut reopen_tab_index: Option<usize> = None;
        let mut reopen_group_index: Option<usize> = None;
        let mut reopen_node_index: Option<usize> = None;
        let area_rect =
            build_list_popup_frame(ui.ctx(), "recently_closed_popup", anchor_rect, 220.0, 320.0, fade, self.recently_closed_open, |ui| {
            for (index, group) in self.closed_groups.iter().enumerate().rev() {
                ui.horizontal(|ui| {
                    ui.label(egui_phosphor::light::FOLDER_SIMPLE);
                    let label = format!("{} ({})", group.name, group.tabs.len());
                    if ui.add(egui::Label::new(label).truncate().sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        reopen_group_index = Some(index);
                    }
                });
            }
            if !self.closed_groups.is_empty() && !self.closed_tabs.is_empty() {
                ui.separator();
            }
            for (index, closed) in self.closed_tabs.iter().enumerate().rev() {
                ui.horizontal(|ui| {
                    engine_dot(ui, closed.kind, 8.0);
                    if ui.add(egui::Label::new(&closed.url).truncate().sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        reopen_tab_index = Some(index);
                    }
                });
            }
            if !self.closed_nodes.is_empty() && (!self.closed_groups.is_empty() || !self.closed_tabs.is_empty()) {
                ui.separator();
            }
            for (index, node) in self.closed_nodes.iter().enumerate().rev() {
                let (icon, label) = match node {
                    ClosedNode::Note(text) => {
                        (egui_phosphor::light::NOTE_PENCIL, text.lines().next().unwrap_or("(empty note)"))
                    }
                    ClosedNode::Image(path) => (egui_phosphor::light::IMAGE, path.as_str()),
                    ClosedNode::Pdf(path) => (egui_phosphor::light::FILE_PDF, path.as_str()),
                    ClosedNode::Json(path) => (egui_phosphor::light::BRACKETS_CURLY, path.as_str()),
                };
                ui.horizontal(|ui| {
                    ui.label(icon);
                    if ui.add(egui::Label::new(label).truncate().sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        reopen_node_index = Some(index);
                    }
                });
            }
        });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some(index) = reopen_group_index {
            let export = self.closed_groups.remove(index);
            self.import_group_export(export);
            close = true;
        }
        if let Some(index) = reopen_tab_index {
            let closed = self.closed_tabs.remove(index);
            self.respawn_closed_tab(closed);
            close = true;
        }
        if let Some(index) = reopen_node_index {
            match self.closed_nodes.remove(index) {
                ClosedNode::Note(text) => self.respawn_closed_note(text),
                ClosedNode::Image(path) => {
                    self.insert_image_node(path);
                    self.save_session();
                }
                ClosedNode::Pdf(path) => {
                    self.insert_pdf_node(path);
                    self.save_session();
                }
                ClosedNode::Json(path) => {
                    self.insert_json_node(path);
                    self.save_session();
                }
            }
            close = true;
        }
        if clicked_outside {
            close = true;
        }
        if close || (self.closed_tabs.is_empty() && self.closed_groups.is_empty() && self.closed_nodes.is_empty()) {
            self.recently_closed_open = false;
        }
    }

    /// Called from `close_node` for whatever `closed_node_for` returns
    /// (Downloads never record) -- same bound-then-push shape `close_tab`'s
    /// own `closed_tabs` push already has.
    fn record_closed_node(&mut self, node: ClosedNode) {
        self.closed_nodes.push(node);
        if self.closed_nodes.len() > MAX_CLOSED_NODES {
            self.closed_nodes.remove(0);
        }
    }

    fn navigate_tab(&mut self, tab_id: &str, url: String) {
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            // Recorded here, not left to `NavigationStateChanged`'s own
            // history-recording: this sets `node.url` immediately
            // (synchronously, before the engine round-trips its own
            // confirmation), so that handler's own `node.url != url` check
            // would already be false by the time its event arrives,
            // silently missing every address-bar-driven navigation. Title
            // isn't known yet at this point (the page hasn't loaded), so
            // this entry shows as its bare url until superseded by a later
            // one -- `build_history_popup` already falls back to the url
            // for an empty title.
            if node.url != url {
                self.history.push(HistoryEntry { title: String::new(), url: url.clone() });
                if self.history.len() > MAX_HISTORY_ENTRIES {
                    self.history.remove(0);
                }
            }
        }
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            node.url = url.clone();
            node.tab
                .send(&HostToEngine::Navigate { url, referrer: None });
        }
    }

    /// Address-bar autocomplete's open-tab source: every *other* tab (an
    /// exact-match-only self isn't a useful "switch to" suggestion) whose
    /// URL or title contains `query`, case-insensitively. Capped and
    /// ordered by `tab_order` for determinism — this is a small in-memory
    /// list, not worth a fuzzy-match/relevance-ranking pass.
    fn compute_open_tab_suggestions(&self, query: &str, exclude: &str) -> Vec<Suggestion> {
        if query.is_empty() {
            return Vec::new();
        }
        let query = query.to_lowercase();
        self.node_order
            .iter()
            .filter(|id| id.as_str() != exclude)
            .filter_map(|id| self.nodes.get(id).and_then(|n| n.as_tab()).map(|node| (id, node)))
            .filter(|(_, node)| {
                node.url.to_lowercase().contains(&query) || node.title.to_lowercase().contains(&query)
            })
            .take(5)
            .map(|(id, node)| Suggestion::OpenTab {
                tab_id: id.clone(),
                url: node.url.clone(),
                title: node.title.clone(),
            })
            .collect()
    }

    /// The combined, ordered list the dropdown actually renders: open tabs
    /// first (jumping to something already loaded beats a fresh
    /// navigation/search), then search-engine suggestions.
    fn combined_suggestions(&self) -> Vec<Suggestion> {
        let mut all = self.open_tab_suggestions.clone();
        all.extend(
            self.search_suggestions
                .iter()
                .cloned()
                .map(|query| Suggestion::Search { query }),
        );
        all
    }

    /// Accepting a suggestion: jump straight to an already-open tab (no
    /// navigation at all), or navigate the focused tab to a search.
    /// Either way the dropdown itself is cleared and the address bar drops
    /// out of its "actively editing" state so it re-syncs to whichever
    /// tab ends up focused, same as any other navigation.
    fn accept_suggestion(&mut self, suggestion: Suggestion, focused_tab_id: &str) {
        match suggestion {
            Suggestion::OpenTab { tab_id, .. } => self.focus_tab(&tab_id),
            Suggestion::Search { query } => {
                let url = self.settings.default_search_engine.search_url(&query);
                self.navigate_tab(focused_tab_id, url);
            }
        }
        self.open_tab_suggestions.clear();
        self.search_suggestions.clear();
        self.address_bar_focused = false;
        self.dropdown_highlighted = None;
    }

    /// Snapshots the current tab graph (shape, url/engine/position per tab,
    /// focused id, view mode) and writes it to disk — called at every point
    /// that shape actually changes (a tab spawns, closes, navigates, or
    /// focus/view changes), not once per frame; see `session::Session`'s
    /// own doc comment for what is/isn't portable across a restart.
    ///
    /// **Concurrency-safe, not just fire-and-forget**: before actually
    /// overwriting `session.json`, checks whether its on-disk fingerprint
    /// still matches what this process last loaded/saved -- if some *other*
    /// process (another `manifold.exe` instance, or anything else sharing
    /// this profile) has written to it in the meantime, this write is
    /// redirected to `session::conflict_path()` instead of clobbering
    /// theirs, and the user is told via a toast. Found live, the hard way:
    /// two of this project's own concurrent development/testing sessions
    /// silently overwrote each other's `session.json` with zero warning,
    /// before this existed -- see the README's own entry on this fix for
    /// the full incident. The first save of a run (`session_fingerprint`
    /// still `None` -- nothing was ever loaded, or this is a fresh install)
    /// always proceeds normally; there's nothing to conflict with yet.
    fn save_session(&mut self) {
        let snapshot = self.build_session_snapshot();
        let on_disk = session::fingerprint();
        if self.session_fingerprint.is_some() && on_disk != self.session_fingerprint {
            if let Some(path) = session::conflict_path() {
                snapshot.save_to(&path);
                eprintln!(
                    "session: session.json changed on disk since last load/save -- wrote {} instead",
                    path.display()
                );
                self.queue_toast(format!(
                    "Session changed by another process -- your changes saved to {}",
                    path.display()
                ));
            }
            return;
        }
        snapshot.save();
        self.session_fingerprint = session::fingerprint();
        self.last_session_save = Some(std::time::Instant::now());
    }

    /// Same concurrency-safety idea as `save_session`, for `settings.json` --
    /// see that function's own doc comment for the full reasoning. Replaces
    /// every direct `self.settings.save()` call site.
    fn save_settings(&mut self) {
        let on_disk = settings::fingerprint();
        if self.settings_fingerprint.is_some() && on_disk != self.settings_fingerprint {
            if let Some(path) = settings::conflict_path() {
                self.settings.save_to(&path);
                eprintln!(
                    "settings: settings.json changed on disk since last load/save -- wrote {} instead",
                    path.display()
                );
                self.queue_toast(format!(
                    "Settings changed by another process -- your changes saved to {}",
                    path.display()
                ));
            }
            return;
        }
        self.settings.save();
        self.settings_fingerprint = settings::fingerprint();
    }

    /// Shared body for the Note/Image/Pdf/Json cases of
    /// `build_session_snapshot` below -- each of those four kinds' own
    /// snapshot is just `(id, one string field, position)`, differing only
    /// in which node accessor supplies the string. Returns id/field/pos_x/
    /// pos_y tuples; the caller maps each into its own concrete `Session*`
    /// struct.
    fn collect_session_field<'a>(
        &'a self,
        extract: impl Fn(&'a GraphNode) -> Option<&'a str>,
    ) -> Vec<(String, String, f32, f32)> {
        self.node_order
            .iter()
            .filter_map(|id| {
                let node = self.nodes.get(id)?;
                let field = extract(node)?;
                Some((id.clone(), field.to_string(), node.pos.x, node.pos.y))
            })
            .collect()
    }

    /// The `session::Session` value both the auto-restore `save_session`
    /// and "Save Session As..." serialize -- factored out so the two never
    /// drift into building this differently.
    fn build_session_snapshot(&self) -> session::Session {
        let tabs = self
            .node_order
            .iter()
            .filter_map(|id| {
                let node = self.nodes.get(id)?;
                let tab = node.as_tab()?;
                Some(session::SessionTab {
                    id: id.clone(),
                    parent: node.parent.clone(),
                    url: tab.url.clone(),
                    engine: tab.tab.kind,
                    pos_x: node.pos.x,
                    pos_y: node.pos.y,
                })
            })
            .collect();
        // Note/Image/Pdf/Json's own snapshots are all "id, one string field,
        // position" -- only a tab's needs the extra `parent`/`engine` fields
        // above, so it stays its own block. `collect_session_field` shares
        // the actual duplicated part (the `node_order` walk, the id clone,
        // the position read); each kind still builds its own concrete
        // `Session*` struct, since there's no shared trait across them worth
        // introducing just for this.
        let notes = self
            .collect_session_field(|n| n.as_note().map(|note| note.text.as_str()))
            .into_iter()
            .map(|(id, text, pos_x, pos_y)| session::SessionNote { id, text, pos_x, pos_y })
            .collect();
        let images = self
            .collect_session_field(|n| n.as_image().map(|image| image.path.as_str()))
            .into_iter()
            .map(|(id, path, pos_x, pos_y)| session::SessionImage { id, path, pos_x, pos_y })
            .collect();
        let pdfs = self
            .collect_session_field(|n| n.as_pdf().map(|pdf| pdf.path.as_str()))
            .into_iter()
            .map(|(id, path, pos_x, pos_y)| session::SessionPdf { id, path, pos_x, pos_y })
            .collect();
        let jsons = self
            .collect_session_field(|n| n.as_json().filter(|json| json.key.is_none()).map(|json| json.path.as_str()))
            .into_iter()
            .map(|(id, path, pos_x, pos_y)| session::SessionJson { id, path, pos_x, pos_y })
            .collect();
        session::Session {
            tabs,
            notes,
            images,
            pdfs,
            jsons,
            focused: self.focused.clone(),
            view_mode_tab: matches!(self.view_mode, ViewMode::Item),
            // `viewed_image` stays `Some` while Settings is open on top of
            // Image view (`open_settings_page` only swaps `view_mode`, so
            // it can restore it via `settings_return_view`) -- the
            // invariant is `viewed_image` is `Some` only while
            // `view_mode == Item` (see the design doc's Plan-time
            // corrections), so a snapshot taken from Settings must not
            // write a `viewed_image` alongside `view_mode_tab: false`.
            viewed_image: self.viewed_image.clone().filter(|_| self.view_mode == ViewMode::Item),
            next_tab_id: self.next_tab_id,
            next_note_id: self.next_note_id,
            next_image_id: self.next_image_id,
            next_pdf_id: self.next_pdf_id,
            next_json_id: self.next_json_id,
        }
    }

    /// Startup: replays a saved `session::Session` instead of spawning one
    /// fresh root tab — returns `false` (having done nothing) if there's no
    /// session file, it's corrupt (moved aside with a toast -- see
    /// `Session::load`), or it has zero tabs, so the caller can
    /// fall back to `spawn_root_tab`, same "never let a bad/missing file
    /// stop the browser from starting" posture `Settings::load` already
    /// takes. All of a session's tabs are spawned concurrently (like
    /// `process_manager::spawn_tab` always is) rather than parent-before-
    /// child — a child referencing a not-yet-spawned parent is already a
    /// case `graph_layout::step` handles gracefully (skips the edge's
    /// spring force until both ends exist), so there's no correctness
    /// reason to serialize the spawns, only a briefly-less-tidy transient
    /// layout.
    fn restore_session(&mut self) -> bool {
        let session = match session::Session::load() {
            Ok(Some(session)) => session,
            Ok(None) => return false,
            Err(err) => {
                self.queue_toast(err);
                return false;
            }
        };
        if session.tabs.is_empty()
            && session.notes.is_empty()
            && session.images.is_empty()
            && session.pdfs.is_empty()
            && session.jsons.is_empty()
        {
            return false;
        }
        // Re-reads the file `session::Session::load()` above already just
        // read -- see `settings_fingerprint`'s own init in `App::default`
        // for why this isn't worth threading out through `load()` itself.
        self.session_fingerprint = session::fingerprint();
        self.apply_session(session);
        true
    }

    /// Replays a `session::Session`'s tabs/focus/view-mode -- shared by
    /// startup's `restore_session` (loads `session.json`) and runtime's
    /// `load_named_session` ("Open Session", loads a named save). Callers
    /// are responsible for checking `session.tabs.is_empty()` themselves
    /// first (each wants a different fallback for that case) and, for a
    /// runtime load, for tearing down whatever was open before calling
    /// this.
    fn apply_session(&mut self, session: session::Session) {
        self.next_tab_id = session.next_tab_id;
        self.next_note_id = session.next_note_id;
        self.next_image_id = session.next_image_id;
        self.next_pdf_id = session.next_pdf_id;
        self.next_json_id = session.next_json_id;
        // A file with a counter missing (they all default to 0) or behind
        // its own ids would otherwise let the next new node reuse an id.
        for tab in &session.tabs {
            bump_past_id(&mut self.next_tab_id, &tab.id, "tab");
        }
        for note in &session.notes {
            bump_past_id(&mut self.next_note_id, &note.id, "note");
        }
        for image in &session.images {
            bump_past_id(&mut self.next_image_id, &image.id, "img");
        }
        for pdf in &session.pdfs {
            bump_past_id(&mut self.next_pdf_id, &pdf.id, "pdf");
        }
        for json in &session.jsons {
            bump_past_id(&mut self.next_json_id, &json.id, "json");
        }
        for note in session.notes {
            self.nodes.insert(
                note.id.clone(),
                GraphNode {
                    parent: None,
                    group: None,
                    pos: egui::vec2(note.pos_x, note.pos_y),
                    vel: egui::Vec2::ZERO,
                    color: None,
                    payload: NodePayload::Note(NotePayload { text: note.text, editing: false }),
                },
            );
            self.node_order.push(note.id);
        }
        // A card whose file is gone stays as a placeholder (`error`), so
        // the next save doesn't forget it.
        for image in session.images {
            let pos = egui::vec2(image.pos_x, image.pos_y);
            if let Err(error) = self.insert_image_node_with_id(image.id.clone(), image.path.clone(), pos, None) {
                let payload = ImagePayload { path: image.path, width: 0, height: 0, error: Some(error) };
                self.insert_placeholder_node(image.id, pos, NodePayload::Image(payload));
            }
        }
        for pdf in session.pdfs {
            let pos = egui::vec2(pdf.pos_x, pdf.pos_y);
            self.insert_pdf_node_with_id(pdf.id, pdf.path, pos, None);
        }
        for json in session.jsons {
            let pos = egui::vec2(json.pos_x, json.pos_y);
            if let Err(error) = self.insert_json_node_with_id(json.id.clone(), json.path.clone(), pos, None) {
                let value = serde_json::Value::Null;
                let payload = JsonPayload { path: json.path, value, key: None, error: Some(error) };
                self.insert_placeholder_node(json.id, pos, NodePayload::Json(payload));
            }
        }
        let valid_focus = session
            .focused
            .as_ref()
            .filter(|id| session.tabs.iter().any(|t| &t.id == *id));
        self.focused = valid_focus
            .cloned()
            .or_else(|| session.tabs.first().map(|t| t.id.clone()));
        self.view_mode = if session.view_mode_tab { ViewMode::Item } else { ViewMode::Graph };
        // Images were inserted above, so a still-valid id is already a node.
        self.viewed_image = session
            .viewed_image
            .clone()
            .filter(|_| self.view_mode == ViewMode::Item)
            .filter(|id| self.nodes.get(id).and_then(|n| n.as_image()).is_some_and(|i| i.error.is_none()));
        // Reset zoom/pan rather than carrying over whatever the previous
        // image (or this same session's own stale on-disk value) left
        // behind -- a loaded session's `viewed_image` always opens at Fit.
        self.image_view = ImageViewState::default();
        if self.showing_tab_view() {
            // The focused tab hasn't actually spawned yet — this is the
            // exact same "still starting up" gap crash recovery has, so
            // reuse its own spinner-instead-of-snapping-to-Graph path
            // (see `build_tab_view_ui`) rather than inventing a second one.
            self.pending_respawn_focus = self.focused.clone();
        }
        for tab in session.tabs {
            let pos = egui::vec2(tab.pos_x, tab.pos_y);
            self.spawn_tab_async(
                tab.id,
                tab.parent,
                None,
                false,
                None,
                self.settings.zoom_factor,
                tab.url,
                pos,
                egui::Vec2::ZERO,
                SpawnPurpose::Restore,
                tab.engine,
            );
        }
    }

    /// "Open Session" (settings popup): loads a named save and replaces the
    /// running session with it. A no-op if the name doesn't exist or its
    /// file is empty/corrupt, so a bad load can't wipe a perfectly good
    /// running session.
    fn load_named_session(&mut self, name: &str) {
        match session::Session::load_named(name) {
            Ok(session) => self.replace_running_session(session),
            Err(err) => self.queue_toast(err),
        }
    }

    /// Tears down every currently-open tab/download/group -- each tab's own
    /// `EngineTab::Drop` handles process shutdown+kill, the same
    /// mechanism `close_tab` already relies on, so clearing the map is
    /// enough, no per-tab teardown call needed -- then replays `session`
    /// via `apply_session`, same as startup replays `session.json`. Shared
    /// by `load_named_session` and the "Import Session..." confirm popup's
    /// own Import button (see `pending_import`); a no-op for an empty
    /// session, so a bad/empty load can't wipe a perfectly good running one.
    fn replace_running_session(&mut self, session: session::Session) {
        if session.tabs.is_empty() {
            return;
        }
        self.nodes.clear();
        self.node_order.clear();
        self.focused = None;
        self.current_group = None;
        self.dragging = None;
        self.hovered = None;
        self.selected_tabs.clear();
        self.rect_select_start = None;
        self.graph_settled_key = None;
        self.session_generation += 1;
        self.apply_session(session);
        self.save_session();
    }

    /// Spawns the very first tab at startup: no parent, becomes focused,
    /// starts in Tab view (preserves "opens straight into a usable
    /// browser" from before M5 — Graph view is reached explicitly).
    fn spawn_root_tab(&mut self) {
        let id = self.alloc_tab_id();
        let home_page = self.settings.home_page.clone();
        self.tab_spawn_error = None;
        let kind = self.settings.default_starting_engine;
        self.spawn_tab_async(
            id,
            None,
            None,
            false,
            None,
            self.settings.zoom_factor,
            home_page,
            egui::Vec2::ZERO,
            egui::Vec2::ZERO,
            SpawnPurpose::Root,
            kind,
        );
    }

    /// The one-time first-launch experience for a brand-new install -- see
    /// `Settings::has_shown_first_launch_demo`'s own doc comment for
    /// exactly when this fires instead of `spawn_root_tab`. A small,
    /// hand-arranged graph -- fixed positions, not `graph_layout::
    /// spawn_jitter`, unlike every other multi-node spawn in this file --
    /// since this is the one place a deliberately curated first impression
    /// matters more than avoiding a few lines of manual layout. Fully
    /// network-independent: every tab points at a `manifold://` page (see
    /// that scheme's own design doc), so it renders identically offline.
    /// `SpawnPurpose::Restore` for every tab, with `self.focused` set
    /// before any of them are issued, mirrors exactly how `apply_session`
    /// starts a real saved session -- landing in Graph view with nothing
    /// stealing focus from the intended main tab, not `Root`'s own "jump
    /// straight to Tab view" behavior, which would hide the very layout
    /// this exists to show off.
    fn spawn_first_launch_demo(&mut self) {
        let kind = self.settings.default_starting_engine;
        let zoom = self.settings.zoom_factor;

        let welcome_id = self.alloc_tab_id();
        self.focused = Some(welcome_id.clone());
        self.view_mode = ViewMode::Graph;
        self.spawn_tab_async(
            welcome_id,
            None,
            None,
            false,
            None,
            zoom,
            "manifold://welcome".to_string(),
            egui::vec2(0.0, 0.0),
            egui::Vec2::ZERO,
            SpawnPurpose::Restore,
            kind,
        );

        let note_id = self.alloc_note_id();
        self.nodes.insert(
            note_id.clone(),
            GraphNode {
                parent: None,
                group: None,
                pos: egui::vec2(-300.0, 0.0),
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Note(NotePayload {
                    text: "This is a Note -- drag it anywhere, or right-click to delete it \
                           (undo via \"Recently Closed\" if you change your mind)."
                        .to_string(),
                    editing: false,
                }),
            },
        );
        self.node_order.push(note_id);

        let group_id = self.alloc_group_id();
        let group_pos = egui::vec2(300.0, 0.0);
        self.nodes.insert(
            group_id.clone(),
            GraphNode {
                parent: None,
                group: None,
                pos: group_pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Group(GroupPayload { name: "Try grouping".to_string() }),
            },
        );
        self.node_order.push(group_id.clone());
        // A small fixed offset, not `graph_layout::spawn_jitter` -- its
        // radius is calibrated to spread a new tab away from a busy live
        // layout (see its own doc comment), which is far too large for
        // "nestle just barely off-center inside a two-member group" and
        // would occasionally land a member tab embarrassingly far from the
        // group it belongs to. These positions are only ever visible if
        // this group is later entered/ungrouped, but they should still
        // look deliberate then, not scattered.
        for (url, offset) in
            [("manifold://welcome", egui::vec2(-40.0, 40.0)), ("manifold://tips", egui::vec2(40.0, 40.0))]
        {
            let id = self.alloc_tab_id();
            let pos = group_pos + offset;
            self.spawn_tab_async(
                id,
                None,
                Some(group_id.clone()),
                false,
                None,
                zoom,
                url.to_string(),
                pos,
                egui::Vec2::ZERO,
                SpawnPurpose::Restore,
                kind,
            );
        }

        // Deliberately left un-expanded -- the welcome page's own "Try it"
        // section already tells the user to right-click this exact kind of
        // node and pick "Expand as Graph" themselves, so pre-expanding it
        // here would rob them of the discovery the copy is pointing at.
        // Synthetic, not file-backed (`path` is a display-only label, not a
        // real path -- see `JsonPayload`'s own doc comment on `key` for the
        // same "inherited path, no real ownership" reasoning `key: Some`
        // leaves use, though this node uses `key: None` since it's a
        // genuine standalone JSON node, not an exploded leaf).
        let json_id = self.alloc_json_id();
        self.nodes.insert(
            json_id.clone(),
            GraphNode {
                parent: None,
                group: None,
                pos: egui::vec2(0.0, -260.0),
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Json(JsonPayload {
                    path: "(demo).json".to_string(),
                    value: serde_json::json!({
                        "project": "Manifold",
                        "tags": ["browser", "graph-ui"],
                        "version": {"major": 0, "minor": 1}
                    }),
                    key: None,
                    error: None,
                }),
            },
        );
        self.node_order.push(json_id);
    }

    /// Runs `process_manager::spawn_tab` on a background thread and posts
    /// its result back via `event_proxy` as `UserEvent::TabSpawned` — see
    /// `TabSpawnOutcome`'s doc comment for why this can't run inline on the
    /// main/event-loop thread. `handle_tab_spawn_result` does what the old
    /// synchronous `Ok`/`Err` branches used to do, once the result actually
    /// arrives.
    fn spawn_tab_async(
        &mut self,
        id: String,
        parent: Option<String>,
        group: Option<String>,
        pinned: bool,
        color: Option<egui::Color32>,
        zoom_factor: f64,
        url: String,
        pos: egui::Vec2,
        vel: egui::Vec2,
        purpose: SpawnPurpose,
        kind: EngineKind,
    ) {
        let proxy = self
            .event_proxy
            .clone()
            .expect("event_proxy set in main before any tab spawn");
        self.tab_spawns_in_flight += 1;
        let generation = self.session_generation;
        std::thread::spawn(move || {
            let result = process_manager::spawn_tab(kind, &id, &url);
            let _ = proxy.send_event(UserEvent::TabSpawned(TabSpawnOutcome {
                id,
                parent,
                group,
                pinned,
                color,
                zoom_factor,
                url,
                pos,
                vel,
                purpose,
                generation,
                result,
            }));
        });
    }

    /// Applies a finished `spawn_tab_async` call — inserts the new tab node
    /// (or records `tab_spawn_error`), with per-`SpawnPurpose` follow-up
    /// matching what `spawn_root_tab`/`spawn_new_tab`/`respawn_tab` used to
    /// do synchronously right after `process_manager::spawn_tab` returned.
    /// Focus/visibility for `Respawn` is decided here, at completion time —
    /// same moment the old inline code decided it, just later in wall time.
    fn handle_tab_spawn_result(&mut self, outcome: TabSpawnOutcome) {
        let TabSpawnOutcome {
            id,
            parent,
            group,
            pinned,
            color,
            zoom_factor,
            url,
            pos,
            vel,
            purpose,
            generation,
            result,
        } = outcome;
        self.tab_spawns_in_flight = self.tab_spawns_in_flight.saturating_sub(1);
        if self.tab_spawns_in_flight == 0 {
            for waiter in self.tabs_spawned_waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
        // Spawned for a session that's since been replaced: dropping the
        // `EngineTab` shuts its engine host down. See `session_generation`.
        if generation != self.session_generation {
            return;
        }
        // A group dissolved while this spawn was still in flight (spawning
        // is async, see this fn's own doc comment) would otherwise leave
        // the new tab pointing at a group id nothing can navigate into —
        // same guard `reopen_closed_tab_shortcut` applies for the same
        // reason, centralized here since every spawn path funnels through
        // this one function.
        let group = group.filter(|g| self.nodes.contains_key(g));
        if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Restore)
            && self.pending_respawn_focus.as_deref() == Some(id.as_str())
        {
            self.pending_respawn_focus = None;
        }
        match result {
            Ok(mut tab) => {
                let is_focused = match purpose {
                    SpawnPurpose::Root => true,
                    SpawnPurpose::New | SpawnPurpose::Split => false,
                    SpawnPurpose::Respawn | SpawnPurpose::Restore => {
                        self.focused.as_deref() == Some(id.as_str())
                    }
                };
                // A split's secondary pane is always live even though it's
                // never `focused` -- true for a fresh `Split` spawn, and
                // also (the `self.split_tab` check) for a crash-respawn of
                // whatever tab currently happens to be playing that role,
                // which arrives as an ordinary `Respawn`, not `Split` --
                // see `SpawnPurpose::Split`'s own doc comment.
                let is_split = matches!(purpose, SpawnPurpose::Split) || self.split_tab.as_deref() == Some(id.as_str());
                // The focused tab stays hidden while Image view is showing
                // in front of it -- e.g. relaunching straight into Image
                // view, or a crash respawn of the tab Image view is
                // currently covering (see `spawned_tab_visible`'s doc).
                let visible = spawned_tab_visible(is_focused, is_split, self.showing_image_view());
                tab.set_visible(visible);
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        parent,
                        group,
                        pos,
                        vel,
                        color,
                        payload: NodePayload::Tab(TabPayload {
                            tab,
                            url: url.clone(),
                            title: String::new(),
                            can_go_back: false,
                            can_go_forward: false,
                            loading: false,
                            audio_playing: false,
                            muted: false,
                            pinned,
                            zoom_factor,
                            swap_target: None,
                            json_body: None,
                        }),
                    },
                );
                if !self.node_order.iter().any(|t| t == &id) {
                    self.node_order.push(id.clone());
                }
                match purpose {
                    SpawnPurpose::Root => {
                        self.focused = Some(id.clone());
                        self.view_mode = ViewMode::Graph;
                        self.address_bar_text = url;
                        self.send_resize_to(&id);
                        self.send_zoom_to(&id);
                    }
                    SpawnPurpose::New => {
                        self.send_zoom_to(&id);
                    }
                    SpawnPurpose::Split => {
                        self.split_tab = Some(id.clone());
                        self.send_zoom_to(&id);
                        self.send_resize_to(&id);
                        // The primary pane was rendering at full width until
                        // now -- it needs to shrink to half now that
                        // `content_size_px` sees `split_tab` as set.
                        if let Some(focused_id) = self.focused.clone() {
                            self.send_resize_to(&focused_id);
                        }
                    }
                    SpawnPurpose::Respawn => {
                        // A respawned tab is a fresh engine-host process
                        // that starts at that engine's own default zoom --
                        // unlike Root/New/Split/Restore above, this arm
                        // never told it about `zoom_factor` at all, so a
                        // crash-recovery or engine-swap respawn silently
                        // reset the page's zoom while Settings kept
                        // showing the configured value.
                        self.send_zoom_to(&id);
                        if is_focused {
                            if !self.address_bar_focused {
                                self.address_bar_text = url;
                            }
                            // Resizing only matters for what's actually
                            // shown -- skip it while Image view hides this
                            // tab, same as `visible` above.
                            if visible {
                                self.send_resize_to(&id);
                            }
                        } else if is_split {
                            self.send_resize_to(&id);
                        }
                    }
                    SpawnPurpose::Restore => {
                        self.send_zoom_to(&id);
                        if is_focused {
                            self.address_bar_text = url;
                            if visible {
                                self.send_resize_to(&id);
                            }
                        }
                    }
                }
                self.save_session();
            }
            Err(err) => {
                eprintln!("spawn tab failed for {id}: {err}");
                self.tab_spawn_error = Some(match purpose {
                    SpawnPurpose::Root => format!("Couldn't start a browser tab: {err}"),
                    SpawnPurpose::New => format!("Couldn't start a new tab: {err}"),
                    SpawnPurpose::Split => format!("Couldn't start split view: {err}"),
                    SpawnPurpose::Respawn => format!("Couldn't restart tab: {err}"),
                    SpawnPurpose::Restore => format!("Couldn't restore tab: {err}"),
                });
            }
        }
    }

    fn alloc_tab_id(&mut self) -> String {
        next_id(&mut self.next_tab_id, "tab")
    }

    /// Removes a download or note node from the graph -- a download's file
    /// on disk is left untouched (this only forgets the node), and neither
    /// kind has anything else to persist/tear down on removal the way a
    /// tab's engine process or a group's membership does, so one plain
    /// `self.nodes`/`node_order` removal (plus the same stale-`dragging`/
    /// `hovered` guard `close_tab`/`finish_swap` also apply) covers both.
    fn remove_node(&mut self, id: &str) {
        self.nodes.remove(id);
        self.node_order.retain(|d| d != id);
        if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
        }
        if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
    }

    /// Applies a Graph-view card's own drag delta, and on drag-release,
    /// drops it onto whichever group card (if any) it's now sitting on top
    /// of -- plus the `self.hovered`/`self.dragging` bookkeeping every card
    /// needs regardless. Shared by the five card kinds (Download, Note,
    /// Image, Pdf, Json) through `draw_graph_card` -- previously hand-
    /// copied once per kind (bar Download), byte-identical apart from which
    /// field's `id`/`.pos` it read. Tab/Group each have their own reasons to
    /// stay separate (a tab can also drop onto another *tab* to form a new
    /// group, and a group itself can't be dropped onto another group), so
    /// this only covers the five kinds whose drop behavior is genuinely
    /// identical.
    fn finish_draggable_node(&mut self, id: &str, response: &egui::Response, is_hovered: bool) {
        if response.dragged() {
            self.dragging = Some(id.to_string());
            if let Some(node) = self.nodes.get_mut(id) {
                node.pos += response.drag_delta();
            }
        } else if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
            // Unlike a tab (see the tab loop's own drop-onto-group logic),
            // a card kind has no competing "drop onto a sibling" gesture of
            // its own to conflict with, so this works at any level, not
            // just the top -- letting an existing item join a nested
            // subgroup by dragging it onto that subgroup's own card while
            // both are visible in the same subgraph.
            if let Some(pos) = self.nodes.get(id).map(|n| n.pos) {
                let dropped_on_group = self.nodes_at_current_level(ItemNodeKind::Group).into_iter().find(|gid| {
                    self.nodes.get(gid).is_some_and(|g| {
                        egui::Rect::from_center_size(egui::Pos2::new(g.pos.x, g.pos.y), GROUP_NODE_SIZE)
                            .contains(egui::Pos2::new(pos.x, pos.y))
                    })
                });
                if let Some(group_id) = dropped_on_group {
                    if let Some(node) = self.nodes.get_mut(id) {
                        node.group = Some(group_id);
                    }
                }
            }
            self.save_session();
        }
        if is_hovered {
            self.hovered = Some(id.to_string());
        } else if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
    }

    /// One Download/Note/Image/Pdf/Json card on the graph: everything the
    /// five kinds share (cull, click/drag and hover interacts, border,
    /// hover ✕, drag and drop-onto-group, filter dim), with the per-kind
    /// content painted by `paint_card_body`. `None` when the node is gone
    /// or culled. The caller applies the kind's own click action from the
    /// returned response. Tab and Group cards keep their own loops.
    fn draw_graph_card(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        kind: ItemNodeKind,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> Option<CardOutput> {
        let rect = node_card_rect(self.nodes.get(id)?);
        // Viewport culling -- see the tab loop's own comment. Never culls
        // the node being dragged, so a drag off-screen keeps tracking.
        if self.dragging.as_deref() != Some(id) && !rect.intersects(self.graph_scene_rect) {
            return None;
        }
        let body = self.paint_card_body(ui, deferred_overlays, id, kind, rect, inv_zoom, zoom_scale);

        // Both interacts are registered *after* the body, never before:
        // egui (0.36, `interaction.rs`) gives the pointer to the
        // last-registered interactive widget under it, and counts a
        // hover-only widget as hovered only when it sits on top of that
        // widget. Registered first, a body widget (a Note's markdown
        // scope or `TextEdit`) outranked them: a Note card was never
        // hovered at all, and a click on rendered markdown never reached
        // the card. The hover-only one covers the whole card, so hovering
        // a Note's text while editing (where `response` is only the
        // header) still counts; being hover-only it takes no clicks.
        let response = ui.interact(body.interact_rect, egui::Id::new(("graph_card", id)), egui::Sense::click_and_drag());
        let response = match body.tooltip {
            Some(tooltip) => response.on_hover_text(tooltip),
            None => response,
        };
        let hover = ui.interact(rect, egui::Id::new(("graph_card_hover", id)), egui::Sense::hover());
        let pointer_on_card = hover.hovered() || response.hovered();
        // Last frame's hover too (as the tab and group loops do), which
        // keeps the ✕ drawn while the pointer is on it: registered last,
        // it takes the hover from both interacts above.
        let is_hovered = self.hovered.as_deref() == Some(id) || pointer_on_card;

        // An inside stroke painted over the body; every body keeps its
        // content inset from the edge.
        let style = card_style(kind, ui.visuals());
        let (border_width, border_color) =
            if is_hovered { (1.5, style.border_hover) } else { (1.0, style.border_idle) };
        ui.painter().rect_stroke(
            rect,
            2.0 * inv_zoom,
            egui::Stroke::new(border_width * inv_zoom, border_color),
            egui::StrokeKind::Inside,
        );

        let close = is_hovered.then(|| {
            draw_node_close_button(
                ui,
                deferred_overlays,
                rect,
                ("graph_card_close", id),
                inv_zoom,
                style.close_fill,
                style.close_border,
                style.close_icon,
            )
        });
        let close_clicked = close.as_ref().is_some_and(|c| c.clicked());

        // This frame's hover only: passing `is_hovered` (which includes
        // `self.hovered`) made a card stay hovered, ✕ and all, after the
        // pointer left it, until another card was hovered.
        let still_hovered = pointer_on_card || close.as_ref().is_some_and(|c| c.hovered());
        self.finish_draggable_node(id, &response, still_hovered);

        if let Some(node) = self.nodes.get(id) {
            dim_if_filtered_out(deferred_overlays, &self.graph_filter, &[card_filter_text(&node.payload)], rect, style.dim_rounding);
        }
        Some(CardOutput { response, close_clicked })
    }

    /// A graph card's own content, per kind -- everything under the
    /// shared frame's border (see `draw_graph_card`). Returns which part
    /// of the card is the click/drag handle and an optional tooltip.
    #[allow(clippy::too_many_arguments)]
    fn paint_card_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        kind: ItemNodeKind,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        match kind {
            // Download: no thumbnail/title-strip/audio-dot machinery (no
            // live content behind it), just the filename, an icon, and a
            // live progress bar while it's still downloading.
            ItemNodeKind::Download => {
                ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                let Some((download_path, completed, total_bytes, bytes_received)) = self
                    .nodes
                    .get(id)
                    .and_then(|n| n.as_download())
                    .map(|d| (d.path.clone(), d.completed, d.total_bytes, d.bytes_received))
                else {
                    return body;
                };
                let filename = download_filename(&download_path);
                // `egui_phosphor::DOWNLOAD_SIMPLE` -- was a raw
                // Unicode arrow-to-bar glyph (`\u{2913}`) that
                // happened to render, despite this comment's own
                // claim of avoiding exactly that gamble; migrated
                // to a real icon for consistency once the icon
                // font was already in place for everything else.
                // LOD: icon/filename text skipped once simplified
                // -- see the tab loop's own doc comment.
                if !self.graph_lod_simplified {
                    let download_icon_pos = rect.center_top();
                    let download_icon_color = ui.visuals().text_color();
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        overlay_painter.text(
                            project(download_icon_pos) + egui::vec2(0.0, 10.0),
                            egui::Align2::CENTER_TOP,
                            egui_phosphor::light::DOWNLOAD_SIMPLE,
                            egui::FontId::proportional(18.0),
                            download_icon_color,
                        );
                    }));
                    // Below `MIN_LABEL_CARD_SCREEN_WIDTH`, skip the
                    // filename entirely rather than truncating to a
                    // near-illegible sliver -- the download-icon glyph
                    // above still marks the node as a download on its
                    // own.
                    if rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                        let filename_font = egui::FontId::proportional(14.0);
                        let filename_max_width = (rect.width() * zoom_scale - 12.0).max(0.0);
                        let filename =
                            truncate_to_width(&mut self.truncate_cache, ui.ctx(), filename, &filename_font, filename_max_width);
                        let filename_pos = rect.center_bottom();
                        let filename_color = ui.visuals().text_color();
                        deferred_overlays.push(Box::new(move |overlay_painter, project| {
                            overlay_painter.text(
                                project(filename_pos) + egui::vec2(0.0, -8.0),
                                egui::Align2::CENTER_BOTTOM,
                                filename,
                                filename_font,
                                filename_color,
                            );
                        }));
                    }
                }
                // Live progress bar, only while not yet complete
                // and only when the server reported a real total
                // (an unknown-total download still shows as a
                // download via the icon/filename above; the
                // Downloads popup's own byte-count-with-no-bar
                // fallback is where that case gets a number). A
                // plain `ui.painter()` world-space rect, same
                // coordinate space `rect`/the card's own fill and
                // border already draw in -- not `overlay_painter`
                // (screen-space, used for text specifically to
                // avoid blurring at non-100% zoom), and not scaled
                // by `inv_zoom` for the same reason `GRAPH_THUMB_
                // MARGIN` isn't: this is card-layout geometry
                // already inside the zoom-scaled Scene transform,
                // not chrome that needs a constant on-screen size.
                if !completed {
                    if let Some(total) = total_bytes.filter(|t| *t > 0) {
                        let fraction = (bytes_received as f32 / total as f32).clamp(0.0, 1.0);
                        let track_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 8.0, rect.bottom() - 26.0),
                            egui::pos2(rect.right() - 8.0, rect.bottom() - 22.0),
                        );
                        ui.painter().rect_filled(track_rect, 2.0, ui.visuals().weak_text_color().gamma_multiply(0.4));
                        let fill_rect = egui::Rect::from_min_max(
                            track_rect.min,
                            egui::pos2(track_rect.min.x + track_rect.width() * fraction, track_rect.max.y),
                        );
                        ui.painter().rect_filled(fill_rect, 2.0, ACCENT);
                    }
                }
            }
            // Note: a plain user-authored text card, no engine process
            // behind it -- see `NotePayload`'s own doc comment. Unlike
            // every other kind, most of the card is a real editable
            // `TextEdit`, not overlay-painted static text -- editing
            // happens directly in place, on the graph. The header strip
            // (not the text body) is the drag handle only while editing,
            // via `CardBody::interact_rect`, so a click inside the text
            // placing a cursor/selecting text is never mistaken for the
            // start of a card drag; the frame's own hover-only interact
            // covers the whole card regardless, so hovering the text body
            // still counts even though it isn't the drag handle.
            ItemNodeKind::Note => {
                let header_h = 20.0 * inv_zoom;
                let header_rect =
                    egui::Rect::from_min_max(rect.min, egui::Pos2::new(rect.max.x, rect.min.y + header_h));
                let text_rect = egui::Rect::from_min_max(egui::Pos2::new(rect.min.x, rect.min.y + header_h), rect.max)
                    .shrink(6.0 * inv_zoom);
                let is_editing = self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);

                ui.painter().rect_filled(rect, 2.0 * inv_zoom, NOTE_CARD_COLOR);
                if !self.graph_lod_simplified {
                    let note_icon_pos = header_rect.left_center();
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        overlay_painter.text(
                            project(note_icon_pos) + egui::vec2(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            egui_phosphor::light::NOTE_PENCIL,
                            egui::FontId::proportional(12.0),
                            egui::Color32::from_black_alpha(160),
                        );
                    }));
                }

                if is_editing {
                    body.interact_rect = header_rect;
                    let mut lost_focus = false;
                    if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                        lost_focus = ui
                            .put(
                                text_rect,
                                egui::TextEdit::multiline(&mut note.text)
                                    .frame(egui::Frame::NONE)
                                    .font(egui::FontId::proportional(13.0))
                                    .text_color(egui::Color32::from_black_alpha(220))
                                    .hint_text("Type a note..."),
                            )
                            .lost_focus();
                    }
                    // Committed on blur, not on every keystroke -- a note is
                    // persisted (unlike every other per-node display state
                    // in `session.json`, see `NotePayload`'s own doc
                    // comment), and `save_session` is real file I/O, so
                    // writing it once per edit *session* rather than once
                    // per character avoids hammering disk while typing.
                    if lost_focus {
                        if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                            note.editing = false;
                        }
                        self.save_session();
                    }
                } else if !self.graph_lod_simplified {
                    // `text` is cloned out before the closure below --
                    // `ui.scope_builder`'s closure borrows `self.
                    // commonmark_cache` mutably, which the borrow
                    // checker only allows alongside a *finished*
                    // borrow of `self.nodes`, not one still live inside
                    // the same closure.
                    // LOD: skip the markdown render (real parsing +
                    // rich text layout, one of the pricier per-node
                    // widgets in this whole view) once simplified --
                    // see the tab loop's own doc comment. Never
                    // applies to the `is_editing` branch above: a
                    // note actively being typed into must never have
                    // its live `TextEdit` yanked out from under the
                    // user just because the rest of the graph got
                    // busy.
                    let text = self.nodes.get(id).and_then(|n| n.as_note()).map(|n| n.text.clone()).unwrap_or_default();
                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::LEFT)),
                        |ui| {
                            ui.style_mut().visuals.override_text_color = Some(egui::Color32::from_black_alpha(220));
                            egui_commonmark::CommonMarkViewer::new().show(ui, &mut self.commonmark_cache, &text);
                        },
                    );
                }
            }
            // Image: a locally-added picture, no engine/IPC behind it --
            // see `ImagePayload`'s own doc comment. Unlike a note, nothing
            // inside the card needs its own clicks (no embedded text
            // editor), so the whole card is drag-sensitive via one plain
            // `Sense::click_and_drag()`, same as a tab or download card.
            // The card's own footprint comes from `image_node_card_size`
            // (the decoded picture's real aspect ratio, capped at
            // `IMAGE_NODE_MAX_SIZE`), not a fixed size the way every other
            // kind's card is.
            ItemNodeKind::Image => {
                let error = self.nodes.get(id).and_then(|n| n.as_image()).and_then(|i| i.error.clone());
                if let Some(error) = error {
                    paint_missing_file_card(ui, deferred_overlays, rect, inv_zoom, self.graph_lod_simplified);
                    body.tooltip = Some(error);
                    return body;
                }
                let tex_id = self.gpu.as_ref().and_then(|g| g.image_id(id));
                // LOD: the real texture blit (and the broken-image
                // glyph text) skipped once simplified -- see the tab
                // loop's own doc comment. Unlike a tab card, an image
                // node has no separate background fill of its own (the
                // picture *is* the fill), so the simplified state gets
                // a plain solid box instead of nothing, matching every
                // other kind's "background + border" simplified look.
                if !self.graph_lod_simplified {
                    match tex_id {
                        Some(tex_id) => {
                            ui.painter().image(
                                tex_id,
                                rect,
                                egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                        // Only reachable if the node somehow outlived its
                        // own texture (freed but not removed) -- a broken-
                        // image glyph is a more honest placeholder here
                        // than the loading `Spinner` a tab/download
                        // placeholder uses, since this picture isn't *about*
                        // to arrive the way a first frame or fetch is.
                        None => {
                            ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                            let broken_icon_pos = rect.center();
                            let broken_icon_color = ui.visuals().weak_text_color();
                            deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                overlay_painter.text(
                                    project(broken_icon_pos),
                                    egui::Align2::CENTER_CENTER,
                                    egui_phosphor::light::IMAGE_BROKEN,
                                    egui::FontId::proportional(24.0),
                                    broken_icon_color,
                                );
                            }));
                        }
                    }
                } else {
                    ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                }
            }
            // Pdf: same card shape as an image node (drag, hover-revealed
            // close button, drop-onto-group), but with three visual states
            // instead of one -- still rendering in the background
            // (`outcome: None`, a plain spinner, same fixed
            // `PDF_PENDING_NODE_SIZE` box a tab respawn placeholder uses
            // `Spinner` for too), failed (`Some(Err(_))`, a real error
            // glyph plus the message as a hover tooltip -- unlike an
            // image's "somehow outlived its own texture" broken-image
            // case, this is an *expected*, real outcome worth explaining,
            // not a rare bug), or rendered (`Some(Ok(_))`, the page's own
            // texture in `GpuState::images` -- the exact same map an
            // `Image` node's texture lives in, since a rendered PDF page
            // is just another RGBA bitmap by the time it's on the GPU).
            ItemNodeKind::Pdf => {
                let Some(state) = self.nodes.get(id).and_then(|n| n.as_pdf()).map(|pdf| match &pdf.outcome {
                    None => None,
                    Some(Err(err)) => Some(Err(err.clone())),
                    Some(Ok(outcome)) => {
                        Some(Ok((self.gpu.as_ref().and_then(|g| g.image_id(id)), outcome.page_count)))
                    }
                }) else {
                    return body;
                };
                // A failed render's error message shown as a hover
                // tooltip on the card itself -- the only place there's
                // room for real text without permanently occupying part
                // of the card the way a label would.
                if let Some(Err(err)) = &state {
                    body.tooltip = Some(err.clone());
                }
                // LOD: same "plain solid box instead of the real
                // content" simplification as the Image arm -- see the tab
                // loop's own doc comment.
                if !self.graph_lod_simplified {
                    match state {
                        Some(Ok((Some(tex_id), page_count))) => {
                            ui.painter().image(
                                tex_id,
                                rect,
                                egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                            // Shown even for a one-page PDF -- a glanceable
                            // "this is a PDF, not a plain image" cue on top
                            // of the file-type distinction the icon in the
                            // context menu/toolbar already gives.
                            if page_count > 0 && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                                let page_count_pos = rect.right_bottom();
                                let page_count_text =
                                    format!("{page_count} page{}", if page_count == 1 { "" } else { "s" });
                                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                    overlay_painter.text(
                                        project(page_count_pos) + egui::vec2(-4.0, -4.0),
                                        egui::Align2::RIGHT_BOTTOM,
                                        page_count_text,
                                        egui::FontId::proportional(11.0),
                                        egui::Color32::WHITE,
                                    );
                                }));
                            }
                        }
                        // Reachable the same rare way an `Image` node's own
                        // `None` case is -- the node outlived its texture.
                        Some(Ok((None, _))) => {
                            ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                            let broken_icon_pos = rect.center();
                            let broken_icon_color = ui.visuals().weak_text_color();
                            deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                overlay_painter.text(
                                    project(broken_icon_pos),
                                    egui::Align2::CENTER_CENTER,
                                    egui_phosphor::light::IMAGE_BROKEN,
                                    egui::FontId::proportional(24.0),
                                    broken_icon_color,
                                );
                            }));
                        }
                        Some(Err(_)) => {
                            ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                            let error_icon_pos = rect.center();
                            deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                overlay_painter.text(
                                    project(error_icon_pos),
                                    egui::Align2::CENTER_CENTER,
                                    egui_phosphor::light::FILE_X,
                                    egui::FontId::proportional(24.0),
                                    DANGER_COLOR,
                                );
                            }));
                        }
                        None => {
                            ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                            ui.put(rect, egui::Spinner::new().size(24.0 * inv_zoom));
                        }
                    }
                } else {
                    ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                }
            }
            // Json: always the same fixed `JSON_NODE_SIZE` box (see its
            // own doc comment for why) -- no GPU texture, just an icon
            // plus `json_summary`'s own compact shape label. Double-click
            // opens the full tree in `build_json_viewer`, the same popup a
            // tab's own "View as JSON" already uses.
            ItemNodeKind::Json => {
                let error = self.nodes.get(id).and_then(|n| n.as_json()).and_then(|j| j.error.clone());
                if let Some(error) = error {
                    paint_missing_file_card(ui, deferred_overlays, rect, inv_zoom, self.graph_lod_simplified);
                    body.tooltip = Some(error);
                    return body;
                }
                let Some(summary) = self.nodes.get(id).and_then(|n| n.as_json()).map(|json| match &json.key {
                    Some(key) => {
                        let key_display = if key.chars().count() > 16 {
                            format!("{}…", key.chars().take(15).collect::<String>())
                        } else {
                            key.clone()
                        };
                        format!("{key_display}: {}", json_summary(&json.value))
                    }
                    None => json_summary(&json.value),
                }) else {
                    return body;
                };
                ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                if !self.graph_lod_simplified && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                    let json_center = rect.center();
                    let summary_color = ui.visuals().text_color();
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        overlay_painter.text(
                            project(json_center) + egui::vec2(0.0, -12.0),
                            egui::Align2::CENTER_CENTER,
                            egui_phosphor::light::BRACKETS_CURLY,
                            egui::FontId::proportional(22.0),
                            ACCENT,
                        );
                        overlay_painter.text(
                            project(json_center) + egui::vec2(0.0, 14.0),
                            egui::Align2::CENTER_CENTER,
                            summary,
                            egui::FontId::proportional(12.0),
                            summary_color,
                        );
                    }));
                }
            }
            ItemNodeKind::Tab | ItemNodeKind::Group => {
                unreachable!("Tab and Group cards are drawn by their own loops in build_graph_ui")
            }
        }
        body
    }

    /// A WebKit-sourced download path is a WSL2-side path (WebKit runs
    /// inside WSL2 — see project docs), not reachable from this Windows
    /// process as a plain path, so it's translated to the
    /// `\\wsl.localhost\<distro>\...` UNC form both Explorer and a pasted
    /// clipboard path understand, using the same "Ubuntu" distro name
    /// `process_manager` already hardcodes for spawning the WebKit
    /// engine-host. Used by `node_file_path`.
    fn windows_path_for_download(&self, id: &str) -> Option<String> {
        let download = self.nodes.get(id)?.as_download()?;
        Some(if let Some(posix) = download.path.strip_prefix('/') {
            format!("\\\\wsl.localhost\\Ubuntu\\{}", posix.replace('/', "\\"))
        } else {
            download.path.clone()
        })
    }

    /// Opens `path`'s containing folder with the file itself pre-selected --
    /// the mechanism behind `reveal_node`.
    fn reveal_in_explorer(path: &str) {
        let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
    }

    /// The real file behind a Download/Image/Pdf/Json node, in the form
    /// Explorer and a pasted clipboard path understand -- what both
    /// "Open Containing Folder" (`reveal_node`) and "Copy Path" use. A
    /// download's path may be a WSL-side POSIX path (WebKit downloads), so
    /// it goes through `windows_path_for_download`; an image/PDF/JSON path
    /// always came from a real Windows file picker, so it's used as-is.
    /// `None` for kinds with no backing file (Tab, Group, Note).
    fn node_file_path(&self, id: &str) -> Option<String> {
        match &self.nodes.get(id)?.payload {
            NodePayload::Download(_) => self.windows_path_for_download(id),
            NodePayload::Image(image) => Some(image.path.clone()),
            NodePayload::Pdf(pdf) => Some(pdf.path.clone()),
            NodePayload::Json(json) => Some(json.path.clone()),
            NodePayload::Tab(_) | NodePayload::Group(_) | NodePayload::Note(_) => None,
        }
    }

    /// Opens a node's file's containing folder with the file pre-selected
    /// (a plain click on a download card does this too -- same "just show
    /// the obvious thing, no custom UI" posture as the download itself, see
    /// `EngineToHost::DownloadCompleted`'s doc comment). No-op for kinds
    /// with no file.
    fn reveal_node(&self, id: &str) {
        if let Some(path) = self.node_file_path(id) {
            Self::reveal_in_explorer(&path);
        }
    }

    /// Distinct prefix from `alloc_tab_id`/the `"dl-"` download-id prefix —
    /// group ids share the same flat id space at the top level
    /// (`graph_layout::step` again sees ids only, not node kinds).
    fn alloc_group_id(&mut self) -> String {
        next_id(&mut self.next_group_id, "grp")
    }

    /// Memoized `favicon_fetch::url_origin(url)` -- that function is a
    /// couple of string scans plus a `format!` allocation, and both of
    /// this app's per-frame callers (the Graph-view tab card, the Tab-
    /// view toolbar's own identity chip) called it fresh every single
    /// frame for a URL that's stable between navigations. **Found on a
    /// general (not just large-graph) performance audit**, the same
    /// category as `truncate_cache`: capped and cleared outright past
    /// `TRUNCATE_CACHE_CAP` rather than evicted incrementally, for the
    /// same "should stay small in steady state, insurance against
    /// unbounded growth" reasoning.
    fn cached_favicon_origin(&mut self, url: &str) -> Option<String> {
        if let Some(cached) = self.favicon_origin_cache.get(url) {
            return cached.clone();
        }
        let origin = favicon_fetch::url_origin(url);
        if self.favicon_origin_cache.len() >= TRUNCATE_CACHE_CAP {
            self.favicon_origin_cache.clear();
        }
        self.favicon_origin_cache.insert(url.to_owned(), origin.clone());
        origin
    }

    /// Same reasoning as `alloc_group_id` -- a distinct `"note-"` prefix in
    /// the same flat id space.
    fn alloc_note_id(&mut self) -> String {
        next_id(&mut self.next_note_id, "note")
    }

    /// Same reasoning as `alloc_note_id` -- a distinct `"img-"` prefix in
    /// the same flat id space.
    fn alloc_image_id(&mut self) -> String {
        next_id(&mut self.next_image_id, "img")
    }

    /// Same reasoning as `alloc_image_id` -- a distinct `"pdf-"` prefix in
    /// the same flat id space.
    fn alloc_pdf_id(&mut self) -> String {
        next_id(&mut self.next_pdf_id, "pdf")
    }

    /// Same reasoning as `alloc_pdf_id` -- a distinct `"json-"` prefix in
    /// the same flat id space.
    fn alloc_json_id(&mut self) -> String {
        next_id(&mut self.next_json_id, "json")
    }

    /// A fresh group's default display name -- derived from its own id's
    /// numeric suffix (`alloc_group_id`'s own monotonic, never-reused
    /// counter) rather than `self.groups.len() + 1`, which the three call
    /// sites all used until this was found live: dissolving/closing a
    /// group shrinks `len()`, so a later new group's default name could
    /// collide with an existing one's (e.g. three groups "Group 1/2/3",
    /// dissolve "Group 2", create a new one -- `len()` is back to 2, so the
    /// new group's name is "Group 3", a duplicate). Manually *renaming* two
    /// groups to the identical string is left alone -- that's a deliberate
    /// user choice, not an accidental default-naming collision.
    fn default_group_name(id: &str) -> String {
        format!("Group {}", id.trim_start_matches("grp-"))
    }

    /// Dissolves a group: members rejoin *its own parent's* level again
    /// (never closed — grouping is purely a display grouping, not an
    /// ownership one), and the group card itself is removed. A top-level
    /// group's parent is `None`, so this still means "become top-level"
    /// exactly as before nesting existed -- reads the group's own parent
    /// before removing it, rather than assuming `None`. `current_group`
    /// moves to that same parent if it happened to be the group just
    /// dissolved (should only actually matter defensively — the close
    /// button that calls this only ever renders one level up from the
    /// group being closed, never on the level currently being viewed).
    fn dissolve_group(&mut self, id: &str) {
        let parent = self.nodes.get(id).and_then(|n| n.group.clone());
        for node in self.nodes.values_mut() {
            if node.group.as_deref() == Some(id) {
                node.group = parent.clone();
            }
        }
        self.nodes.remove(id);
        self.node_order.retain(|g| g != id);
        if self.current_group.as_deref() == Some(id) {
            self.current_group = parent;
        }
        if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
        }
        if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
        if self.renaming_group.as_ref().is_some_and(|(g, _)| g == id) {
            self.renaming_group = None;
        }
    }

    /// `parent`'s own group, if it has one and still exists — the natural
    /// "a tab opened from another tab lands in the same group" rule used
    /// by every `spawn_new_tab` call site that has a real opener (Ctrl+T,
    /// the context menu's "Open in new tab", a page-triggered popup). The
    /// toolbar's own "+ New Tab" button uses `self.current_group` directly
    /// instead, since `self.focused` (its `parent`) isn't necessarily a
    /// tab inside whichever group's subgraph is currently being viewed.
    fn group_of(&self, id: &Option<String>) -> Option<String> {
        id.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.group.clone())
    }

    /// `node_order`, filtered to nodes of `kind` at whichever level
    /// `self.current_group` is currently viewing -- the id list each of
    /// `build_graph_ui`'s per-kind card loops iterates, kept in stable
    /// insertion order (unlike filtering `self.nodes` directly, a
    /// `HashMap`'s iteration order isn't stable frame-to-frame). The kind
    /// filter matters even for one kind: a top-level group's own `group`
    /// is `None`, same as `current_group` at the top level, so a level
    /// check alone would leak group cards into every other kind's list.
    fn nodes_at_current_level(&self, kind: ItemNodeKind) -> Vec<String> {
        self.node_order
            .iter()
            .filter(|id| {
                self.nodes
                    .get(*id)
                    .is_some_and(|n| n.group == self.current_group && item_node_kind(&n.payload) == kind)
            })
            .cloned()
            .collect()
    }

    /// Would setting `node_id`'s parent to `new_parent_id` create a cycle?
    /// Walks `new_parent_id`'s own parent chain upward -- if it ever
    /// reaches `node_id`, `new_parent_id` is currently a descendant of
    /// `node_id`, so re-parenting `node_id` under it would loop. See the
    /// tab-onto-tab drag-to-reparent gesture (inside a group's own
    /// subgraph) this guards.
    fn would_create_cycle(&self, node_id: &str, new_parent_id: &str) -> bool {
        let mut current = Some(new_parent_id.to_string());
        while let Some(cur) = current {
            if cur == node_id {
                return true;
            }
            current = self.nodes.get(&cur).and_then(|n| n.parent.clone());
        }
        false
    }

    /// Every ancestor above `self.current_group`, root-most first, as
    /// `(id, name)` pairs -- the toolbar's own breadcrumb trail now that
    /// groups nest. Empty at the true top level. Walks `GraphNode::group`
    /// upward from the current group exactly the way `subtree_ids`/
    /// `dissolve_group` already do to find a single ancestor; this just
    /// collects the whole chain instead of stopping at the first one.
    fn group_breadcrumb(&self) -> Vec<(String, String)> {
        let mut chain = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut current = self.current_group.clone();
        // Guards against an infinite loop if a cycle ever existed in the
        // `.group` parent chain -- not reachable today (the only path that
        // reparents an *existing* group, the merge-drag gesture, already
        // refuses a cycle at the source -- see `group_merge_requested`'s own
        // `creates_cycle` check), but this would hang the whole app with no
        // error if it ever did happen, called every frame as it is, so the
        // guard is cheap insurance rather than something to skip.
        while let Some(id) = current {
            if !visited.insert(id.clone()) {
                break;
            }
            let Some(node) = self.nodes.get(&id) else { break };
            let name = node.as_group().map(|g| g.name.clone()).unwrap_or_default();
            current = node.group.clone();
            chain.push((id, name));
        }
        chain.reverse();
        chain
    }

    /// "Reset View"'s scene rect: bounds every node currently visible at
    /// this level (every kind -- same level rules `nodes_at_current_level`
    /// already uses) instead of always framing
    /// the same fixed world-space rect regardless of how many nodes exist
    /// or where they've drifted to. Each node contributes its own
    /// half-size (a
    /// node's *card*, not just its center point, must fit on screen) plus
    /// a flat margin so a node isn't left flush against the viewport edge.
    /// Falls back to `default_graph_scene_rect()` when there's nothing to
    /// fit (an empty group, or before any tab has spawned) — an empty
    /// bounding box has no sensible center/size to frame.
    fn fit_all_nodes_scene_rect(&self) -> egui::Rect {
        self.bounds_for_level(
            self.node_order
                .iter()
                .filter(|id| self.nodes.get(*id).is_some_and(|n| n.group == self.current_group)),
        )
    }

    /// Same computation `fit_all_nodes_scene_rect` needs, but taking
    /// already-known ids for a level instead of recomputing them -- lets
    /// the minimap (drawn every frame, well after `build_graph_ui`'s own
    /// per-frame `*_at_level` locals are already in hand) reuse this
    /// without redoing the level scans a second time per frame.
    fn bounds_for_level<'a>(&self, ids: impl IntoIterator<Item = &'a String>) -> egui::Rect {
        const MARGIN: f32 = 60.0;
        let bounds = ids
            .into_iter()
            .filter_map(|id| self.nodes.get(id))
            .fold(egui::Rect::NOTHING, |bounds, node| bounds.union(node_card_rect(node)));
        if !bounds.is_finite() {
            return default_graph_scene_rect();
        }
        bounds.expand(MARGIN)
    }

    /// `root_id` plus every descendant reachable through `parent` links,
    /// restricted to the same level `root_id` itself is on (`node.group`
    /// must match) -- a descendant that ended up in a different group is
    /// invisible at this level, and would otherwise pull the fit rect
    /// toward an off-screen position. Not the most efficient traversal
    /// (checks every tab per frontier node), but this graph's tab counts
    /// stay small enough that it's not worth a real adjacency structure.
    fn subtree_ids(&self, root_id: &str) -> Vec<String> {
        let Some(level_group) = self.nodes.get(root_id).map(|n| n.group.clone()) else {
            return Vec::new();
        };
        let mut result = vec![root_id.to_string()];
        let mut frontier = vec![root_id.to_string()];
        while let Some(current) = frontier.pop() {
            for (id, node) in &self.nodes {
                if node.parent.as_deref() == Some(current.as_str())
                    && node.group == level_group
                    && !result.contains(id)
                {
                    result.push(id.clone());
                    frontier.push(id.clone());
                }
            }
        }
        result
    }

    /// The tab context menu's "Fit Subtree" entry -- same
    /// `bounds_for_level` margin/fallback shape as `fit_all_nodes_scene_rect`,
    /// just scoped to `subtree_ids` instead of the whole level. Sizing each
    /// member by its own `node_card_rect` (via `bounds_for_level`) rather
    /// than a flat `GRAPH_NODE_SIZE` matters here specifically: `subtree_ids`
    /// follows `parent` links, which include Download children, so a
    /// same-`GRAPH_NODE_SIZE` guess would mis-size those.
    fn fit_subtree_scene_rect(&self, root_id: &str) -> egui::Rect {
        self.bounds_for_level(&self.subtree_ids(root_id))
    }

    /// `(level, node_count)` for whichever level `self.current_group` is
    /// currently viewing -- the key `build_graph_ui`'s settle-check
    /// compares against `graph_settled_key`, factored out so every call
    /// site that changes `current_group` (the per-frame check, and
    /// entering/leaving a group) computes it the same way rather than
    /// duplicating the formula.
    fn current_level_settle_key(&self) -> (Option<String>, usize) {
        (
            self.current_group.clone(),
            self.nodes_at_current_level(ItemNodeKind::Tab).len()
                + self.nodes_at_current_level(ItemNodeKind::Download).len()
                + self.nodes_at_current_level(ItemNodeKind::Note).len()
                + self.nodes_at_current_level(ItemNodeKind::Image).len()
                + self.nodes_at_current_level(ItemNodeKind::Pdf).len()
                + self.nodes_at_current_level(ItemNodeKind::Json).len()
                + self.nodes_at_current_level(ItemNodeKind::Group).len(),
        )
    }

    /// Directional spatial navigation for arrow-key graph focus: among tab
    /// nodes at the current level lying roughly in `dir` from `from`,
    /// returns whichever minimizes `along + perp * 2.0` (distance in the
    /// travel direction, penalized for straying sideways) -- the standard
    /// heuristic behind CSS's spatial navigation, picking the node that
    /// "feels like" the next one over rather than just the closest by raw
    /// distance (which would favor a diagonal neighbor over a same-row one).
    /// Only tabs are candidates, not downloads/groups: `self.focused` is a
    /// tab id everywhere else it's read (address bar sync, Ctrl+W, "+ New
    /// Tab" parent), so keeping it tab-only here avoids handing those call
    /// sites an id type they don't expect.
    fn nearest_tab_in_direction(&self, from: egui::Vec2, dir: egui::Vec2) -> Option<String> {
        let mut best: Option<(String, f32)> = None;
        for id in self.nodes_at_current_level(ItemNodeKind::Tab) {
            let Some(node) = self.nodes.get(&id) else { continue };
            let delta = node.pos - from;
            let along = delta.dot(dir);
            if along <= 0.5 {
                continue;
            }
            let perp = (delta - dir * along).length();
            let score = along + perp * 2.0;
            if best.as_ref().map_or(true, |(_, s)| score < *s) {
                best = Some((id, score));
            }
        }
        best.map(|(id, _)| id)
    }

    /// Moves `self.focused` to the nearest tab node in `dir` (see
    /// `nearest_tab_in_direction`), or -- if nothing at this level is
    /// currently focused -- just picks a starting node so the first arrow
    /// press has something to move from next time. Shared by
    /// `build_graph_ui`'s real-keyboard arrow handling and the headless
    /// `GRAPHLEFT`/`GRAPHRIGHT`/`GRAPHUP`/`GRAPHDOWN` debug commands (a real
    /// `WindowEvent::KeyboardInput` can't be synthesized from application
    /// code -- see `debug_control`'s own doc comment -- so headless mode
    /// calls this directly instead).
    fn move_graph_focus(&mut self, dir: egui::Vec2) {
        let from_at_level = self
            .focused
            .as_ref()
            .filter(|id| self.nodes.get(id.as_str()).is_some_and(|n| n.group == self.current_group))
            .and_then(|id| self.nodes.get(id))
            .map(|n| n.pos);
        match from_at_level {
            Some(from) => {
                if let Some(next) = self.nearest_tab_in_direction(from, dir) {
                    self.focused = Some(next);
                }
            }
            None => {
                if let Some(first) = self.nodes_at_current_level(ItemNodeKind::Tab).into_iter().next() {
                    self.focused = Some(first);
                }
            }
        }
    }

    /// Opens whichever node `move_graph_focus` last landed on, the same way
    /// clicking its card does -- the Enter-key equivalent of graph-view
    /// keyboard navigation, and the headless `GRAPHOPEN` command.
    fn open_focused_graph_node(&mut self) {
        if let Some(id) = self.focused.clone() {
            if self.nodes.get(&id).is_some_and(|n| n.group == self.current_group) {
                self.start_transition_to_item(&id);
            }
        }
    }

    /// Runs the force-directed layout to convergence for whichever level
    /// `self.current_group` is currently viewing, then stops -- unlike the
    /// old always-on-every-frame simulation, positions are static between
    /// calls to this. Continuous physics had a real, user-visible cost:
    /// every unpinned node (the drop target included) kept reacting to
    /// forces *while* being dragged onto, so a slow drag toward a group or
    /// another tab could chase a target that kept fleeing the incoming
    /// drag's own repulsion -- confirmed live while building the
    /// drag-to-group feature, where only a single fast cursor jump (no
    /// intermediate positions for the target to react to) reliably landed
    /// a drop. Calling this only when the visible node set actually
    /// changes (see `graph_settled_key`) keeps the one real benefit of the
    /// old approach -- new/remaining nodes still auto-spread apart instead
    /// of stacking -- without anything ever moving on its own while the
    /// user is trying to point at it.
    fn settle_current_level(&mut self) {
        // 90 iterations at a fixed 1/60s step is ~1.5 simulated seconds --
        // comfortably past the "well under a second" convergence this
        // layout already documented under continuous 60fps physics, with
        // margin for a slightly larger graph. Trivial cost for the tens
        // of nodes this app's graph view actually holds.
        const SETTLE_ITERATIONS: u32 = 90;
        const SETTLE_DT: f32 = 1.0 / 60.0;

        // Not doubled via a stray `.chain()` of the identical filter over
        // itself any more -- **found during a performance audit**: all
        // three of these (plus the write-back loop below) collected the
        // exact same filtered `self.nodes.iter()` twice, a duplicate-loop
        // artifact from the tabs/downloads/groups -> one `self.nodes` map
        // migration (the same recurring bug this project has already found
        // and fixed four separate times elsewhere -- `group_merge_requested`,
        // `dissolve_group` twice, `replace_running_session`). Harmless for
        // `positions`/`velocities` (a `HashMap` insert of the same key
        // twice just overwrites itself), but a real correctness bug for
        // `edges`: a plain `Vec`, so every spring edge was fed into
        // `graph_layout::step` *twice* -- doubling the effective spring
        // strength for every one of this function's 90 warmup iterations.
        // `settle_current_level` is `graph_layout::step`'s only call site
        // in this whole app (the layout doesn't run continuously every
        // frame -- see this function's own doc comment on why: it's a
        // one-time burst triggered only when the visible node set actually
        // changes), so this bug affected every settle animation, not a
        // background steady-state cost.
        let mut positions: HashMap<String, egui::Vec2> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.group == self.current_group)
            .map(|(id, n)| (id.clone(), n.pos))
            .collect();
        let mut velocities: HashMap<String, egui::Vec2> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.group == self.current_group)
            .map(|(id, n)| (id.clone(), n.vel))
            .collect();
        let edges: Vec<(String, String)> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.group == self.current_group)
            .filter_map(|(id, n)| n.parent.clone().map(|p| (p, id.clone())))
            .filter(|(p, c)| positions.contains_key(p) && positions.contains_key(c))
            .collect();
        let pinned: HashSet<String> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.group == self.current_group && n.as_tab().is_some_and(|t| t.pinned))
            .map(|(id, _)| id.clone())
            .collect();

        for _ in 0..SETTLE_ITERATIONS {
            graph_layout::step(
                &mut positions,
                &mut velocities,
                &edges,
                &pinned,
                GRAPH_WORLD_CENTER,
                SETTLE_DT,
            );
        }

        for (id, node) in self.nodes.iter_mut() {
            if let Some(p) = positions.get(id) {
                node.pos = *p;
            }
            if let Some(v) = velocities.get(id) {
                node.vel = *v;
            }
        }
    }

    /// Graph view's "+ New Tab" action: a child of `parent` (or a new root
    /// if `parent` is `None`), spawned invisible — it stays in Graph view
    /// rather than auto-focusing (see plan's M5 scope decision 5), and
    /// background tabs render nothing until focused anyway. Also used for
    /// a page-triggered new tab (`EngineToHost::OpenTabRequested`), with
    /// that request's URL instead of the configured home page.
    /// Tab context menu's "Duplicate": a new tab at `id`'s current url and
    /// engine, sharing its parent and group -- a sibling, not a child of
    /// `id` itself, matching what "duplicate" means in a real browser's
    /// own tab strip.
    fn duplicate_tab(&mut self, id: &str) {
        let Some((parent, group, url, kind)) = self.nodes.get(id).and_then(|n| {
            n.as_tab().map(|t| (n.parent.clone(), n.group.clone(), t.url.clone(), t.tab.kind))
        }) else {
            return;
        };
        self.spawn_new_tab_with_kind(parent, group, &url, kind, SpawnPurpose::New);
    }

    fn spawn_new_tab(&mut self, parent: Option<String>, group: Option<String>, url: &str) {
        self.spawn_new_tab_with_kind(
            parent,
            group,
            url,
            self.settings.default_starting_engine,
            SpawnPurpose::New,
        );
    }

    /// Shared by `spawn_new_tab` (always `default_starting_engine`),
    /// `reopen_closed_tab_shortcut` (the closed tab's own engine, not
    /// necessarily today's default), and `start_split` (`SpawnPurpose::
    /// Split` instead of `New`, everything else identical) -- `purpose` is
    /// threaded through to `spawn_tab_async` rather than hardcoded, since
    /// those three cases genuinely differ in what should happen once the
    /// spawn completes (see `handle_tab_spawn_result`'s own per-purpose
    /// match). Returns the new tab's id, allocated synchronously even
    /// though the real engine-host process spawn (and thus the node's own
    /// insertion into `self.nodes`) completes asynchronously -- see
    /// `spawn_tab_async`'s own doc comment.
    fn spawn_new_tab_with_kind(
        &mut self,
        parent: Option<String>,
        group: Option<String>,
        url: &str,
        kind: EngineKind,
        purpose: SpawnPurpose,
    ) -> String {
        let id = self.alloc_tab_id();
        // `parent`'s own position only if it actually shares `group` --
        // the toolbar's "+ New Tab" passes `self.current_group` as `group`
        // but `self.focused` (unconditionally) as `parent`, and those can
        // point at different levels (see `spawn_new_tab`'s own doc
        // comment). Falling back to any existing member of `group` keeps
        // the new tab near its actual new groupmates instead of near an
        // unrelated tab at a different level; `ZERO` (same as a fresh
        // group's own spawn point) covers a brand new, still-empty group.
        let parent_pos = parent
            .as_ref()
            .and_then(|p| self.nodes.get(p))
            .filter(|n| n.group == group)
            .map(|n| n.pos)
            .or_else(|| self.nodes.values().find(|n| n.group == group).map(|n| n.pos))
            .unwrap_or(egui::Vec2::ZERO);
        let pos = parent_pos + graph_layout::spawn_jitter(&id);
        self.tab_spawn_error = None;
        self.spawn_tab_async(
            id.clone(),
            parent,
            group,
            false,
            None,
            self.settings.zoom_factor,
            url.to_string(),
            pos,
            egui::Vec2::ZERO,
            purpose,
            kind,
        );
        id
    }

    /// Stops the focused tab rendering (and drops its find bar, context
    /// menu, and split) -- what switching the maximized node away from it
    /// needs, whether to another tab (`focus_tab`) or an Image
    /// (`focus_item`). `focused` itself is left alone.
    fn hide_focused_tab(&mut self) {
        self.close_find_bar();
        self.context_menu = None;
        self.close_split();
        if let Some(old_id) = self.focused.clone() {
            if let Some(old) = self.nodes.get_mut(&old_id).and_then(|n| n.as_tab_mut()) {
                old.tab.set_visible(false);
            }
            if let Some(gpu) = &mut self.gpu {
                gpu.free_tab_texture(&old_id);
            }
        }
    }

    /// Peek-click commits focus: the tab losing focus (if any, and if
    /// different) is told to stop rendering, the newly focused one is told
    /// to start, and the view switches to Tab. At most one tab is ever the
    /// primary/focused one at a time -- a split's own secondary pane
    /// (`split_tab`) is the one exception, also visible but never focused;
    /// switching primary focus to a genuinely different tab always closes
    /// whatever split was open, since a split is only ever meaningful
    /// paired with the specific tab it was opened against.
    fn focus_tab(&mut self, id: &str) {
        if self.focused.as_deref() != Some(id) {
            self.hide_focused_tab();
        }
        if let Some(node) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) {
            node.tab.set_visible(true);
            if !self.address_bar_focused {
                self.address_bar_text = node.url.clone();
            }
        }
        self.focused = Some(id.to_string());
        self.viewed_image = None;
        self.view_mode = ViewMode::Item;
        self.send_resize_to(id);
        self.save_session();
    }

    /// Maximizes `id` into Item view, whatever its kind -- a Tab goes
    /// through `focus_tab`; an Image becomes `viewed_image` (the last tab
    /// stays `focused`, hidden, exactly as in Graph view). Kinds with no
    /// Item view are a no-op (and a debug-build assertion).
    fn focus_item(&mut self, id: &str) {
        match self.nodes.get(id).map(|n| item_node_kind(&n.payload)) {
            Some(ItemNodeKind::Tab) => self.focus_tab(id),
            Some(ItemNodeKind::Image) => {
                self.hide_focused_tab();
                self.viewed_image = Some(id.to_string());
                self.image_view = ImageViewState::default();
                self.view_mode = ViewMode::Item;
                self.save_session();
            }
            Some(kind) => debug_assert!(false, "focus_item: {kind:?} nodes have no Item view"),
            None => {}
        }
    }

    /// Opens (or replaces) split view: the Tab-view toolbar's "Split"
    /// picker -- spawns a new tab at the focused tab's own current url on
    /// `kind`, parented to it and sharing its group, shown as the secondary
    /// pane once it finishes spawning (`SpawnPurpose::Split` -- see
    /// `handle_tab_spawn_result`). A no-op if there's no focused tab to
    /// compare against. Replacing an already-open split just spawns a
    /// fresh secondary tab and overwrites `split_tab` once *that* finishes
    /// spawning -- the old secondary tab is left exactly as `close_split`
    /// would leave it (unpaired, but not closed), since a real tab node no
    /// longer being shown in a split isn't otherwise different from any
    /// other background tab.
    fn start_split(&mut self, kind: EngineKind) {
        let Some(focused_id) = self.focused.clone() else { return };
        let Some(url) = self.nodes.get(&focused_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) else {
            return;
        };
        let group = self.group_of(&Some(focused_id.clone()));
        self.spawn_new_tab_with_kind(Some(focused_id), group, &url, kind, SpawnPurpose::Split);
    }

    /// Closes split view -- unpairs the secondary pane and stops its
    /// rendering/GPU texture, but doesn't close the tab itself: it remains
    /// a normal graph node, reachable and closable from Graph view like any
    /// other tab. A no-op if no split is open.
    fn close_split(&mut self) {
        let Some(id) = self.split_tab.take() else { return };
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.tab.set_visible(false);
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_texture(&id);
        }
        // The primary pane was rendering at half width for the split --
        // needs to grow back to the full content width now that
        // `content_size_px` no longer sees `split_tab` as set.
        if let Some(focused_id) = self.focused.clone() {
            self.send_resize_to(&focused_id);
        }
    }

    /// Leaves Item view for Graph view -- starts a shrink animation (see
    /// `Transition`) toward whichever node is shown, rather than switching
    /// instantly. For a Tab (`viewed_image` is `None`), the tab keeps
    /// rendering (`set_visible(true)`, unchanged) for the animation's whole
    /// duration, so the thumbnail it shrinks into shows real, current
    /// content instead of whatever frame happened to be on screen when the
    /// animation started, only actually suspended once the animation
    /// finishes (`finish_transition_if_done`). For an Image, the texture is
    /// already there either way, so there's nothing equivalent to suspend.
    fn leave_to_graph(&mut self) {
        self.context_menu = None;
        match self.viewed_image.clone().or_else(|| self.focused.clone()) {
            Some(id) => {
                self.transition = Some(Transition {
                    node_id: id,
                    kind: TransitionKind::ToGraph,
                    start: std::time::Instant::now(),
                });
            }
            // No focused tab and no viewed image (shouldn't normally
            // happen — Item view always has one — but nothing to animate
            // either way).
            None => {
                self.view_mode = ViewMode::Graph;
                self.save_session();
            }
        }
        self.dropdown_highlighted = None;
    }

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
    fn start_transition_to_item(&mut self, id: &str) {
        if self.transition.is_some() {
            return;
        }
        self.focus_item(id);
        // `focus_item` already set `view_mode = Item`, but `build_ui` renders
        // Graph view as the backdrop for as long as `self.transition` is
        // `Some` regardless of `view_mode` (see its own doc comment) —
        // `finish_transition_if_done` is what actually matters once the
        // animation completes.
        self.transition = Some(Transition {
            node_id: id.to_string(),
            kind: TransitionKind::ToItem,
            start: std::time::Instant::now(),
        });
    }

    /// Checked once per frame, before rendering: once a `Transition`'s
    /// duration has elapsed, applies whatever `focus_tab`/`leave_to_graph`
    /// would have done instantly (the view-mode switch, and — for
    /// `ToGraph` specifically — finally suspending the tab now that its
    /// last real frame has been captured into the thumbnail it just
    /// shrank into).
    fn finish_transition_if_done(&mut self) {
        let Some(transition) = &self.transition else {
            return;
        };
        if transition.start.elapsed() < TRANSITION_DURATION {
            return;
        }
        let node_id = transition.node_id.clone();
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
    fn graph_world_to_screen_transform(&self, scene_outer_rect: egui::Rect) -> (f32, egui::Vec2) {
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
    fn graph_world_rect_to_screen(&self, world: egui::Rect) -> egui::Rect {
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

    /// The rect (in egui points, screen space) a `Transition` should draw
    /// its tab's live content quad at *this frame* — eased interpolation
    /// between Tab view's full content area and the tab's graph-node
    /// thumbnail spot, direction depending on `transition.kind`.
    fn transition_content_rect_points(&self, transition: &Transition) -> egui::Rect {
        let screen = self.screen_size_points();
        let full_content_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, CHROME_HEIGHT_POINTS),
            egui::vec2(screen.x, (screen.y - CHROME_HEIGHT_POINTS).max(0.0)),
        );
        let node_rect = self
            .nodes
            .get(&transition.node_id)
            .map(|node| {
                self.graph_world_rect_to_screen(
                    egui::Rect::from_center_size(egui::pos2(node.pos.x, node.pos.y), GRAPH_NODE_SIZE)
                        .shrink(GRAPH_THUMB_MARGIN),
                )
            })
            .unwrap_or(full_content_rect);
        transition_eased_rect(transition, full_content_rect, node_rect)
    }

    /// Image counterpart of `transition_content_rect_points`: the image's
    /// texture and where to paint it this frame, between its graph card
    /// and its current Image-view rect (Fit on entry; whatever zoom/pan
    /// the user left on exit), so the aspect ratio is right throughout.
    /// `None` for a non-Image transition or a missing node/texture.
    fn image_transition_frame(&self, transition: &Transition) -> Option<(egui::TextureId, egui::Rect)> {
        let node = self.nodes.get(&transition.node_id)?;
        let image = node.as_image()?;
        let tex_id = self.gpu.as_ref()?.image_id(&transition.node_id)?;
        let size = image_pixel_size(image);
        let card = self.graph_world_rect_to_screen(egui::Rect::from_center_size(
            egui::pos2(node.pos.x, node.pos.y),
            image_node_card_size(image.width, image.height),
        ));
        let body = self.item_body_rect();
        let (scale, pan) = self.image_view.effective(size, body.size());
        let view = image_display_rect(body, size, scale, pan);
        Some((tex_id, transition_eased_rect(transition, view, card)))
    }

    /// Closes a tab: tears down its process (via `EngineTab`'s `Drop` —
    /// `Shutdown` + kill), frees its graph-view thumbnail, and reparents
    /// its children to its own parent so the forest stays connected rather
    /// than orphaning that whole subtree. Falls back to Graph view if the
    /// closed tab was focused.
    fn close_tab(&mut self, id: &str) {
        let Some((parent, url, kind, group)) = self.nodes.get(id).and_then(|n| {
            n.as_tab().map(|t| (n.parent.clone(), t.url.clone(), t.tab.kind, n.group.clone()))
        }) else {
            return;
        };
        self.closed_tabs.push(ClosedTab { url, kind, parent: parent.clone(), group });
        if self.closed_tabs.len() > MAX_CLOSED_TABS {
            self.closed_tabs.remove(0);
        }
        for node in self.nodes.values_mut() {
            if node.parent.as_deref() == Some(id) {
                node.parent = parent.clone();
            }
        }
        self.nodes.remove(id);
        self.node_order.retain(|t| t != id);
        // A rectangle-selected tab closed through any path other than
        // "Close Selected" itself (its own card close button, or its
        // containing group being closed) otherwise leaves a stale id in
        // `selected_tabs` -- harmless to the no-op `close_tab`/`nodes.get`
        // callers that already tolerate a missing id, but it skews
        // "Group Selected"'s centroid (divides by a count that includes
        // dead ids) and overcounts the toolbar's own "(N)" labels.
        self.selected_tabs.remove(id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
        }
        if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
        }
        if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
        // A split only ever makes sense paired with a specific focused tab
        // (see `focus_tab`'s own doc comment) -- closing either half of it
        // (the tab node is already gone either way, so no `set_visible`/
        // resize cleanup to do the way `close_split` normally would) clears
        // the whole thing rather than leaving a dangling reference to a
        // node that no longer exists.
        if self.split_tab.as_deref() == Some(id) || self.focused.as_deref() == Some(id) {
            self.split_tab = None;
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
            // Only drop to Graph if a Tab was actually showing -- while an
            // Image is maximized, `focused` is just the hidden last tab
            // (see the Image Item View spec's "Plan-time corrections"), so
            // closing it should leave Image view showing exactly as
            // closing it from Graph view wouldn't change `view_mode`
            // either.
            if self.showing_tab_view() {
                self.view_mode = ViewMode::Graph;
            }
            // The tab (and its `EngineTab`) is already gone — nothing left
            // to send `FindStop` to, just reset the UI-side state directly
            // rather than going through `close_find_bar`'s normal send.
            self.find_bar_open = false;
            self.find_result = None;
        }
        self.save_session();
    }

    /// M2's swap sequence, now per-tab and driven by the engine-picker
    /// dropdown: `GetState` -> spawn new engine at the same tab_id -> new
    /// engine's first `FrameReady` becomes the displayed texture (once
    /// focused) -> old engine-host dropped when its graph node is replaced.
    fn start_swap_to(&mut self, tab_id: &str, target: EngineKind) {
        let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) else {
            return;
        };
        if node.tab.kind == target || node.swap_target.is_some() {
            return; // no-op swap, or one swap at a time for this tab
        }
        node.swap_target = Some(target);
        node.tab.send(&HostToEngine::GetState);
    }

    fn finish_swap(&mut self, tab_id: &str, target: EngineKind, url: String) {
        let Some((parent, group, pinned, color, zoom_factor, pos, vel)) = self.nodes.get(tab_id).and_then(|n| {
            n.as_tab()
                .map(|t| (n.parent.clone(), n.group.clone(), t.pinned, n.color, t.zoom_factor, n.pos, n.vel))
        }) else {
            return;
        };
        // Must drop (and thus Shutdown + kill) the old tab before spawning
        // the new one at the same tab_id — the new engine-host's named-pipe
        // listeners would collide with the old ones otherwise.
        self.nodes.remove(tab_id);
        // Same stale-state guard `close_tab` already applies: without it, a
        // drag in progress on this exact tab when its engine swap/crash
        // fires would leave `self.dragging` pointing at an id invisible for
        // the whole respawn window, then get misread as "just dropped" the
        // instant the respawned node starts rendering again.
        if self.dragging.as_deref() == Some(tab_id) {
            self.dragging = None;
        }
        if self.hovered.as_deref() == Some(tab_id) {
            self.hovered = None;
        }
        self.respawn_tab(tab_id, target, url, parent, group, pinned, color, zoom_factor, pos, vel);
    }

    /// Shared by swap-finish and crash recovery: (re)spawn `kind` at `url`
    /// for `tab_id`, preserving its place in the graph (`parent`/`group`/
    /// `pinned`/`pos`/`vel`) and visibility (only the focused tab is ever
    /// visible).
    fn respawn_tab(
        &mut self,
        tab_id: &str,
        kind: EngineKind,
        url: String,
        parent: Option<String>,
        group: Option<String>,
        pinned: bool,
        color: Option<egui::Color32>,
        zoom_factor: f64,
        pos: egui::Vec2,
        vel: egui::Vec2,
    ) {
        if self.focused.as_deref() == Some(tab_id) {
            self.pending_respawn_focus = Some(tab_id.to_string());
        }
        self.spawn_tab_async(
            tab_id.to_string(),
            parent,
            group,
            pinned,
            color,
            zoom_factor,
            url,
            pos,
            vel,
            SpawnPurpose::Respawn,
            kind,
        );
    }

    /// Browser-chrome keyboard shortcuts — checked via egui's own input
    /// polling (same mechanism `build_tab_view_ui`'s own `Escape` check uses),
    /// so they fire regardless of which widget has focus and in either
    /// view. `consume_key` removes the matching key event from this
    /// frame's input first, so a page never sees e.g. `Ctrl+T` leak
    /// through as a plain `T` keypress the way an unhandled key would
    /// (see `window_event`'s `KeyboardInput` arm). `Ctrl+Shift+Tab` and
    /// `Ctrl+Shift+T` are each checked before their plain (no-Shift)
    /// counterpart — egui's `matches_logically` ignores an extra Shift/Alt,
    /// so checking the less specific one first would consume a
    /// `Ctrl+Shift+X` press as if it were plain `Ctrl+X`.
    fn handle_global_shortcuts(&mut self, ui: &mut egui::Ui) {
        let (
            reopen_tab,
            new_tab,
            close_all_level,
            close_tab,
            focus_address,
            open_find,
            zoom_in,
            zoom_out,
            zoom_reset,
            open_devtools,
            prev_tab,
            next_tab,
            quick_switch,
            new_group,
            view_toggle,
            pin_toggle,
            reload,
            rename_group,
            duplicate_tab,
            shortcuts_help,
        ) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::T),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::T),
                // Checked ahead of plain `Ctrl+W` below, same "extra Shift
                // matches the plain binding too" ordering reason `Ctrl+Shift+
                // T`/`Ctrl+Shift+G` already need.
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::W),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::W),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::L),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::F),
                // `Ctrl+=` is the physical key most keyboards actually send
                // for "zoom in" (`+` needs Shift on most layouts) — accept
                // both so either a real `Ctrl++` or the more common
                // `Ctrl+=` works.
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Equals)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::Plus),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Minus),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Num0),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F12),
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Tab),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::K),
                // Checked ahead of plain `Ctrl+G` below -- `matches_logically`
                // ignores an extra Shift, so checking `Ctrl+G` first would
                // consume a `Ctrl+Shift+G` press as if it were plain
                // `Ctrl+G`, same ordering reason `Ctrl+Shift+T` already needs.
                i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::G),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::G),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::P),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::R)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::F5),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F2),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::D),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F1),
            )
        });

        if reopen_tab {
            self.reopen_closed_tab_shortcut();
        }
        if new_tab {
            self.new_tab_shortcut();
        }
        if close_all_level {
            self.close_all_at_current_level();
        }
        if close_tab {
            self.close_focused_tab_shortcut();
        }
        if focus_address {
            self.focus_address_bar_shortcut();
        }
        if open_find {
            self.open_find_bar_shortcut();
        }
        if zoom_in {
            self.zoom_in_shortcut();
        }
        if zoom_out {
            self.zoom_out_shortcut();
        }
        if zoom_reset {
            self.zoom_reset_shortcut();
        }
        if open_devtools {
            self.open_devtools_shortcut();
        }
        if next_tab || prev_tab {
            self.cycle_focused_tab(next_tab);
        }
        if quick_switch {
            self.quick_switcher = Some(QuickSwitcher { query: String::new(), selected: 0 });
            self.pending_focus_quick_switcher = true;
        }
        if new_group {
            self.new_group_shortcut();
        }
        // Neutralized while the quick-switcher is open, same reasoning as
        // its own Escape/arrow handling in `build_quick_switcher` -- a
        // `Ctrl+G` typed while it's open shouldn't also flip the view
        // underneath it.
        if view_toggle && self.quick_switcher.is_none() {
            self.toggle_view_mode_shortcut();
        }
        if pin_toggle {
            self.pin_focused_tab_shortcut();
        }
        if reload {
            self.reload_focused_tab_shortcut();
        }
        if rename_group {
            self.rename_current_group_shortcut();
        }
        if duplicate_tab {
            self.duplicate_focused_tab_shortcut();
        }
        if shortcuts_help {
            self.shortcuts_help_open = !self.shortcuts_help_open;
        }
    }

    /// `Ctrl+P` / headless `TOGGLEPIN` — pins/unpins the focused tab, same
    /// effect as its graph card's own pin dot or the tab context menu's
    /// Pin/Unpin entry.
    fn pin_focused_tab_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.pinned = !node.pinned;
        }
    }

    /// `Ctrl+R`/`F5` / headless `RELOAD` — reloads the focused tab. Every
    /// engine exposes a plain no-argument reload (see `HostToEngine::
    /// Reload`'s own doc comment), so there's no can-reload guard the way
    /// `GoBack`/`GoForward` need one.
    fn reload_focused_tab_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.tab.send(&HostToEngine::Reload);
        }
    }

    /// Graph view's own `Escape` handling / headless `DISMISSPEEK` —
    /// dismisses the currently-hovered node's peek preview early, without
    /// needing the pointer to move away first. See `peek_suppressed`'s own
    /// doc comment for why this can't just clear `hovered` directly.
    fn dismiss_hover_peek(&mut self) {
        self.peek_suppressed = self.hovered.clone();
    }

    /// `F2` / headless `RENAMEGROUP` — starts renaming whichever group's
    /// own subgraph is currently being viewed, same effect as clicking its
    /// name button in the Graph toolbar (see that button's own click
    /// handler, which this mirrors exactly). A no-op at the top level
    /// (`current_group` is `None` there, nothing to rename) or while
    /// already renaming.
    fn rename_current_group_shortcut(&mut self) {
        let Some(group_id) = self.current_group.clone() else { return };
        if self.renaming_group.is_some() {
            return;
        }
        let Some(name) = self.nodes.get(&group_id).and_then(|n| n.as_group()).map(|g| g.name.clone()) else { return };
        self.renaming_group = Some((group_id, name));
        self.pending_focus_rename = true;
    }

    /// `Ctrl+Shift+G` / toolbar "+ New Group" / headless `NEWGROUP` —
    /// creates an empty group at whichever level is currently being
    /// viewed, nested under `current_group` if that's not the top level
    /// (see `GroupPayload`'s own doc comment on nesting).
    fn new_group_shortcut(&mut self) {
        let id = self.alloc_group_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        self.insert_group_at(id, pos);
    }

    /// Shared by `new_group_shortcut` (fresh id/jittered position) and the
    /// empty-canvas "New Tab/Note/Group/File" card (`pending_new_node`),
    /// which passes the exact position that was right-clicked instead.
    fn insert_group_at(&mut self, id: String, pos: egui::Vec2) {
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
        self.node_order.push(id);
    }

    /// The Graph toolbar's "+ Note" button -- a note spawns at whichever
    /// level is currently being viewed, same as "+ New Tab" and (now)
    /// "+ New Group" itself, since a note is a
    /// plain member of a level's subgraph like a tab or download is.
    fn new_note_shortcut(&mut self) {
        let id = self.alloc_note_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        self.insert_note_at(id, pos);
    }

    /// "Recently Closed" reopening a deleted `ClosedNode::Note` -- unlike
    /// `insert_note_at` (always blank, `editing: true`, for a genuinely new
    /// note), this restores real saved text and starts rendered rather than
    /// mid-edit, matching how a restored/imported note should look.
    fn respawn_closed_note(&mut self, text: String) {
        let id = self.alloc_note_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group: self.current_group.clone(),
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Note(NotePayload { text, editing: false }),
            },
        );
        self.node_order.push(id);
        self.save_session();
    }

    /// Shared by `new_note_shortcut` (fresh id/jittered position) and the
    /// empty-canvas "New Tab/Note/Group/File" card (`pending_new_node`),
    /// which passes the exact position that was right-clicked instead.
    fn insert_note_at(&mut self, id: String, pos: egui::Vec2) {
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group: self.current_group.clone(),
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Note(NotePayload { text: String::new(), editing: true }),
            },
        );
        self.node_order.push(id);
        self.save_session();
    }

    /// Decodes `path` and inserts a new image node with a fresh id at the
    /// current level, jittered around world center -- the interactive
    /// "+ Image" button (`add_image_node`, after its own file picker) and
    /// the headless `ADDIMAGE <path>` debug command. Returns `false` (doing
    /// nothing) if the file doesn't exist or isn't a decodable image, same
    /// "never let one bad file stop everything else" posture
    /// `Settings::load`/`session::Session::load` already have. Doesn't
    /// itself call `save_session` -- both callers mutate persisted state
    /// and save themselves right after.
    /// A download node for `path`, started from tab `tab_id`: placed next
    /// to that tab, in the tab's group, with an edge from it. Shared by an
    /// engine's `DownloadStarted` and the headless `ADDDOWNLOAD <path>`
    /// debug command (downloads otherwise only ever come from a real
    /// engine download, so headless runs had no way to get one). A no-op
    /// if `tab_id` is gone. Downloads aren't persisted, so no save.
    fn insert_download_node(&mut self, tab_id: String, path: String, total_bytes: Option<u64>) {
        // "dl-" prefix so a download node's id can never collide with a
        // tab's -- both share the same graph/edge id space
        // (`graph_layout::step` sees them as one flat set of nodes).
        let Some((node_pos, node_group)) = self.nodes.get(&tab_id).map(|n| (n.pos, n.group.clone())) else {
            return;
        };
        let id = format!("dl-{}", self.next_download_id);
        self.next_download_id += 1;
        let pos = node_pos + graph_layout::spawn_jitter(&id);
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: Some(tab_id),
                group: node_group,
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Download(DownloadPayload {
                    path,
                    bytes_received: 0,
                    total_bytes,
                    completed: false,
                }),
            },
        );
        self.node_order.push(id);
    }

    /// Session restore's stand-in for an image/JSON card whose file
    /// couldn't be loaded -- see `ImagePayload::error`.
    fn insert_placeholder_node(&mut self, id: String, pos: egui::Vec2, payload: NodePayload) {
        let node = GraphNode { parent: None, group: None, pos, vel: egui::Vec2::ZERO, color: None, payload };
        self.nodes.insert(id.clone(), node);
        self.node_order.push(id);
    }

    /// Tries a placeholder's file again (a click on the card). Loaded, the
    /// real card takes the placeholder's place: same id, position, group,
    /// color and draw order. Still failing, it stays a placeholder with the
    /// new error, which the insert has already toasted. A failed PDF card
    /// just renders again (back to its spinner).
    fn retry_missing_file(&mut self, id: &str) {
        if let Some(pdf) = self.nodes.get_mut(id).and_then(|n| n.as_pdf_mut()) {
            if matches!(pdf.outcome, Some(Err(_))) {
                pdf.outcome = None;
                self.pdf_fetcher.request(id.to_string(), pdf.path.clone());
            }
            return;
        }
        let Some(node) = self.nodes.get(id) else { return };
        let (path, is_image) = match &node.payload {
            NodePayload::Image(image) if image.error.is_some() => (image.path.clone(), true),
            NodePayload::Json(json) if json.error.is_some() => (json.path.clone(), false),
            _ => return,
        };
        let (pos, group, parent, color) = (node.pos, node.group.clone(), node.parent.clone(), node.color);
        let in_order = self.node_order.iter().any(|n| n == id);
        let result = if is_image {
            self.insert_image_node_with_id(id.to_string(), path, pos, group)
        } else {
            self.insert_json_node_with_id(id.to_string(), path, pos, group)
        };
        match result {
            Ok(()) => {
                // The insert replaced the node and pushed its id again;
                // keep it where it was.
                if in_order {
                    self.node_order.pop();
                }
                if let Some(node) = self.nodes.get_mut(id) {
                    node.parent = parent;
                    node.color = color;
                }
                self.save_session();
            }
            Err(error) => match self.nodes.get_mut(id).map(|n| &mut n.payload) {
                Some(NodePayload::Image(image)) => image.error = Some(error),
                Some(NodePayload::Json(json)) => json.error = Some(error),
                _ => {}
            },
        }
    }

    fn insert_image_node(&mut self, path: String) -> bool {
        let id = self.alloc_image_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let group = self.current_group.clone();
        self.insert_image_node_with_id(id, path, pos, group).is_ok()
    }

    /// Shared by `insert_image_node` (fresh id/jittered position/current
    /// level) and session restore (the exact saved id/position -- and,
    /// like a restored tab's `group` staying unset regardless of where it
    /// was before restart, always `None` here too).
    fn insert_image_node_with_id(
        &mut self,
        id: String,
        path: String,
        pos: egui::Vec2,
        group: Option<String>,
    ) -> Result<(), String> {
        // Toasted here, not by callers: session restore, add-file and
        // reopen all used to drop a missing or undecodable file silently.
        // The error comes back too, for restore's placeholder card.
        let img = match image::open(&path) {
            Ok(img) => img,
            Err(err) => {
                eprintln!("image: couldn't open {path}: {err}");
                let name = crate::json_file::file_name(std::path::Path::new(&path));
                let err = format!("Couldn't open image {name}: {err}");
                self.queue_toast(err.clone());
                return Err(err);
            }
        };
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        if let Some(gpu) = &mut self.gpu {
            gpu.upload_image(&id, rgba.as_raw(), width, height);
        }
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group,
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Image(ImagePayload { path, width, height, error: None }),
            },
        );
        self.node_order.push(id);
        Ok(())
    }

    /// Converts a tab whose page turned out to be a raw image response into
    /// a real `Image` graph node at the same position -- see `ipc_protocol::
    /// EngineToHost::RedirectableContent`'s own doc comment. `path` is
    /// wherever `redirect_fetch` already saved the downloaded bytes; reuses
    /// `insert_image_node_with_id` verbatim, so a redirected image is
    /// indistinguishable from one added via "+ Image" from this point on.
    /// Reparents the tab's own children to its own parent and clears the
    /// same dragging/hovered/focused guards `close_tab` does -- but doesn't
    /// push to `closed_tabs`, since this isn't a user-initiated close to
    /// remember as reopenable.
    fn redirect_tab_to_image(&mut self, tab_id: &str, path: String) {
        let Some((parent, group, pos)) =
            self.nodes.get(tab_id).map(|n| (n.parent.clone(), n.group.clone(), n.pos))
        else {
            return;
        };
        for node in self.nodes.values_mut() {
            if node.parent.as_deref() == Some(tab_id) {
                node.parent = parent.clone();
            }
        }
        self.nodes.remove(tab_id);
        self.node_order.retain(|t| t != tab_id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(tab_id);
        }
        if self.dragging.as_deref() == Some(tab_id) {
            self.dragging = None;
        }
        if self.hovered.as_deref() == Some(tab_id) {
            self.hovered = None;
        }
        if self.focused.as_deref() == Some(tab_id) {
            self.focused = None;
            // Same "only a Tab showing actually leaves to Graph" guard as
            // `close_tab` -- see its own comment.
            if self.showing_tab_view() {
                self.view_mode = ViewMode::Graph;
            }
            self.find_bar_open = false;
            self.find_result = None;
        }
        let id = self.alloc_image_id();
        // Failure is already toasted; the tab it replaces is gone either way.
        let _ = self.insert_image_node_with_id(id, path, pos, group);
        self.save_session();
    }

    /// Inserts a new PDF node with a fresh id at the current level,
    /// jittered around world center, and starts its background render --
    /// the interactive "+ PDF" button (`add_pdf_node`, after its own file
    /// picker) and the headless `ADDPDF <path>` debug command. Unlike
    /// `insert_image_node`, this always inserts the node immediately
    /// (`outcome: None`, "still loading") rather than checking the file
    /// synchronously first -- decoding a PDF is genuinely slow (see
    /// `PdfPayload`'s own doc comment), so there's nothing worth checking
    /// synchronously here; a bad/missing file surfaces later as a
    /// `Some(Err(_))` outcome instead of the node never having existed the
    /// way a bad image file does. Doesn't itself call `save_session` --
    /// both callers mutate persisted state and save themselves right after.
    fn insert_pdf_node(&mut self, path: String) {
        let id = self.alloc_pdf_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let group = self.current_group.clone();
        self.insert_pdf_node_with_id(id, path, pos, group);
    }

    /// Shared by `insert_pdf_node` (fresh id/jittered position/current
    /// level) and session restore (the exact saved id/position -- and,
    /// like a restored image's `group` staying unset regardless of where it
    /// was before restart, always `None` here too).
    fn insert_pdf_node_with_id(&mut self, id: String, path: String, pos: egui::Vec2, group: Option<String>) {
        self.pdf_fetcher.request(id.clone(), path.clone());
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group,
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Pdf(PdfPayload { path, outcome: None }),
            },
        );
        self.node_order.push(id);
    }

    /// Decodes `path` as JSON and inserts a new JSON node with a fresh id at
    /// the current level, jittered around world center -- the interactive
    /// "+ File" button (`add_file_node`, when the picked file is `.json`)
    /// and the headless `ADDJSON <path>` debug command. Returns `false`
    /// (doing nothing) if the file doesn't exist or isn't valid JSON, same
    /// fail-fast posture `insert_image_node` already has (a JSON parse is
    /// just as fast as decoding an image, so there's nothing here that
    /// needs `Pdf`'s own background-render treatment). Doesn't itself call
    /// `save_session` -- both callers mutate persisted state and save
    /// themselves right after.
    fn insert_json_node(&mut self, path: String) -> bool {
        let id = self.alloc_json_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let group = self.current_group.clone();
        self.insert_json_node_with_id(id, path, pos, group).is_ok()
    }

    /// Shared by `insert_json_node` (fresh id/jittered position/current
    /// level) and session restore (the exact saved id/position -- and,
    /// like a restored image's `group` staying unset regardless of where it
    /// was before restart, always `None` here too).
    fn insert_json_node_with_id(
        &mut self,
        id: String,
        path: String,
        pos: egui::Vec2,
        group: Option<String>,
    ) -> Result<(), String> {
        // Toasted here, not by callers -- see `insert_image_node_with_id`.
        let value = match crate::json_file::load::<serde_json::Value>(std::path::Path::new(&path), "JSON") {
            Ok(value) => value,
            Err(err) => {
                self.queue_toast(err.clone());
                return Err(err);
            }
        };
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group,
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Json(JsonPayload { path, value, key: None, error: None }),
            },
        );
        self.node_order.push(id);
        Ok(())
    }

    /// "Expand as Graph" -- explodes a JSON node's structure into real,
    /// nested graph nodes: an object/array becomes a `Group` (matching this
    /// app's existing unlimited Group-nesting model), a scalar becomes a
    /// plain `JsonPayload` leaf (see that struct's own doc comment for why
    /// no new node kind exists for this). Read-only, one-way, and
    /// session-only, per this feature's own design doc
    /// (`docs/superpowers/specs/2026-09-19-json-expand-as-graph-design.md`):
    /// there's no "collapse back," and nothing here is written to
    /// `session.json`. The original node is converted in place (`payload`
    /// is swapped, not the node removed and reinserted) so its existing
    /// `pos`/`group`/`color` (all on the shared `GraphNode`) carry over
    /// untouched -- but since a `Group` is never persisted, this also means
    /// the *source file itself* drops out of `session.json` entirely the
    /// next time it saves, not just the exploded subtree: after a restart
    /// there's no compact card left to click back to, only "Add File..."
    /// to bring the file back as a fresh node. A bare scalar at the JSON's
    /// own root has nothing to explode into -- this just leaves such a
    /// node as it already is, a harmless no-op rather than something worth
    /// guarding against.
    fn expand_json_as_graph(&mut self, json_node_id: &str) {
        let Some((path, value)) =
            self.nodes.get(json_node_id).and_then(|n| n.as_json().map(|j| (j.path.clone(), j.value.clone())))
        else {
            return;
        };
        let has_entries = match &value {
            serde_json::Value::Object(map) => !map.is_empty(),
            serde_json::Value::Array(items) => !items.is_empty(),
            _ => false,
        };
        if has_entries {
            let root_name = download_filename(&path).to_string();
            if let Some(node) = self.nodes.get_mut(json_node_id) {
                node.payload = NodePayload::Group(GroupPayload { name: root_name });
            }
            self.expand_json_entries(json_node_id, &value, &path);
        }
    }

    /// Recursive step shared by `expand_json_as_graph`: creates one child
    /// node per entry of `value` (already known to be an `Object`/`Array`)
    /// inside `parent_group_id`, recursing into any entry that's itself a
    /// container. `path` is the *source file's* path, inherited by every
    /// leaf purely for "Reveal in Explorer"/"Copy Path" display purposes --
    /// it does not mean the leaf independently owns that file (see
    /// `JsonPayload`'s own doc comment on `key`).
    fn expand_json_entries(&mut self, parent_group_id: &str, value: &serde_json::Value, path: &str) {
        let entries: Vec<(String, &serde_json::Value)> = match value {
            serde_json::Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v)).collect(),
            serde_json::Value::Array(items) => {
                items.iter().enumerate().map(|(i, v)| (format!("[{i}]"), v)).collect()
            }
            _ => return,
        };
        let parent_pos = self.nodes.get(parent_group_id).map(|n| n.pos).unwrap_or(GRAPH_WORLD_CENTER);
        for (key, entry_value) in entries {
            if matches!(entry_value, serde_json::Value::Object(_) | serde_json::Value::Array(_)) {
                let id = self.alloc_group_id();
                let pos = parent_pos + graph_layout::spawn_jitter(&id);
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        parent: None,
                        group: Some(parent_group_id.to_string()),
                        pos,
                        vel: egui::Vec2::ZERO,
                        color: None,
                        payload: NodePayload::Group(GroupPayload { name: key }),
                    },
                );
                self.node_order.push(id.clone());
                self.expand_json_entries(&id, entry_value, path);
            } else {
                let id = self.alloc_json_id();
                let pos = parent_pos + graph_layout::spawn_jitter(&id);
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        parent: None,
                        group: Some(parent_group_id.to_string()),
                        pos,
                        vel: egui::Vec2::ZERO,
                        color: None,
                        payload: NodePayload::Json(JsonPayload {
                            path: path.to_string(),
                            value: entry_value.clone(),
                            key: Some(key),
                            error: None,
                        }),
                    },
                );
                self.node_order.push(id);
            }
        }
    }

    /// The Graph toolbar's single "+ File" button -- one file picker
    /// covering every locally-addable node kind (Image/PDF/JSON), replacing
    /// three separate buttons/pickers (`add_image_node`/`add_pdf_node`/
    /// `add_json_node`, all since removed) that only ever differed in
    /// which extensions they filtered for and which `insert_*_node` got
    /// called after picking a file -- the same "dispatch by type, one
    /// allowlist" shape `redirect_fetch::classify` already established for
    /// auto-redirected tabs, applied here to a local file's own extension
    /// instead of a fetched `Content-Type` (there's no HTTP response to
    /// read a real MIME type from for a file already on disk). A real file
    /// picker blocks on OS/human input neither exists in headless mode, so
    /// this is never called there; `ADDIMAGE`/`ADDPDF`/`ADDJSON <path>`
    /// remain the headless equivalents, each still supplying its own path
    /// directly to the matching `insert_*_node` without going through this
    /// dispatch at all.
    /// `pos`: `None` for the toolbar button (fresh id per kind, jittered
    /// around world center internally by `insert_image_node`/
    /// `insert_pdf_node`/`insert_json_node`, same as always); `Some(pos)`
    /// for the empty-canvas "New Tab/Note/Group/File" card
    /// (`pending_new_node`), which already knows exactly where it should
    /// land and calls the `_with_id` variants directly instead.
    fn add_file_node(&mut self, pos: Option<egui::Vec2>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Image, PDF, or JSON",
                &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "pdf", "json"],
            )
            .pick_file()
        else {
            return;
        };
        let path = path.to_string_lossy().into_owned();
        let extension = std::path::Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let inserted = match (extension.as_deref(), pos) {
            (Some("pdf"), Some(pos)) => {
                let id = self.alloc_pdf_id();
                let group = self.current_group.clone();
                self.insert_pdf_node_with_id(id, path, pos, group);
                true
            }
            (Some("pdf"), None) => {
                self.insert_pdf_node(path);
                true
            }
            (Some("json"), Some(pos)) => {
                let id = self.alloc_json_id();
                let group = self.current_group.clone();
                self.insert_json_node_with_id(id, path, pos, group).is_ok()
            }
            (Some("json"), None) => self.insert_json_node(path),
            (Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"), Some(pos)) => {
                let id = self.alloc_image_id();
                let group = self.current_group.clone();
                self.insert_image_node_with_id(id, path, pos, group).is_ok()
            }
            (Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"), None) => {
                self.insert_image_node(path)
            }
            _ => false,
        };
        if inserted {
            self.save_session();
        }
    }

    /// `Ctrl+G` / headless `TOGGLEVIEW` — switches to whichever of Tab/Graph
    /// view isn't currently showing. Tab -> Graph reuses `leave_to_graph`'s
    /// own shrink animation. Graph -> Tab jumps to `self.focused` directly
    /// (`focus_tab`, not the graph node's expand animation) and updates
    /// `current_group` to match it first -- same reasoning the quick-
    /// switcher's own jump has: `self.focused` isn't guaranteed to be a tab
    /// at the level currently being viewed (e.g. after `Ctrl+Tab` cycling
    /// or a quick-switcher jump into a different group), so there's no
    /// reliable on-screen node to animate from.
    fn toggle_view_mode_shortcut(&mut self) {
        match self.view_mode {
            ViewMode::Item => self.leave_to_graph(),
            ViewMode::Settings => self.close_settings_page(),
            ViewMode::Graph => {
                let Some(id) = self.focused.clone() else { return };
                if self.nodes.contains_key(&id) {
                    self.current_group = self.nodes.get(&id).and_then(|n| n.group.clone());
                    self.focus_tab(&id);
                }
            }
        }
    }

    /// Gear-icon button (Tab or Graph toolbar) / headless
    /// `TOGGLESETTINGSPAGE` — enters the Settings page from whichever of
    /// Tab/Graph is currently showing, remembering it in
    /// `settings_return_view` so `close_settings_page` knows where to go
    /// back to. A no-op if already on the Settings page (doesn't clobber
    /// the return view with `Settings` itself).
    fn open_settings_page(&mut self) {
        if self.view_mode != ViewMode::Settings {
            self.settings_return_view = self.view_mode;
            self.view_mode = ViewMode::Settings;
        }
    }

    /// The Settings page's own Back button / `Escape` / `Ctrl+G` — returns
    /// to `settings_return_view`. A no-op if the Settings page isn't
    /// showing.
    fn close_settings_page(&mut self) {
        if self.view_mode == ViewMode::Settings {
            self.view_mode = self.settings_return_view;
        }
    }

    /// Gear-icon button's actual click handler -- opens the Settings page,
    /// or closes it back to `settings_return_view` if it's already open
    /// (the button doubles as Back while on the page, same toggle feel the
    /// old gear-icon popup had).
    fn toggle_settings_page(&mut self) {
        if self.view_mode == ViewMode::Settings {
            self.close_settings_page();
        } else {
            self.open_settings_page();
        }
    }

    /// `Ctrl+=`/`Ctrl++` / headless `ZOOMIN` — zooms the maximized Image
    /// while one is showing (see the Image item view spec's "2. Image
    /// view" section); otherwise the same step/cap the settings popup's
    /// own "+" button uses for the focused Tab's page zoom.
    fn zoom_in_shortcut(&mut self) {
        if self.showing_image_view() {
            self.apply_image_zoom_action(ZoomChipAction::In);
        } else {
            self.set_focused_zoom((self.focused_zoom_factor() + 0.1).min(2.0));
        }
    }

    /// `Ctrl+-` / headless `ZOOMOUT` — same Image-view/Tab-view split as
    /// `zoom_in_shortcut`; the Tab branch matches the settings popup's own
    /// "−" button.
    fn zoom_out_shortcut(&mut self) {
        if self.showing_image_view() {
            self.apply_image_zoom_action(ZoomChipAction::Out);
        } else {
            self.set_focused_zoom((self.focused_zoom_factor() - 0.1).max(0.5));
        }
    }

    /// `Ctrl+0` / headless `ZOOMRESET` — same Image-view/Tab-view split as
    /// `zoom_in_shortcut`; the Tab branch matches the settings popup's own
    /// "Reset" button.
    fn zoom_reset_shortcut(&mut self) {
        if self.showing_image_view() {
            self.apply_image_zoom_action(ZoomChipAction::Reset100);
        } else {
            self.set_focused_zoom(1.0);
        }
    }

    /// `F12` / headless `OPENDEVTOOLS` — opens the focused tab's engine's
    /// own native DevTools UI (a separate real window the engine manages
    /// itself; nothing comes back to `manifold` for this, so there's no
    /// state to track here beyond sending the message). See
    /// `HostToEngine::OpenDevTools`'s doc comment for per-engine support.
    fn open_devtools_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        node.tab.send(&HostToEngine::OpenDevTools);
    }

    /// `Ctrl+T` / headless `NEWTAB` — same call the "+ New Tab" button
    /// makes — a child of whichever tab is currently focused, staying put
    /// rather than auto-focusing (see `spawn_new_tab`'s own doc comment).
    fn new_tab_shortcut(&mut self) {
        let home_page = self.settings.home_page.clone();
        let parent = self.focused.clone();
        let group = self.group_of(&parent);
        self.spawn_new_tab(parent, group, &home_page);
    }

    /// `Ctrl+Shift+T` / headless `REOPENTAB` — respawns the most recently
    /// closed tab (the end of `self.closed_tabs`; a no-op if the list is
    /// empty). Its original parent respawns it as a new root instead if
    /// that parent was itself closed/no longer exists — `spawn_new_tab_
    /// with_kind` already treats an unknown parent id this way for
    /// graph-position purposes, and `process_manager::spawn_tab` doesn't
    /// require a real parent tab to exist either.
    fn reopen_closed_tab_shortcut(&mut self) {
        let Some(closed) = self.closed_tabs.pop() else { return };
        self.respawn_closed_tab(closed);
    }

    /// Shared by `reopen_closed_tab_shortcut` (always the last entry) and
    /// the "Recently Closed" toolbar dropdown (any specific entry the user
    /// picked, already removed from `self.closed_tabs` by the caller).
    fn respawn_closed_tab(&mut self, closed: ClosedTab) {
        // A stale/dissolved `closed.group` is sanitized centrally in
        // `handle_tab_spawn_result` once this spawn actually completes.
        self.spawn_new_tab_with_kind(closed.parent, closed.group, &closed.url, closed.kind, SpawnPurpose::New);
    }

    /// `Ctrl+W` / headless `CLOSETAB` — closes the focused tab, a no-op if
    /// none is focused (Graph view with nothing selected).
    fn close_focused_tab_shortcut(&mut self) {
        if let Some(id) = self.focused.clone() {
            self.close_tab(&id);
        }
    }

    /// `Ctrl+D` / headless `DUPLICATETAB` — keyboard parity for the tab
    /// context menu's own "Duplicate" entry, applied to the focused tab. A
    /// no-op if none is focused (Graph view with nothing selected).
    fn duplicate_focused_tab_shortcut(&mut self) {
        if let Some(id) = self.focused.clone() {
            self.duplicate_tab(&id);
        }
    }

    /// `Ctrl+Shift+W` / headless `CLOSEALLLEVEL` — closes every tab
    /// `nodes_at_current_level(Tab)` returns (top level: every ungrouped tab,
    /// leaving groups and their members untouched; inside a group: every
    /// member, same effect as that group's own "Close Group" context-menu
    /// entry, just reachable without opening it). Each closes through the
    /// normal `close_tab` path, same "just that action run once per tab"
    /// shape `close_all_except`/`close_group` already have.
    fn close_all_at_current_level(&mut self) {
        for id in self.nodes_at_current_level(ItemNodeKind::Tab) {
            self.close_tab(&id);
        }
    }

    /// Toggles `id`'s mute state and sends the real `SetMuted` — shared by
    /// Tab view's "Muted" toolbar button and Graph view's own audio-dot
    /// click (see `TabPayload::muted`'s doc comment for the WebView2/WebKit-
    /// only caveat).
    fn toggle_muted(&mut self, id: &str) {
        if let Some(node) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) {
            node.muted = !node.muted;
            let muted = node.muted;
            node.tab.send(&HostToEngine::SetMuted { muted });
        }
    }

    /// "Print to PDF": routes through the exact same GPU screenshot
    /// capture `pending_screenshot` already drives (a temp PNG, cleaned up
    /// once converted), rather than teaching `gpu::GpuState` anything
    /// about PDF -- `RedrawRequested`'s handling of `RenderOutcome::
    /// screenshot_result` checks `pending_print_to_pdf` once that capture
    /// actually completes and does the PNG-to-PDF conversion there. Called
    /// by both the real toolbar button (after a real file-picker chooses
    /// `pdf_path`) and the headless `PRINTTOPDF` debug command (which
    /// supplies `pdf_path` directly, bypassing the picker).
    fn start_print_to_pdf(&mut self, pdf_path: String) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let temp_png = std::env::temp_dir().join(format!("manifold-print-{timestamp}.png"));
        self.pending_screenshot = Some(temp_png.to_string_lossy().into_owned());
        self.pending_print_to_pdf = Some(pdf_path);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Tab view's "Bookmark" toggle: adds `id`'s current title/url if not
    /// already bookmarked (matched by url), removes it if it is. Persists
    /// immediately, same "no separate Save step" posture every other
    /// instant-apply control in this chrome already has.
    fn toggle_bookmark(&mut self, id: &str) {
        let Some(node) = self.nodes.get(id).and_then(|n| n.as_tab()) else { return };
        let url = node.url.clone();
        if let Some(index) = self.bookmarks.iter().position(|b| b.url == url) {
            self.bookmarks.remove(index);
        } else {
            let title = if node.title.is_empty() { url.clone() } else { node.title.clone() };
            self.bookmarks.push(bookmarks::Bookmark { title, url, folder: None });
        }
        bookmarks::save(&self.bookmarks);
    }

    /// Tab view's "Bookmarks (N)" dropdown -- every saved bookmark, click
    /// one to navigate the focused tab there, "x" to remove it. Grouped by
    /// `Bookmark::folder` (unfoldered entries first, then each named
    /// folder alphabetically) -- a single level, not a real tree, same
    /// scope limit `Bookmark::folder`'s own doc comment states. Same
    /// Area/Frame pattern as `build_recently_closed_popup`.
    fn build_bookmarks_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut navigate_url: Option<String> = None;
        let mut remove_index: Option<usize> = None;
        let mut folder_rename_start: Option<(String, String)> = None;
        let mut folder_rename_commit: Option<String> = None;
        let mut folder_rename_cancel = false;
        let area_rect =
            build_list_popup_frame(ui.ctx(), "bookmarks_popup", anchor_rect, 220.0, 320.0, fade, self.bookmarks_open, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.bookmarks_filter)
                    .desired_width(f32::INFINITY)
                    .hint_text("Filter..."),
            );
            let matches: Vec<(usize, &bookmarks::Bookmark)> = self
                .bookmarks
                .iter()
                .enumerate()
                .filter(|(_, b)| filter_matches(&self.bookmarks_filter, &[&b.title, &b.url]))
                .collect();
            let mut folders: Vec<&str> = matches
                .iter()
                .filter_map(|(_, b)| b.folder.as_deref())
                .collect();
            folders.sort_unstable();
            folders.dedup();
            // `None` (unfoldered) first, then each named folder
            // alphabetically -- `folders` is already sorted.
            let groups: Vec<Option<&str>> =
                std::iter::once(None).chain(folders.into_iter().map(Some)).collect();
            if matches.is_empty() {
                ui.label(egui::RichText::new("No bookmarks yet.").weak());
            }
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for folder in groups {
                    let rows: Vec<&(usize, &bookmarks::Bookmark)> = matches
                        .iter()
                        .filter(|(_, b)| b.folder.as_deref() == folder)
                        .collect();
                    if rows.is_empty() {
                        continue;
                    }
                    if let Some(name) = folder {
                        ui.label(egui::RichText::new(name).small().strong().color(TEXT_PRIMARY));
                    }
                    for (index, bookmark) in rows {
                        let (index, bookmark) = (*index, *bookmark);
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Label::new(&bookmark.title)
                                        .truncate()
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                navigate_url = Some(bookmark.url.clone());
                            }
                            if ui.small_button(egui_phosphor::light::X).clicked() {
                                remove_index = Some(index);
                            }
                        });
                        let is_renaming = self
                            .renaming_bookmark_folder
                            .as_ref()
                            .is_some_and(|(url, _)| url == &bookmark.url);
                        if is_renaming {
                            if let Some((_, draft)) = &mut self.renaming_bookmark_folder {
                                let response = ui.add(
                                    egui::TextEdit::singleline(draft)
                                        .desired_width(140.0)
                                        .hint_text("Folder (blank for none)")
                                        .font(egui::TextStyle::Small),
                                );
                                if std::mem::take(&mut self.pending_focus_bookmark_folder) {
                                    response.request_focus();
                                }
                                if response.lost_focus() {
                                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        folder_rename_cancel = true;
                                    } else {
                                        folder_rename_commit = Some(draft.clone());
                                    }
                                }
                            }
                        } else {
                            let label = bookmark.folder.as_deref().unwrap_or("No folder");
                            if ui
                                .add(
                                    egui::Label::new(egui::RichText::new(label).small().weak())
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_text("Click to set folder")
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                folder_rename_start = Some((
                                    bookmark.url.clone(),
                                    bookmark.folder.clone().unwrap_or_default(),
                                ));
                            }
                        }
                        ui.add_space(2.0);
                    }
                }
            });
        });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some((url, draft)) = folder_rename_start {
            self.renaming_bookmark_folder = Some((url, draft));
            self.pending_focus_bookmark_folder = true;
        }
        if folder_rename_cancel {
            self.renaming_bookmark_folder = None;
        }
        if let Some(draft) = folder_rename_commit {
            if let Some((url, _)) = self.renaming_bookmark_folder.take() {
                let trimmed = draft.trim();
                if let Some(bookmark) = self.bookmarks.iter_mut().find(|b| b.url == url) {
                    bookmark.folder = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
                    bookmarks::save(&self.bookmarks);
                }
            }
        }
        if let Some(url) = navigate_url {
            if let Some(id) = self.focused.clone() {
                self.navigate_tab(&id, url);
            }
            close = true;
        }
        if let Some(index) = remove_index {
            self.bookmarks.remove(index);
            bookmarks::save(&self.bookmarks);
        }
        if clicked_outside && self.renaming_bookmark_folder.is_none() {
            close = true;
        }
        // Not `|| self.bookmarks.is_empty()` any more -- the Settings page's
        // own "Lists" section can open this popup with an empty list, and
        // that used to close it again the very next frame.
        if close {
            self.bookmarks_open = false;
            self.renaming_bookmark_folder = None;
        }
    }

    /// Tab view's "History (N)" dropdown -- every entry in `self.history`,
    /// most-recently-visited first, click a title to navigate the focused
    /// tab there, "Clear" to empty the whole list. Same Area/Frame pattern
    /// as `build_bookmarks_popup`/`build_recently_closed_popup`, scrollable
    /// since history can grow much longer than either of those.
    fn build_history_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut navigate_url: Option<String> = None;
        let mut clear_requested = false;
        let area_rect =
            build_list_popup_frame(ui.ctx(), "history_popup", anchor_rect, 220.0, 320.0, fade, self.history_open, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.history_filter)
                    .desired_width(f32::INFINITY)
                    .hint_text("Filter..."),
            );
            if self.history.is_empty() {
                ui.label(egui::RichText::new("No history yet.").weak());
            }
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for entry in self.history.iter().rev() {
                    let label = if entry.title.is_empty() { &entry.url } else { &entry.title };
                    if !filter_matches(&self.history_filter, &[&entry.title, &entry.url]) {
                        continue;
                    }
                    if ui.add(egui::Label::new(label).truncate().sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        navigate_url = Some(entry.url.clone());
                    }
                }
            });
            ui.separator();
            if ui.button("Clear").clicked() {
                clear_requested = true;
            }
        });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some(url) = navigate_url {
            if let Some(id) = self.focused.clone() {
                self.navigate_tab(&id, url);
            }
            close = true;
        }
        if clear_requested {
            self.history.clear();
            close = true;
        }
        if clicked_outside {
            close = true;
        }
        // Not `|| self.history.is_empty()` any more -- see the matching
        // comment in `build_bookmarks_popup`.
        if close {
            self.history_open = false;
        }
    }

    /// Tab view's "Downloads (N)" dropdown -- every entry in
    /// `self.downloads` (the same data the graph view's own download
    /// nodes/context menu show, see `App::node_menu`), most-
    /// recently-downloaded first, click a filename to open its containing
    /// folder (same as `reveal_node`), "x" to remove it from the list
    /// (same as `close_node_and_save`). Exists so a completed download can
    /// be found without first switching to Graph view.
    fn build_downloads_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut reveal_id: Option<String> = None;
        let mut remove_id: Option<String> = None;
        let area_rect =
            build_list_popup_frame(ui.ctx(), "downloads_popup", anchor_rect, 220.0, 320.0, fade, self.downloads_open, |ui| {
            if !self.node_order.iter().any(|id| self.nodes.get(id).is_some_and(|n| n.as_download().is_some())) {
                ui.label(egui::RichText::new("No downloads yet.").weak());
            }
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for id in self.node_order.iter().rev() {
                    let Some(node) = self.nodes.get(id).and_then(|n| n.as_download()) else { continue };
                    ui.horizontal(|ui| {
                        let filename = download_filename(&node.path);
                        if ui
                            .add(egui::Label::new(filename).truncate().sense(egui::Sense::click()))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            reveal_id = Some(id.clone());
                        }
                        if ui.small_button(egui_phosphor::light::X).clicked() {
                            remove_id = Some(id.clone());
                        }
                    });
                    // In-progress downloads get a live progress row below
                    // the filename; a completed one is left exactly as
                    // before this feature -- just the filename row above.
                    if !node.completed {
                        match node.total_bytes {
                            Some(total) if total > 0 => {
                                let fraction = (node.bytes_received as f32 / total as f32).clamp(0.0, 1.0);
                                ui.add(
                                    egui::ProgressBar::new(fraction)
                                        .text(format!(
                                            "{} / {}",
                                            human_bytes(node.bytes_received),
                                            human_bytes(total)
                                        ))
                                        .desired_width(f32::INFINITY),
                                );
                            }
                            // No `Content-Length` to size a fraction
                            // against (chunked transfer) -- a byte count
                            // with no bar/percentage is honest; a bar stuck
                            // at some arbitrary fraction (or a fake
                            // "indeterminate" scroll egui has no built-in
                            // support for) would just be a guess.
                            _ => {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "Downloading… {}",
                                            human_bytes(node.bytes_received)
                                        ))
                                        .small()
                                        .weak(),
                                    )
                                    .wrap(),
                                );
                            }
                        }
                    }
                }
            });
        });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some(id) = reveal_id {
            self.reveal_node(&id);
            close = true;
        }
        if let Some(id) = remove_id {
            self.close_node_and_save(&id);
        }
        if clicked_outside {
            close = true;
        }
        // Not `|| self.nodes.is_empty()` any more: that counted every graph
        // node (tabs, notes, images, ...), not just downloads, so it almost
        // never actually fired -- and per the same fix as
        // `build_bookmarks_popup`/`build_history_popup`, an empty list is no
        // longer a reason to self-close at all now that the "Lists" menu can
        // open this popup while empty.
        if close {
            self.downloads_open = false;
        }
    }

    /// Tab view's "Tab actions" dropdown -- "Mute Others", "Close Others",
    /// and the "Pin All"/"Unpin All" toggle in one popup, folded from three
    /// always-visible toolbar buttons (a later session pass flagged the
    /// toolbar as cramped -- confirmed live, text touching the window edge
    /// even at 1300pt wide). Same anchored Area/Frame pattern
    /// `build_downloads_popup` already uses.
    fn build_tab_actions_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, focused_id: &str, fade: f32) {
        let mut close = false;
        let mut mute_others = false;
        let mut close_others = false;
        let mut toggle_pin_all = false;
        let tab_pin_states: Vec<bool> =
            self.nodes.values().filter_map(|n| n.as_tab()).map(|t| t.pinned).collect();
        let all_pinned = !tab_pin_states.is_empty() && tab_pin_states.iter().all(|&p| p);
        let area = egui::Area::new(egui::Id::new("tab_actions_popup"))
            .fixed_pos(anchor_rect.left_bottom())
            .order(egui::Order::Foreground)
            .interactable(self.tab_actions_open)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                popup_frame(ui)
                    .show(ui, |ui| {
                        ui.set_min_width(140.0);
                        if ui.button("Mute Others").on_hover_text("Mute every other open tab").clicked() {
                            mute_others = true;
                            close = true;
                        }
                        if ui.button("Close Others").on_hover_text("Close every other open tab").clicked()
                        {
                            close_others = true;
                            close = true;
                        }
                        let label = if all_pinned { "Unpin All" } else { "Pin All" };
                        let hover = if all_pinned { "Unpin every open tab" } else { "Pin every open tab" };
                        if ui.button(label).on_hover_text(hover).clicked() {
                            toggle_pin_all = true;
                            close = true;
                        }
                    });
            });

        let clicked_outside = clicked_outside_popup(ui, area.response.rect, Some(anchor_rect));

        if mute_others {
            self.mute_all_except(focused_id);
        }
        if close_others {
            self.close_all_except(focused_id);
        }
        if toggle_pin_all {
            self.set_all_pinned(!all_pinned);
        }
        if clicked_outside {
            close = true;
        }
        if close {
            self.tab_actions_open = false;
        }
    }

    /// Tab view's "Mute Others" button: mutes every open tab except
    /// `keep_id`, regardless of any already-muted state, without touching
    /// `keep_id`'s own mute state either way. A one-shot action, not a
    /// toggle -- a second click would just mute everything again, a no-op
    /// for anything already muted.
    fn mute_all_except(&mut self, keep_id: &str) {
        let other_ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|(id, n)| id.as_str() != keep_id && n.as_tab().is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in other_ids {
            if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                if !node.muted {
                    node.muted = true;
                    node.tab.send(&HostToEngine::SetMuted { muted: true });
                }
            }
        }
    }

    /// Tab view's "Close Others" button: closes every open tab except
    /// `keep_id` through the normal `close_tab` path (teardown, reparenting,
    /// the `closed_tabs` stack all still apply -- this is just that same
    /// action run once per other tab, not a bulk shortcut around it).
    fn close_all_except(&mut self, keep_id: &str) {
        let other_ids: Vec<String> = self
            .node_order
            .iter()
            .filter(|id| id.as_str() != keep_id && self.nodes.get(id.as_str()).is_some_and(|n| n.as_tab().is_some()))
            .cloned()
            .collect();
        for id in other_ids {
            self.close_tab(&id);
        }
    }

    /// Tab view's "Pin All"/"Unpin All" buttons: sets every open tab's
    /// `pinned` flag to `pin`, regardless of its current state. One-shot
    /// actions, not toggles -- same "just run the single-tab effect once
    /// per tab" shape as `mute_all_except`, minus the "except" since pinning
    /// has no natural tab to exclude the way muting excludes the one
    /// they're presumably still listening to.
    fn set_all_pinned(&mut self, pin: bool) {
        for node in self.nodes.values_mut().filter_map(|n| n.as_tab_mut()) {
            node.pinned = pin;
        }
    }

    /// `Ctrl+L` / headless `FOCUSADDRESS` — only meaningful in Tab view,
    /// since Graph view has no address bar to focus. Sets a pending flag
    /// rather than `address_bar_focused` directly: the address bar's
    /// `TextEdit` doesn't exist as a widget until `build_tab_view_ui` builds
    /// it, so actually moving real egui focus onto it needs to happen there,
    /// via `response.request_focus()` — see `pending_focus_address_bar`'s
    /// own doc comment.
    fn focus_address_bar_shortcut(&mut self) {
        if self.showing_tab_view() {
            self.pending_focus_address_bar = true;
            // `request_focus()` only takes hold starting the *next* frame
            // after it's called (immediate-mode: the widget this frame
            // already painted before `build_tab_view_ui` gets to call it) — a
            // real keypress always gets a following frame for free, but
            // this command can otherwise land with nothing further
            // scheduled to redraw (Tab view, unlike Graph view's physics,
            // has no continuous redraw loop of its own), leaving the
            // pending flag applied-but-invisible until some unrelated
            // event happens to trigger another frame.
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// `Ctrl+F` / headless `OPENFINDBAR` — only meaningful in Tab view, same
    /// as `Ctrl+L`. Opening with existing text immediately re-searches for
    /// it (matches a real browser's find bar remembering the last term).
    fn open_find_bar_shortcut(&mut self) {
        if !self.showing_tab_view() {
            return;
        }
        self.find_bar_open = true;
        self.pending_focus_find_bar = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if !self.find_text.is_empty() {
            self.send_find_start();
        }
    }

    /// Sends `FindStart` for `self.find_text` to the focused tab, or
    /// `FindStop` if it's empty (clearing any existing highlight) — shared
    /// by the find bar's own text-changed handling and `open_find_bar_shortcut`
    /// re-opening with a remembered term.
    fn send_find_start(&mut self) {
        self.find_result = None;
        // Tab view has no continuous redraw loop of its own (unlike Graph
        // view's physics) — without this, a headless `FINDTEXT` (or a
        // remembered-term re-open) can land with nothing scheduled to ever
        // paint the bar's updated count, the same gap `focus_address_bar_
        // shortcut` already has its own note about.
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        if self.find_text.is_empty() {
            node.tab.send(&HostToEngine::FindStop);
        } else {
            node.tab.send(&HostToEngine::FindStart {
                text: self.find_text.clone(),
                match_case: self.find_match_case,
            });
        }
    }

    /// Closes the find bar: sends `FindStop` (clears highlighted matches)
    /// and resets its text/result — called on `Escape`, its own close
    /// button, headless `CLOSEFINDBAR`, and whenever focus leaves the tab
    /// it was searching (tab switch/close), so a stale search never keeps
    /// running silently in the background.
    fn close_find_bar(&mut self) {
        if !self.find_bar_open {
            return;
        }
        self.find_bar_open = false;
        self.find_result = None;
        self.find_match_case = false;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if let Some(id) = self.focused.clone() {
            if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                node.tab.send(&HostToEngine::FindStop);
            }
        }
    }

    /// `Enter`/`Shift+Enter` in the find bar, or its Prev/Next buttons.
    fn send_find_navigate(&mut self, forward: bool) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        node.tab.send(&if forward { HostToEngine::FindNext } else { HostToEngine::FindPrevious });
    }

    /// `Ctrl+F`'s floating find bar: a text field, prev/next, a match count,
    /// and a close button. Anchored top-right of the content area in
    /// screen space rather than off some toolbar button's rect — unlike
    /// Settings, there's no persistent visible button to anchor from (this
    /// only ever opens via `Ctrl+F`/headless `OPENFINDBAR`).
    fn build_find_bar(&mut self, ui: &mut egui::Ui) {
        let mut close = false;
        let mut text_changed = false;
        let mut go_next = false;
        let mut go_prev = false;
        egui::Area::new(egui::Id::new("find_bar"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, CHROME_HEIGHT_POINTS + 6.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                popup_frame(ui).inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.find_text)
                                    .desired_width(160.0)
                                    .hint_text("Find in page"),
                            );
                            // Same "needs a frame to exist as a widget"
                            // reason as `pending_focus_address_bar`.
                            if std::mem::take(&mut self.pending_focus_find_bar) {
                                response.request_focus();
                            }
                            if response.changed() {
                                text_changed = true;
                            }
                            if ui
                                .toggle_value(&mut self.find_match_case, "Aa")
                                .on_hover_text("Match case")
                                .changed()
                            {
                                text_changed = true;
                            }
                            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                if ui.input(|i| i.modifiers.shift) {
                                    go_prev = true;
                                } else {
                                    go_next = true;
                                }
                            }

                            match self.find_result {
                                Some((0, _)) => {
                                    ui.label("No results");
                                }
                                // WebKit never reports `active_index` (see
                                // `EngineToHost::FindResult`'s doc comment)
                                // — show just the count, not "0 of N".
                                Some((count, 0)) => {
                                    ui.label(format!("{count}"));
                                }
                                Some((count, active)) => {
                                    ui.label(format!("{active}/{count}"));
                                }
                                None => {}
                            }

                            // Prev/Next stay plain ASCII labels, not Unicode
                            // arrow glyphs — confirmed live that `▲`/`▼`
                            // render as empty tofu boxes with this app's own
                            // font (unlike `◀`/`▶`/`⚙` elsewhere in this same
                            // toolbar, which do render). Not migrated to
                            // `egui_phosphor` alongside the close button
                            // below -- out of scope for the cosmetic-only
                            // "x"/"X" pass that touched everything else here.
                            let has_matches = matches!(self.find_result, Some((count, _)) if count > 0);
                            if ui.add_enabled(has_matches, egui::Button::new("Prev")).clicked() {
                                go_prev = true;
                            }
                            if ui.add_enabled(has_matches, egui::Button::new("Next")).clicked() {
                                go_next = true;
                            }
                            // Was a literal capital "X" -- confirmed live
                            // that `✕` tofu-boxed here (same finding the
                            // comment above already documents), so this one
                            // used the ASCII fallback rather than a Unicode
                            // glyph. Now the real `X` icon instead, once the
                            // icon font was already in place for the other
                            // close buttons in this app's chrome.
                            if ui.button(egui_phosphor::light::X).clicked() {
                                close = true;
                            }
                        });
                    });
            });

        if text_changed {
            self.send_find_start();
        }
        if go_next {
            self.send_find_navigate(true);
        }
        if go_prev {
            self.send_find_navigate(false);
        }
        if close {
            self.close_find_bar();
        }
    }

    /// `self.node_order`, filtered by a case-insensitive substring match
    /// against title-or-url — factored out so the quick-switcher's render
    /// function and its Enter-key jump always agree on the exact same list
    /// (computed once per frame, not separately in each place that needs
    /// it).
    fn quick_switcher_matches(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        self.node_order
            .iter()
            .filter(|id| {
                self.nodes.get(id.as_str()).and_then(|n| n.as_tab()).is_some_and(|n| {
                    query.is_empty()
                        || n.title.to_lowercase().contains(&query)
                        || n.url.to_lowercase().contains(&query)
                })
            })
            .cloned()
            .collect()
    }

    /// Moves the quick-switcher's selection by `delta` (+1/-1), clamped to
    /// the current match list — shared by the real ArrowUp/ArrowDown
    /// handling in `build_quick_switcher` and the headless `QUICKSWITCHUP`/
    /// `QUICKSWITCHDOWN` debug commands (see `debug_control`'s own doc
    /// comment on why a real `KeyEvent` can't be synthesized).
    fn quick_switcher_move(&mut self, delta: i32) {
        let Some(query) = self.quick_switcher.as_ref().map(|s| s.query.clone()) else { return };
        let len = self.quick_switcher_matches(&query).len();
        let Some(switcher) = &mut self.quick_switcher else { return };
        if len == 0 {
            switcher.selected = 0;
        } else if delta > 0 {
            switcher.selected = (switcher.selected + 1).min(len - 1);
        } else {
            switcher.selected = switcher.selected.saturating_sub(1);
        }
    }

    /// Jumps to whichever match is currently selected and closes the
    /// switcher — shared by the real Enter handling in
    /// `build_quick_switcher` and headless `QUICKSWITCHENTER`. See
    /// `build_quick_switcher`'s own doc comment for why this calls
    /// `focus_tab` directly rather than the graph-node expand animation.
    fn quick_switcher_confirm(&mut self) {
        let Some(switcher) = &self.quick_switcher else { return };
        let matches = self.quick_switcher_matches(&switcher.query);
        if let Some(id) = matches.get(switcher.selected).cloned() {
            let group = self.nodes.get(&id).and_then(|n| n.group.clone());
            self.current_group = group;
            self.focus_tab(&id);
        }
        self.quick_switcher = None;
    }

    /// `Ctrl+K`'s tab quick-switcher: a centered search box plus a
    /// scrollable list of matches (see `quick_switcher_matches`), arrow
    /// keys move the selection, Enter jumps to it, Escape closes. Called
    /// from `build_ui` itself rather than `build_item_ui`/`build_graph_ui`
    /// so it works the same from either view.
    fn build_quick_switcher(&mut self, ui: &mut egui::Ui) {
        let current = self.quick_switcher.clone();
        let is_live = current.is_some();
        let Some((switcher, fade)) = fade_popup_state(
            ui.ctx(),
            egui::Id::new("quick_switcher_fade"),
            &current,
            &mut self.quick_switcher_cache,
        ) else {
            return;
        };

        if is_live {
            // Read once, outside the `Area`, so arrow/Enter/Escape work
            // regardless of whether the search field inside currently has
            // egui focus — same reasoning the address-bar dropdown's own
            // arrow-key handling already documents. Gated on `is_live`:
            // once actually closing (rendering the frozen `cache` snapshot
            // below instead), there's no live switcher left to move a
            // selection in or confirm.
            let (arrow_down, arrow_up, enter, escape) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if arrow_down {
                self.quick_switcher_move(1);
            }
            if arrow_up {
                self.quick_switcher_move(-1);
            }
            if escape {
                self.quick_switcher = None;
            }
            if enter {
                self.quick_switcher_confirm();
            }
        }

        let query = switcher.query.clone();
        let selected_index = switcher.selected;
        let matches = self.quick_switcher_matches(&query);
        let mut jump_to: Option<String> = None;

        egui::Area::new(egui::Id::new("quick_switcher"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .order(egui::Order::Foreground)
            .interactable(is_live)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                dialog_frame(ui)
                    .show(ui, |ui| {
                        ui.set_width(360.0);
                        if is_live {
                            if let Some(switcher) = &mut self.quick_switcher {
                                let response = ui.add(
                                    egui::TextEdit::singleline(&mut switcher.query)
                                        .desired_width(f32::INFINITY)
                                        .hint_text("Search open tabs..."),
                                );
                                if std::mem::take(&mut self.pending_focus_quick_switcher) {
                                    response.request_focus();
                                }
                            }
                        } else {
                            // Closing: a live `TextEdit` needs `&mut self.
                            // quick_switcher`, which no longer exists once
                            // it's actually `None` -- the fade past this
                            // point is purely a visual echo of the last
                            // real state, not something still editable.
                            let placeholder = if query.is_empty() { "Search open tabs..." } else { &query };
                            ui.label(placeholder);
                        }
                        ui.separator();
                        if matches.is_empty() {
                            ui.label("No matching tabs");
                        }
                        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                            for (i, id) in matches.iter().enumerate() {
                                let Some(node) = self.nodes.get(id).and_then(|n| n.as_tab()) else { continue };
                                let label = if node.title.is_empty() {
                                    node.url.clone()
                                } else {
                                    node.title.clone()
                                };
                                if is_live {
                                    if ui.selectable_label(i == selected_index, label).clicked() {
                                        jump_to = Some(id.clone());
                                    }
                                } else {
                                    // Non-interactive while closing --
                                    // clicking a row already fading out
                                    // would be a confusing "did that even
                                    // do anything" moment.
                                    ui.add_enabled(
                                        false,
                                        egui::Button::selectable(i == selected_index, label),
                                    );
                                }
                            }
                        });
                    });
            });

        if let Some(id) = jump_to {
            let group = self.nodes.get(&id).and_then(|n| n.group.clone());
            self.current_group = group;
            self.focus_tab(&id);
            self.quick_switcher = None;
        }
    }

    /// `F1` / headless `TOGGLESHORTCUTSHELP` -- a centered reference popup
    /// listing every global keyboard shortcut `handle_global_shortcuts`
    /// handles, so a shortcut added to that function doesn't just live in a
    /// code comment nobody using the app ever sees. Static content (no
    /// state to stage/commit), so unlike the quick-switcher this is just a
    /// plain `Area`/`Frame` with a scrollable list and a Close button.
    fn build_shortcuts_help(&mut self, ui: &mut egui::Ui, anchor_rect: Option<egui::Rect>) {
        if !self.shortcuts_help_open {
            return;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.shortcuts_help_open = false;
            return;
        }

        const SHORTCUTS: &[(&str, &str)] = &[
            ("Ctrl+T", "New tab"),
            ("Ctrl+W", "Close focused tab"),
            ("Ctrl+Shift+W", "Close all tabs at the current level"),
            ("Ctrl+Shift+T", "Reopen last closed tab"),
            ("Ctrl+D", "Duplicate focused tab"),
            ("Ctrl+Tab / Ctrl+Shift+Tab", "Next / previous tab"),
            ("Ctrl+K", "Quick switcher (jump to any open tab)"),
            ("Ctrl+G", "Toggle Tab / Graph view"),
            ("Ctrl+Shift+G", "New group"),
            ("F2", "Rename the group currently being viewed"),
            ("Ctrl+P", "Pin / unpin focused tab"),
            ("Ctrl+L", "Focus the address bar"),
            ("Ctrl+F", "Find in page"),
            ("Ctrl+R / F5", "Reload focused tab"),
            ("Ctrl+= / Ctrl+-", "Zoom in / out"),
            ("Ctrl+0", "Reset zoom"),
            ("F12", "Open devtools"),
            ("F1", "Toggle this shortcut reference"),
        ];

        let area = egui::Area::new(egui::Id::new("shortcuts_help"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui)
                    .show(ui, |ui| {
                        ui.set_width(360.0);
                        ui.label(heading_text("Keyboard Shortcuts"));
                        ui.separator();
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            egui::Grid::new("shortcuts_help_grid")
                                .num_columns(2)
                                .spacing(egui::vec2(16.0, 6.0))
                                .show(ui, |ui| {
                                    for (keys, desc) in SHORTCUTS {
                                        ui.monospace(*keys);
                                        ui.label(*desc);
                                        ui.end_row();
                                    }
                                });
                        });
                        ui.separator();
                        if ui.button("Close").clicked() {
                            self.shortcuts_help_open = false;
                        }
                    });
            });

        if clicked_outside_popup(ui, area.response.rect, anchor_rect) {
            self.shortcuts_help_open = false;
        }
    }

    /// The minimal right-click context menu: one "Open in new tab" item at
    /// `self.context_menu`'s position, if a menu is currently showing.
    /// Dismissed by clicking the item (and acting on it), a click anywhere
    /// outside the menu's own rect, or Escape (see the Escape-priority
    /// chain in `build_tab_view_ui`).
    fn build_context_menu(&mut self, ui: &mut egui::Ui) {
        let current = self.context_menu.clone();
        let Some((menu, fade)) = fade_popup_state(
            ui.ctx(),
            egui::Id::new("context_menu_fade"),
            &current,
            &mut self.context_menu_cache,
        ) else {
            return;
        };
        let mut open_tab = false;
        let mut close = false;

        let area = egui::Area::new(egui::Id::new("context_menu"))
            .fixed_pos(egui::pos2(menu.x, menu.y))
            .order(egui::Order::Foreground)
            .interactable(current.is_some())
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                popup_frame(ui)
                    .show(ui, |ui| {
                        if ui.button("Open in new tab").clicked() {
                            open_tab = true;
                            close = true;
                        }
                    });
            });

        let clicked_outside = clicked_outside_popup(ui, area.response.rect, None);
        if clicked_outside {
            close = true;
        }

        if open_tab {
            let parent = self.focused.clone();
            let group = self.group_of(&parent);
            self.spawn_new_tab(parent, group, &menu.href);
        }
        if close {
            self.context_menu = None;
        }
    }

    /// Closes every member of a group via `close_node` (a tab
    /// through the normal `close_tab` path -- teardown, reparenting,
    /// thumbnail cleanup all still apply, same as `close_all_except`/
    /// `close_selected`; a nested subgroup recurses into this same
    /// function; a Note/Image/Pdf/Json/Download member is destroyed too,
    /// via the same cleanup its own "Delete" context-menu action already
    /// applies -- see `close_node`'s own doc comment for why this
    /// exists), then dissolves the now-empty group itself. Snapshots
    /// the group into `closed_groups` *before* touching anything, via the
    /// same `export_group` "Export Group..." itself uses, so "Reopen Closed
    /// Group" can recreate it -- skipped when the export has no tabs (a
    /// group containing only Downloads/Notes/Images/Pdfs/Jsons/nested-empty-groups,
    /// e.g. an "Expand as Graph" subtree), since reopening that snapshot
    /// would only recreate an empty, useless group and would otherwise
    /// flood the 5-slot stack and evict genuinely useful entries -- the
    /// individual `ClosedTab` entries
    /// `close_tab` pushes along the way are then thrown away (`closed_tabs`
    /// truncated back to its pre-loop length) rather than left behind
    /// alongside the group-level snapshot, which would otherwise let a user
    /// reopen the same tabs twice (once via the group, once via the plain
    /// tab list) and end up with duplicates.
    /// `closed_nodes` (Download/Note/Image/Pdf/Json members, via `close_node`)
    /// needs no equivalent truncation -- `export_group`'s own snapshot only
    /// ever captures tab members, never those five kinds, so there's no
    /// analogous group-level copy for an individual `ClosedNode` entry to
    /// duplicate.
    fn close_group(&mut self, id: &str) {
        if let Some(export) = self.export_group(id) {
            if !export.tabs.is_empty() {
                self.closed_groups.push(export);
                if self.closed_groups.len() > MAX_CLOSED_GROUPS {
                    self.closed_groups.remove(0);
                }
            }
        }
        let closed_tabs_before = self.closed_tabs.len();
        let member_ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.group.as_deref() == Some(id))
            .map(|(tid, _)| tid.clone())
            .collect();
        for tid in member_ids {
            self.close_node(&tid);
        }
        self.closed_tabs.truncate(closed_tabs_before);
        self.dissolve_group(id);
        // `close_node` doesn't save, and there's no save on exit -- without
        // this, a group whose members were only Downloads/Notes/Images/Pdfs/Jsons
        // (no Tab member whose `close_tab` happens to save) would have
        // those members reappear after a restart unless some unrelated
        // action saved first.
        self.save_session();
    }

    /// Closes one node, applying its kind's full cleanup: a Group recurses
    /// (`close_group`), a Tab goes through `close_tab` (feeding "Recently
    /// Closed" tabs), and a Download/Note/Image/Pdf/Json records whatever
    /// `closed_node_for` says (feeding "Recently Closed" nodes), frees its
    /// GPU texture if it's an Image/Pdf, clears `json_viewer_node` if it
    /// was showing this node, then `remove_node`s it. The single path
    /// behind a card's hover ✕, its context-menu Delete/"Remove from
    /// List", the Downloads popup's Remove, and `close_group`'s per-member
    /// loop -- which is why it doesn't save: callers decide -- the single-node
    /// UI paths go through `close_node_and_save`, and `close_group` saves
    /// once after its whole loop.
    /// (Before this was shared, `close_tab`'s own no-op-on-non-tab guard
    /// left every non-tab, non-group member of a closed group merely
    /// reparented by `dissolve_group`, never actually destroyed -- a gap
    /// confirmed while building "Expand as Graph", whose whole output is
    /// `JsonPayload` leaves nested inside Groups.)
    fn close_node(&mut self, id: &str) {
        let Some((kind, closed)) =
            self.nodes.get(id).map(|node| (item_node_kind(&node.payload), closed_node_for(&node.payload)))
        else {
            return;
        };
        match kind {
            ItemNodeKind::Group => self.close_group(id),
            ItemNodeKind::Tab => self.close_tab(id),
            ItemNodeKind::Download | ItemNodeKind::Note | ItemNodeKind::Image | ItemNodeKind::Pdf | ItemNodeKind::Json => {
                if let Some(closed) = closed {
                    self.record_closed_node(closed);
                }
                if matches!(kind, ItemNodeKind::Image | ItemNodeKind::Pdf) {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.free_image(id);
                    }
                }
                if self.json_viewer_node.as_deref() == Some(id) {
                    self.json_viewer_node = None;
                }
                self.remove_node(id);
            }
        }
    }

    /// `close_node` for a single user action (hover ✕, context-menu
    /// Delete/Close, Downloads popup Remove), then persists the session --
    /// except for a Download, which isn't part of the saved session at
    /// all, and a tab or group, whose `close_tab`/`close_group` already
    /// save.
    fn close_node_and_save(&mut self, id: &str) {
        let persisted = self.nodes.get(id).is_some_and(|node| {
            matches!(
                item_node_kind(&node.payload),
                ItemNodeKind::Note | ItemNodeKind::Image | ItemNodeKind::Pdf | ItemNodeKind::Json
            )
        });
        self.close_node(id);
        if persisted {
            self.save_session();
        }
    }

    /// Builds an exportable snapshot of `group_id`'s own subtree -- its
    /// name, tag color, and every member tab's url/engine/position, with
    /// `parent` links preserved only between members of this same group
    /// (see `group_export::GroupExportTab`'s own doc comment). `None` if
    /// the group doesn't exist.
    fn export_group(&self, group_id: &str) -> Option<group_export::GroupExport> {
        let group_node = self.nodes.get(group_id)?;
        let group = group_node.as_group()?;
        let group_color = group_node.color;
        let member_ids: std::collections::HashSet<&str> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.group.as_deref() == Some(group_id))
            .map(|(id, _)| id.as_str())
            .collect();
        let tabs = self
            .nodes
            .iter()
            .filter(|(id, _)| member_ids.contains(id.as_str()))
            .filter_map(|(id, node)| {
                let tab = node.as_tab()?;
                Some(group_export::GroupExportTab {
                    id: id.clone(),
                    parent: node.parent.clone().filter(|p| member_ids.contains(p.as_str())),
                    url: tab.url.clone(),
                    engine: tab.tab.kind,
                    pos_x: node.pos.x,
                    pos_y: node.pos.y,
                })
            })
            .collect();
        Some(group_export::GroupExport {
            name: group.name.clone(),
            color: group_color.map(|c| (c.r(), c.g(), c.b())),
            tabs,
        })
    }

    /// Recreates an exported group at the top level -- a new group `GraphNode`
    /// plus each member tab, spawned via the normal async path. Every tab
    /// gets a freshly allocated id (`alloc_tab_id`) rather than reusing the
    /// exported one, since importing into an already-running session can't
    /// assume the old ids are free; `parent` links are remapped through the
    /// same old-id -> new-id map built while allocating them. Uses
    /// `SpawnPurpose::Restore` so imported tabs never steal focus, same as
    /// startup session restore.
    fn import_group_export(&mut self, export: group_export::GroupExport) {
        let group_id = self.alloc_group_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&group_id);
        let color = export.color.map(|(r, g, b)| egui::Color32::from_rgb(r, g, b));
        self.nodes.insert(
            group_id.clone(),
            GraphNode {
                parent: None,
                group: self.current_group.clone(),
                pos,
                vel: egui::Vec2::ZERO,
                color,
                payload: NodePayload::Group(GroupPayload { name: export.name }),
            },
        );
        self.node_order.push(group_id.clone());

        let id_map: std::collections::HashMap<String, String> =
            export.tabs.iter().map(|tab| (tab.id.clone(), self.alloc_tab_id())).collect();
        for tab in export.tabs {
            let Some(new_id) = id_map.get(&tab.id).cloned() else { continue };
            let parent = tab.parent.and_then(|p| id_map.get(&p).cloned());
            let pos = egui::vec2(tab.pos_x, tab.pos_y);
            self.spawn_tab_async(
                new_id,
                parent,
                Some(group_id.clone()),
                false,
                None,
                self.settings.zoom_factor,
                tab.url,
                pos,
                egui::Vec2::ZERO,
                SpawnPurpose::Restore,
                tab.engine,
            );
        }
    }

    /// The one right-click context menu for every node card. Fades via the
    /// shared `node_menu`/`node_menu_cache` (one fade Id for all kinds --
    /// per-kind Ids would go unpolled while another kind's menu is open,
    /// breaking `fade_popup_state`'s continuous-polling rule), draws the
    /// items `node_menu_config` lists for the node's kind, and applies the
    /// clicked one. A node that's gone (closed from its own menu, or
    /// elsewhere) closes the menu. While fading out, the menu draws its
    /// frozen `NodeMenuView` and is click-through, so a second click (say,
    /// the rest of a double-click) can't act again. While a group is being
    /// renamed, a click outside commits the name (the field loses focus)
    /// rather than just closing the menu.
    fn build_node_menu(&mut self, ui: &mut egui::Ui) {
        if self.node_menu.as_ref().is_some_and(|(id, _)| !self.nodes.contains_key(id)) {
            self.node_menu = None;
        }
        let current = self
            .node_menu
            .clone()
            .and_then(|(id, pos)| self.node_menu_view(&id).map(|view| (id, pos, view)));
        let Some(((id, pos, view), fade)) =
            fade_popup_state(ui.ctx(), egui::Id::new("node_menu_fade"), &current, &mut self.node_menu_cache)
        else {
            return;
        };
        let live = current.is_some();
        let config = node_menu_config(&view, self.headless);
        let mut frozen_draft = view.renaming.clone();
        let rename = match &mut self.renaming_group {
            Some((gid, draft)) if live && *gid == id => Some((draft, std::mem::take(&mut self.pending_focus_rename))),
            _ => frozen_draft.as_mut().map(|draft| (draft, false)),
        };
        let (action, clicked_outside) = build_node_context_menu(ui, pos, fade, live, &config, view.color, rename);
        let close = match action {
            Some(NodeMenuAction::Rename) => false,
            Some(_) => true,
            None => clicked_outside && view.renaming.is_none(),
        };
        if let Some(action) = action {
            self.apply_node_menu_action(ui.ctx(), &id, action);
        }
        if close {
            self.node_menu = None;
        }
    }

    fn node_menu_view(&self, id: &str) -> Option<NodeMenuView> {
        let node = self.nodes.get(id)?;
        let tab = node.as_tab();
        Some(NodeMenuView {
            kind: item_node_kind(&node.payload),
            has_group: node.group.is_some(),
            color: node.color,
            pinned: tab.is_some_and(|t| t.pinned),
            has_json: tab.is_some_and(|t| t.json_body.is_some()),
            renaming: self.renaming_group.as_ref().filter(|(gid, _)| gid == id).map(|(_, draft)| draft.clone()),
        })
    }

    /// Applies whichever item was clicked in a node's right-click menu.
    /// Each action only ever appears on kinds where it makes sense
    /// (`node_menu_config`), so none of these re-check the kind.
    fn apply_node_menu_action(&mut self, ctx: &egui::Context, id: &str, action: NodeMenuAction) {
        match action {
            NodeMenuAction::CopyText => {
                if let Some(text) = self.nodes.get(id).and_then(|n| n.as_note()).map(|n| n.text.clone()) {
                    ctx.copy_text(text);
                }
            }
            NodeMenuAction::Reveal => self.reveal_node(id),
            NodeMenuAction::CopyPath => {
                if let Some(path) = self.node_file_path(id) {
                    ctx.copy_text(path);
                }
            }
            NodeMenuAction::ExpandAsGraph => {
                self.expand_json_as_graph(id);
                self.save_session();
            }
            NodeMenuAction::TogglePin => {
                if let Some(tab) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) {
                    tab.pinned = !tab.pinned;
                }
            }
            NodeMenuAction::Duplicate => self.duplicate_tab(id),
            NodeMenuAction::CopyUrl => {
                if let Some(url) = self.nodes.get(id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) {
                    ctx.copy_text(url);
                }
            }
            NodeMenuAction::ViewJson => self.json_viewer_node = Some(id.to_string()),
            NodeMenuAction::FitSubtree => self.graph_scene_rect = self.fit_subtree_scene_rect(id),
            NodeMenuAction::Rename => {
                let name = self.nodes.get(id).and_then(|n| n.as_group()).map(|g| g.name.clone()).unwrap_or_default();
                self.renaming_group = Some((id.to_string(), name));
                self.pending_focus_rename = true;
            }
            NodeMenuAction::CommitRename => {
                if let Some((gid, name)) = self.renaming_group.take() {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        if let Some(group) = self.nodes.get_mut(&gid).and_then(|n| n.as_group_mut()) {
                            group.name = trimmed.to_string();
                        }
                    }
                }
            }
            NodeMenuAction::CancelRename => self.renaming_group = None,
            NodeMenuAction::SetColor(color) => {
                if let Some(node) = self.nodes.get_mut(id) {
                    node.color = color;
                }
            }
            NodeMenuAction::RemoveFromGroup => remove_from_group(&mut self.nodes, id),
            NodeMenuAction::Dissolve => self.dissolve_group(id),
            NodeMenuAction::ExportGroup => {
                if let Some(export) = self.export_group(id) {
                    if let Some((path, toast)) = save_dialog_with_toast(format!("{}.json", export.name), "Group") {
                        group_export::save_to(&export, &path);
                        self.queue_toast(toast.0);
                    }
                }
            }
            NodeMenuAction::Delete => self.close_node_and_save(id),
        }
    }

    /// A brief bottom-left toast, for `self.save_toast` — cleared once
    /// `SAVE_TOAST_DURATION` elapses. No dismiss interaction (unlike the
    /// context menu): it just times out. Fades in over its first
    /// `TOAST_FADE_IN` and out over its last `TOAST_FADE_OUT` rather than
    /// popping on and cutting off instantly -- driven directly off
    /// `shown_at.elapsed()` (this app's winit loop runs `ControlFlow::Poll`,
    /// so it's already redrawing every frame regardless; no extra
    /// `request_repaint` needed for a time-based fade to animate smoothly),
    /// not `ctx.animate_bool` -- unlike the toggle-bool dropdown popups,
    /// this already has a real wall-clock lifetime to derive opacity from
    /// directly. Despite the field's own name, the message is the caller's
    /// full text now, not just a path this function used to prepend
    /// "Saved " to -- every original "Saved <path>" call site now includes
    /// that itself, freeing this up for unrelated short-lived messages too
    /// (e.g. a crash/respawn notice, see `EngineToHost` handling below).
    /// Queues `message` behind whatever toast is showing. `build_save_toast`
    /// takes it off the queue when it first draws it, and its
    /// `SAVE_TOAST_DURATION` starts then, not here: a toast queued during
    /// startup (an unreadable settings/session file) used to spend part of
    /// its time before the first frame.
    fn queue_toast(&mut self, message: String) {
        self.save_toast_queue.push_back(message);
    }

    fn build_save_toast(&mut self, ui: &mut egui::Ui) {
        const TOAST_FADE_IN: std::time::Duration = std::time::Duration::from_millis(150);
        const TOAST_FADE_OUT: std::time::Duration = std::time::Duration::from_millis(400);
        if self.save_toast.is_none() {
            self.save_toast = self.save_toast_queue.pop_front().map(|message| (message, std::time::Instant::now()));
        }
        let Some((message, shown_at)) = &self.save_toast else { return };
        let elapsed = shown_at.elapsed();
        if elapsed >= SAVE_TOAST_DURATION {
            self.save_toast = None;
            return;
        }
        let fade_in = (elapsed.as_secs_f32() / TOAST_FADE_IN.as_secs_f32()).min(1.0);
        let remaining = SAVE_TOAST_DURATION.saturating_sub(elapsed);
        let fade_out = (remaining.as_secs_f32() / TOAST_FADE_OUT.as_secs_f32()).min(1.0);
        let opacity = fade_in.min(fade_out);
        let message = message.clone();
        egui::Area::new(egui::Id::new("save_toast"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(opacity);
                // Same ACCENT-tinted glow the graph cards use, tying the
                // toast into that visual language -- but this Frame
                // auto-sizes to its own label, so the rect `card_glow_
                // shapes` needs isn't known until *after* `.show()` below
                // lays it out. Reserve two placeholder shapes now (glow is
                // always two layers) so they still paint *behind* the
                // frame by paint order, then fill them in for real once
                // `response.rect` exists -- see `card_glow_shapes`'s own
                // doc comment for why this indirection, unlike graph
                // cards, is needed here specifically.
                let glow_slots = [ui.painter().add(egui::Shape::Noop), ui.painter().add(egui::Shape::Noop)];
                let response = popup_frame(ui).inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.label(message);
                    });
                let shapes = card_glow_shapes(response.response.rect, 3.0, ACCENT, 1.0, 1.0);
                for (slot, shape) in glow_slots.into_iter().zip(shapes) {
                    ui.painter().set(slot, shape);
                }
            });
    }

    /// `Ctrl+Tab`/`Ctrl+Shift+Tab`: focuses the next/previous tab in
    /// `tab_order` (insertion order — this graph-based UI has no tab strip
    /// to define a more meaningful order), wrapping at both ends. A no-op
    /// with zero or one tab open.
    fn cycle_focused_tab(&mut self, forward: bool) {
        // `node_order` holds every graph node (tabs, notes, images, PDFs,
        // JSON files), not just tabs -- filter to tabs first (same filter
        // `quick_switcher_matches` applies) so this never lands on a
        // non-tab node, which `focus_tab` -> `render_tab_toolbar_content`
        // assumes can't happen (`as_tab().unwrap()`).
        let tab_ids: Vec<&String> = self
            .node_order
            .iter()
            .filter(|id| self.nodes.get(id.as_str()).and_then(|n| n.as_tab()).is_some())
            .collect();
        if tab_ids.len() < 2 {
            return;
        }
        let current = self
            .focused
            .as_ref()
            .and_then(|id| tab_ids.iter().position(|t| *t == id));
        let len = tab_ids.len() as isize;
        let next = match current {
            Some(i) => {
                let delta = if forward { 1 } else { -1 };
                (i as isize + delta).rem_euclid(len) as usize
            }
            None => 0,
        };
        let id = tab_ids[next].clone();
        self.focus_tab(&id);
    }

    fn build_ui(&mut self, ui: &mut egui::Ui) {
        self.handle_global_shortcuts(ui);
        // Every view, so a toast queued in Graph view (a crash/respawn
        // notice, an unreadable session file) is seen where it happened.
        self.build_save_toast(ui);
        // A `Transition` (either direction) always renders Graph view's
        // node layout as the backdrop — it's the one screen both a
        // shrinking-away Tab view and an expanding-toward Tab view have a
        // coherent shared position in (the target/source node). The actual
        // animated content quad itself is drawn separately, in
        // `RedrawRequested` — see `transition_content_rect_points`.
        if self.transition.is_some() {
            self.build_graph_ui(ui);
            // A Tab's transition quad is a GPU pane (`content_panes_pts`);
            // an Image's is painted here, above the graph backdrop.
            if let Some((tex_id, rect)) = self.transition.as_ref().and_then(|t| self.image_transition_frame(t)) {
                // Clipped to the area below the toolbar `build_graph_ui`
                // (the backdrop, drawn just above) just painted -- at the
                // ToGraph end's t≈0, `rect` starts at the zoomed-in Image
                // view's own rect, which can reach above
                // `CHROME_HEIGHT_POINTS` into the toolbar row; this is a
                // plain `Order::Foreground` layer with no clip of its own,
                // so without this it painted over the toolbar for a frame.
                // Same height as both the Graph and Item toolbars
                // (`GRAPH_TOOLBAR_HEIGHT_POINTS == CHROME_HEIGHT_POINTS`),
                // so this is correct at either end of the animation.
                let screen = self.screen_size_points();
                let clip_rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, CHROME_HEIGHT_POINTS),
                    egui::vec2(screen.x, (screen.y - CHROME_HEIGHT_POINTS).max(0.0)),
                );
                ui.ctx()
                    .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("image_transition")))
                    .with_clip_rect(clip_rect)
                    .image(
                        tex_id,
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
            }
            return;
        }
        match self.view_mode {
            ViewMode::Item => self.build_item_ui(ui),
            ViewMode::Graph => self.build_graph_ui(ui),
            ViewMode::Settings => self.build_settings_page(ui),
        }
        self.build_quick_switcher(ui);
        self.build_import_confirm_popup(ui);
        self.build_json_viewer(ui);
    }

    /// "Import Session..." confirm popup -- shown whenever `pending_import`
    /// is staged (a real file pick, or headless `IMPORTSESSION`), comparing
    /// the picked file's tab count against the currently running session's
    /// own before actually replacing anything, so swapping in the wrong
    /// file is a deliberate second click rather than a silent one-shot
    /// dialog pick. Import applies it via the same `replace_running_session`
    /// `load_named_session` already uses; Cancel just discards the staged
    /// session, leaving the running one untouched.
    fn build_import_confirm_popup(&mut self, ui: &mut egui::Ui) {
        let Some(session) = &self.pending_import else { return };
        let incoming = session.tabs.len();
        let current = self.nodes.len();
        let mut confirm = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::new("import_confirm_popup"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui)
                    .show(ui, |ui| {
                        ui.set_min_width(240.0);
                        ui.label(heading_text("Import Session"));
                        ui.add_space(6.0);
                        ui.label(format!(
                            "Current session: {} tab{}",
                            current,
                            if current == 1 { "" } else { "s" }
                        ));
                        ui.label(format!(
                            "Importing: {} tab{}",
                            incoming,
                            if incoming == 1 { "" } else { "s" }
                        ));
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("Importing replaces every currently open tab.")
                                    .small()
                                    .weak(),
                            )
                            .wrap(),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Import").clicked() {
                                confirm = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel = true;
                            }
                        });
                    });
            });
        if confirm {
            if let Some(session) = self.pending_import.take() {
                self.replace_running_session(session);
            }
        }
        if cancel {
            self.pending_import = None;
        }
    }

    /// Full-screen popup showing one node's JSON content as a real
    /// collapsible tree -- either a tab's own `TabPayload::json_body`
    /// (opened from Tab view's toolbar button or Graph view's tab context
    /// menu's "View as JSON") or a `JsonPayload::value` (opened by double-
    /// clicking a JSON node's card). Not tied to whichever view is
    /// currently active (see `json_viewer_node`'s own doc comment), so this
    /// renders every frame regardless of `view_mode`.
    fn build_json_viewer(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.json_viewer_node.clone() else { return };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.json_viewer_node = None;
            return;
        }
        // A JSON node's own `value` is already parsed -- shown directly, no
        // fallback needed. A tab's `json_body` is raw text: the engine's
        // own `document.contentType` check already confirmed
        // `application/json` before this ever got set, but a server can lie
        // about its own header, so re-parsing here still needs a fallback.
        enum Source {
            Parsed(serde_json::Value),
            Raw(String),
        }
        let Some(source) = self.nodes.get(&id).and_then(|n| {
            if let Some(json) = n.as_json() {
                Some(Source::Parsed(json.value.clone()))
            } else {
                n.as_tab().and_then(|t| t.json_body.clone()).map(Source::Raw)
            }
        }) else {
            self.json_viewer_node = None;
            return;
        };
        // Only a tab's own live `json_body` is worth offering "Save as
        // Node" for -- a JSON node's own popup is already a saved node,
        // nothing to save. Cloned out before `source` is moved into the
        // render match below.
        let raw_body = match &source {
            Source::Raw(body) => Some(body.clone()),
            Source::Parsed(_) => None,
        };
        let mut close = false;
        let mut save_as_node = false;
        egui::Area::new(egui::Id::new("json_viewer"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                dialog_frame(ui)
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        ui.set_height(520.0);
                        ui.horizontal(|ui| {
                            ui.label(heading_text("JSON"));
                            // See `ipc_protocol::EngineToHost::
                            // RedirectableContent`'s own doc comment for
                            // why this is the *only* opt-in, user-
                            // triggered way a JSON tab ever becomes a
                            // node -- unlike image/etc., which redirect
                            // automatically, a JSON response is often a
                            // live, refetchable API call worth keeping as
                            // an ordinary tab.
                            if raw_body.is_some()
                                && ui.button("Save as Node").on_hover_text(
                                    "Keep this JSON as a permanent graph node -- the tab keeps navigating normally",
                                ).clicked()
                            {
                                save_as_node = true;
                            }
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                        });
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| match source {
                            Source::Parsed(value) => draw_json_value(ui, "root", &value, true),
                            Source::Raw(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                                Ok(value) => draw_json_value(ui, "root", &value, true),
                                Err(err) => {
                                    ui.colored_label(
                                        DANGER_COLOR,
                                        format!("Invalid JSON: {err}"),
                                    );
                                    ui.add(egui::Label::new(&body).wrap());
                                }
                            },
                        });
                    });
            });
        if save_as_node {
            if let Some(body) = raw_body {
                self.save_json_body_as_node(&body);
            }
        }
        if close {
            self.json_viewer_node = None;
        }
    }

    /// The JSON viewer popup's "Save as Node" button -- materializes the
    /// *currently showing* tab's own `json_body` as a real, permanent JSON
    /// node, without touching the tab itself (it keeps navigating/
    /// refreshing normally). Writes `body` to a real temp file first and
    /// reuses `insert_json_node_with_id` completely unchanged, same
    /// "downloaded bytes become a real file first" pattern `redirect_fetch`
    /// uses for the automatic kinds -- a node made this way is
    /// indistinguishable from one added via "+ JSON" from this point on.
    fn save_json_body_as_node(&mut self, body: &str) {
        let dir = std::env::temp_dir().join("manifold-redirects");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let id = self.alloc_json_id();
        let path = dir.join(format!("{id}.bin"));
        if std::fs::write(&path, body).is_err() {
            return;
        }
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let group = self.current_group.clone();
        if self.insert_json_node_with_id(id, path.to_string_lossy().into_owned(), pos, group).is_ok() {
            self.save_session();
            self.queue_toast("Saved as JSON node".to_string());
        }
    }

    /// The "Keyboard Shortcuts (F1)" half of `render_universal_chip`.
    /// Returns its own rect, which the shortcuts-help popup anchors to.
    fn build_help_button(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let help_response = chip_icon_button(ui, egui::Id::new("icon_btn_shortcuts_help"), egui_phosphor::light::QUESTION)
            .on_hover_text("Keyboard Shortcuts (F1)");
        if help_response.clicked() {
            self.shortcuts_help_open = !self.shortcuts_help_open;
        }
        help_response.rect
    }

    /// The Settings-gear half of `render_universal_chip`.
    fn build_settings_button(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let response = chip_icon_button(ui, egui::Id::new("icon_btn_settings_gear"), egui_phosphor::light::GEAR)
            .on_hover_text("Settings");
        if response.clicked() {
            self.toggle_settings_page();
        }
        response.rect
    }

    /// Help + Settings, present in every toolbar (see the design spec's
    /// "universal chip" layer -- layer 1, drawn frame-level by
    /// `render_item_toolbar` for every Item-view kind, not just Tab).
    /// Returns the same rect `build_help_button` already returns -- the
    /// shortcuts-help popup anchors to that specific button, not to the
    /// chip as a whole, so callers that need it (`ItemToolbarFrame`'s own
    /// `shortcuts_help_button_rect`) keep working unchanged.
    ///
    /// Reads "Help, Settings" left to right whether `ui` is a left-to-right
    /// row (the Graph toolbar) or a right-to-left child (the Item toolbar's
    /// trailing cluster) -- see `in_reading_order`. Drawing both in one
    /// fixed call order once silently reversed them in the right-to-left
    /// caller (confirmed live: Tab and Graph view screenshotted side by side
    /// disagreed).
    fn render_universal_chip(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let mut help_rect = egui::Rect::NOTHING;
        toolbar_chip(ui, |ui| {
            let (help, settings) =
                in_reading_order(ui, self, Self::build_help_button, Self::build_settings_button);
            help_rect = help;
            help.union(settings)
        });
        help_rect
    }

    /// The Item toolbar's shared frame -- the design spec's three layers,
    /// outer to inner. Layer 2 ("Graph") sits at the leading edge. Layer 1
    /// (the Universal chip) is drawn *first* into a right-to-left trailing
    /// child `Ui` so it's always rightmost, for every kind. Layer 3 (the
    /// focused kind's own content) then gets both the row `ui` for leading
    /// widgets and `trailing_ui` for trailing ones; a kind that needs the
    /// space in between (Tab's address bar) measures `trailing_ui` to size
    /// it. The frame finally advances the row past the trailing cluster.
    fn render_item_toolbar(&mut self, node_id: &str, ui: &mut egui::Ui) -> ItemToolbarFrame {
        let mut back_to_graph = false;
        let mut shortcuts_help_button_rect = None;
        let mut actions = ItemToolbarActions::default();
        toolbar_panel("toolbar", ui, |ui| {
            // Not `horizontal_centered` (`Align::Center`): measured live
            // that `egui::ComboBox` paints itself ~4.5pt lower than a
            // plain `Button`/`TextEdit` even when all three report the
            // exact same `response.rect.height()` (confirmed via
            // temporary debug prints: 27.0 for all three, but the combo
            // box's `rect.min.y` alone sat 4.5pt below the back button's)
            // — an internal quirk of `ComboBox::show_ui` wrapping its
            // content in its own nested `ui.horizontal(...)`, whose
            // layout-pass size estimate apparently doesn't quite match
            // what it actually paints. Since every item in this row is
            // now confirmed the same real height (the address-bar-margin
            // fix above made sure of that), top-aligning instead of
            // center-aligning sidesteps the mismatch entirely rather
            // than compensating for a specific pixel offset that could
            // silently drift if padding/fonts ever change.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                if render_return_to_graph(ui) {
                    back_to_graph = true;
                }
                let mut trailing_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(ui.available_rect_before_wrap())
                        .layout(egui::Layout::right_to_left(egui::Align::Min)),
                );
                shortcuts_help_button_rect = Some(self.render_universal_chip(&mut trailing_ui));
                actions = self.render_item_toolbar_content(node_id, ui, &mut trailing_ui);
                ui.advance_cursor_after_rect(trailing_ui.min_rect());
            });
        });
        ItemToolbarFrame { back_to_graph, shortcuts_help_button_rect, actions }
    }

    fn render_item_toolbar_content(&mut self, node_id: &str, ui: &mut egui::Ui, trailing_ui: &mut egui::Ui) -> ItemToolbarActions {
        let kind = match self.nodes.get(node_id) {
            Some(node) => item_node_kind(&node.payload),
            None => return ItemToolbarActions::default(),
        };
        match kind {
            ItemNodeKind::Tab => self.render_tab_toolbar_content(node_id, ui, trailing_ui),
            ItemNodeKind::Image => self.render_image_toolbar_content(node_id, ui, trailing_ui),
            ItemNodeKind::Download | ItemNodeKind::Group | ItemNodeKind::Note | ItemNodeKind::Pdf | ItemNodeKind::Json => {
                ItemToolbarActions::default()
            }
        }
    }

    /// Image view's layer-3 toolbar content: identity (filename · pixel
    /// size, full path on hover) and the zoom chip on the leading side;
    /// Open Containing Folder + Copy Path in the trailing cluster.
    fn render_image_toolbar_content(
        &mut self,
        image_id: &str,
        ui: &mut egui::Ui,
        trailing_ui: &mut egui::Ui,
    ) -> ItemToolbarActions {
        let Some((path, image)) = self
            .nodes
            .get(image_id)
            .and_then(|n| n.as_image())
            .map(|i| (i.path.clone(), image_pixel_size(i)))
        else {
            return ItemToolbarActions::default();
        };

        // (reveal, copy) clicked
        let mut clicked = (false, false);
        toolbar_chip(trailing_ui, |ui| {
            let (reveal, copy) = in_reading_order(
                ui,
                &mut clicked,
                |clicked, ui| {
                    let response = chip_icon_button(ui, egui::Id::new("icon_btn_image_reveal"), egui_phosphor::light::FOLDER_OPEN)
                        .on_hover_text("Open Containing Folder");
                    clicked.0 = response.clicked();
                    response.rect
                },
                |clicked, ui| {
                    let response = chip_icon_button(ui, egui::Id::new("icon_btn_image_copy_path"), egui_phosphor::light::COPY)
                        .on_hover_text("Copy Path");
                    clicked.1 = response.clicked();
                    response.rect
                },
            );
            reveal.union(copy)
        });

        // Vertically centered in the row (which is top-aligned -- see
        // `render_item_toolbar`), the same problem the Tab toolbar's
        // favicon slot solves; width capped so a long filename truncates
        // instead of running under the trailing cluster (Reveal/Copy/Help/
        // Settings, already in `trailing_ui` by this point -- see
        // `render_item_toolbar`) or the zoom chip that follows this label
        // in the same row. `IMAGE_ZOOM_CHIP_RESERVED_WIDTH` is a measured
        // constant rather than `trailing_width`-style live measurement
        // because the chip hasn't been laid out yet when this label needs
        // its own width: it's drawn after the label, in this same
        // leading-side `ui` (see the call below).
        let trailing_width = trailing_ui.min_rect().width();
        let identity = format!("{} · {}×{}", download_filename(&path), image.x as u32, image.y as u32);
        let label_max_width =
            (ui.available_width() - trailing_width - IMAGE_ZOOM_CHIP_RESERVED_WIDTH).max(80.0).min(360.0);
        ui.allocate_ui_with_layout(
            egui::vec2(label_max_width, ui.available_height()),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.add(egui::Label::new(identity).truncate()).on_hover_text(&path),
        );

        let (scale, _) = self.image_view.effective(image, self.item_body_rect().size());
        if let Some(action) = render_zoom_chip(
            ui,
            scale * 100.0,
            scale > IMAGE_ZOOM_MIN + 1e-3,
            scale < IMAGE_ZOOM_MAX - 1e-3,
            "Fit to window",
        ) {
            self.apply_image_zoom_action(action);
        }

        if clicked.0 {
            self.reveal_node(image_id);
        }
        if clicked.1 {
            ui.ctx().copy_text(path);
        }
        ItemToolbarActions::default()
    }

    /// Zoom-chip click or its keyboard equivalent, applied to the Image
    /// currently in Item view. A no-op outside Image view.
    fn apply_image_zoom_action(&mut self, action: ZoomChipAction) {
        let Some(image) = self
            .viewed_image
            .as_ref()
            .and_then(|id| self.nodes.get(id))
            .and_then(|n| n.as_image())
            .map(image_pixel_size)
        else {
            return;
        };
        self.image_view = image_view_after_action(self.image_view, action, image, self.item_body_rect().size());
    }

    /// Image view: the shared Item toolbar plus the image itself, zoomable
    /// (wheel/pinch about the cursor) and pannable (drag) -- see
    /// `ImageViewState`. Leaves to Graph if the image node is gone.
    fn build_image_view_ui(&mut self, ui: &mut egui::Ui, image_id: &str) {
        let Some(image) = self.nodes.get(image_id).and_then(|n| n.as_image()).map(image_pixel_size) else {
            self.viewed_image = None;
            self.view_mode = ViewMode::Graph;
            self.save_session();
            return;
        };
        let frame = self.render_item_toolbar(image_id, ui);
        self.build_shortcuts_help(ui, frame.shortcuts_help_button_rect);

        let body = self.item_body_rect();
        let response = ui.interact(body, egui::Id::new("image_view_body"), egui::Sense::drag());
        let (orig_scale, orig_pan) = self.image_view.effective(image, body.size());
        let (mut scale, mut pan) = (orig_scale, orig_pan);
        let mut changed = false;
        if response.hovered() {
            let factor = ui.input(|i| i.zoom_delta() * (i.smooth_scroll_delta.y * IMAGE_WHEEL_ZOOM_RATE).exp());
            if factor != 1.0 {
                let anchor = ui
                    .input(|i| i.pointer.hover_pos())
                    .map_or(egui::Vec2::ZERO, |p| p - body.center());
                (scale, pan) = zoom_about(scale, pan, factor, anchor);
                changed = true;
            }
        }
        if response.dragged() {
            pan += response.drag_delta();
            changed = true;
        }
        if changed {
            pan = clamp_image_pan(pan, image * scale, body.size());
            // A drag attempt while at Fit with the image fully contained
            // (no room to pan) reaches here with `scale`/`pan` clamped
            // right back to what `effective` already reported -- committing
            // a fixed `Scale` in that case would silently turn off Fit's
            // "re-fit on window resize" behavior for no visible change.
            // Only commit when something about the view actually moved.
            if scale != orig_scale || pan != orig_pan {
                self.image_view = ImageViewState { zoom: ImageZoom::Scale(scale), pan };
            }
        }

        let painter = ui.painter_at(body);
        painter.rect_filled(body, 0.0, BG);
        let rect = image_display_rect(body, image, scale, pan);
        match self.gpu.as_ref().and_then(|g| g.image_id(image_id)) {
            Some(tex_id) => {
                painter.image(
                    tex_id,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            // Same placeholder the graph card uses for a missing texture.
            None => {
                painter.text(
                    body.center(),
                    egui::Align2::CENTER_CENTER,
                    egui_phosphor::light::IMAGE_BROKEN,
                    egui::FontId::proportional(48.0),
                    ui.visuals().weak_text_color(),
                );
            }
        }

        let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape)) && self.quick_switcher.is_none();
        if frame.back_to_graph || escape_pressed {
            self.leave_to_graph();
        }
    }

    fn render_tab_toolbar_content(&mut self, focused_id: &str, ui: &mut egui::Ui, trailing_ui: &mut egui::Ui) -> ItemToolbarActions {
        let mut actions = ItemToolbarActions::default();
        let node = self.nodes[focused_id].as_tab().unwrap();
        toolbar_chip(ui, |ui| {
            let mut bounds: Option<egui::Rect> = None;
            let mut grow = |r: egui::Rect| {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            };
            // `◀`/`▶` were already a proven-safe Unicode gamble
            // (confirmed live elsewhere in this app), not a real
            // problem -- migrated to `egui_phosphor::CARET_LEFT`/
            // `CARET_RIGHT` anyway once the icon font was already
            // in place, for the same visual consistency the other
            // icon migrations this session already have.
            let back = chip_icon_button_enabled(ui, egui::Id::new("icon_btn_back"), egui_phosphor::light::CARET_LEFT, node.can_go_back);
            grow(back.rect);
            if back.clicked() {
                actions.go_back = true;
            }
            let forward = chip_icon_button_enabled(ui, egui::Id::new("icon_btn_forward"), egui_phosphor::light::CARET_RIGHT, node.can_go_forward);
            grow(forward.rect);
            if forward.clicked() {
                actions.go_forward = true;
            }
            // Was plain text ("Reload") to avoid a refresh-arrows
            // glyph gamble in this app's own font -- now a real
            // `egui_phosphor` icon instead, same reasoning as every
            // other icon migrated once the icon font was already
            // wired in.
            let reload = chip_icon_button(ui, egui::Id::new("icon_btn_reload"), egui_phosphor::light::ARROW_CLOCKWISE)
                .on_hover_text("Reload");
            grow(reload.rect);
            if reload.clicked() {
                actions.reload_requested = true;
            }
            let home = chip_icon_button(ui, egui::Id::new("icon_btn_home"), egui_phosphor::light::HOUSE)
                .on_hover_text("Go to home page");
            grow(home.rect);
            if home.clicked() {
                actions.home_requested = true;
            }
            bounds.unwrap_or(ui.cursor())
        });

        // Tab identity (was a separate strip above the toolbar,
        // merged into this single bar): a small loading spinner
        // or the fetched favicon, right before the address bar --
        // the same "site identity chip at the address bar's own
        // left edge" spot Chrome/Firefox use. Explicitly centered
        // in its own row-height slot -- this row is `Align::Min`
        // (top-aligned, see the doc comment above on why), which
        // a bare 16x16 image or 12pt spinner dropped in directly
        // renders flush against the row's *top* edge instead of
        // centered like its taller icon-button neighbors,
        // visibly poking up and overlapping the address bar's
        // own top border (confirmed live, reported as "favicon
        // is unaligned").
        let identity_slot_height = ui.available_height();
        if node.loading {
            ui.allocate_ui_with_layout(
                egui::vec2(16.0, identity_slot_height),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| ui.add(egui::Spinner::new().size(12.0)),
            );
        } else if let Some(tex_id) = {
            let url = node.url.clone();
            self.cached_favicon_origin(&url)
        }
        .and_then(|origin| self.gpu.as_ref().and_then(|g| g.favicon_id(&origin)))
        {
            ui.allocate_ui_with_layout(
                egui::vec2(FAVICON_SIZE.x, identity_slot_height),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(tex_id, FAVICON_SIZE)))
                },
            );
        }

        // Address bar moved up here (right after nav/identity,
        // before any per-tab action button) and given most of
        // the row's width -- mirrors every mainstream browser's
        // own layout (nav controls, then a wide address bar,
        // then a handful of page-action icons to its right, then
        // an overflow/app menu at the far end), rather than the
        // stack of buttons that used to sit entirely to its
        // left, pushing it almost to the row's far end.
        //
        // The trailing cluster (Bookmark, View-as-JSON, Split +
        // Close Split, Tab actions) is drawn *first*, into the frame's
        // right-to-left `trailing_ui` (`render_item_toolbar`, which has
        // already put the Universal chip at its far right), so its real
        // consumed width can be measured -- not
        // guessed the way a former `trailing_reserve = 360.0`
        // constant was (sized "generously for the worst case,"
        // with nothing to catch a future addition silently
        // exceeding it). Added in reverse of their final visual
        // order since a right-to-left `Ui` places each new
        // widget immediately to the *left* of the previous one.
        let tab_actions_response = trailing_ui.button("Tab actions");
        if tab_actions_response.clicked() {
            self.tab_actions_open = !self.tab_actions_open;
        }
        actions.tab_actions_button_rect = Some(tab_actions_response.rect);
        if self.split_tab.is_some()
            && icon_button_eased(
                trailing_ui,
                egui::Id::new("icon_btn_close_split"),
                egui_phosphor::light::X,
            )
            .on_hover_text("Close Split")
            .clicked()
        {
            actions.close_split_requested = true;
        }
        // Engine-vs-engine compare view -- see `App::split_tab`'s
        // own doc comment. Picking an engine here (re-)opens a
        // split against the focused tab's current url; the
        // secondary pane is view-only in this first cut, so its
        // own toolbar/address-bar controls only ever apply to
        // the primary (left) pane.
        let split_engine_response =
            trailing_ui.button(if self.split_tab.is_some() { "Split ▾" } else { "Split" });
        if split_engine_response.clicked() {
            self.split_engine_picker_open = !self.split_engine_picker_open;
        }
        actions.split_engine_button_rect = Some(split_engine_response.rect);
        let has_json = self.nodes.get(focused_id).and_then(|n| n.as_tab()).is_some_and(|n| n.json_body.is_some());
        let is_bookmarked = self
            .nodes
            .get(focused_id)
            .and_then(|n| n.as_tab())
            .is_some_and(|n| self.bookmarks.iter().any(|b| b.url == n.url));
        toolbar_chip(trailing_ui, |ui| {
            let mut bounds: Option<egui::Rect> = None;
            let mut grow = |r: egui::Rect| {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            };
            if has_json {
                let response = chip_icon_button(ui, egui::Id::new("icon_btn_view_json"), egui_phosphor::light::BRACKETS_CURLY)
                    .on_hover_text("View as JSON");
                grow(response.rect);
                if response.clicked() {
                    self.json_viewer_node = Some(focused_id.to_string());
                }
            }
            // `selectable_label`, same reasoning as elsewhere in
            // this toolbar -- a pressed look for "already
            // bookmarked", a real `BOOKMARK_SIMPLE` icon rather than
            // a star glyph's tofu risk. Sits right after the address
            // bar, mirroring Chrome/Firefox's own star icon at the
            // address bar's right edge. Deliberately still
            // `selectable_label` here rather than `chip_icon_button`
            // (unlike its sibling `chip_icon_button` above for the
            // JSON-viewer icon): no filled/bold bookmark glyph exists
            // in `egui_phosphor::light` to express the pressed state
            // another way, and losing the bookmarked-state affordance
            // entirely is worse than this one icon's border/height not
            // quite matching its `chip_icon_button` neighbor in the
            // same chip.
            let bookmark = ui
                .selectable_label(is_bookmarked, egui_phosphor::light::BOOKMARK_SIMPLE)
                .on_hover_text("Bookmark");
            grow(bookmark.rect);
            if bookmark.clicked() {
                actions.toggle_bookmark_requested = true;
            }
            bounds.unwrap_or(ui.cursor())
        });
        let trailing_width = trailing_ui.min_rect().width();

        // Now the address bar gets whatever's actually left,
        // not a guess -- drawn into the original `ui` (still
        // mid-way through its own left-to-right cursor, just
        // after the leading nav/identity cluster above),
        // sized to stop exactly where the trailing cluster's
        // real measured width begins.
        let text_width = (ui.available_rect_before_wrap().width() - trailing_width).max(40.0);
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.address_bar_text)
                .desired_width(text_width)
                // Match `style.spacing.button_padding` (set in
                // `apply_style`) so the address bar renders the
                // same height as the back/forward buttons next
                // to it — `TextEdit`'s own default margin
                // (`Margin::symmetric(4, 2)`) doesn't share that
                // style value, so left unset the row's tallest
                // item (the text field) stood out with visibly
                // more height than its neighbors instead of a
                // level toolbar.
                .margin(egui::Margin::symmetric(8, 4)),
        );
        if std::mem::take(&mut self.pending_focus_address_bar) {
            response.request_focus();
        }
        self.address_bar_focused = response.has_focus();
        actions.address_bar_rect = Some(response.rect);

        if response.changed() {
            self.open_tab_suggestions =
                self.compute_open_tab_suggestions(&self.address_bar_text, focused_id);
            self.suggestion_fetcher.note_text_changed(&self.address_bar_text);
            // A changed query invalidates whatever row was
            // highlighted for the *previous* suggestion list —
            // keeping a stale index could silently highlight
            // (and Enter-accept) an unrelated row.
            self.dropdown_highlighted = None;
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            actions.enter_pressed = true;
        }

        actions
    }

    /// `ViewMode::Item`: whichever node is maximized -- `viewed_image` if
    /// set, else the focused Tab.
    fn build_item_ui(&mut self, ui: &mut egui::Ui) {
        match self.viewed_image.clone() {
            Some(image_id) => self.build_image_view_ui(ui, &image_id),
            None => self.build_tab_view_ui(ui),
        }
    }

    fn build_tab_view_ui(&mut self, ui: &mut egui::Ui) {
        let Some(focused_id) = self.focused.clone() else {
            self.view_mode = ViewMode::Graph;
            return;
        };
        if !self.nodes.contains_key(&focused_id) {
            if self.pending_respawn_focus.as_deref() == Some(focused_id.as_str()) {
                // An engine swap or crash recovery is respawning the tab
                // the user is currently looking at — stay in Tab view
                // (don't bounce to Graph, see `pending_respawn_focus`'s doc
                // comment) and show a lightweight placeholder (with just
                // enough chrome to still escape to Graph) instead of
                // nothing until it reappears in `self.nodes`.
                let mut back_to_graph = false;
                toolbar_panel("tab_strip_respawning", ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        if render_return_to_graph(ui) {
                            back_to_graph = true;
                        }
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label("Reconnecting…");
                    });
                });
                if back_to_graph {
                    self.view_mode = ViewMode::Graph;
                    return;
                }
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Spinner::new().size(32.0));
                });
            } else {
                self.view_mode = ViewMode::Graph;
            }
            return;
        }
        // Not just `!self.address_bar_focused`: found live that the moment
        // the suggestion dropdown's buttons first appear, egui's own
        // keyboard-focus tracking transiently moves off the `TextEdit` for
        // a frame (a real, reproducible bug, not a one-off) — `has_focus()`
        // then reports false for that one frame, which (without this) blew
        // away whatever the user had just typed by resyncing to the tab's
        // real URL mid-keystroke. Suggestions being non-empty is a good
        // proxy for "still actively editing" that survives that blip.
        let editing_address = self.address_bar_focused
            || !self.open_tab_suggestions.is_empty()
            || !self.search_suggestions.is_empty();
        // `focused` is tab-only (see `App::focused`), so this always finds
        // a Tab; `if let` rather than `.unwrap()` so a future path that
        // breaks that invariant degrades to a stale address bar, not a
        // panic.
        if !editing_address {
            if let Some(tab) = self.nodes.get(&focused_id).and_then(|n| n.as_tab()) {
                self.address_bar_text = tab.url.clone();
            }
        }
        let mut navigate = None;
        // Whether Enter fired this frame (real keypress or headless `ENTER`)
        // — deferred here rather than acted on immediately inside the
        // toolbar closure below, since deciding what Enter *does* (accept
        // the highlighted dropdown row vs. plain-navigate) needs `combined`
        // (the suggestion list), which isn't computed until after this
        // closure returns.
        // Exhaustive (no `..`): a new `ItemToolbarActions` field that isn't
        // wired up here fails to compile.
        let ItemToolbarFrame {
            back_to_graph,
            shortcuts_help_button_rect,
            actions:
                ItemToolbarActions {
                    go_back,
                    go_forward,
                    reload_requested,
                    home_requested,
                    enter_pressed,
                    toggle_bookmark_requested,
                    close_split_requested,
                    tab_actions_button_rect,
                    split_engine_button_rect,
                    address_bar_rect,
                },
        } = self.render_item_toolbar(&focused_id, ui);

        self.build_shortcuts_help(ui, shortcuts_help_button_rect);

        if self.find_bar_open {
            self.build_find_bar(ui);
        }

        self.build_context_menu(ui);

        if let Some(results) = self.suggestion_fetcher.tick() {
            self.search_suggestions = results;
        }

        // Autocomplete dropdown: a separate `egui::Area` anchored right
        // below the address bar (screen space, not the toolbar `Panel`'s
        // own layout — the dropdown needs to float over the content area
        // below chrome, which a plain widget inside the toolbar couldn't
        // do without stretching the toolbar itself). Clicking a row accepts
        // it directly; Up/Down move `dropdown_highlighted` (wrapping at
        // both ends) and Enter accepts whichever row is highlighted (see
        // `enter_pressed`'s resolution below) — reading arrow keys via
        // `ui.input(|i| i.key_pressed(...))` here, after the `TextEdit`
        // above already had its turn, is safe: `key_pressed` is a plain
        // per-frame boolean read, not a consumed event, and a singleline
        // `TextEdit` has no vertical text to move a cursor through, so
        // there's no competing behavior to fight over (Enter was the one
        // that actually needed care, via `enter_pressed`'s deferred
        // resolution — a singleline `TextEdit` losing focus on Enter is a
        // real, meaningful interaction to not double-handle).
        // Gated on `editing_address`, not raw `self.address_bar_focused`:
        // the same one-frame focus blip documented above (egui's focus
        // tracking transiently leaving the `TextEdit` when the dropdown's
        // buttons appear) was also hiding this whole Area, including the
        // buttons, on the exact frame a click landed on one — so the click
        // never reached anything and just silently dismissed the dropdown.
        let combined = self.combined_suggestions();
        if editing_address && !combined.is_empty() {
            let arrow_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
            let arrow_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
            if arrow_down {
                self.dropdown_highlighted = Some(match self.dropdown_highlighted {
                    Some(i) if i + 1 < combined.len() => i + 1,
                    _ => 0,
                });
            }
            if arrow_up {
                self.dropdown_highlighted = Some(match self.dropdown_highlighted {
                    Some(i) if i > 0 => i - 1,
                    _ => combined.len() - 1,
                });
            }

            if let Some(rect) = address_bar_rect {
                let mut accepted = None;
                egui::Area::new(egui::Id::new("address_suggestions"))
                    .fixed_pos(rect.left_bottom())
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        // Inner width minus the frame's padding and border, so the
                        // dropdown lines up with the address bar's edges.
                        let frame = popup_frame(ui);
                        let inner_width = rect.width() - frame.total_margin().sum().x;
                        frame
                            .show(ui, |ui| {
                                ui.set_width(inner_width);
                                // `ui.set_width` alone only sets the *container's*
                                // layout width (affects wrapping/available_width),
                                // not an individual `Button`'s own size — left
                                // unset, each row was only as wide as its own
                                // text, producing a ragged right edge instead of
                                // a uniform list. `min_size`'s width stretches
                                // each row to fill the container; height stays
                                // `0.0` (no minimum) so natural row height/padding
                                // is untouched.
                                let full_width = ui.available_width();
                                for (idx, suggestion) in combined.iter().enumerate() {
                                    let highlighted = self.dropdown_highlighted == Some(idx);
                                    let mut button = egui::Button::new(suggestion.label())
                                        .truncate()
                                        .min_size(egui::vec2(full_width, 0.0));
                                    if highlighted {
                                        button = button.fill(ACCENT);
                                    }
                                    if ui.add(button).clicked() {
                                        accepted = Some(suggestion.clone());
                                    }
                                }
                            });
                    });
                if let Some(suggestion) = accepted {
                    self.accept_suggestion(suggestion, &focused_id);
                }
            }
        }

        if enter_pressed {
            match self.dropdown_highlighted.filter(|&i| i < combined.len()) {
                Some(idx) => self.accept_suggestion(combined[idx].clone(), &focused_id),
                None => navigate = Some(self.address_bar_text.clone()),
            }
        }

        let has_suggestions =
            !self.open_tab_suggestions.is_empty() || !self.search_suggestions.is_empty();
        // Neutralized while the quick-switcher is open: it does its own,
        // separate Escape check in `build_quick_switcher` to close itself,
        // and `key_pressed` is a non-consuming peek -- without this, the
        // same Escape press would also fire this chain (leaving to Graph,
        // or dismissing the address-bar dropdown) underneath it.
        let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape)) && self.quick_switcher.is_none();
        if back_to_graph
            || (!has_suggestions && !self.find_bar_open && self.context_menu.is_none() && escape_pressed)
        {
            self.leave_to_graph();
            return;
        }
        if escape_pressed {
            // Context menu, then find bar, take priority over the
            // address-bar dropdown if more than one somehow ended up open
            // at once (independent widgets — nothing stops a user from
            // right-clicking while the find bar is also open) — closes one
            // thing per Escape press, same layered-dismiss shape the
            // dropdown-vs-Graph-view case above already has.
            if self.context_menu.is_some() {
                self.context_menu = None;
            } else if self.find_bar_open {
                self.close_find_bar();
            } else if has_suggestions {
                self.open_tab_suggestions.clear();
                self.search_suggestions.clear();
                self.dropdown_highlighted = None;
            }
        }
        if go_back {
            if let Some(node) = self.nodes.get_mut(&focused_id).and_then(|n| n.as_tab_mut()) {
                node.tab.send(&HostToEngine::GoBack);
            }
        }
        if go_forward {
            if let Some(node) = self.nodes.get_mut(&focused_id).and_then(|n| n.as_tab_mut()) {
                node.tab.send(&HostToEngine::GoForward);
            }
        }
        if reload_requested {
            self.reload_focused_tab_shortcut();
        }
        if home_requested {
            let home_page = self.settings.home_page.clone();
            self.navigate_tab(&focused_id, home_page);
        }
        if close_split_requested {
            self.close_split();
        }
        // The secondary pane's own small "which engine is this" label plus
        // a thin divider line -- drawn here (real egui, on top of both raw
        // content quads) rather than as part of either pane's own content,
        // since the secondary pane has no toolbar of its own to put it in
        // (view-only in this first cut, see `split_tab`'s own doc comment).
        if let Some(split_id) = self.split_tab.clone() {
            let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
            let screen_size = self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
            let screen_max_y = screen_size.height as f32 / ppp;
            let divider_x = (screen_size.width as f32 / ppp) / 2.0;
            let kind = self.nodes.get(&split_id).and_then(|n| n.as_tab()).map(|t| t.tab.kind);
            ui.painter().line_segment(
                [egui::pos2(divider_x, CHROME_HEIGHT_POINTS), egui::pos2(divider_x, screen_max_y)],
                egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
            );
            egui::Area::new(egui::Id::new("split_pane_label"))
                .fixed_pos(egui::pos2(divider_x + 8.0, CHROME_HEIGHT_POINTS + 6.0))
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::default()
                        .fill(TOOLBAR_FILL)
                        .corner_radius(4u8)
                        .inner_margin(egui::Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if let Some(kind) = kind {
                                    engine_dot(ui, kind, 8.0);
                                }
                                ui.label(kind.map(engine_label).unwrap_or("?"));
                            });
                        });
                });
        }
        // A freshly-spawned or just-respawned tab's content pane draws as
        // plain black until its engine-host's first real frame arrives (see
        // `record_content_pass`'s own doc comment -- the pane still clears,
        // it just has nothing to draw) -- confirmed live, a genuinely jarring
        // blank screen with zero feedback, unlike the Graph-view card's own
        // spinner for the exact same "no thumbnail yet" state. Only covers
        // the primary pane; the split view's secondary pane (view-only,
        // still a first cut per `split_tab`'s own doc comment) is left as-is.
        let has_content = self.gpu.as_ref().is_some_and(|g| g.has_tab_texture(&focused_id));
        if !has_content {
            let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
            let screen_size = self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
            let screen_width = screen_size.width as f32 / ppp;
            let screen_height = screen_size.height as f32 / ppp;
            let primary_width = if self.split_tab.is_some() { screen_width / 2.0 } else { screen_width };
            let content_rect = egui::Rect::from_min_max(
                egui::pos2(0.0, CHROME_HEIGHT_POINTS),
                egui::pos2(primary_width, screen_height),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Spinner::new().size(32.0));
                });
            });
        }
        if toggle_bookmark_requested {
            self.toggle_bookmark(&focused_id);
        }
        // Each dropdown fades in/out (`ui.set_opacity` inside the popup's
        // own Area) off `ctx.animate_bool` on its own open flag -- read
        // every frame regardless of open state (not just while open), since
        // egui's animation manager needs continuous polling to track real
        // elapsed time; polling only while open made it jump straight to
        // its target the instant re-opened instead of visibly easing, the
        // gap while closed having gone untracked.
        let tab_actions_fade =
            ui.ctx().animate_bool(egui::Id::new("tab_actions_popup_fade"), self.tab_actions_open);
        if tab_actions_fade > 0.0 {
            if let Some(rect) = tab_actions_button_rect {
                self.build_tab_actions_popup(ui, rect, &focused_id, tab_actions_fade);
            }
        }
        // Same tier/pros/cons popup Settings' own two engine pickers use --
        // see `build_engine_picker_popup`'s doc comment. Highlights the
        // split's own current engine (if one is already open) rather than
        // nothing, so re-opening this shows what's already showing.
        let split_engine_fade = ui
            .ctx()
            .animate_bool(egui::Id::new("split_engine_popup_fade"), self.split_engine_picker_open);
        if split_engine_fade > 0.0 {
            if let Some(rect) = split_engine_button_rect {
                let split_kind =
                    self.split_tab.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|t| t.tab.kind);
                let (area_rect, picked) = build_engine_picker_popup(
                    ui.ctx(),
                    "split_engine_popup",
                    rect,
                    split_kind,
                    split_engine_fade,
                    self.split_engine_picker_open,
                );
                if let Some(kind) = picked {
                    self.start_split(kind);
                    self.split_engine_picker_open = false;
                } else if clicked_outside_popup(ui, area_rect, Some(rect)) {
                    self.split_engine_picker_open = false;
                }
            }
        }
        if let Some(text) = navigate {
            let url = resolve_navigation_url(&text, self.settings.default_search_engine);
            self.navigate_tab(&focused_id, url);
            self.open_tab_suggestions.clear();
            self.search_suggestions.clear();
            self.dropdown_highlighted = None;
        }
    }

    fn build_graph_ui(&mut self, ui: &mut egui::Ui) {
        let mut new_tab_parent: Option<Option<String>> = None;
        let mut clicked_focus: Option<String> = None;
        let mut close_requested: Option<String> = None;
        // Set by a Download/Note/Image/Pdf/Json card's hover ✕ -- all five
        // close through the same `close_node_and_save` path their context
        // menu's Delete uses. A tab's ✕ keeps `close_requested` (above) and
        // a group's ✕ keeps `group_close_requested` (it dissolves, not closes).
        let mut node_close_requested: Option<String> = None;
        let mut download_clicked: Option<String> = None;
        let mut pin_toggle_requested: Option<String> = None;
        let mut remove_from_group_requested: Option<String> = None;
        let mut tab_color_cycle_requested: Option<String> = None;
        let mut mute_toggle_requested: Option<String> = None;
        // Set by `pending_new_node`'s own card -- unlike `new_tab_parent`
        // above (the toolbar's own jittered-position spawn), each of these
        // carries the exact world position that was right-clicked.
        let mut pending_new_tab_at: Option<egui::Vec2> = None;
        let mut pending_new_note_at: Option<egui::Vec2> = None;
        let mut pending_new_group_at: Option<egui::Vec2> = None;
        let mut pending_new_file_at: Option<egui::Vec2> = None;
        let mut image_clicked: Option<String> = None;
        let mut missing_file_clicked: Option<String> = None;
        let mut leave_group_requested = false;
        let mut group_close_requested: Option<String> = None;
        let mut color_cycle_requested: Option<String> = None;
        let mut group_merge_requested: Option<(String, String)> = None;
        let mut group_entered: Option<String> = None;
        let mut rename_start: Option<(String, String)> = None;
        let mut rename_commit: Option<String> = None;
        let mut rename_cancel = false;

        let mut reset_view = false;
        let mut reset_zoom = false;
        let mut graph_zoom_step: Option<f32> = None;
        let mut group_selected_requested = false;
        let mut close_selected_requested = false;
        let mut ungroup_all_requested = false;
        let mut import_group_requested = false;
        let mut shortcuts_help_button_rect: Option<egui::Rect> = None;
        let mut recently_closed_button_rect: Option<egui::Rect> = None;
        let mut leave_group_button_rect: Option<egui::Rect> = None;

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
        // frame. `self.current_group` can't change until `group_entered`/
        // `leave_group_requested` are applied at the very end of this same
        // function, so every unconditional per-frame use below sees the
        // exact same, correct-for-this-frame level throughout. Left as
        // fresh calls: the two `current_level_settle_key()` call sites
        // that run *after* a group change is applied further down (they
        // need a genuinely new key for the new level, not this frame's
        // stale one) and the handful of conditional sites deeper in this
        // function (drag-and-drop-into-group checks, the right-click
        // hit-test chain) that don't run every idle frame, so caching them
        // here would have added surface area without the same payoff.
        let tabs_at_level = self.nodes_at_current_level(ItemNodeKind::Tab);
        let downloads_at_level = self.nodes_at_current_level(ItemNodeKind::Download);
        let notes_at_level = self.nodes_at_current_level(ItemNodeKind::Note);
        let images_at_level = self.nodes_at_current_level(ItemNodeKind::Image);
        let pdfs_at_level = self.nodes_at_current_level(ItemNodeKind::Pdf);
        let jsons_at_level = self.nodes_at_current_level(ItemNodeKind::Json);
        let groups_at_level = self.nodes_at_current_level(ItemNodeKind::Group);

        // Level-of-detail: count how many nodes at this level actually
        // intersect the viewport, then flip `graph_lod_simplified` with a
        // hysteresis gap -- see `LOD_SIMPLIFY_COUNT`'s own doc comment for
        // why. Uses this frame's not-yet-updated `graph_scene_rect`, the
        // same one-frame-stale viewport the render loops' own cull check
        // already accepts.
        let lod_visible_count: usize = [
            &tabs_at_level,
            &downloads_at_level,
            &notes_at_level,
            &images_at_level,
            &pdfs_at_level,
            &jsons_at_level,
            &groups_at_level,
        ]
        .into_iter()
        .map(|ids| count_visible_nodes(&self.nodes, ids, self.graph_scene_rect))
        .sum();
        if self.graph_lod_simplified {
            if lod_visible_count < LOD_RESTORE_COUNT {
                self.graph_lod_simplified = false;
            }
        } else if lod_visible_count > LOD_SIMPLIFY_COUNT {
            self.graph_lod_simplified = true;
        }

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
                            leave_group_requested = true;
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
                                                rename_cancel = true;
                                            } else {
                                                rename_commit = Some(draft.clone());
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
                                    rename_start = Some((id.clone(), name.clone()));
                                }
                            } else {
                                // An ancestor, not the current group itself
                                // -- click to jump straight there (reuses
                                // `group_entered`, the exact same "switch
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
                                    group_entered = Some(id.clone());
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
                    // completely empty (see the early-return branch below),
                    // where the right-click empty-canvas card can't appear
                    // yet. New Note/Add File/New Group used to sit here too,
                    // but that card already offers all three identically
                    // once any node exists, and while the graph is fully
                    // empty those buttons' own flags were silently dropped
                    // anyway (their sole consumption site is unreachable
                    // from the early-return branch) -- removing them is a
                    // pure simplification, not a capability loss.
                    if ui.button("+ New Tab").clicked() {
                        new_tab_parent = Some(self.focused.clone());
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
                                    import_group_requested = true;
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
                                    ungroup_all_requested = true;
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
                    if !self.closed_tabs.is_empty() || !self.closed_groups.is_empty() || !self.closed_nodes.is_empty() {
                        let count = self.closed_tabs.len() + self.closed_groups.len() + self.closed_nodes.len();
                        let label = format!("Recently Closed ({count})");
                        let response = ui.button(label);
                        if response.clicked() {
                            self.recently_closed_open = !self.recently_closed_open;
                        }
                        recently_closed_button_rect = Some(response.rect);
                    }
                    // Rectangle-select's own action bar: only shown once a
                    // right-drag has actually selected something (see the
                    // gesture handling below, near `Scene::new()`).
                    if !self.selected_tabs.is_empty() {
                        // Matches "+ New Group" above: groups the selection
                        // into a new subgroup of whichever level is
                        // currently being viewed, not just the top.
                        if ui.button(format!("Group Selected ({})", self.selected_tabs.len())).clicked() {
                            group_selected_requested = true;
                        }
                        if ui
                            .button(format!("Close Selected ({})", self.selected_tabs.len()))
                            .clicked()
                        {
                            close_selected_requested = true;
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
        // `build_shortcuts_help`/`build_recently_closed_popup`/the three
        // context menus below are all deferred to the very end of this
        // function (past the Scene/`overlay_painter` node-rendering block),
        // not called here where their button rects are computed -- found
        // live that a card's title/pin-icon decorations (painted via
        // `overlay_painter`, a *separate* `Order::Foreground` layer created
        // further down) rendered on top of these popups, since within one
        // egui `Order` tier, layers paint in the order they're first
        // touched each frame, and `overlay_painter` didn't exist yet at
        // this point in the function.
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
            if let Some(parent) = new_tab_parent {
                let home_page = self.settings.home_page.clone();
                let group = self.current_group.clone();
                self.spawn_new_tab(parent, group, &home_page);
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
                tabs_at_level.len()
                    + downloads_at_level.len()
                    + notes_at_level.len()
                    + images_at_level.len()
                    + pdfs_at_level.len()
                    + jsons_at_level.len()
                    + groups_at_level.len(),
            );
            if self.graph_settled_key.as_ref() != Some(&current_key) {
                self.settle_current_level();
                self.graph_settled_key = Some(current_key);
            }
        }

        // Arrow-key graph navigation: moves `self.focused` to the nearest
        // tab node in the pressed direction, reusing the same field the
        // ACCENT border below already highlights and every other
        // "act on the focused tab" path (Ctrl+W, "+ New Tab"'s parent,
        // Enter/click-to-open) already reads -- so a keyboard-selected node
        // behaves exactly like a clicked one, just without the view
        // transition. Gated on the group-rename text field and quick-
        // switcher both being closed so arrow keys still do their normal
        // job there instead of hijacking focus underneath an open widget --
        // the Settings page itself needs no such check here, since it's now
        // a separate `ViewMode` this function never runs under at all.
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
            // node loop below re-detecting the pointer still sitting over
            // it) and `peek_suppressed` is set instead, checked by the peek
            // drawing code further down so it skips rendering for this
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
        // paint its label over the toolbar above.
        let overlay_painter = egui::Painter::new(
            ui.ctx().clone(),
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("graph_decoration_text")),
            scene_outer_rect,
        );

        // Rectangle-select: a right-button drag over the graph panel
        // selects every tab node its screen-space rectangle overlaps.
        // Deliberately the secondary button, not primary -- primary is
        // Scene's own pan gesture, and `.drag_pan_buttons(PRIMARY)` below
        // restricts Scene to just that button so the two never compete for
        // the same drag. Read directly off `ui.input`'s raw pointer state
        // rather than through a `Response`: this needs to work anywhere
        // over the panel, including empty space no widget claims, and
        // egui's own hit-testing/consumption (which a `Response` reflects)
        // is irrelevant to a plain "where is the mouse" query.
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
                // reach: Group is drawn last, then Json, Pdf, Image, Note,
                // Download, and Tab first. Within one kind, later in
                // `node_order` draws later, hence `.rev()`.
                let hit = [
                    ItemNodeKind::Group,
                    ItemNodeKind::Json,
                    ItemNodeKind::Pdf,
                    ItemNodeKind::Image,
                    ItemNodeKind::Note,
                    ItemNodeKind::Download,
                    ItemNodeKind::Tab,
                ]
                .into_iter()
                .find_map(|kind| {
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

        // Eased camera move into/out of a group -- see `GraphCameraTween`'s
        // doc comment for why this is a separate mechanism from
        // `Transition`. Recomputed fresh from the tween's fixed `from`/`to`
        // every frame (not derived from `self.graph_scene_rect`, which
        // `Scene::show` below may also nudge from user pan/zoom input this
        // same frame) so an in-progress user pan can't throw off where the
        // animation is actually headed.
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

        // Minimap: a small fixed overview in the graph panel's bottom-right
        // corner, screen-space (drawn via `overlay_painter`, so it's
        // unaffected by Scene's own pan/zoom the same way node labels are)
        // showing every current-level node as a dot and the main view's
        // current viewport as an outlined rect. World bounds reuse
        // `fit_all_nodes_scene_rect()` -- the same extent "Reset View" and
        // entering/leaving a group already frame to, so the minimap and the
        // main view always agree on "the whole graph". Clicking anywhere in
        // it recenters the main viewport there without changing zoom.
        let minimap_size = egui::vec2(160.0, 110.0);
        let minimap_rect = egui::Rect::from_min_size(
            scene_outer_rect.right_bottom() - minimap_size - egui::Vec2::splat(12.0),
            minimap_size,
        );
        let world_bounds = self.bounds_for_level(
            [
                &tabs_at_level,
                &downloads_at_level,
                &notes_at_level,
                &images_at_level,
                &pdfs_at_level,
                &jsons_at_level,
                &groups_at_level,
            ]
            .into_iter()
            .flatten(),
        );
        let minimap_scale = (minimap_rect.width() / world_bounds.width().max(1.0))
            .min(minimap_rect.height() / world_bounds.height().max(1.0));
        let world_to_minimap = |p: egui::Pos2| -> egui::Pos2 {
            minimap_rect.center() + (p - world_bounds.center()) * minimap_scale
        };
        overlay_painter.rect_filled(minimap_rect, 4.0, egui::Color32::from_black_alpha(160));
        for id in tabs_at_level.clone() {
            if let Some(node) = self.nodes.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 2.0, ui.visuals().text_color());
            }
        }
        for id in downloads_at_level.clone() {
            if let Some(node) = self.nodes.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 2.0, ui.visuals().weak_text_color());
            }
        }
        for id in notes_at_level.clone() {
            if let Some(node) = self.nodes.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 2.0, NOTE_CARD_COLOR);
            }
        }
        for id in images_at_level.clone() {
            if let Some(node) = self.nodes.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 2.0, ACCENT);
            }
        }
        for id in pdfs_at_level.clone() {
            if let Some(node) = self.nodes.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 2.0, ACCENT);
            }
        }
        for id in jsons_at_level.clone() {
            if let Some(node) = self.nodes.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 2.0, ACCENT);
            }
        }
        for id in groups_at_level.clone() {
            if let Some(node) = self.nodes.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 3.0, ACCENT);
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
                // Dot grid first, under everything -- a fixed-world-spacing
                // reference so panning/zooming reads as moving through a
                // real space rather than an empty void, the same idea every
                // node-graph editor (Blender, Figma, ComfyUI) uses. Spacing
                // is in *world* units (dots get farther apart on screen
                // when zoomed in, closer together zoomed out -- the
                // intended feel), while the dot's own radius is scaled by
                // `inv_zoom` so it stays a constant ~1.5 screen px
                // regardless of zoom. Reads `self.graph_scene_rect` (the
                // previous frame's visible world rect) rather than the
                // local `scene_rect` above -- that one is already mutably
                // borrowed for the whole `show` call, including this
                // closure, so it can't also be read from in here. One frame
                // of lag, same as `graph_zoom_pct`'s own toolbar readout --
                // imperceptible at any real frame rate.
                //
                // Spacing is adaptive (doubled/halved in world units, never
                // below a 20 screen-px floor) rather than a fixed 40 world
                // units -- a fixed spacing looks right near 100% zoom but
                // turns into tens of thousands of `circle_filled` calls a
                // frame at `GRAPH_ZOOM_MIN` (0.15x: a full-HD window's
                // visible world rect alone would need ~57,000 dots at 40
                // world units apart), a real per-frame cost this avoids by
                // keeping on-screen density roughly constant regardless of
                // zoom level instead.
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

                // Edges first so node cards paint on top of the lines feeding them.
                // Both endpoints must share the currently-viewed level —
                // see the physics block's own edge filter above for why.
                // One pass over every node kind (tabs, downloads, ...), so
                // each parent->child edge is drawn exactly once.
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

                for id in tabs_at_level.clone() {
                    let Some(node) = self.nodes.get(&id) else {
                        continue;
                    };
                    let rect = node_card_rect(node);
                    // Viewport culling: skip every bit of interact/paint
                    // work below for a node that isn't actually visible --
                    // the single highest-leverage fix for a graph with many
                    // more nodes than fit on screen at once. Never culls
                    // the node currently being dragged, even once it's
                    // moved outside the viewport -- `ui.interact`'s own
                    // drag tracking is what keeps a drag alive frame to
                    // frame, so skipping it mid-drag would freeze the node
                    // in place the instant it crossed the visible edge.
                    if self.dragging.as_deref() != Some(id.as_str())
                        && !rect.intersects(self.graph_scene_rect)
                    {
                        continue;
                    }
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("graph_node", id.as_str())),
                        egui::Sense::click_and_drag(),
                    );

                    let is_focused = self.focused.as_deref() == Some(id.as_str());
                    let is_hovered = self.hovered.as_deref() == Some(id.as_str()) || response.hovered();
                    let is_selected = self.selected_tabs.contains(&id);
                    // One lookup instead of the six separate `self.nodes.
                    // get(&id)` calls this card's render used to make for
                    // its own data (color, title, url, engine kind, muted,
                    // audio-playing, pinned) -- **found on a general
                    // performance audit**: each was a fresh hash + lookup
                    // for the same key, paid every frame for every tab
                    // regardless of graph size. `title`/`url` are cloned
                    // once here since they're needed past this point (the
                    // label text, the favicon origin) and can't borrow
                    // from `self.nodes` across the `&mut self` mutations
                    // (drag, hover bookkeeping) later in this same
                    // iteration.
                    let Some((tag_color, title, url, engine_kind, node_muted, node_audio_playing, node_pinned)) =
                        self.nodes.get(&id).and_then(|n| {
                            let tab = n.as_tab()?;
                            Some((
                                n.color,
                                tab.title.clone(),
                                tab.url.clone(),
                                tab.tab.kind,
                                tab.muted,
                                tab.audio_playing,
                                tab.pinned,
                            ))
                        })
                    else {
                        continue;
                    };

                    // Shared by the fill/border/glow eases below -- one
                    // hover transition, not three separately-timed ones.
                    let hover_t = ui.ctx().animate_bool_responsive(
                        egui::Id::new(("graph_node_hover", id.as_str())),
                        is_hovered,
                    );
                    let card_fill = ui.visuals().extreme_bg_color.gamma_multiply(1.0 + hover_t * 0.35);
                    // A tag color only shows through in the resting state --
                    // focused stays its own established color so "this is
                    // the open tab" remains the strongest, least ambiguous
                    // signal (not eased -- a hard override regardless of
                    // whichever hover state it happens to be transitioning
                    // through). The resting<->hovered width/color themselves
                    // now ease via `hover_t` instead of snapping.
                    let resting_color = tag_color.unwrap_or(ui.visuals().weak_text_color());
                    let (border_width, border_color) = if is_focused {
                        (2.5, ACCENT)
                    } else {
                        (
                            1.0 + hover_t * 0.5,
                            lerp_color32(resting_color, ui.visuals().text_color(), hover_t),
                        )
                    };
                    draw_card_glow(ui.painter(), rect, 2.0, border_color, inv_zoom, 1.0 + hover_t * 0.8);
                    ui.painter().rect_filled(rect, 2.0 * inv_zoom, card_fill);
                    ui.painter().rect_stroke(
                        rect,
                        2.0 * inv_zoom,
                        egui::Stroke::new(border_width * inv_zoom, border_color),
                        egui::StrokeKind::Inside,
                    );
                    // Rectangle-select's own highlight: a translucent accent
                    // wash on top of everything painted so far, distinct
                    // from the focus border above (solid outline only) so a
                    // multi-selected node still reads differently from the
                    // single keyboard-focused one when both apply at once.
                    if is_selected {
                        ui.painter().rect_filled(
                            rect,
                            2.0 * inv_zoom,
                            egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 45),
                        );
                    }

                    // LOD: the thumbnail texture blit and the title/
                    // favicon text below are this card's two genuinely
                    // expensive parts (a real GPU texture sample, and
                    // glyph rasterization queued on `overlay_painter`) --
                    // skipped once `graph_lod_simplified` is set (see its
                    // own doc comment), leaving the plain background/
                    // border/color-swatch painted above and below as the
                    // simplified card's whole appearance.
                    if !self.graph_lod_simplified {
                        let thumb_rect = rect.shrink(GRAPH_THUMB_MARGIN);
                        let thumb_id = self.gpu.as_ref().and_then(|g| g.thumbnail_id(&id));
                        if let Some(tex_id) = thumb_id {
                            ui.painter().image(
                                tex_id,
                                thumb_rect,
                                egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        } else {
                            ui.put(
                                egui::Rect::from_center_size(thumb_rect.center(), egui::vec2(24.0, 24.0) * inv_zoom),
                                egui::Spinner::new().size(24.0 * inv_zoom),
                            );
                        }

                        {
                            let label = if title.is_empty() { url.as_str() } else { title.as_str() };
                            // A flat scrim behind the title strip so it stays
                            // legible over a busy/light thumbnail — most real
                            // pages are light-background, and plain text on top
                            // of one was unreadable without this.
                            let title_strip = egui::Rect::from_min_max(
                                egui::Pos2::new(thumb_rect.left(), thumb_rect.bottom() - 18.0 * inv_zoom),
                                thumb_rect.right_bottom(),
                            );
                            ui.painter().rect_filled(
                                title_strip,
                                0.0,
                                egui::Color32::from_black_alpha(160),
                            );
                            // Engine identity as a small colored dot rather than
                            // repeating the "[Servo]"-style bracket text on every
                            // single node — the dot alone is enough once you've
                            // learned the color coding (also used in the tab
                            // strip and engine picker), and reads faster at this
                            // node's small size than another word would.
                            ui.painter().circle_filled(
                                egui::Pos2::new(title_strip.left() + 8.0 * inv_zoom, title_strip.center().y),
                                4.0 * inv_zoom,
                                engine_color(engine_kind),
                            );
                            // Below `MIN_LABEL_CARD_SCREEN_WIDTH`, skip the
                            // label entirely rather than truncating to a
                            // near-illegible sliver -- the engine-color dot
                            // above still conveys engine identity on its own.
                            if rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                                let label_font = egui::FontId::proportional(14.0);
                                // Available on-screen width for the label -- the
                                // strip's own screen width (zoom shrinks the card,
                                // but not this fixed-size text, so at low zoom the
                                // label can be wider than the card it's on) minus
                                // the fixed 16px left offset and a little breathing
                                // room on the right.
                                let label_max_width = (title_strip.width() * zoom_scale - 16.0 - 6.0).max(0.0);
                                let label =
                                    truncate_to_width(&mut self.truncate_cache, ui.ctx(), label, &label_font, label_max_width);
                                let label_pos = egui::Pos2::new(title_strip.left(), title_strip.center().y);
                                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                    overlay_painter.text(
                                        project(label_pos) + egui::vec2(16.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        label,
                                        label_font,
                                        egui::Color32::WHITE,
                                    );
                                }));
                            }
                        }
                        if let Some(tex_id) = self
                            .cached_favicon_origin(&url)
                            .and_then(|origin| self.gpu.as_ref().and_then(|g| g.favicon_id(&origin)))
                        {
                            let favicon_rect = egui::Rect::from_min_size(
                                rect.left_top() + egui::vec2(4.0, 4.0) * inv_zoom,
                                FAVICON_SIZE * inv_zoom,
                            );
                            ui.painter().image(
                                tex_id,
                                favicon_rect,
                                egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                    }
                    // Audio-playing/muted indicator: a plain colored dot
                    // (like the engine-identity one below the thumbnail),
                    // not a speaker glyph — confirmed elsewhere in this
                    // project that several Unicode symbol glyphs render as
                    // empty boxes in this app's font, so a painter-drawn
                    // shape is the reliable choice, not text. `audio_playing`
                    // is only ever true for WebView2/WebKit tabs — see
                    // `TabPayload::audio_playing`'s doc comment — but the dot
                    // also shows (and stays clickable) whenever `muted` is
                    // set, even after audio stops, so there's always a way
                    // to see and undo a mute from Graph view without first
                    // switching to Tab view.
                    let mut audio_clicked = false;
                    if node_audio_playing || node_muted {
                        let audio_rect = egui::Rect::from_center_size(
                            rect.right_top() + egui::vec2(-8.0, 8.0) * inv_zoom,
                            egui::vec2(16.0, 16.0) * inv_zoom,
                        );
                        let audio_response = ui
                            .interact(
                                audio_rect,
                                egui::Id::new(("graph_node_audio", id.as_str())),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if node_muted {
                            // Muted stays static -- nothing is actually
                            // playing to animate. A real `SPEAKER_X` icon
                            // rather than a plain red dot (migrated
                            // alongside the pin/eject icons): a bare dot
                            // only meant "muted" once you'd learned the
                            // convention, where the icon states it. Text,
                            // so drawn on `overlay_painter` like the pin/
                            // eject icons, not `ui.painter()` like the
                            // waveform's own vector bars below.
                            if !self.graph_lod_simplified {
                                let icon_pos = audio_rect.center();
                                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                    overlay_painter.text(
                                        project(icon_pos),
                                        egui::Align2::CENTER_CENTER,
                                        egui_phosphor::light::SPEAKER_X,
                                        egui::FontId::proportional(12.0),
                                        DANGER_COLOR,
                                    );
                                }));
                            }
                        } else {
                            // Actually playing: a tiny 3-bar waveform
                            // instead of a static dot -- painter-drawn
                            // rects, not a speaker/equalizer glyph, same
                            // "this app's font can't be trusted with
                            // symbol glyphs" reasoning as everywhere else.
                            // Animated off the real OS clock (`SystemTime`)
                            // rather than `ui.input(|i| i.time)` — found
                            // live that the latter reads as frozen across
                            // separate headless `SCREENSHOT`-triggered
                            // redraws despite each one genuinely re-running
                            // this code (confirmed via the also-fresh
                            // `audio_playing` state each time), while
                            // `SystemTime::now()` reliably ticks between
                            // them. Three bars on staggered sine phases so
                            // they don't all move in lockstep. This app
                            // already requests a redraw unconditionally at
                            // the end of every frame (see `RedrawRequested`'s
                            // own handling), so nothing further is needed
                            // to keep it animating on its own.
                            // A warm sage rather than a minty green -- still
                            // unambiguously "active/live" (the universal
                            // recording-light color family), just shifted to
                            // sit with the rest of this app's warm palette
                            // instead of against it.
                            let waveform_color = egui::Color32::from_rgb(150, 190, 90);
                            let t = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs_f64())
                                .unwrap_or(0.0);
                            let bar_w = 2.5 * inv_zoom;
                            let gap = 1.5 * inv_zoom;
                            let max_h = 10.0 * inv_zoom;
                            let min_h = 3.0 * inv_zoom;
                            let total_w = bar_w * 3.0 + gap * 2.0;
                            let base_x = audio_rect.center().x - total_w / 2.0;
                            let base_y = audio_rect.center().y + max_h / 2.0;
                            for i in 0..3 {
                                let phase = i as f64 * 1.7;
                                let h = min_h
                                    + (max_h - min_h) * 0.5 * (1.0 + (t * 6.0 + phase).sin() as f32);
                                let x = base_x + i as f32 * (bar_w + gap);
                                let bar_rect = egui::Rect::from_min_max(
                                    egui::Pos2::new(x, base_y - h),
                                    egui::Pos2::new(x + bar_w, base_y),
                                );
                                ui.painter().rect_filled(bar_rect, 1.0 * inv_zoom, waveform_color);
                            }
                        }
                        if audio_response.clicked() {
                            audio_clicked = true;
                            mute_toggle_requested = Some(id.clone());
                        }
                    }

                    // Pin toggle: bottom-left corner, always visible (not
                    // hover-only like the close button) so a pinned node's
                    // state stays legible at a glance. A pinned node is
                    // excluded from the force-directed layout entirely —
                    // see `TabPayload::pinned`'s own doc comment. A real
                    // `PUSH_PIN` icon (accent-colored when pinned, weak gray
                    // otherwise) rather than a plain dot -- migrated once
                    // `egui_phosphor` was already wired in for the Graph
                    // toolbar's own icons, since a pin glyph reads its own
                    // meaning at a glance where a dot only did once you'd
                    // learned "filled = pinned" as a convention. Same corner
                    // decoration as the title-strip text/eject/close "x",
                    // so drawn on the untransformed `overlay_painter` layer
                    // for crisp text at any zoom rather than `ui.painter()`
                    // (see that layer's own doc comment for why). This
                    // hit-rect is centered exactly on the card's corner (so
                    // it overlaps `rect` below) — the `response.clicked()`
                    // check further down excludes `pin_response.clicked()`
                    // so a pin click doesn't also focus the node into Tab
                    // view.
                    let pin_rect =
                        egui::Rect::from_center_size(rect.left_bottom(), egui::vec2(16.0, 16.0) * inv_zoom);
                    let pin_response = ui
                        .interact(
                            pin_rect,
                            egui::Id::new(("graph_node_pin", id.as_str())),
                            egui::Sense::click(),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    let pin_color = if node_pinned {
                        ACCENT
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    if !self.graph_lod_simplified {
                        let pin_icon_pos = pin_rect.center();
                        deferred_overlays.push(Box::new(move |overlay_painter, project| {
                            overlay_painter.text(
                                project(pin_icon_pos),
                                egui::Align2::CENTER_CENTER,
                                egui_phosphor::light::PUSH_PIN,
                                egui::FontId::proportional(14.0),
                                pin_color,
                            );
                        }));
                    }
                    if pin_response.clicked() {
                        pin_toggle_requested = Some(id.clone());
                    }

                    // Close button: a small square just outside the node's own
                    // top-right corner, given its own interact() id so it doesn't
                    // compete with the node's own click/drag sense over the same
                    // pixels. Only shown while the node is hovered — decluttering
                    // the graph at a glance was worth the tiny risk of "hover,
                    // then move a few px to reach the button" (same tradeoff
                    // real browser tab strips make for their own close buttons).
                    // Its "x" glyph (here, and on the group/download node's
                    // identical close button further down) is `egui_phosphor::
                    // regular::X` now, not the plain ASCII letter it used to be
                    // — cosmetic polish once the icon font was already wired
                    // in for the pin/eject/mute icons above.
                    if is_hovered {
                        let close_rect =
                            egui::Rect::from_center_size(rect.right_top(), egui::vec2(18.0, 18.0) * inv_zoom);
                        let close_response = ui
                            .interact(
                                close_rect,
                                egui::Id::new(("graph_node_close", id.as_str())),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        let close_fill = if close_response.hovered() {
                            DANGER_COLOR
                        } else {
                            ui.visuals().extreme_bg_color
                        };
                        ui.painter().rect_filled(close_rect, 2.0 * inv_zoom, close_fill);
                        ui.painter().rect_stroke(
                            close_rect,
                            2.0 * inv_zoom,
                            egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color()),
                            egui::StrokeKind::Inside,
                        );
                        let close_icon_pos = close_rect.center();
                        let close_icon_color = ui.visuals().text_color();
                        deferred_overlays.push(Box::new(move |overlay_painter, project| {
                            overlay_painter.text(
                                project(close_icon_pos),
                                egui::Align2::CENTER_CENTER,
                                egui_phosphor::light::X,
                                egui::FontId::proportional(14.0),
                                close_icon_color,
                            );
                        }));
                        if close_response.clicked() {
                            close_requested = Some(id.clone());
                        }
                    }
                    // Middle-click anywhere on the card closes it too, same
                    // convention a real browser's tab strip already has --
                    // an alternative to the small "x" rather than a
                    // replacement for it.
                    if response.clicked_by(egui::PointerButton::Middle) {
                        close_requested = Some(id.clone());
                    }

                    // Remove-from-group ("eject"): only shown while
                    // actually viewing a group's own subgraph -- pops just
                    // this one tab back to the top level without dissolving
                    // the whole group, which used to be the only way out
                    // (the group's own "x" returns every member at once).
                    // Opposite corner from the close button above so the
                    // two hover-revealed buttons never overlap each other.
                    let mut eject_clicked = false;
                    if is_hovered && self.current_group.is_some() {
                        let eject_rect =
                            egui::Rect::from_center_size(rect.left_top(), egui::vec2(18.0, 18.0) * inv_zoom);
                        let eject_response = ui
                            .interact(
                                eject_rect,
                                egui::Id::new(("graph_node_eject", id.as_str())),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        let eject_fill = if eject_response.hovered() {
                            ACCENT
                        } else {
                            ui.visuals().extreme_bg_color
                        };
                        ui.painter().rect_filled(eject_rect, 2.0 * inv_zoom, eject_fill);
                        ui.painter().rect_stroke(
                            eject_rect,
                            2.0 * inv_zoom,
                            egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color()),
                            egui::StrokeKind::Inside,
                        );
                        // `SIGN_OUT` rather than the bare "^" this used to
                        // be -- migrated for the same reason the pin dot
                        // was: a caret is genuinely ambiguous here (reads
                        // as "expand" as easily as "leave"), where a real
                        // "step out" icon states its own meaning.
                        let eject_icon_pos = eject_rect.center();
                        let eject_icon_color = ui.visuals().text_color();
                        deferred_overlays.push(Box::new(move |overlay_painter, project| {
                            overlay_painter.text(
                                project(eject_icon_pos),
                                egui::Align2::CENTER_CENTER,
                                egui_phosphor::light::SIGN_OUT,
                                egui::FontId::proportional(14.0),
                                eject_icon_color,
                            );
                        }));
                        if eject_response.clicked() {
                            eject_clicked = true;
                            remove_from_group_requested = Some(id.clone());
                        }
                    }

                    // Color swatch: the one corner not already claimed by
                    // the pin dot, close "x", or (inside a group) the eject
                    // button. Always shown, not hover-gated, same
                    // "glanceable identifier" reasoning as a group's own
                    // swatch.
                    let swatch_rect =
                        egui::Rect::from_center_size(rect.right_bottom(), egui::vec2(16.0, 16.0) * inv_zoom);
                    let swatch_response = ui
                        .interact(
                            swatch_rect,
                            egui::Id::new(("graph_node_swatch", id.as_str())),
                            egui::Sense::click(),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    let mut swatch_clicked = false;
                    match tag_color {
                        Some(c) => {
                            ui.painter().circle_filled(swatch_rect.center(), 6.0 * inv_zoom, c);
                        }
                        None => {
                            ui.painter().circle_stroke(
                                swatch_rect.center(),
                                6.0 * inv_zoom,
                                egui::Stroke::new(1.5 * inv_zoom, ui.visuals().weak_text_color()),
                            );
                        }
                    }
                    if swatch_response.clicked() {
                        swatch_clicked = true;
                        tab_color_cycle_requested = Some(id.clone());
                    }

                    if response.dragged() {
                        self.dragging = Some(id.clone());
                        let delta = response.drag_delta();
                        // Dragging one member of an active rectangle-
                        // selection moves the *whole* selection together,
                        // not just the card the pointer happened to grab --
                        // same "selection acts as one unit" reasoning the
                        // drop-onto-a-group-card handling just below
                        // already has for joining a group; found live that
                        // plain repositioning didn't have this yet (the
                        // other selected cards stayed put, breaking the
                        // visual selection's cohesion mid-drag).
                        if self.selected_tabs.contains(&id) {
                            for tab_id in self.selected_tabs.clone() {
                                if let Some(node) = self.nodes.get_mut(&tab_id) {
                                    node.pos += delta;
                                }
                            }
                        } else if let Some(node) = self.nodes.get_mut(&id) {
                            node.pos += delta;
                        }
                    } else if self.dragging.as_deref() == Some(id.as_str()) {
                        self.dragging = None;
                        // Dropping a tab onto a group's card joins that
                        // group, or (dropped on a plain tab instead) starts
                        // a new one with both -- kept top-level-only for
                        // now even though groups can nest (see
                        // `GroupPayload`'s own doc comment): inside a
                        // group's own subgraph, dropping onto a sibling tab
                        // already means something else (re-parent, the
                        // `else if` below), and resolving that overlap
                        // wasn't part of this pass. A nested group still
                        // gains members today via "+ New Tab"/"Group
                        // Selected" while viewing its own subgraph, or by
                        // dragging a tab already inside it onto a sibling
                        // to reorganize -- just not by dragging one in from
                        // a different branch of the tree.
                        if self.current_group.is_none() {
                            if let Some(tab_pos) = self.nodes.get(&id).map(|n| n.pos) {
                                let dropped_on_group = self.nodes_at_current_level(ItemNodeKind::Group).into_iter().find(|gid| {
                                    self.nodes.get(gid).is_some_and(|g| {
                                        egui::Rect::from_center_size(
                                            egui::Pos2::new(g.pos.x, g.pos.y),
                                            GROUP_NODE_SIZE,
                                        )
                                        .contains(egui::Pos2::new(tab_pos.x, tab_pos.y))
                                    })
                                });
                                if let Some(group_id) = dropped_on_group {
                                    // Dragging one member of an active
                                    // rectangle-selection joins the *whole*
                                    // selection, not just the node the
                                    // pointer happened to grab -- matches
                                    // "Group Selected"/"Close Selected"
                                    // already treating the selection as one
                                    // unit. A plain (unselected) drag keeps
                                    // the original single-tab behavior.
                                    if self.selected_tabs.contains(&id) {
                                        for tab_id in self.selected_tabs.drain() {
                                            if let Some(node) = self.nodes.get_mut(&tab_id) {
                                                node.group = Some(group_id.clone());
                                            }
                                        }
                                    } else if let Some(node) = self.nodes.get_mut(&id) {
                                        node.group = Some(group_id);
                                    }
                                } else {
                                    // Not dropped on a group -- check
                                    // another ungrouped tab's card instead,
                                    // creating a new group with both.
                                    // `nodes_at_current_level(Tab)` only ever
                                    // returns ungrouped tabs at the top
                                    // level (its own filter is `n.group ==
                                    // self.current_group`, i.e. `None`
                                    // here), so anything it finds is
                                    // already a valid drop target with no
                                    // extra check needed.
                                    let dropped_on_tab =
                                        self.nodes_at_current_level(ItemNodeKind::Tab).into_iter().find(|other_id| {
                                            other_id != &id
                                                && self.nodes.get(other_id).is_some_and(|other| {
                                                    egui::Rect::from_center_size(
                                                        egui::Pos2::new(other.pos.x, other.pos.y),
                                                        GRAPH_NODE_SIZE,
                                                    )
                                                    .contains(egui::Pos2::new(tab_pos.x, tab_pos.y))
                                                })
                                        });
                                    if let Some(other_id) = dropped_on_tab {
                                        let group_id = self.alloc_group_id();
                                        let pos =
                                            self.nodes.get(&other_id).map(|n| n.pos).unwrap_or(tab_pos);
                                        let name = Self::default_group_name(&group_id);
                                        self.nodes.insert(
                                            group_id.clone(),
                                            GraphNode {
                                                parent: None,
                                                group: None,
                                                pos,
                                                vel: egui::Vec2::ZERO,
                                                color: None,
                                                payload: NodePayload::Group(GroupPayload { name }),
                                            },
                                        );
                                        self.node_order.push(group_id.clone());
                                        if let Some(node) = self.nodes.get_mut(&other_id) {
                                            node.group = Some(group_id.clone());
                                        }
                                        // Same "whole selection joins, not
                                        // just the dragged node" rule the
                                        // drop-onto-an-existing-group branch
                                        // above already applies -- this
                                        // sibling gesture (drop onto a plain
                                        // tab instead) was missing it.
                                        if self.selected_tabs.contains(&id) {
                                            for tab_id in self.selected_tabs.drain() {
                                                if let Some(node) = self.nodes.get_mut(&tab_id) {
                                                    node.group = Some(group_id.clone());
                                                }
                                            }
                                        } else if let Some(node) = self.nodes.get_mut(&id) {
                                            node.group = Some(group_id);
                                        }
                                    }
                                }
                            }
                        } else if leave_group_button_rect
                            .is_some_and(|rect| ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| rect.contains(p)))
                        {
                            // Dragging a group member onto the "◀" back
                            // button ejects it one level up (to the
                            // group's own parent -- the top level, if the
                            // group itself isn't nested) -- the button's
                            // rect is screen-space (it's a toolbar widget),
                            // so this checks the raw pointer position
                            // directly rather than the node's world-space
                            // `pos` the top-level group-card/tab-card drop
                            // targets above use. Same effect as the
                            // hover-revealed eject button, just reached by
                            // dragging the card itself out through the way
                            // back instead.
                            remove_from_group_requested = Some(id.clone());
                        } else if let Some(tab_pos) = self.nodes.get(&id).map(|n| n.pos) {
                            // Dropping one member of a group's own subgraph
                            // onto a sibling tab's card re-parents it there
                            // -- the top-level equivalent gesture (drop tab
                            // onto tab) already means "create a new group
                            // with both," so this only applies once inside
                            // a group's own subgraph, where that meaning
                            // doesn't apply and no drop gesture existed at
                            // all yet. `nodes_at_current_level(Tab)` already only
                            // returns this group's own members here (its
                            // filter is `n.group == self.current_group`),
                            // so any match is a valid sibling to reparent
                            // onto. Guarded against creating a cycle (e.g.
                            // dropping a tab onto its own descendant).
                            let dropped_on_sibling =
                                self.nodes_at_current_level(ItemNodeKind::Tab).into_iter().find(|other_id| {
                                    other_id != &id
                                        && self.nodes.get(other_id).is_some_and(|other| {
                                            egui::Rect::from_center_size(
                                                egui::Pos2::new(other.pos.x, other.pos.y),
                                                GRAPH_NODE_SIZE,
                                            )
                                            .contains(egui::Pos2::new(tab_pos.x, tab_pos.y))
                                        })
                                });
                            if let Some(new_parent_id) = dropped_on_sibling {
                                if !self.would_create_cycle(&id, &new_parent_id) {
                                    if let Some(node) = self.nodes.get_mut(&id) {
                                        node.parent = Some(new_parent_id);
                                    }
                                }
                            }
                        }
                    }
                    if response.hovered() {
                        self.hovered = Some(id.clone());
                    } else if self.hovered.as_deref() == Some(id.as_str()) {
                        self.hovered = None;
                    }
                    // Excludes a click on the pin dot, eject button, or
                    // color swatch, or audio dot -- all their hit-rects
                    // overlap this card's corners, and without this guard
                    // any of them also focuses the node into Tab view (see
                    // `pin_response`'s own doc comment).
                    if response.clicked()
                        && !pin_response.clicked()
                        && !eject_clicked
                        && !swatch_clicked
                        && !audio_clicked
                    {
                        clicked_focus = Some(id.clone());
                    }

                    dim_if_filtered_out(&mut deferred_overlays, &self.graph_filter, &[&title, &url], rect, 6.0);
                }

                // Download, Note, Image, Pdf and Json cards: one shared
                // frame each (`draw_graph_card`), kind by kind in this
                // order, which is also their stacking order: a later kind
                // draws, and takes the pointer, on top of an earlier one.
                // Each kind's own click action is applied here.
                for (kind, ids) in [
                    (ItemNodeKind::Download, &downloads_at_level),
                    (ItemNodeKind::Note, &notes_at_level),
                    (ItemNodeKind::Image, &images_at_level),
                    (ItemNodeKind::Pdf, &pdfs_at_level),
                    (ItemNodeKind::Json, &jsons_at_level),
                ] {
                    for id in ids.iter() {
                        // Read before the frame: a Note's `TextEdit` can lose
                        // focus to a click on its own header this very frame,
                        // clearing `editing`, and that click must not
                        // immediately re-enter editing.
                        let note_was_editing = kind == ItemNodeKind::Note
                            && self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);
                        let Some(card) =
                            self.draw_graph_card(ui, &mut deferred_overlays, id, kind, inv_zoom, zoom_scale)
                        else {
                            continue;
                        };
                        if card.close_clicked {
                            // The ✕ consumed this frame's click, so the
                            // kind's own click action below (e.g. Image
                            // maximize) is skipped.
                            node_close_requested = Some(id.clone());
                            continue;
                        }
                        // A placeholder (see `ImagePayload::error`) has nothing
                        // to open; a click tries its file again instead.
                        let missing_file = self.nodes.get(id).is_some_and(|n| {
                            n.as_image().is_some_and(|i| i.error.is_some())
                                || n.as_json().is_some_and(|j| j.error.is_some())
                                || n.as_pdf().is_some_and(|p| matches!(p.outcome, Some(Err(_))))
                        });
                        match kind {
                            _ if missing_file => {
                                if card.response.clicked() {
                                    missing_file_clicked = Some(id.clone());
                                }
                            }
                            // Click (not drag -- egui doesn't report
                            // `clicked()` for a drag) reveals the file.
                            ItemNodeKind::Download if card.response.clicked() => download_clicked = Some(id.clone()),
                            // Click on the rendered note enters editing in place.
                            ItemNodeKind::Note if card.response.clicked() && !note_was_editing => {
                                if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                                    note.editing = true;
                                }
                            }
                            // Maximizes the image into Item view, the same
                            // gesture a Tab card uses.
                            ItemNodeKind::Image if card.response.clicked() => image_clicked = Some(id.clone()),
                            // Opens the full tree in `build_json_viewer`.
                            ItemNodeKind::Json if card.response.double_clicked() => {
                                self.json_viewer_node = Some(id.clone())
                            }
                            _ => {}
                        }
                    }
                }

                // Group nodes: rendered at whichever level `self.current_group`
                // is (groups nest -- see `GroupPayload`'s own doc comment).
                // Double-click drills into the group's own subgraph; a
                // dragged tab dropped onto one joins it (handled above, in
                // the tab loop, since that's where the drag-release fires).
                for id in groups_at_level.clone() {
                        let Some(node) = self.nodes.get(&id) else {
                            continue;
                        };
                        let rect = node_card_rect(node);
                        // Viewport culling -- see the tab loop's own comment.
                        if self.dragging.as_deref() != Some(id.as_str())
                            && !rect.intersects(self.graph_scene_rect)
                        {
                            continue;
                        }
                        let response = ui.interact(
                            rect,
                            egui::Id::new(("graph_group_node", id.as_str())),
                            egui::Sense::click_and_drag(),
                        );

                        let is_hovered = self.hovered.as_deref() == Some(id.as_str()) || response.hovered();
                        // Whether the currently-dragged node (if any) is
                        // positioned over this group right now -- the same
                        // rect-containment check `finish_draggable_node`'s
                        // own drop-release logic uses, just evaluated live
                        // during the drag itself so there's some signal
                        // *before* letting go, confirmed missing entirely
                        // live (dragging a tab toward a group showed no
                        // feedback at all until release). Not tied to
                        // `hover_t`'s own eased transition below -- real-time
                        // drag feedback should track the cursor immediately,
                        // not ease in -- and drawn as a separate, much
                        // bolder ring (not just a stronger version of the
                        // ordinary hover glow) since the dragged card itself
                        // is larger than a group's own footprint
                        // (`GRAPH_NODE_SIZE` vs `GROUP_NODE_SIZE`) and would
                        // otherwise cover a same-style highlight almost
                        // entirely.
                        let is_drag_target = self.dragging.as_deref().is_some_and(|dragging_id| {
                            dragging_id != id.as_str()
                                && self
                                    .nodes
                                    .get(dragging_id)
                                    .is_some_and(|n| rect.contains(egui::Pos2::new(n.pos.x, n.pos.y)))
                        });
                        let tag_color = self.nodes.get(&id).and_then(|n| n.color);
                        let member_ids: Vec<String> = self
                            .nodes
                            .iter()
                            .filter(|(_, n)| n.group.as_deref() == Some(id.as_str()))
                            .map(|(tid, _)| tid.clone())
                            .collect();
                        let member_count = member_ids.len();
                        let overloaded = member_count > GROUP_OVERLOAD_THRESHOLD;
                        // Shared by the fill/border/glow eases below -- one
                        // hover transition, not three separately-timed ones.
                        let hover_t = ui.ctx().animate_bool_responsive(
                            egui::Id::new(("graph_group_hover", id.as_str())),
                            is_hovered,
                        );
                        let card_fill = ui.visuals().extreme_bg_color.gamma_multiply(1.0 + hover_t * 0.35);
                        let border_color =
                            tag_color.unwrap_or(if overloaded { DANGER_COLOR } else { ACCENT });
                        draw_card_glow(ui.painter(), rect, 2.0, border_color, inv_zoom, 1.0 + hover_t * 0.8);
                        ui.painter().rect_filled(rect, 2.0 * inv_zoom, card_fill);
                        ui.painter().rect_stroke(
                            rect,
                            2.0 * inv_zoom,
                            egui::Stroke::new((1.0 + hover_t * 0.5) * inv_zoom, border_color),
                            egui::StrokeKind::Inside,
                        );
                        if is_drag_target {
                            // Expanded well past the dragged card's own
                            // (larger) footprint so it still peeks out
                            // visibly from behind it -- see `is_drag_target`'s
                            // own doc comment above.
                            ui.painter().rect_stroke(
                                rect.expand(16.0 * inv_zoom),
                                10.0 * inv_zoom,
                                egui::Stroke::new(4.0 * inv_zoom, ACCENT),
                                egui::StrokeKind::Outside,
                            );
                        }

                        // LOD: the whole member-thumbnail-grid/name/count
                        // block below (up to 4 real texture blits plus
                        // several text draws) skipped once simplified --
                        // see the tab loop's own doc comment. The plain
                        // background/border/glow above is a group's own
                        // simplified appearance, same as every other kind.
                        if !self.graph_lod_simplified {
                        if let Some(node) = self.nodes.get(&id).and_then(|n| n.as_group()) {
                            // Below `MIN_LABEL_CARD_SCREEN_WIDTH`, skip both
                            // labels entirely rather than truncating to a
                            // near-illegible sliver -- a group card has no
                            // other decoration to fall back on, so it's just
                            // a small colored card (or thumbnail grid) at
                            // this size.
                            let show_labels = rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH;
                            if member_count > 0 && !self.groups_collapsed {
                                // Member thumbnails: up to 4, a 2x2 grid
                                // filling the card above a bottom name strip
                                // -- like a folder icon previewing its own
                                // contents, using the exact same
                                // `GpuState::thumbnail_id` lookup a tab's
                                // own card already does. A cell with no
                                // thumbnail yet (still loading) falls back
                                // to a plain tinted square rather than
                                // leaving a hole.
                                let title_strip_h = 18.0 * inv_zoom;
                                let grid_rect = egui::Rect::from_min_max(
                                    rect.min,
                                    egui::Pos2::new(rect.max.x, rect.max.y - title_strip_h),
                                )
                                .shrink(4.0 * inv_zoom);
                                let gap = 3.0 * inv_zoom;
                                let cell_size = egui::vec2(
                                    (grid_rect.width() - gap) / 2.0,
                                    (grid_rect.height() - gap) / 2.0,
                                );
                                for (i, member_id) in member_ids.iter().take(4).enumerate() {
                                    let (col, row) = (i as f32 % 2.0, (i / 2) as f32);
                                    let cell_min = grid_rect.min
                                        + egui::vec2(col * (cell_size.x + gap), row * (cell_size.y + gap));
                                    let cell_rect = egui::Rect::from_min_size(cell_min, cell_size);
                                    match self.gpu.as_ref().and_then(|g| g.thumbnail_id(member_id)) {
                                        Some(tex_id) => {
                                            ui.painter().image(
                                                tex_id,
                                                cell_rect,
                                                egui::Rect::from_min_max(
                                                    egui::Pos2::new(0.0, 0.0),
                                                    egui::Pos2::new(1.0, 1.0),
                                                ),
                                                egui::Color32::WHITE,
                                            );
                                        }
                                        None => {
                                            ui.painter().rect_filled(
                                                cell_rect,
                                                2.0 * inv_zoom,
                                                ui.visuals().extreme_bg_color.gamma_multiply(1.2),
                                            );
                                        }
                                    }
                                }
                                if show_labels {
                                    if member_count > 4 {
                                        let overflow_pos = grid_rect.right_bottom();
                                        let overflow_text = format!("+{}", member_count - 4);
                                        deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                            overlay_painter.text(
                                                project(overflow_pos) + egui::vec2(-3.0, -3.0),
                                                egui::Align2::RIGHT_BOTTOM,
                                                overflow_text,
                                                egui::FontId::proportional(12.0),
                                                egui::Color32::WHITE,
                                            );
                                        }));
                                    }
                                    let title_strip = egui::Rect::from_min_max(
                                        egui::Pos2::new(rect.left(), rect.bottom() - title_strip_h),
                                        rect.right_bottom(),
                                    );
                                    ui.painter().rect_filled(
                                        title_strip,
                                        0.0,
                                        egui::Color32::from_black_alpha(160),
                                    );
                                    let name_font = egui::FontId::proportional(13.0);
                                    let name_max_width = (rect.width() * zoom_scale - 12.0).max(0.0);
                                    let name = truncate_to_width(
                                        &mut self.truncate_cache,
                                        ui.ctx(),
                                        &node.name,
                                        &name_font,
                                        name_max_width,
                                    );
                                    let name_pos = title_strip.center();
                                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                        overlay_painter.text(
                                            project(name_pos),
                                            egui::Align2::CENTER_CENTER,
                                            name,
                                            name_font,
                                            egui::Color32::WHITE,
                                        );
                                    }));
                                }
                            } else if show_labels {
                                // Nothing to preview (empty group), or
                                // previews deliberately hidden by "Collapse
                                // Groups" -- the original plain centered
                                // name + tab-count layout, count included
                                // either way so a collapsed non-empty group
                                // doesn't read as empty.
                                let name_font = egui::FontId::proportional(16.0);
                                let name_max_width = (rect.width() * zoom_scale - 12.0).max(0.0);
                                let name = truncate_to_width(
                                    &mut self.truncate_cache,
                                    ui.ctx(),
                                    &node.name,
                                    &name_font,
                                    name_max_width,
                                );
                                let name_pos = rect.center_top();
                                let name_color = ui.visuals().text_color();
                                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                    overlay_painter.text(
                                        project(name_pos) + egui::vec2(0.0, 14.0),
                                        egui::Align2::CENTER_TOP,
                                        name,
                                        name_font,
                                        name_color,
                                    );
                                }));
                                let count_label = format!(
                                    "{} member{}",
                                    member_count,
                                    if member_count == 1 { "" } else { "s" }
                                );
                                let count_pos = rect.center_bottom();
                                let count_color = ui.visuals().weak_text_color();
                                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                    overlay_painter.text(
                                        project(count_pos) + egui::vec2(0.0, -10.0),
                                        egui::Align2::CENTER_BOTTOM,
                                        count_label,
                                        egui::FontId::proportional(14.0),
                                        count_color,
                                    );
                                }));
                            }
                        }
                        }

                        if is_hovered {
                            let close_rect = egui::Rect::from_center_size(
                                rect.right_top(),
                                egui::vec2(18.0, 18.0) * inv_zoom,
                            );
                            let close_response = ui
                                .interact(
                                    close_rect,
                                    egui::Id::new(("graph_group_node_close", id.as_str())),
                                    egui::Sense::click(),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            let close_fill = if close_response.hovered() {
                                DANGER_COLOR
                            } else {
                                ui.visuals().extreme_bg_color
                            };
                            ui.painter().rect_filled(close_rect, 2.0 * inv_zoom, close_fill);
                            ui.painter().rect_stroke(
                                close_rect,
                                2.0 * inv_zoom,
                                egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color()),
                                egui::StrokeKind::Inside,
                            );
                            let close_icon_pos = close_rect.center();
                            let close_icon_color = ui.visuals().text_color();
                            deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                overlay_painter.text(
                                    project(close_icon_pos),
                                    egui::Align2::CENTER_CENTER,
                                    egui_phosphor::light::X,
                                    egui::FontId::proportional(14.0),
                                    close_icon_color,
                                );
                            }));
                            if close_response.clicked() {
                                group_close_requested = Some(id.clone());
                            }
                        }

                        // Color swatch: same bottom-left corner a tab card's
                        // pin dot occupies (groups have no pin concept of
                        // their own, so the spot is free). Always shown, not
                        // just on hover -- a group's tag color is meant to
                        // be a glanceable identifier, not a hidden control.
                        let swatch_rect =
                            egui::Rect::from_center_size(rect.left_bottom(), egui::vec2(16.0, 16.0) * inv_zoom);
                        let swatch_response = ui
                            .interact(
                                swatch_rect,
                                egui::Id::new(("graph_group_node_swatch", id.as_str())),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        match tag_color {
                            Some(c) => {
                                ui.painter().circle_filled(swatch_rect.center(), 6.0 * inv_zoom, c);
                            }
                            None => {
                                ui.painter().circle_stroke(
                                    swatch_rect.center(),
                                    6.0 * inv_zoom,
                                    egui::Stroke::new(1.5 * inv_zoom, ui.visuals().weak_text_color()),
                                );
                            }
                        }
                        if swatch_response.clicked() {
                            color_cycle_requested = Some(id.clone());
                        }

                        if response.dragged() {
                            self.dragging = Some(id.clone());
                            if let Some(node) = self.nodes.get_mut(&id) {
                                node.pos += response.drag_delta();
                            }
                        } else if self.dragging.as_deref() == Some(id.as_str()) {
                            self.dragging = None;
                            // Dropping one group's card onto another merges
                            // the dragged group's members into the target
                            // and removes the dragged group -- same
                            // "drop onto a card" language a tab already
                            // uses to join a group.
                            if let Some(dragged_pos) = self.nodes.get(&id).map(|n| n.pos) {
                                let dropped_on_group = self.nodes_at_current_level(ItemNodeKind::Group).into_iter().find(|other_id| {
                                    if other_id == &id {
                                        return false;
                                    }
                                    self.nodes.get(other_id).is_some_and(|other| {
                                        egui::Rect::from_center_size(
                                            egui::Pos2::new(other.pos.x, other.pos.y),
                                            GROUP_NODE_SIZE,
                                        )
                                        .contains(egui::Pos2::new(dragged_pos.x, dragged_pos.y))
                                    })
                                });
                                if let Some(target_id) = dropped_on_group {
                                    group_merge_requested = Some((id.clone(), target_id));
                                }
                            }
                        }
                        if response.hovered() {
                            self.hovered = Some(id.clone());
                        } else if self.hovered.as_deref() == Some(id.as_str()) {
                            self.hovered = None;
                        }
                        if response.double_clicked() {
                            group_entered = Some(id.clone());
                        }

                        if let Some(node) = self.nodes.get(&id).and_then(|n| n.as_group()) {
                            dim_if_filtered_out(&mut deferred_overlays, &self.graph_filter, &[&node.name], rect, 6.0);
                        }
                    }

                // The empty-canvas "New Tab/Note/Group/File" card -- see
                // `pending_new_node`'s own doc comment for what spawns/
                // dismisses it. A vertical text-label list, same visual
                // language every other right-click menu in this app
                // already uses (tab/group/note/image/pdf/json), rather
                // than the icon-only horizontal row this started as --
                // this was the one context menu here that didn't match.
                // Drawn the same "real egui widgets embedded in the
                // Scene's own transformed `Ui`" way a note's `TextEdit`
                // already is, so it scales/repositions with zoom exactly
                // like a real card's own contents would, without this
                // needing to be a real node itself.
                if let Some(pos) = self.pending_new_node {
                    let center_pos = egui::Pos2::new(pos.x, pos.y);
                    let row_count = if self.headless { 3.0 } else { 4.0 };
                    // `* inv_zoom`: the background box's own world-space
                    // size must grow as zoom shrinks, the same trick
                    // `draw_node_close_button`'s own `close_rect` already
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
                        &mut deferred_overlays,
                        next_row_rect(),
                        "pending_new_node_tab",
                        inv_zoom,
                        &format!("{} New Tab", egui_phosphor::light::PLUS),
                    );
                    let clicked_note = draw_new_node_menu_row(
                        ui,
                        &mut deferred_overlays,
                        next_row_rect(),
                        "pending_new_node_note",
                        inv_zoom,
                        &format!("{} New Note", egui_phosphor::light::NOTE_PENCIL),
                    );
                    let clicked_group = draw_new_node_menu_row(
                        ui,
                        &mut deferred_overlays,
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
                            &mut deferred_overlays,
                            next_row_rect(),
                            "pending_new_node_file",
                            inv_zoom,
                            &format!("{} Add File...", egui_phosphor::light::FILE_PLUS),
                        );
                    if clicked_tab {
                        pending_new_tab_at = Some(pos);
                        self.pending_new_node = None;
                    }
                    if clicked_note {
                        pending_new_note_at = Some(pos);
                        self.pending_new_node = None;
                    }
                    if clicked_group {
                        pending_new_group_at = Some(pos);
                        self.pending_new_node = None;
                    }
                    if clicked_file {
                        pending_new_file_at = Some(pos);
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

                // Peek preview: an enlarged view of the hovered node's cached
                // thumbnail, floating near it, without changing focus. Painted
                // directly in this same world-space layer (not a separate
                // `egui::Area`, which would paint in un-transformed screen
                // space) so it pans/zooms together with its node and simply
                // ends up on top by paint order, drawn after every node
                // above. Skipped while `peek_suppressed` matches -- an
                // Escape press dismissing this exact node's peek early,
                // see that field's own doc comment.
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

        // Pan momentum bookkeeping -- see `PanMomentum`'s own doc
        // comment. `actively_panning` requires the pointer to still be
        // held down (ruling out a single-frame minimap-click jump, which
        // never has "held down across multiple frames" to sample a
        // velocity from), no node currently grabbed (a node drag moves
        // the node, not the camera, so it'd contribute a bogus near-zero
        // sample anyway, but this is the explicit, intended guard), and
        // no `GraphCameraTween` in progress (that already owns
        // `graph_scene_rect` this frame).
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

        // Minimap click-to-recenter: deliberately registered *after*
        // `Scene::show` above (not alongside the minimap's own drawing,
        // which runs before it) -- egui resolves overlapping hit-testing in
        // the same layer by allocation order, last-registered on top, so an
        // `interact()` added before Scene's own background pan sense would
        // lose the minimap's screen area to it. `world_bounds`/
        // `minimap_rect`/`minimap_scale` are the same locals the drawing
        // above already computed.
        // Read raw pointer state rather than `ui.interact()`: Scene's own
        // background pan sense already covers this whole panel (it's
        // allocated over `scene_outer_rect`, which the minimap sits inside)
        // and wins egui's hit-testing for any widget added here, so a
        // normal `interact()`-based click never actually saw the pointer --
        // confirmed live via temporary logging showing zero hover events
        // even with the cursor sitting directly over the minimap. The same
        // "read `ui.input` directly, ignore widget hit-testing entirely"
        // fix the rectangle-select marquee already needed for an identical
        // reason (see that code's own doc comment).
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

        if let Some(parent) = new_tab_parent {
            let home_page = self.settings.home_page.clone();
            let group = self.current_group.clone();
            self.spawn_new_tab(parent, group, &home_page);
        }
        if let Some(id) = close_requested {
            self.close_tab(&id);
        }
        if let Some(id) = clicked_focus {
            if self.nodes.contains_key(&id) {
                self.selected_tabs.clear();
                self.start_transition_to_item(&id);
            }
        }
        if let Some(id) = image_clicked {
            if self.nodes.contains_key(&id) {
                self.selected_tabs.clear();
                self.start_transition_to_item(&id);
            }
        }
        if let Some(id) = missing_file_clicked {
            self.retry_missing_file(&id);
        }
        if let Some(id) = node_close_requested {
            self.close_node_and_save(&id);
        }
        if let Some(id) = download_clicked {
            self.reveal_node(&id);
        }
        if let Some(id) = pin_toggle_requested {
            if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                node.pinned = !node.pinned;
            }
        }
        if let Some(id) = remove_from_group_requested {
            remove_from_group(&mut self.nodes, &id);
        }
        if let Some(id) = mute_toggle_requested {
            self.toggle_muted(&id);
        }
        // The empty-canvas card's own four actions -- each spawns at the
        // exact position that was right-clicked, unlike the toolbar
        // buttons above (always jittered around world center). New Tab is
        // the one exception: `spawn_new_tab` has no explicit-position
        // parameter at all (a tab's real position is only ever derived
        // from its parent/group, since the actual engine-host process
        // spawn this triggers is asynchronous -- see `spawn_new_tab_with_kind`'s
        // own doc comment) plumbing one through wasn't part of this pass,
        // so this still lands wherever the normal "+ New Tab" jitter puts
        // it, just parented/grouped the same as any other new tab here.
        if pending_new_tab_at.is_some() {
            let home_page = self.settings.home_page.clone();
            let group = self.current_group.clone();
            self.spawn_new_tab(self.focused.clone(), group, &home_page);
        }
        if let Some(pos) = pending_new_note_at {
            let id = self.alloc_note_id();
            self.insert_note_at(id, pos);
        }
        if let Some(pos) = pending_new_group_at {
            let id = self.alloc_group_id();
            self.insert_group_at(id, pos);
        }
        if pending_new_file_at.is_some() {
            self.add_file_node(pending_new_file_at);
        }
        if group_selected_requested && !self.selected_tabs.is_empty() {
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
        if close_selected_requested {
            for id in std::mem::take(&mut self.selected_tabs) {
                self.close_tab(&id);
            }
        }
        if ungroup_all_requested {
            for id in self.nodes_at_current_level(ItemNodeKind::Group) {
                self.dissolve_group(&id);
            }
        }
        if import_group_requested {
            if let Some(path) = rfd::FileDialog::new().add_filter("Group", &["json"]).pick_file() {
                match group_export::load_from(&path) {
                    Ok(export) => self.import_group_export(export),
                    Err(err) => self.queue_toast(err),
                }
            }
        }
        // Applied before `leave_group_requested` below: the "◀ Groups"
        // button and the rename `TextEdit` share one toolbar row, so
        // clicking "◀ Groups" while renaming causes the text field to lose
        // focus the same frame (egui's ordinary click-away focus
        // transfer), setting both `rename_commit` and
        // `leave_group_requested` at once. Committing first means
        // `self.renaming_group.take()` here observes the real value;
        // `leave_group_requested`'s own clear becomes a harmless no-op on
        // whatever `take()` already emptied, instead of the other order
        // silently discarding the typed name before it's ever read.
        if let Some(new_name) = rename_commit {
            if let Some((id, _)) = self.renaming_group.take() {
                let trimmed = new_name.trim();
                if !trimmed.is_empty() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_group_mut()) {
                        node.name = trimmed.to_string();
                    }
                }
            }
        }
        if rename_cancel {
            self.renaming_group = None;
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
        // from the Tab<->Graph `Transition` below.
        if let Some(id) = group_entered {
            self.current_group = Some(id);
            self.settle_current_level();
            self.graph_settled_key = Some(self.current_level_settle_key());
            self.graph_camera_tween = Some(GraphCameraTween {
                from: self.graph_scene_rect,
                to: self.fit_all_nodes_scene_rect(),
                start: std::time::Instant::now(),
            });
            // A rectangle-selection belongs to the level it was drawn on —
            // carrying it into a different level left the toolbar's "Group
            // Selected"/"Close Selected" buttons acting on tabs that aren't
            // even visible here anymore.
            self.selected_tabs.clear();
            self.rect_select_start = None;
            self.pending_new_node = None;
            self.node_menu = None;
        }
        if leave_group_requested {
            // One level up, not straight to the top -- same "go to this
            // group's own parent" rule `dissolve_group`/the drag-to-eject
            // handler above both use, now that groups nest.
            self.current_group =
                self.current_group.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.group.clone());
            self.settle_current_level();
            self.graph_settled_key = Some(self.current_level_settle_key());
            self.graph_camera_tween = Some(GraphCameraTween {
                from: self.graph_scene_rect,
                to: self.fit_all_nodes_scene_rect(),
                start: std::time::Instant::now(),
            });
            self.renaming_group = None;
            self.selected_tabs.clear();
            self.rect_select_start = None;
            self.pending_new_node = None;
            self.node_menu = None;
        }
        if let Some(id) = group_close_requested {
            self.dissolve_group(&id);
        }
        // A tab's and a group's color-cycle swatch click both just advance
        // the same `node.color` field the same way -- `tab_color_cycle_
        // requested`/`color_cycle_requested` can't both be Some in the same
        // frame (each is only set by its own card's swatch, one click at a
        // time), so `.or()` just picks whichever fired.
        if let Some(id) = tab_color_cycle_requested.or(color_cycle_requested) {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.color = next_tag_color(node.color);
            }
        }
        if let Some((source_id, target_id)) = group_merge_requested {
            // Refuse if `target_id` is nested inside `source_id` (directly
            // or transitively, walking the same `.group` chain
            // `group_breadcrumb` does) -- reassigning source's own members
            // to target while target is one of them would leave it (or a
            // group between them) pointing at itself once source is
            // removed below, a self-reference cycle the tab-reparent drag
            // path already guards against via `would_create_cycle`.
            let mut ancestor = Some(target_id.clone());
            let mut creates_cycle = false;
            while let Some(cur) = ancestor {
                if cur == source_id {
                    creates_cycle = true;
                    break;
                }
                ancestor = self.nodes.get(&cur).and_then(|n| n.group.clone());
            }
            if !creates_cycle {
                for node in self.nodes.values_mut() {
                    if node.group.as_deref() == Some(source_id.as_str()) {
                        node.group = Some(target_id.clone());
                    }
                }
                self.nodes.remove(&source_id);
                self.node_order.retain(|g| g != &source_id);
            }
        }
        if let Some((id, draft)) = rename_start {
            self.renaming_group = Some((id, draft));
            self.pending_focus_rename = true;
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
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes().with_title("manifold").with_window_icon(load_app_icon());
        if self.headless {
            // NOT `.with_visible(false)`: found live that a genuinely
            // invisible window's swapchain never successfully acquires a
            // frame on Windows (DWM permanently reports it occluded, so
            // `Surface::get_current_texture()` never returns `Success` —
            // confirmed by instrumenting `gpu::GpuState::render` and seeing
            // it bail out at that exact point on *every* frame, forever).
            // Positioning a real, visible-but-off-screen window instead
            // keeps DWM compositing it normally (frames actually render,
            // screenshots actually work) while it never overlaps the user's
            // actual screen — solving the real problem (focus/visual
            // contention with the user's own desktop use) without hitting
            // this platform limitation. `with_skip_taskbar` keeps it out of
            // the taskbar/alt-tab list since there's nothing to switch to.
            attrs = attrs
                .with_position(winit::dpi::PhysicalPosition::new(-32000, -32000))
                .with_decorations(false)
                .with_skip_taskbar(true);
        } else {
            // Restore the window to where the user last left it -- see
            // `Settings::window_width`'s own doc comment for why this is
            // `LogicalSize`, not `PhysicalSize`: winit/Windows converts a
            // logical size to whatever physical size is correct for the
            // monitor the window actually lands on, rather than reapplying
            // a stale physical pixel count that Per-Monitor-V2 would then
            // rescale (and this app would then re-save, compounding it
            // further on every launch). `None` (no saved geometry yet)
            // just leaves `attrs` untouched, falling back to winit's own
            // default the same as before this existed.
            if let (Some(width), Some(height)) = (self.settings.window_width, self.settings.window_height) {
                attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(width as f64, height as f64));
            }
            if let (Some(x), Some(y)) = (self.settings.window_x, self.settings.window_y) {
                attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
            }
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        apply_dark_titlebar(&window);
        self.gpu = Some(GpuState::new(window.clone()));
        self.egui_state = Some(egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        ));
        self.window = Some(window);
        if !self.restore_session() {
            if self.settings.has_shown_first_launch_demo {
                self.spawn_root_tab();
            } else {
                self.spawn_first_launch_demo();
                self.settings.has_shown_first_launch_demo = true;
                self.save_settings();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Debug(cmd) => {
                // egui hit-tests pointer input against the widgets of the
                // last frame, which may predate whatever changed since the
                // previous command (a tab that just spawned, suggestions
                // that just arrived); a CLICK on a node right after
                // WAITTABS missed 3 runs out of 3. So draw the current
                // state first.
                let pointer = matches!(
                    cmd,
                    DebugCommand::MouseMove { .. }
                        | DebugCommand::MouseDown { .. }
                        | DebugCommand::MouseUp { .. }
                        | DebugCommand::Click { .. }
                        | DebugCommand::Scroll { .. }
                );
                if self.headless && pointer {
                    self.dispatch_window_event(event_loop, WindowEvent::RedrawRequested);
                }
                self.handle_debug_command(event_loop, cmd);
                // The off-screen headless window can get `RedrawRequested`
                // (WM_PAINT) only sporadically -- measured ~40 frames in
                // 20 s under load (another process on the GPU, engines
                // starting), ~60 fps on a quiet machine -- so several commands could
                // land in one frame: a `MOUSEDOWN` and the `MOUSEMOVE`s after
                // it were hit-tested at the pointer's final position, turning
                // a card drag into a background pan. Rendering a frame right
                // here gives every command its own frame.
                if self.headless {
                    self.dispatch_window_event(event_loop, WindowEvent::RedrawRequested);
                }
            }
            UserEvent::TabSpawned(outcome) => self.handle_tab_spawn_result(outcome),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed_by_egui = match (&mut self.egui_state, &self.window) {
            (Some(state), Some(window)) => state.on_window_event(window.as_ref(), &event).consumed,
            _ => false,
        };
        let in_tab_view = self.showing_tab_view();

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // Fires when the window moves to a monitor with a different
            // scale factor (also on a live resolution/scaling change).
            // Confirmed live this was a real gap, not just theoretical:
            // instrumenting a live cross-monitor drag showed a transitional
            // `send_resize_to` call computed with the size already updated
            // for the new monitor but `window.scale_factor()` still
            // reporting the *old* monitor's value -- a genuine race between
            // winit's size and scale-factor updates that `Resized` alone
            // doesn't protect against. This handler re-triggers the same
            // tab-resize notification `Resized` does, so whichever event
            // actually lands last with the fully-settled state (typically
            // this one, since it specifically announces the scale change
            // completed) corrects any bad intermediate value the other
            // sent. `window.inner_size()` (not the stale pre-change size)
            // is used for the GPU surface for the same reason.
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(gpu) = &mut self.gpu {
                    if let Some(size) = self.window.as_ref().map(|w| w.inner_size()) {
                        gpu.resize(size.width, size.height);
                    }
                }
                if let Some(id) = self.focused.clone() {
                    self.send_resize_to(&id);
                }
                if let Some(id) = self.split_tab.clone() {
                    self.send_resize_to(&id);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                if let Some(id) = self.focused.clone() {
                    self.send_resize_to(&id);
                }
                if let Some(id) = self.split_tab.clone() {
                    self.send_resize_to(&id);
                }
                // Not saved in headless mode -- see `Settings::window_width`'s
                // own doc comment for why that would corrupt a real saved
                // geometry with the deliberate off-screen placeholder size.
                // Converted to logical points here (dividing out `ppp`) --
                // see that same doc comment for why saving raw physical
                // pixels compounded into runaway window growth across
                // launches on a mixed-DPI multi-monitor setup.
                if !self.headless && size.width > 0 && size.height > 0 {
                    let ppp = self.ppp();
                    self.settings.window_width = Some((size.width as f64 / ppp).round() as u32);
                    self.settings.window_height = Some((size.height as f64 / ppp).round() as u32);
                    self.save_settings();
                }
            }
            WindowEvent::Moved(position) => {
                if !self.headless {
                    self.settings.window_x = Some(position.x);
                    self.settings.window_y = Some(position.y);
                    self.save_settings();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                let chrome_height = self.chrome_height_px() as f64;
                let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| position.x < divider);
                if in_tab_view && !consumed_by_egui && position.y >= chrome_height && in_primary_pane {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            node.tab.send(&HostToEngine::Input(InputEvent::MouseMove {
                                x: position.x,
                                y: position.y - chrome_height,
                                modifiers: self.key_modifiers,
                            }));
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                // `egui_winit` reports the right button as "consumed"
                // unconditionally (confirmed live via temporary logging:
                // `consumed_by_egui=true` even over this project's own raw
                // content quad, which isn't a real egui widget and has
                // nothing there to legitimately claim it) — bypassed for
                // right and middle specifically so a right/middle-click on
                // tab content still reaches `InputEvent::MouseDown`/
                // `MouseUp` the same way every left-click already does.
                // Middle was missing this even though every engine-host's
                // own `translate_mouse_button`/native input path already
                // fully implements `MouseButton::Middle`.
                let egui_wants_it = consumed_by_egui
                    && button != WinitMouseButton::Right
                    && button != WinitMouseButton::Middle;
                let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| x < divider);
                let in_content = in_tab_view && !egui_wants_it && y >= chrome_height && in_primary_pane;
                if let Some(button) = translate_mouse_button(button) {
                    // A release always goes to whichever tab is mid-drag,
                    // even when it lands outside the content area -- see
                    // `native_drag_tab`'s doc comment for the stuck-button
                    // bug that gating it the same way as a press caused.
                    let target = match state {
                        ElementState::Pressed if in_content => self.focused.clone(),
                        ElementState::Released => if matches!(button, MouseButton::Left) {
                            self.native_drag_tab.take()
                        } else {
                            None
                        }
                        .or_else(|| in_content.then(|| self.focused.clone()).flatten()),
                        ElementState::Pressed => None,
                    };
                    if let Some(id) = target {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            let (x, y) = (x, y - chrome_height);
                            let event = match state {
                                ElementState::Pressed => {
                                    InputEvent::MouseDown { x, y, button, modifiers: self.key_modifiers }
                                }
                                ElementState::Released => {
                                    InputEvent::MouseUp { x, y, button, modifiers: self.key_modifiers }
                                }
                            };
                            node.tab.send(&HostToEngine::Input(event));
                            if state == ElementState::Pressed && matches!(button, MouseButton::Left) {
                                self.native_drag_tab = Some(id);
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                let in_primary_pane = self.split_divider_x_px().is_none_or(|divider| x < divider);
                if in_tab_view && !consumed_by_egui && y >= chrome_height && in_primary_pane {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            // `LineDelta` is a fixed synthetic unit
                            // (`* 20.0`), not a measurement of anything;
                            // `PixelDelta` is already a real physical-pixel
                            // amount, which is the space every engine's
                            // surface (and so this event's `x`/`y`) is in --
                            // neither needs converting.
                            let (dx, dy) = match delta {
                                winit::event::MouseScrollDelta::LineDelta(x, y) => {
                                    (x as f64 * 20.0, y as f64 * 20.0)
                                }
                                winit::event::MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                            };
                            node.tab.send(&HostToEngine::Input(InputEvent::Scroll {
                                x,
                                y: y - chrome_height,
                                delta_x: dx,
                                delta_y: dy,
                                modifiers: self.key_modifiers,
                            }));
                        }
                    }
                }
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.key_modifiers = winit_modifiers_to_bits(new_modifiers.state());
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if in_tab_view && !consumed_by_egui {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                            if let Some(key) = translate_key_code(event.physical_key) {
                                let input = match event.state {
                                    ElementState::Pressed => InputEvent::KeyDown {
                                        key,
                                        modifiers: self.key_modifiers,
                                    },
                                    ElementState::Released => InputEvent::KeyUp {
                                        key,
                                        modifiers: self.key_modifiers,
                                    },
                                };
                                node.tab.send(&HostToEngine::Input(input));
                            }
                            // The produced text (after layout/shift/dead-key
                            // processing) is separate from the physical key
                            // above — a page's text field wants this, a
                            // shortcut wants the key. winit only reports
                            // `text` on the press, not the release.
                            if event.state == ElementState::Pressed {
                                if let Some(text) = &event.text {
                                    for ch in text.chars() {
                                        node.tab.send(&HostToEngine::Input(InputEvent::Char {
                                            codepoint: ch as u32,
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let focused_tab = self
                    .focused
                    .as_deref()
                    .and_then(|id| self.nodes.get(id))
                    .and_then(|n| n.as_tab())
                    .map(|t| format!("{:?} {}", t.tab.kind, t.url));
                crash_report::record_state(format!(
                    "view_mode={:?} headless={} node_count={} focused_tab={:?}",
                    self.view_mode, self.headless, self.nodes.len(), focused_tab
                ));
                self.drain_tab_events();
                // Checked before building this frame's UI so that, the
                // instant a `Transition` completes, this same frame already
                // renders the real destination view instead of an extra
                // stale transitioning frame.
                self.finish_transition_if_done();

                let egui_output = match (self.egui_state.take(), self.window.clone()) {
                    (Some(mut state), Some(window)) => {
                        let mut raw_input = state.take_egui_input(window.as_ref());
                        // `egui::Scene` (the graph view's pan/zoom container)
                        // hardcodes ctrl+scroll as its zoom gesture and plain
                        // scroll as pan -- not configurable via its builder,
                        // since `register_pan_and_zoom` reads
                        // `zoom_delta()`/`smooth_scroll_delta()` straight off
                        // `InputState`. Forcing every `MouseWheel` event's
                        // modifiers to ctrl+command here, before egui ever
                        // computes those derived deltas, makes plain scroll
                        // register as the zoom gesture instead -- the graph
                        // already has a separate pan gesture (dragging empty
                        // background), so losing scroll-as-pan costs nothing.
                        // Tab view's own page-content scroll is untouched:
                        // that's a distinct `InputEvent::Scroll` forwarded to
                        // the focused tab's engine-host, not egui's Scene.
                        // `egui` treats "is this scroll a zoom gesture" as a
                        // single frame-wide flag (`InputState::begin_pass`
                        // computes it once from the whole event list, not
                        // per-widget or per-hover-region) -- forcing it
                        // unconditionally would silently zero out
                        // `smooth_scroll_delta` for *any* `ScrollArea` shown
                        // over Graph view, not just `Scene`. Scoped to
                        // `ViewMode::Graph` specifically, so the Settings
                        // page's own `ScrollArea` (a separate `ViewMode` now)
                        // always keeps normal scroll.
                        if self.view_mode == ViewMode::Graph {
                            for event in &mut raw_input.events {
                                if let egui::Event::MouseWheel { modifiers, .. } = event {
                                    modifiers.ctrl = true;
                                    modifiers.command = true;
                                }
                            }
                        }
                        // The hidden headless window never has OS focus, and egui
                        // reports no widget focus (`Response::has_focus`) in an
                        // unfocused window, so the address bar could never take
                        // the typed text.
                        if self.headless {
                            raw_input.focused = true;
                        }
                        raw_input.events.append(&mut self.debug_input_events);
                        if self.address_bar_focused {
                            if let Some(text) = self.debug_address_bar_typing.take() {
                                raw_input.events.push(debug_key_event(egui::Key::A, egui::Modifiers::COMMAND));
                                raw_input.events.push(egui::Event::Text(text));
                            }
                        }
                        self.update_window_title();
                        let ctx = self.egui_ctx.clone();
                        let output = ctx.run_ui(raw_input, |ui| self.build_ui(ui));
                        state.handle_platform_output(window.as_ref(), output.platform_output.clone());
                        self.egui_state = Some(state);
                        Some(output)
                    }
                    (state, _) => {
                        self.egui_state = state;
                        None
                    }
                };

                // The content quad's rect(s) (points, screen space) and
                // which tab's texture to draw at each — a `Transition`
                // overrides this to one entry, the animated in-between rect
                // and that tab's id, regardless of `view_mode`; otherwise
                // the normal Tab-view rule, which is two entries (primary +
                // secondary, side by side) while a split is open, one
                // otherwise, none in Graph view.
                let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
                let content_panes_pts: Vec<(egui::Rect, String)> = if let Some(transition) = &self.transition {
                    // Only a Tab has a GPU pane; an Image's transition is
                    // painted through egui instead (see `build_ui`).
                    if self.nodes.get(&transition.node_id).is_some_and(|n| n.as_tab().is_some()) {
                        vec![(self.transition_content_rect_points(transition), transition.node_id.clone())]
                    } else {
                        vec![]
                    }
                } else if self.showing_tab_view() {
                    let screen = self
                        .window
                        .as_ref()
                        .map(|w| w.inner_size())
                        .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
                    let top = self.chrome_height_points();
                    let full_rect = egui::Rect::from_min_size(
                        egui::pos2(0.0, top),
                        egui::vec2(screen.width as f32 / ppp, (screen.height as f32 / ppp - top).max(0.0)),
                    );
                    match (&self.focused, &self.split_tab) {
                        (Some(focused_id), Some(split_id)) => {
                            let mid_x = full_rect.center().x;
                            let left = egui::Rect::from_min_max(full_rect.min, egui::pos2(mid_x, full_rect.max.y));
                            let right = egui::Rect::from_min_max(egui::pos2(mid_x, full_rect.min.y), full_rect.max);
                            vec![(left, focused_id.clone()), (right, split_id.clone())]
                        }
                        (Some(focused_id), None) => vec![(full_rect, focused_id.clone())],
                        (None, _) => vec![],
                    }
                } else {
                    vec![]
                };
                let content_panes_px: Vec<((f32, f32, f32, f32), &str)> = content_panes_pts
                    .iter()
                    .map(|(rect, id)| {
                        (
                            (rect.min.x * ppp, rect.min.y * ppp, rect.width() * ppp, rect.height() * ppp),
                            id.as_str(),
                        )
                    })
                    .collect();

                let content_on_top = self.transition.is_some();
                // Not taken yet -- a transient surface-acquire failure
                // (Outdated/Occluded/...) means the screenshot was never
                // attempted this frame, and the request must survive to
                // retry on a later one instead of being silently dropped.
                let screenshot_path = self.pending_screenshot.clone();
                if let Some(gpu) = &mut self.gpu {
                    let outcome = match egui_output {
                        Some(output) => gpu.render(
                            &content_panes_px,
                            content_on_top,
                            Some((&self.egui_ctx, output)),
                            screenshot_path.as_deref(),
                        ),
                        None => gpu.render(
                            &content_panes_px,
                            content_on_top,
                            None,
                            screenshot_path.as_deref(),
                        ),
                    };
                    if outcome.surface_ok {
                        self.pending_screenshot = None;
                        // The PNG is written by now (`render` saves it).
                        if outcome.screenshot_result.is_some() {
                            if let Some(waiter) = self.screenshot_waiter.take() {
                                let _ = waiter.send(());
                            }
                        }
                        match outcome.screenshot_result {
                            Some(Ok(())) => {
                                if let Some(path) = screenshot_path {
                                    if let Some(pdf_path) = self.pending_print_to_pdf.take() {
                                        match pdf::write_image_as_pdf(
                                            std::path::Path::new(&path),
                                            std::path::Path::new(&pdf_path),
                                        ) {
                                            Ok(()) => {
                                                self.queue_toast(format!("Saved {pdf_path}"));
                                            }
                                            Err(err) => eprintln!("print to pdf: {err}"),
                                        }
                                        let _ = std::fs::remove_file(&path);
                                    } else {
                                        self.queue_toast(format!("Saved {path}"));
                                    }
                                }
                            }
                            Some(Err(err)) => eprintln!("screenshot: {err}"),
                            None => {}
                        }
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }

                // `ADDRESSBAR` text is typed in the first frame the address
                // bar has focus; keep frames coming until then. Dropped if
                // Tab view went away, since the bar won't be built again.
                if self.debug_address_bar_typing.is_some() {
                    if self.showing_tab_view() || self.transition.is_some() {
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    } else {
                        eprintln!("ADDRESSBAR: left Tab view before the address bar took focus; text dropped");
                        self.debug_address_bar_typing = None;
                    }
                }

                // See `quit_after_redraws`'s doc comment.
                if let Some(left) = self.quit_after_redraws {
                    if !self.debug_input_pending() || left == 0 {
                        event_loop.exit();
                    } else {
                        self.quit_after_redraws = Some(left - 1);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl App {
    /// Feeds a synthetic `WindowEvent` through the exact same handling real
    /// OS input goes through (`Self::window_event`) — used by
    /// `handle_debug_command` for mouse commands, so headless mouse-driven
    /// testing exercises real egui hit-testing/rendering-timing rather than
    /// bypassing it (see `debug_control`'s doc comment for why this matters:
    /// it's the only way headless testing could have caught the dropdown
    /// click-swallowing bug fixed in the autocomplete feature). A no-op if
    /// the window doesn't exist yet (shouldn't happen in practice — commands
    /// only arrive after `resumed` has created it).
    fn dispatch_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(window) = &self.window else { return };
        let id = window.id();
        self.window_event(event_loop, id, event);
    }

    /// Debug input that still needs a frame: queued keys, `ADDRESSBAR`
    /// text not typed yet, a focus request, or a screenshot.
    fn debug_input_pending(&self) -> bool {
        !self.debug_input_events.is_empty()
            || self.debug_address_bar_typing.is_some()
            || self.pending_focus_address_bar
            || self.pending_screenshot.is_some()
    }

    /// Applies one headless `DebugCommand` (see `debug_control`'s doc
    /// comment for the overall design: mouse commands are real synthetic
    /// `WindowEvent`s, keyboard/text commands call the equivalent effect
    /// directly since `winit::event::KeyEvent` has no public constructor).
    fn handle_debug_command(&mut self, event_loop: &ActiveEventLoop, cmd: DebugCommand) {
        // A transition only ends in a frame, and frames can stall for
        // seconds on a busy GPU, so one that ran out long ago could still
        // be `Some` here and make the ENTER/ESCAPE/arrow guards below drop
        // their command. It's timed by the clock, so end it by the clock.
        self.finish_transition_if_done();
        match cmd {
            DebugCommand::MouseMove { x, y } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
            }
            DebugCommand::MouseDown { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state: ElementState::Pressed,
                        button,
                    },
                );
            }
            DebugCommand::MouseUp { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state: ElementState::Released,
                        button,
                    },
                );
            }
            DebugCommand::Click { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state: ElementState::Pressed,
                        button,
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state: ElementState::Released,
                        button,
                    },
                );
            }
            DebugCommand::Scroll { x, y, delta_x, delta_y } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(x, y),
                    },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseWheel {
                        device_id: DeviceId::dummy(),
                        delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(delta_x, delta_y)),
                        phase: TouchPhase::Moved,
                    },
                );
            }
            // A click on the address bar, then select-all and typing: focus
            // is requested now, and the text is typed as egui `Text` input in
            // the first frame the bar has it (see `debug_address_bar_typing`).
            DebugCommand::AddressBar(text) if self.showing_tab_view() => {
                self.pending_focus_address_bar = true;
                self.debug_address_bar_typing = Some(text);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            DebugCommand::AddressBar(_) => eprintln!("debug: ADDRESSBAR ignored: not in Tab view"),
            // Real key presses for the next frame's egui input, so they go
            // through the same checks a keyboard does, in any view.
            DebugCommand::Enter => self.debug_input_events.push(debug_key_event(egui::Key::Enter, egui::Modifiers::NONE)),
            DebugCommand::Escape => self.debug_input_events.push(debug_key_event(egui::Key::Escape, egui::Modifiers::NONE)),
            DebugCommand::ArrowDown => self.debug_input_events.push(debug_key_event(egui::Key::ArrowDown, egui::Modifiers::NONE)),
            DebugCommand::ArrowUp => self.debug_input_events.push(debug_key_event(egui::Key::ArrowUp, egui::Modifiers::NONE)),
            DebugCommand::GraphFocusLeft => self.move_graph_focus(egui::vec2(-1.0, 0.0)),
            DebugCommand::GraphFocusRight => self.move_graph_focus(egui::vec2(1.0, 0.0)),
            DebugCommand::GraphFocusUp => self.move_graph_focus(egui::vec2(0.0, -1.0)),
            DebugCommand::GraphFocusDown => self.move_graph_focus(egui::vec2(0.0, 1.0)),
            DebugCommand::GraphOpenFocused => self.open_focused_graph_node(),
            DebugCommand::TypeIntoPage(text) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        for ch in text.chars() {
                            node.tab
                                .send(&HostToEngine::Input(InputEvent::Char { codepoint: ch as u32 }));
                        }
                    }
                }
            }
            DebugCommand::KeyPage(key) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        node.tab.send(&HostToEngine::Input(InputEvent::KeyDown {
                            key,
                            modifiers: self.key_modifiers,
                        }));
                        node.tab.send(&HostToEngine::Input(InputEvent::KeyUp {
                            key,
                            modifiers: self.key_modifiers,
                        }));
                    }
                }
            }
            DebugCommand::ReopenTab => self.reopen_closed_tab_shortcut(),
            DebugCommand::ZoomIn => self.zoom_in_shortcut(),
            DebugCommand::ZoomOut => self.zoom_out_shortcut(),
            DebugCommand::ZoomReset => self.zoom_reset_shortcut(),
            DebugCommand::OpenDevTools => self.open_devtools_shortcut(),
            DebugCommand::NewTab => self.new_tab_shortcut(),
            DebugCommand::CloseTab => self.close_focused_tab_shortcut(),
            DebugCommand::FocusAddressBar => self.focus_address_bar_shortcut(),
            DebugCommand::NextTab => self.cycle_focused_tab(true),
            DebugCommand::PrevTab => self.cycle_focused_tab(false),
            DebugCommand::OpenFindBar => self.open_find_bar_shortcut(),
            DebugCommand::FindText(text) => {
                self.find_text = text;
                self.send_find_start();
            }
            DebugCommand::FindMatchCase(match_case) => {
                self.find_match_case = match_case;
                self.send_find_start();
            }
            DebugCommand::FindNext => self.send_find_navigate(true),
            DebugCommand::FindPrevious => self.send_find_navigate(false),
            DebugCommand::CloseFindBar => self.close_find_bar(),
            DebugCommand::SetVisible(visible) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        node.tab.set_visible(visible);
                    }
                }
            }
            // Only ever intercepted by the stdin reader itself.
            DebugCommand::Screenshot(_) | DebugCommand::Sleep(_) => {}
            DebugCommand::NotifyWhenScreenshotTaken(path, waiter) => {
                self.pending_screenshot = Some(path);
                self.screenshot_waiter = Some(waiter);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            DebugCommand::Resize(width, height) => {
                if let Some(window) = &self.window {
                    let _ = window.request_inner_size(PhysicalSize::new(width, height));
                }
            }
            DebugCommand::QuickSwitchOpen => {
                self.quick_switcher = Some(QuickSwitcher { query: String::new(), selected: 0 });
                self.pending_focus_quick_switcher = true;
            }
            DebugCommand::QuickSwitchText(text) => {
                if let Some(switcher) = &mut self.quick_switcher {
                    switcher.query = text;
                    switcher.selected = 0;
                }
            }
            DebugCommand::QuickSwitchDown => self.quick_switcher_move(1),
            DebugCommand::QuickSwitchUp => self.quick_switcher_move(-1),
            DebugCommand::QuickSwitchEnter => self.quick_switcher_confirm(),
            DebugCommand::QuickSwitchEscape => self.quick_switcher = None,
            DebugCommand::SessionNameText(text) => self.session_name_draft = text,
            DebugCommand::GraphFilterText(text) => self.graph_filter = text,
            DebugCommand::HistoryFilterText(text) => self.history_filter = text,
            DebugCommand::BookmarksFilterText(text) => self.bookmarks_filter = text,
            DebugCommand::ToggleView => self.toggle_view_mode_shortcut(),
            DebugCommand::ToggleSettingsPage => self.toggle_settings_page(),
            DebugCommand::ExportSession(path) => {
                self.build_session_snapshot().save_to(std::path::Path::new(&path));
            }
            DebugCommand::ExportState(path) => {
                let focused_url = self.focused.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|t| t.url.clone());
                let suggestions: Vec<String> = self
                    .combined_suggestions()
                    .into_iter()
                    .map(|s| match s {
                        Suggestion::OpenTab { tab_id, .. } => format!("tab:{tab_id}"),
                        Suggestion::Search { query } => query,
                    })
                    .collect();
                let state = serde_json::json!({
                    "tab_view": self.showing_tab_view(),
                    "transition": self.transition.is_some(),
                    "focused_url": focused_url,
                    "address_bar_text": self.address_bar_text,
                    "suggestions": suggestions,
                    "highlighted": self.dropdown_highlighted,
                });
                if let Err(err) = std::fs::write(&path, state.to_string()) {
                    eprintln!("EXPORTSTATE: couldn't write {path}: {err}");
                }
            }
            DebugCommand::ImportSession(path) => {
                match session::Session::load_from(std::path::Path::new(&path)) {
                    Ok(session) => self.pending_import = Some(session),
                    Err(err) => self.queue_toast(err),
                }
            }
            DebugCommand::ConfirmImport => {
                if let Some(session) = self.pending_import.take() {
                    self.replace_running_session(session);
                }
            }
            DebugCommand::CancelImport => {
                self.pending_import = None;
            }
            DebugCommand::RenameGroup => self.rename_current_group_shortcut(),
            DebugCommand::BookmarkFolderText(text) => {
                if let Some((_, draft)) = &mut self.renaming_bookmark_folder {
                    *draft = text;
                }
            }
            DebugCommand::DismissPeek => self.dismiss_hover_peek(),
            DebugCommand::ExportGroup(path) => {
                if let Some((group_id, _)) = self
                    .node_menu
                    .clone()
                    .filter(|(id, _)| self.nodes.get(id).is_some_and(|node| node.as_group().is_some()))
                {
                    if let Some(export) = self.export_group(&group_id) {
                        group_export::save_to(&export, std::path::Path::new(&path));
                    }
                }
            }
            DebugCommand::ImportGroup(path) => {
                match group_export::load_from(std::path::Path::new(&path)) {
                    Ok(export) => self.import_group_export(export),
                    Err(err) => self.queue_toast(err),
                }
            }
            DebugCommand::DuplicateTab => self.duplicate_focused_tab_shortcut(),
            DebugCommand::ExportBookmarks(path) => {
                bookmarks::save_to(&self.bookmarks, std::path::Path::new(&path));
            }
            DebugCommand::ImportBookmarks(path) => {
                match bookmarks::load_from(std::path::Path::new(&path)) {
                    Ok(imported) => {
                        self.bookmarks = imported;
                        bookmarks::save(&self.bookmarks);
                    }
                    Err(err) => self.queue_toast(err),
                }
            }
            DebugCommand::PrintToPdf(path) => self.start_print_to_pdf(path),
            DebugCommand::TogglePin => self.pin_focused_tab_shortcut(),
            DebugCommand::Reload => self.reload_focused_tab_shortcut(),
            DebugCommand::NewGroup => self.new_group_shortcut(),
            DebugCommand::NewNote => self.new_note_shortcut(),
            DebugCommand::AddImage(path) => {
                if self.insert_image_node(path) {
                    self.save_session();
                }
            }
            DebugCommand::AddPdf(path) => {
                self.insert_pdf_node(path);
                self.save_session();
            }
            DebugCommand::AddJson(path) => {
                if self.insert_json_node(path) {
                    self.save_session();
                }
            }
            // Only ever intercepted by the stdin reader itself.
            DebugCommand::WaitTabs(_) => {}
            DebugCommand::NotifyWhenTabsSpawned(waiter) => {
                if self.tab_spawns_in_flight == 0 {
                    let _ = waiter.send(());
                } else {
                    self.tabs_spawned_waiters.push(waiter);
                }
            }
            DebugCommand::AddDownload(path) => {
                if let Some(tab_id) = self.focused.clone() {
                    self.insert_download_node(tab_id, path, None);
                }
            }
            DebugCommand::Split(kind) => self.start_split(kind),
            DebugCommand::CloseSplit => self.close_split(),
            DebugCommand::ExpandJsonAsGraph(id) => {
                self.expand_json_as_graph(&id);
                self.save_session();
            }
            DebugCommand::CloseGroup(id) => {
                if self.nodes.get(&id).is_some_and(|n| n.as_group().is_some()) {
                    self.close_group(&id);
                }
            }
            DebugCommand::TriggerFirstLaunchDemo => self.spawn_first_launch_demo(),
            DebugCommand::Help => debug_control::print_help(),
            DebugCommand::CloseAllLevel => self.close_all_at_current_level(),
            DebugCommand::ToggleShortcutsHelp => {
                self.shortcuts_help_open = !self.shortcuts_help_open;
            }
            DebugCommand::Quit => {
                // See `quit_after_redraws`'s doc comment: exiting right
                // now could drop a pending Enter/Escape/arrow command, or a
                // just-requested Screenshot that hasn't been captured yet
                // (`pending_screenshot` is only consumed inside a
                // `RedrawRequested` — a `Screenshot` immediately followed by
                // `Quit`, with no frame in between, would otherwise silently
                // lose the screenshot the same way an unconsumed Enter
                // could).
                if self.debug_input_pending() {
                    self.quit_after_redraws = Some(3);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                } else {
                    event_loop.exit();
                }
            }
        }
    }

    /// Drains every tab's `events_rx` (not just the focused one — a
    /// background tab can still crash, or finish an in-flight swap, while
    /// suspended) and applies the results. Frame uploads are collected and
    /// applied after crash/swap handling, since either of those can remove
    /// the very graph node a queued `FrameReady` refers to — applying to a
    /// since-removed tab_id is harmless (checked at upload time via `gpu`
    /// only, no `self.nodes` lookup needed there) but there's no reason to
    /// upload a stale frame for a tab that's mid-replacement.
    fn drain_tab_events(&mut self) {
        struct FrameUpdate {
            tab_id: String,
            /// Socket/shared-memory id this tab's engine-host actually used
            /// (see `EngineTab::socket_id`) — usually equal to `tab_id`,
            /// but can diverge after the stale-pipe fallback in
            /// `process_manager::spawn_tab`. Only used to look up the
            /// shared frame buffer below; GPU texture bookkeeping stays
            /// keyed by `tab_id`.
            socket_id: String,
            kind: EngineKind,
            width: u32,
            height: u32,
            pixels: Option<Vec<u8>>,
        }

        let tab_ids: Vec<String> = self.node_order.clone();
        // Keyed by tab id, not a plain `Vec` -- only the *latest* `FrameReady`
        // for a tab ever matters (each one fully supersedes the last in
        // `GpuState::upload_frame`'s own `tab_textures.insert`), matching the
        // same "keep only the newest, drop the rest" policy the WebView2
        // engine-host's own capture code already applies one layer down (see
        // `capture.rs`'s `latest` field). A plain `Vec` here used to `push`
        // every buffered `FrameReady` this drain found and then process each
        // one in turn -- harmless while the main loop keeps up with the
        // engine-host's own frame rate, but a real, confirmed bug once it
        // falls behind even slightly: found live via temporary
        // instrumentation showing bursts of up to 81 queued frames processed
        // in a single tick for one idle tab, each one a real, wasted GPU
        // texture+bind-group+thumbnail creation immediately thrown away by
        // the next -- severe enough burst GPU churn to both stall the main
        // thread for whole seconds at a time (Windows' own "(Not
        // Responding)", confirmed live) and outpace wgpu's own resource
        // reclamation, observed as continuous unbounded memory growth
        // (roughly 20MB/s from one idle default WebView2 tab). This bug
        // itself is engine-agnostic -- every engine's `FrameReady` messages
        // go through this same drain loop -- but WebView2 is the one that
        // actually triggers it in practice: its `Windows.Graphics.Capture`
        // pipeline captures continuously at the compositor's own refresh
        // rate regardless of whether the page changed, while CEF/Servo/
        // WebKit's paint-driven capture sends far fewer frames for a static,
        // idle page, staying well within what this loop could already keep
        // up with (confirmed live: an otherwise-identical idle-memory test
        // with CEF as the default engine stayed flat).
        let mut frame_updates: std::collections::HashMap<String, FrameUpdate> =
            std::collections::HashMap::new();
        let mut swap_finishes: Vec<(String, EngineKind, String)> = Vec::new();
        let mut crashes: Vec<(String, String)> = Vec::new();
        let mut open_tab_requests: Vec<(String, String)> = Vec::new();
        // `(tab_id, url, kind)` triples to hand to `redirect_fetcher` once
        // this whole per-tab loop finishes -- deferred for the same reason
        // as `download_events` below (`node` holds a live mutable borrow of
        // this one `self.nodes` entry for the whole drain; the actual
        // tab-to-node conversion, once the fetch completes, needs
        // `self.nodes.remove`/`insert`, so even just *starting* the fetch
        // here stays consistent with that rule rather than being a special
        // case).
        let mut redirect_content_tabs: Vec<(String, String, RedirectKind)> = Vec::new();
        // Download events, deferred the same way as `frame_updates`/
        // `crashes`/etc. above rather than applied inline: unlike before the
        // Tab/Download merge onto one shared `self.nodes` map, `node` here
        // is itself a live mutable borrow of one `self.nodes` entry for the
        // whole `try_recv` drain below -- a same-loop `self.nodes.insert`/
        // `values_mut()` for a download would now conflict with that borrow
        // (previously fine: download bookkeeping lived in a *different*
        // field, `self.downloads`, than the `self.tabs` entry `node`
        // borrowed).
        // One ordered `Vec`, not three separate ones, so a fast download's
        // `Started`+`Progress`+`Completed` (all arriving within the same
        // `try_recv` drain) still get applied in that same relative order
        // after the loop -- three separate `Vec`s would lose the ordering
        // between event *kinds*, only preserving it within each kind.
        enum DownloadEvent {
            Started { tab_id: String, path: String, total_bytes: Option<u64> },
            Progress { path: String, bytes_received: u64 },
            Completed { path: String },
        }
        let mut download_events: Vec<DownloadEvent> = Vec::new();
        // Whether any tab's real (settled) url changed this pass — used to
        // trigger one `save_session()` after the loop below rather than one
        // per `NavigationStateChanged` (title/history-only changes and a
        // same-url reload both leave this `false`; url actually changing is
        // the one thing worth persisting here).
        let mut nav_url_changed = false;
        // Read up front: the loop below holds a mutable borrow of `self.nodes`
        // throughout, so `self.ppp()` can't be called from inside it.
        let ppp = self.ppp() as f32;

        for tab_id in &tab_ids {
            let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) else {
                continue;
            };
            while let Ok(msg) = node.tab.events_rx.try_recv() {
                match msg {
                    EngineToHost::FrameReady {
                        width,
                        height,
                        pixels,
                        ..
                    } => {
                        frame_updates.insert(
                            tab_id.clone(),
                            FrameUpdate {
                                tab_id: tab_id.clone(),
                                socket_id: node.tab.socket_id.clone(),
                                kind: node.tab.kind,
                                width,
                                height,
                                pixels,
                            },
                        );
                    }
                    EngineToHost::StateSnapshot(state) => {
                        if let Some(target) = node.swap_target.take() {
                            swap_finishes.push((tab_id.clone(), target, state.url));
                        }
                    }
                    EngineToHost::NavigationStateChanged {
                        url,
                        title,
                        can_go_back,
                        can_go_forward,
                        loading,
                    } => {
                        if node.url != url {
                            nav_url_changed = true;
                            node.json_body = None;
                            self.history.push(HistoryEntry { title: title.clone(), url: url.clone() });
                            if self.history.len() > MAX_HISTORY_ENTRIES {
                                self.history.remove(0);
                            }
                            // The origin is known as soon as the url is --
                            // no need to wait for the page to actually
                            // finish loading, unlike a page's own favicon
                            // link tag this project doesn't parse anyway
                            // (see `favicon_fetch`'s own doc comment).
                            // `favicon_resolved`/`FaviconFetcher`'s own
                            // `in_flight` set both dedupe repeat requests,
                            // so this can fire on every url change with no
                            // extra bookkeeping needed here.
                            if let Some(origin) = favicon_fetch::url_origin(&url) {
                                if !self.favicon_resolved.contains(&origin) {
                                    self.favicon_fetcher.request(origin);
                                }
                            }
                        }
                        node.url = url;
                        node.title = title;
                        node.can_go_back = can_go_back;
                        node.can_go_forward = can_go_forward;
                        node.loading = loading;
                    }
                    EngineToHost::Crashed { reason } => {
                        crashes.push((tab_id.clone(), reason));
                    }
                    EngineToHost::OpenTabRequested { url } => {
                        open_tab_requests.push((tab_id.clone(), url));
                    }
                    EngineToHost::FindResult { count, active_index } => {
                        // Ignore a result from a tab that isn't the one the
                        // find bar is currently searching — shouldn't
                        // normally happen (`close_find_bar` sends
                        // `FindStop` on every focus change), but a stray
                        // late result from just before a switch is cheap to
                        // guard against.
                        if self.focused.as_deref() == Some(tab_id.as_str()) {
                            self.find_result = Some((count, active_index));
                        }
                    }
                    EngineToHost::AudioPlayingChanged { playing } => {
                        node.audio_playing = playing;
                    }
                    EngineToHost::JsonBody { body } => {
                        node.json_body = Some(body);
                    }
                    EngineToHost::RedirectableContent { content_type } => {
                        if let Some(kind) = redirect_fetch::classify(&content_type) {
                            redirect_content_tabs.push((tab_id.clone(), node.url.clone(), kind));
                        }
                    }
                    EngineToHost::ContextMenuLink { x, y, href } => {
                        // Content-area coordinates -> window point space.
                        // The engine echoes back the same physical-pixel
                        // surface coordinates the right-click was sent in
                        // (see `ipc_protocol::InputEvent`'s doc comment), so
                        // this is the one place that has to divide out the
                        // DPI scale: egui lays out in logical points. Then
                        // shifted down past the toolbar, since Tab view's
                        // content starts below it (see
                        // `CHROME_HEIGHT_POINTS`'s other call sites). Only
                        // meaningful for the tab currently on screen.
                        if self.focused.as_deref() == Some(tab_id.as_str()) {
                            self.context_menu = Some(ContextMenuState {
                                x: x as f32 / ppp,
                                y: y as f32 / ppp + CHROME_HEIGHT_POINTS,
                                href,
                            });
                        }
                    }
                    // Deferred to after this whole `tab_ids` loop (see
                    // `download_events`'s own doc comment) -- `node` here
                    // holds a live mutable borrow of this one `self.nodes`
                    // entry for the whole `try_recv` drain, so a same-loop
                    // `self.nodes.insert`/`values_mut()` for a download
                    // would conflict with it.
                    EngineToHost::DownloadStarted { path, total_bytes } => {
                        download_events.push(DownloadEvent::Started {
                            tab_id: tab_id.clone(),
                            path,
                            total_bytes,
                        });
                    }
                    EngineToHost::DownloadProgress { path, bytes_received } => {
                        // Only the *last* Progress value for a given path
                        // ever has any visible effect -- the apply loop
                        // below just overwrites `bytes_received`
                        // unconditionally, so a run of same-path Progress
                        // events back to back is the same "processed every
                        // one, only the newest visible" waste `FrameReady`'s
                        // own drain had, just far cheaper per instance (an
                        // O(n) node scan, not a GPU texture). Coalesced here
                        // by updating the last-pushed entry in place when
                        // it's already a Progress for this same path,
                        // rather than a separate HashMap -- keeps
                        // Started/Progress/Completed ordering (see this
                        // `Vec`'s own doc comment) exactly as before for
                        // everything else, since only adjacent same-path
                        // entries coalesce.
                        if let Some(DownloadEvent::Progress { path: last_path, bytes_received: last_bytes }) =
                            download_events.last_mut()
                        {
                            if *last_path == path {
                                *last_bytes = bytes_received;
                            } else {
                                download_events.push(DownloadEvent::Progress { path, bytes_received });
                            }
                        } else {
                            download_events.push(DownloadEvent::Progress { path, bytes_received });
                        }
                    }
                    EngineToHost::DownloadCompleted { path } => {
                        // Not `self.queue_toast(...)` -- `node` above still
                        // holds a live borrow of `self.nodes` for this whole
                        // drain, which a method call on `self` can't see
                        // past; the disjoint field can be pushed to directly.
                        self.save_toast_queue.push_back(format!("Saved {path}"));
                        download_events.push(DownloadEvent::Completed { path });
                    }
                }
            }
        }
        for event in download_events {
            match event {
                DownloadEvent::Started { tab_id, path, total_bytes } => {
                    self.insert_download_node(tab_id, path, total_bytes);
                }
                DownloadEvent::Progress { path, bytes_received } => {
                    if let Some(dl) =
                        self.nodes.values_mut().find_map(|n| n.as_download_mut().filter(|d| d.path == path))
                    {
                        dl.bytes_received = bytes_received;
                    }
                }
                DownloadEvent::Completed { path } => {
                    if let Some(dl) =
                        self.nodes.values_mut().find_map(|n| n.as_download_mut().filter(|d| d.path == path))
                    {
                        dl.completed = true;
                    }
                }
            }
        }
        if nav_url_changed {
            self.save_session();
        }

        for (tab_id, reason) in crashes {
            let Some((kind, url, parent, group, pinned, color, zoom_factor, pos, vel)) =
                self.nodes.get(&tab_id).and_then(|n| {
                    n.as_tab().map(|t| {
                        (
                            t.tab.kind,
                            t.url.clone(),
                            n.parent.clone(),
                            n.group.clone(),
                            t.pinned,
                            n.color,
                            t.zoom_factor,
                            n.pos,
                            n.vel,
                        )
                    })
                })
            else {
                continue;
            };
            eprintln!("engine-host crashed ({reason}) for {tab_id}, restarting at {url}");
            // Previously silent from the actual app's own UI -- this only
            // ever showed up in the console, so a user with no terminal
            // open had no way to know a tab had crashed and quietly come
            // back, e.g. mid-navigation with unsaved page state lost.
            // Reuses the same toast infrastructure "Saved <path>" already
            // has (generalized to a plain message, see `save_toast`'s own
            // doc comment) rather than a second, separate notification
            // mechanism.
            self.queue_toast(format!("Tab crashed, restarted: {url}"));
            self.nodes.remove(&tab_id);
            // Same stale-state guard `close_tab`/`finish_swap` apply — see
            // `finish_swap`'s own comment on this.
            if self.dragging.as_deref() == Some(tab_id.as_str()) {
                self.dragging = None;
            }
            if self.hovered.as_deref() == Some(tab_id.as_str()) {
                self.hovered = None;
            }
            self.respawn_tab(&tab_id, kind, url, parent, group, pinned, color, zoom_factor, pos, vel);
        }

        for (tab_id, target, url) in swap_finishes {
            self.finish_swap(&tab_id, target, url);
        }

        // Page-triggered new tab (target=_blank/window.open()/ctrl+click,
        // intercepted by CEF/WebKit - see EngineToHost::OpenTabRequested's
        // doc comment): a child of whichever tab requested it, staying in
        // the current view rather than auto-focusing, same as an explicit
        // "+ New Tab" click.
        for (requesting_tab_id, url) in open_tab_requests {
            let parent = Some(requesting_tab_id);
            let group = self.group_of(&parent);
            self.spawn_new_tab(parent, group, &url);
        }

        // See `ipc_protocol::EngineToHost::RedirectableContent`'s own doc
        // comment -- kicks off `manifold`'s own independent fetch of this
        // tab's current url; the actual tab-to-node conversion happens once
        // that completes, in `redirect_fetcher`'s own `tick()` drain below,
        // not here.
        for (tab_id, url, kind) in redirect_content_tabs {
            self.redirect_fetcher.request(tab_id, url, kind);
        }

        for (origin, result) in self.favicon_fetcher.tick() {
            self.favicon_resolved.insert(origin.clone());
            if let Some(image) = result {
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_favicon(&origin, &image.rgba, image.width, image.height);
                }
            }
        }

        for (id, result) in self.pdf_fetcher.tick() {
            // The node may have been deleted while this render was still
            // in flight -- skip a late-arriving result entirely rather
            // than uploading a texture nothing will ever free (`delete`'s
            // own `gpu.free_image` already ran, before this result could
            // possibly have arrived, so a late upload here would be a real
            // leak, not just wasted work).
            if self.nodes.get(&id).and_then(|n| n.as_pdf()).is_none() {
                continue;
            }
            match result {
                Ok(rendered) => {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.upload_image(&id, &rendered.rgba, rendered.width, rendered.height);
                    }
                    if let Some(pdf) = self.nodes.get_mut(&id).and_then(|n| n.as_pdf_mut()) {
                        pdf.outcome = Some(Ok(PdfOutcome {
                            width: rendered.width,
                            height: rendered.height,
                            page_count: rendered.page_count,
                        }));
                    }
                }
                // Toasted like a failed image/JSON load, and kept as the
                // card's tooltip with the file named.
                Err(err) => {
                    let Some(path) = self.nodes.get(&id).and_then(|n| n.as_pdf()).map(|p| p.path.clone()) else {
                        continue;
                    };
                    eprintln!("pdf: couldn't open {path}: {err}");
                    let name = crate::json_file::file_name(std::path::Path::new(&path));
                    let err = format!("Couldn't open PDF {name}: {err}");
                    self.queue_toast(err.clone());
                    if let Some(pdf) = self.nodes.get_mut(&id).and_then(|n| n.as_pdf_mut()) {
                        pdf.outcome = Some(Err(err));
                    }
                }
            }
        }

        for result in self.redirect_fetcher.tick() {
            // The tab may have been closed, or navigated somewhere else,
            // while this fetch was still in flight -- only convert it if
            // it's still the same tab still showing the same url that
            // triggered the fetch in the first place.
            let still_current = self
                .nodes
                .get(&result.tab_id)
                .and_then(|n| n.as_tab())
                .is_some_and(|t| t.url == result.url);
            if let (true, Some(path)) = (still_current, result.path) {
                match result.kind {
                    RedirectKind::Image => self.redirect_tab_to_image(&result.tab_id, path),
                }
            }
        }

        for update in frame_updates.into_values() {
            let format = match update.kind {
                EngineKind::Servo | EngineKind::Webkit => wgpu::TextureFormat::Rgba8UnormSrgb,
                EngineKind::Cef | EngineKind::Webview2 => wgpu::TextureFormat::Bgra8UnormSrgb,
            };
            if let Some(pixels) = update.pixels {
                // RemoteTransport engine-host (WebKit over WSL2): no shared
                // memory with this process, so the pixels travel inline in
                // the message itself.
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_frame(&update.tab_id, &pixels, update.width, update.height, format);
                }
            } else if let Ok(buf) = SharedFrameBuffer::open_existing(
                frame_path(&update.socket_id),
                update.width,
                update.height,
            ) {
                // The engine-host truncates-then-grows this file when
                // (re)creating it (see SharedFrameBuffer::create), which
                // briefly exposes a 0-byte or otherwise wrong-sized file to
                // a concurrent reader. Skip this frame rather than panic on
                // a mismatched slice — the next redraw tries again.
                let expected_len = (update.width as usize) * (update.height as usize) * 4;
                if buf.as_slice().len() == expected_len {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.upload_frame(&update.tab_id, buf.as_slice(), update.width, update.height, format);
                    }
                }
            }
        }
    }
}

fn translate_mouse_button(button: WinitMouseButton) -> Option<MouseButton> {
    match button {
        WinitMouseButton::Left => Some(MouseButton::Left),
        WinitMouseButton::Right => Some(MouseButton::Right),
        WinitMouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

fn winit_modifiers_to_bits(state: ModifiersState) -> u8 {
    let mut bits = 0u8;
    if state.shift_key() {
        bits |= modifiers::SHIFT;
    }
    if state.control_key() {
        bits |= modifiers::CONTROL;
    }
    if state.alt_key() {
        bits |= modifiers::ALT;
    }
    if state.super_key() {
        bits |= modifiers::SUPER;
    }
    bits
}

/// Translates winit's cross-platform physical key into our own
/// engine-agnostic `ipc_protocol::KeyCode` — deliberately not forwarding
/// winit's `KeyCode` directly over IPC (not meant to be a stable wire
/// format across winit versions); see that type's own doc comment. Common
/// keys only, matching `KeyCode`'s own scope.
fn translate_key_code(key: PhysicalKey) -> Option<KeyCode> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        WinitKeyCode::KeyA => KeyCode::KeyA,
        WinitKeyCode::KeyB => KeyCode::KeyB,
        WinitKeyCode::KeyC => KeyCode::KeyC,
        WinitKeyCode::KeyD => KeyCode::KeyD,
        WinitKeyCode::KeyE => KeyCode::KeyE,
        WinitKeyCode::KeyF => KeyCode::KeyF,
        WinitKeyCode::KeyG => KeyCode::KeyG,
        WinitKeyCode::KeyH => KeyCode::KeyH,
        WinitKeyCode::KeyI => KeyCode::KeyI,
        WinitKeyCode::KeyJ => KeyCode::KeyJ,
        WinitKeyCode::KeyK => KeyCode::KeyK,
        WinitKeyCode::KeyL => KeyCode::KeyL,
        WinitKeyCode::KeyM => KeyCode::KeyM,
        WinitKeyCode::KeyN => KeyCode::KeyN,
        WinitKeyCode::KeyO => KeyCode::KeyO,
        WinitKeyCode::KeyP => KeyCode::KeyP,
        WinitKeyCode::KeyQ => KeyCode::KeyQ,
        WinitKeyCode::KeyR => KeyCode::KeyR,
        WinitKeyCode::KeyS => KeyCode::KeyS,
        WinitKeyCode::KeyT => KeyCode::KeyT,
        WinitKeyCode::KeyU => KeyCode::KeyU,
        WinitKeyCode::KeyV => KeyCode::KeyV,
        WinitKeyCode::KeyW => KeyCode::KeyW,
        WinitKeyCode::KeyX => KeyCode::KeyX,
        WinitKeyCode::KeyY => KeyCode::KeyY,
        WinitKeyCode::KeyZ => KeyCode::KeyZ,
        WinitKeyCode::Digit0 => KeyCode::Digit0,
        WinitKeyCode::Digit1 => KeyCode::Digit1,
        WinitKeyCode::Digit2 => KeyCode::Digit2,
        WinitKeyCode::Digit3 => KeyCode::Digit3,
        WinitKeyCode::Digit4 => KeyCode::Digit4,
        WinitKeyCode::Digit5 => KeyCode::Digit5,
        WinitKeyCode::Digit6 => KeyCode::Digit6,
        WinitKeyCode::Digit7 => KeyCode::Digit7,
        WinitKeyCode::Digit8 => KeyCode::Digit8,
        WinitKeyCode::Digit9 => KeyCode::Digit9,
        WinitKeyCode::Enter => KeyCode::Enter,
        WinitKeyCode::Backspace => KeyCode::Backspace,
        WinitKeyCode::Tab => KeyCode::Tab,
        WinitKeyCode::Space => KeyCode::Space,
        WinitKeyCode::Escape => KeyCode::Escape,
        WinitKeyCode::Delete => KeyCode::Delete,
        WinitKeyCode::ArrowLeft => KeyCode::ArrowLeft,
        WinitKeyCode::ArrowRight => KeyCode::ArrowRight,
        WinitKeyCode::ArrowUp => KeyCode::ArrowUp,
        WinitKeyCode::ArrowDown => KeyCode::ArrowDown,
        WinitKeyCode::Home => KeyCode::Home,
        WinitKeyCode::End => KeyCode::End,
        WinitKeyCode::PageUp => KeyCode::PageUp,
        WinitKeyCode::PageDown => KeyCode::PageDown,
        WinitKeyCode::ShiftLeft => KeyCode::ShiftLeft,
        WinitKeyCode::ShiftRight => KeyCode::ShiftRight,
        WinitKeyCode::ControlLeft => KeyCode::ControlLeft,
        WinitKeyCode::ControlRight => KeyCode::ControlRight,
        WinitKeyCode::AltLeft => KeyCode::AltLeft,
        WinitKeyCode::AltRight => KeyCode::AltRight,
        _ => return None,
    })
}

fn main() {
    // Must happen before the event loop/window are created. Confirmed live
    // that this process was *not* actually DPI-aware despite the assumption
    // that `winit` sets this up automatically: `window.scale_factor()`
    // (`self.ppp()`) reported `1.0` on a real 125%-scaled display -- the
    // exact same DPI-virtualization symptom `manifold-engine-webview2`'s own
    // `main()` had (see that module's doc comment on
    // `SetProcessDpiAwarenessContext` for the full mechanism). Without this,
    // the whole app runs on virtualized, bitmap-stretched coordinates:
    // `content_size_px()` (and so every engine's rendered surface) comes out
    // at the wrong real size, and the cursor positions winit reports don't
    // line up with where the content actually is on screen.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    crash_report::install();
    let headless = std::env::args().any(|arg| arg == "--headless");
    // Independent of `--headless` -- accepts the exact same stdin-driven
    // `DebugCommand` protocol, but on a completely normal, visible,
    // on-screen window (real position, file dialogs available, window
    // geometry saved) rather than headless mode's off-screen one. Doesn't
    // touch `app.headless` itself, since none of that other headless-only
    // behavior is what this flag is for -- only `spawn_stdin_reader`'s own
    // gate below cares about it. Lets a real running window be driven by
    // debug commands (e.g. `SCREENSHOT`, which already just captures
    // whatever `GpuState::render` actually drew, headless or not) instead
    // of scripting an OS-level window capture around it.
    let debug_commands = std::env::args().any(|arg| arg == "--debug-commands");

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    app.headless = headless;
    // Must be set before `run_app` — `resumed()` calls `spawn_root_tab`,
    // which needs this to hand its background spawn thread a way to report
    // back (see `TabSpawnOutcome`'s doc comment).
    app.event_proxy = Some(event_loop.create_proxy());
    if headless || debug_commands {
        debug_control::spawn_stdin_reader(event_loop.create_proxy());
    }

    event_loop.run_app(&mut app).expect("run event loop");
}

#[cfg(test)]
mod tests {
    use super::*;

    // `Tab` is left out: `TabPayload` owns a live `EngineTab`, which a unit
    // test can't construct. Its arm is covered by live Tab-view runs instead.
    #[test]
    fn maps_each_constructible_payload_variant_to_its_own_kind() {
        assert_eq!(item_node_kind(&NodePayload::Download(DownloadPayload { path: "file.bin".into(), bytes_received: 0, total_bytes: Some(1024), completed: false })), ItemNodeKind::Download);
        assert_eq!(item_node_kind(&NodePayload::Group(GroupPayload { name: "g".into() })), ItemNodeKind::Group);
        assert_eq!(item_node_kind(&NodePayload::Note(NotePayload { text: "n".into(), editing: false })), ItemNodeKind::Note);
        assert_eq!(item_node_kind(&NodePayload::Image(ImagePayload { path: "img.png".into(), width: 100, height: 100, error: None })), ItemNodeKind::Image);
        assert_eq!(item_node_kind(&NodePayload::Pdf(PdfPayload { path: "doc.pdf".into(), outcome: None })), ItemNodeKind::Pdf);
        assert_eq!(item_node_kind(&NodePayload::Json(JsonPayload { path: "data.json".into(), value: serde_json::Value::Null, key: None, error: None })), ItemNodeKind::Json);
    }

    #[test]
    fn in_reading_order_reads_first_then_second_in_both_directions() {
        for layout in [egui::Layout::left_to_right(egui::Align::Min), egui::Layout::right_to_left(egui::Align::Min)] {
            egui::__run_test_ui(|ui| {
                ui.with_layout(layout, |ui| {
                    let slot = |_: &mut (), ui: &mut egui::Ui| {
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover()).0
                    };
                    let (first, second) = in_reading_order(ui, &mut (), slot, slot);
                    assert!(first.min.x < second.min.x, "{layout:?}: first {first:?} not left of second {second:?}");
                });
            });
        }
    }

    #[test]
    fn step_scene_rect_zooms_about_center_and_clamps() {
        let screen = egui::vec2(1000.0, 800.0);
        let scale_of = |r: egui::Rect| (screen / r.size()).min_elem();

        let at_100 = egui::Rect::from_center_size(egui::pos2(50.0, 60.0), screen);
        let zoomed_in = step_scene_rect(at_100, ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert_eq!(zoomed_in.center(), at_100.center());
        assert!((scale_of(zoomed_in) - ZOOM_STEP).abs() < 1e-4);

        let zoomed_out = step_scene_rect(at_100, 1.0 / ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(zoomed_out) - 1.0 / ZOOM_STEP).abs() < 1e-4);

        let near_max = egui::Rect::from_center_size(egui::pos2(0.0, 0.0), screen / 2.8);
        let clamped = step_scene_rect(near_max, ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(clamped) - GRAPH_ZOOM_MAX).abs() < 1e-3);

        let near_min = egui::Rect::from_center_size(egui::pos2(0.0, 0.0), screen / 0.16);
        let clamped = step_scene_rect(near_min, 1.0 / ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(clamped) - GRAPH_ZOOM_MIN).abs() < 1e-3);
    }

    #[test]
    fn image_fit_scale_contains_and_never_upscales() {
        let body = egui::vec2(1000.0, 800.0);
        assert_eq!(image_fit_scale(egui::vec2(2000.0, 1000.0), body), 0.5);
        assert_eq!(image_fit_scale(egui::vec2(1000.0, 4000.0), body), 0.2);
        assert_eq!(image_fit_scale(egui::vec2(64.0, 64.0), body), 1.0);
    }

    #[test]
    fn spawned_tab_visible_hides_focused_tab_behind_image_view() {
        // Ordinary focused tab, no Image view up: visible, as before.
        assert!(spawned_tab_visible(true, false, false));
        // Focused tab, but Image view is showing in front of it: hidden.
        assert!(!spawned_tab_visible(true, false, true));
        // Split's secondary pane is always visible, even behind Image view.
        assert!(spawned_tab_visible(false, true, true));
        assert!(spawned_tab_visible(false, true, false));
        // Unfocused, non-split tab: never visible.
        assert!(!spawned_tab_visible(false, false, false));
        assert!(!spawned_tab_visible(false, false, true));
    }

    #[test]
    fn zoom_about_keeps_the_anchor_point_fixed() {
        let (scale, pan) = (0.5, egui::vec2(30.0, -20.0));
        let anchor = egui::vec2(100.0, 50.0);
        let (new_scale, new_pan) = zoom_about(scale, pan, 2.0, anchor);
        assert_eq!(new_scale, 1.0);
        let before = (anchor - pan) / scale;
        let after = (anchor - new_pan) / new_scale;
        assert!((before - after).length() < 1e-4, "{before:?} vs {after:?}");
    }

    #[test]
    fn zoom_about_clamps_scale() {
        assert_eq!(zoom_about(15.0, egui::Vec2::ZERO, 2.0, egui::Vec2::ZERO).0, IMAGE_ZOOM_MAX);
        assert_eq!(zoom_about(0.11, egui::Vec2::ZERO, 0.5, egui::Vec2::ZERO).0, IMAGE_ZOOM_MIN);
    }

    #[test]
    fn clamp_image_pan_limits_each_axis_independently() {
        let body = egui::vec2(1000.0, 800.0);
        let scaled = egui::vec2(2000.0, 500.0); // wider than the body, shorter than it
        assert_eq!(clamp_image_pan(egui::vec2(900.0, 40.0), scaled, body), egui::vec2(500.0, 0.0));
        assert_eq!(clamp_image_pan(egui::vec2(-120.0, 0.0), scaled, body), egui::vec2(-120.0, 0.0));
    }

    #[test]
    fn image_view_after_action_steps_from_fit_and_resets() {
        let image = egui::vec2(2000.0, 1000.0);
        let body = egui::vec2(1000.0, 800.0);
        let zoomed = image_view_after_action(ImageViewState::default(), ZoomChipAction::In, image, body);
        assert_eq!(zoomed.zoom, ImageZoom::Scale(0.5 * ZOOM_STEP));
        let actual = image_view_after_action(zoomed, ZoomChipAction::Reset100, image, body);
        assert_eq!(actual, ImageViewState { zoom: ImageZoom::Scale(1.0), pan: egui::Vec2::ZERO });
        assert_eq!(image_view_after_action(zoomed, ZoomChipAction::Fit, image, body), ImageViewState::default());
    }

    // `Tab` is left out for the same reason as the test above: `TabPayload`
    // can't be constructed in a unit test. Its arm returns `None` by the
    // match's exhaustiveness, and `close_node` routes tabs to `close_tab`
    // (which keeps its own Recently Closed stack) before this is consulted.
    #[test]
    fn closed_node_for_records_only_undo_worthy_payloads() {
        assert!(matches!(
            closed_node_for(&NodePayload::Note(NotePayload { text: "n".into(), editing: false })),
            Some(ClosedNode::Note(text)) if text == "n"
        ));
        // A blank note (never typed in, or only whitespace): nothing to restore.
        for text in ["", " 
	"] {
            assert!(closed_node_for(&NodePayload::Note(NotePayload { text: text.into(), editing: false })).is_none());
        }
        assert!(matches!(
            closed_node_for(&NodePayload::Image(ImagePayload { path: "img.png".into(), width: 1, height: 1, error: None })),
            Some(ClosedNode::Image(path)) if path == "img.png"
        ));
        assert!(matches!(
            closed_node_for(&NodePayload::Pdf(PdfPayload { path: "doc.pdf".into(), outcome: None })),
            Some(ClosedNode::Pdf(path)) if path == "doc.pdf"
        ));
        assert!(matches!(
            closed_node_for(&NodePayload::Json(JsonPayload {
                path: "data.json".into(),
                value: serde_json::Value::Null,
                key: None,
                error: None,
            })),
            Some(ClosedNode::Json(path)) if path == "data.json"
        ));
        // A synthetic "Expand as Graph" leaf: never recorded.
        assert!(closed_node_for(&NodePayload::Json(JsonPayload {
            path: "data.json".into(),
            value: serde_json::Value::Null,
            key: Some("k".into()),
            error: None,
        }))
        .is_none());
        assert!(closed_node_for(&NodePayload::Group(GroupPayload { name: "g".into() })).is_none());
        assert!(closed_node_for(&NodePayload::Download(DownloadPayload {
            path: "file.bin".into(),
            bytes_received: 0,
            total_bytes: None,
            completed: true,
        }))
        .is_none());
    }

    // One row per kind, pinned to the exact menus each kind showed before
    // the per-kind builders were merged (see the node-menu spec), and
    // before the tab and group menus joined them.
    #[test]
    fn node_menu_config_matches_each_kinds_menu() {
        fn menu(kind: ItemNodeKind, has_group: bool, pinned: bool, has_json: bool, headless: bool) -> (&'static str, f32, f32, String) {
            let view = NodeMenuView { kind, has_group, color: None, pinned, has_json, renaming: None };
            let config = node_menu_config(&view, headless);
            let items: Vec<&str> = config
                .items
                .iter()
                .map(|item| match item {
                    NodeMenuItem::Button(label, _) => *label,
                    NodeMenuItem::Colors => "<colors>",
                    NodeMenuItem::Separator => "---",
                })
                .collect();
            (config.area_id, config.min_width, config.max_width, items.join(" | "))
        }
        for has_group in [false, true] {
            // A download can join a group like any card (drag onto it), so
            // its menu offers "Remove from Group" when grouped.
            let g = if has_group { " | --- | Remove from Group" } else { "" };
            let menu = |kind, pinned, has_json| menu(kind, has_group, pinned, has_json, false);
            assert_eq!(
                menu(ItemNodeKind::Download, false, false),
                ("download_context_menu", 170.0, 170.0, format!("Open Containing Folder | Copy Path{g} | --- | Remove from List"))
            );
            assert_eq!(menu(ItemNodeKind::Note, false, false), ("note_context_menu", 150.0, 150.0, format!("Copy Text{g} | --- | Delete")));
            assert_eq!(
                menu(ItemNodeKind::Image, false, false),
                ("image_context_menu", 170.0, 170.0, format!("Open Containing Folder | Copy Path{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Pdf, false, false),
                ("pdf_context_menu", 170.0, 170.0, format!("Open Containing Folder | Copy Path{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Json, false, false),
                ("json_context_menu", 170.0, 170.0, format!("Open Containing Folder | Copy Path | Expand as Graph{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Tab, false, false),
                ("tab_context_menu", 150.0, f32::INFINITY, format!("Pin | Duplicate | Copy URL | Fit Subtree | <colors>{g} | --- | Close"))
            );
            assert_eq!(
                menu(ItemNodeKind::Tab, true, true).3,
                format!("Unpin | Duplicate | Copy URL | View as JSON | Fit Subtree | <colors>{g} | --- | Close")
            );
            // A group's menu has no Remove from Group row, grouped or not.
            assert_eq!(
                menu(ItemNodeKind::Group, false, false),
                ("group_context_menu", 150.0, f32::INFINITY, "Rename | <colors> | --- | Dissolve | Close Group | Export Group...".to_string())
            );
        }
        // Export Group's file dialog can't run headless.
        assert_eq!(
            menu(ItemNodeKind::Group, false, false, false, true).3,
            "Rename | <colors> | --- | Dissolve | Close Group"
        );
    }

    #[test]
    fn remove_from_group_moves_up_one_level() {
        let node = |group: Option<&str>, payload| GraphNode {
            parent: None,
            group: group.map(String::from),
            pos: egui::Vec2::ZERO,
            vel: egui::Vec2::ZERO,
            color: None,
            payload,
        };
        let group = |name: &str| NodePayload::Group(GroupPayload { name: name.into() });
        let note = || NodePayload::Note(NotePayload { text: String::new(), editing: false });
        let mut nodes = NodeMap::default();
        nodes.insert("outer".into(), node(None, group("outer")));
        nodes.insert("inner".into(), node(Some("outer"), group("inner")));
        nodes.insert("deep".into(), node(Some("inner"), note()));
        nodes.insert("shallow".into(), node(Some("outer"), note()));

        remove_from_group(&mut nodes, "deep");
        assert_eq!(nodes["deep"].group.as_deref(), Some("outer"));
        remove_from_group(&mut nodes, "shallow");
        assert_eq!(nodes["shallow"].group, None);
        remove_from_group(&mut nodes, "shallow");
        assert_eq!(nodes["shallow"].group, None);
    }

    #[test]
    fn node_card_size_matches_each_kinds_card() {
        let download = NodePayload::Download(DownloadPayload { path: "f.bin".into(), bytes_received: 0, total_bytes: None, completed: false });
        assert_eq!(node_card_size(&download), DOWNLOAD_NODE_SIZE);
        assert_eq!(node_card_size(&NodePayload::Note(NotePayload { text: String::new(), editing: false })), NOTE_NODE_SIZE);
        assert_eq!(node_card_size(&NodePayload::Group(GroupPayload { name: "g".into() })), GROUP_NODE_SIZE);
        let json = NodePayload::Json(JsonPayload { path: "a.json".into(), value: serde_json::json!({}), key: None, error: None });
        assert_eq!(node_card_size(&json), JSON_NODE_SIZE);

        // Image: a small picture keeps its own size; an oversized one is
        // scaled down to fit `IMAGE_NODE_MAX_SIZE`, aspect ratio kept.
        let image = |width, height| NodePayload::Image(ImagePayload { path: "p.png".into(), width, height, error: None });
        assert_eq!(node_card_size(&image(40, 20)), egui::vec2(40.0, 20.0));
        let big = node_card_size(&image(100_000, 50_000));
        assert!(big.x <= IMAGE_NODE_MAX_SIZE.x && big.y <= IMAGE_NODE_MAX_SIZE.y);
        assert!((big.x / big.y - 2.0).abs() < 1e-3);

        // Pdf: pending and failed use the fixed pending box; rendered uses
        // the page's own size through the same image fitting.
        let pdf = |outcome| NodePayload::Pdf(PdfPayload { path: "d.pdf".into(), outcome });
        assert_eq!(node_card_size(&pdf(None)), PDF_PENDING_NODE_SIZE);
        assert_eq!(node_card_size(&pdf(Some(Err("bad".into())))), PDF_PENDING_NODE_SIZE);
        assert_eq!(node_card_size(&pdf(Some(Ok(PdfOutcome { width: 40, height: 20, page_count: 3 })))), egui::vec2(40.0, 20.0));
        assert_eq!(node_card_size(&pdf(Some(Ok(PdfOutcome { width: 100_000, height: 50_000, page_count: 1 })))), big);
    }

    #[test]
    fn card_filter_text_matches_each_kinds_filter_input() {
        let download = NodePayload::Download(DownloadPayload { path: "C:/dl/f.bin".into(), bytes_received: 0, total_bytes: None, completed: true });
        // Download filters on its full path; the file kinds on the filename only.
        assert_eq!(card_filter_text(&download), "C:/dl/f.bin");
        assert_eq!(card_filter_text(&NodePayload::Note(NotePayload { text: "buy milk".into(), editing: false })), "buy milk");
        assert_eq!(card_filter_text(&NodePayload::Image(ImagePayload { path: "C:/pics/cat.png".into(), width: 1, height: 1, error: None })), "cat.png");
        assert_eq!(card_filter_text(&NodePayload::Pdf(PdfPayload { path: "C:/docs/paper.pdf".into(), outcome: None })), "paper.pdf");
        assert_eq!(card_filter_text(&NodePayload::Json(JsonPayload { path: "C:/data/x.json".into(), value: serde_json::json!(1), key: None, error: None })), "x.json");
    }
    #[test]
    fn bump_past_id_clears_restored_ids() {
        let mut counter = 0;
        bump_past_id(&mut counter, "tab-10", "tab");
        bump_past_id(&mut counter, "tab-3", "tab");
        assert_eq!(counter, 11);
        bump_past_id(&mut counter, "note-50", "tab");
        bump_past_id(&mut counter, "tabby-50", "tab");
        bump_past_id(&mut counter, "tab-x", "tab");
        assert_eq!(counter, 11);
        assert_eq!(next_id(&mut counter, "tab"), "tab-11");
    }
}
