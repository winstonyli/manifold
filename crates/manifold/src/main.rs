mod bookmarks;
mod card;
mod crash_report;
mod debug_control;
mod engine_info;
mod event_loop;
mod favicon_fetch;
mod gpu;
mod graph_layout;
mod graph_ui;
mod group_export;
mod image_view;
mod input;
mod json_file;
mod node;
mod pdf;
mod pdf_fetch;
mod popups;
mod process_manager;
mod redirect_fetch;
mod session;
mod session_sync;
mod settings;
mod settings_page;
mod store;
mod suggestions;
mod tab_view;
mod theme;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use favicon_fetch::FaviconFetcher;
use pdf_fetch::PdfFetcher;
use redirect_fetch::{RedirectFetcher, RedirectKind};
use suggestions::{resolve_navigation_url, Suggestion, SuggestionFetcher};

use card::*;
use debug_control::DebugCommand;
use engine_info::*;
use gpu::GpuState;
use graph_ui::*;
use image_view::*;
use input::*;
use ipc_protocol::{
    frame_path, modifiers, EngineToHost, HostToEngine, InputEvent, KeyCode, MouseButton, SharedFrameBuffer,
};
use node::*;
use popups::*;
use process_manager::{EngineKind, EngineTab};
use settings::{SearchEngine, Settings};
use tab_view::*;
use theme::*;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{
    DeviceId, ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
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

/// Something done to one node, from its right-click menu or its card's
/// own buttons, applied by `App::apply_node_action`.
/// `build_node_context_menu` and the card loops only report which one was
/// clicked; the caller owns every effect.
#[derive(Clone, Debug, PartialEq)]
enum NodeAction {
    /// Opens the node into Item view (a tab or image card's click).
    Open,
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
    /// The rename field lost focus by any means but Escape. Carries the
    /// typed name rather than reading `renaming_group` when applied, so it
    /// still lands if leaving the group (which clears the draft) is applied
    /// first: clicking the toolbar's back button while renaming does both
    /// in one frame.
    CommitRename(String),
    /// Escape in the rename field.
    CancelRename,
    SetColor(Option<egui::Color32>),
    /// A card's color swatch: the next color in `TAG_COLOR_PALETTE`.
    CycleColor,
    ToggleMute,
    RetryMissingFile,
    /// A failed tab's card: tries again on the default engine.
    OpenInDefaultEngine,
    RemoveFromGroup,
    Dissolve,
    ExportGroup,
    /// Delete for a file card, "Remove from List" for a download, Close
    /// for a tab or group.
    Delete,
}

impl NodeAction {
    fn hover_text(&self) -> Option<&'static str> {
        match self {
            NodeAction::FitSubtree => Some("Zoom the graph to frame just this tab and its children"),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
enum NodeMenuItem {
    Button(&'static str, NodeAction),
    /// The "Color" label and tag-color swatch row, ending in a "no color"
    /// ring.
    Colors,
    Separator,
}

/// A node's right-click menu: its items, in order. `area_id` is the
/// popup's egui `Area` id, distinct per kind so egui's per-area memory
/// never bleeds between kinds. Every menu has a minimum width and no
/// maximum, so it grows to fit the swatch row.
#[derive(Debug, PartialEq)]
struct NodeMenuConfig {
    area_id: &'static str,
    min_width: f32,
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
) -> (Option<NodeAction>, bool) {
    let mut action = None;
    let anchor = egui::Rect::from_min_size(pos, egui::Vec2::ZERO);
    let area_rect = build_list_popup_frame(
        ui.ctx(),
        config.area_id,
        anchor,
        config.min_width,
        f32::INFINITY,
        fade,
        interactable,
        |ui| {
            if let Some((draft, focus)) = rename {
                let response = ui.add(egui::TextEdit::singleline(draft).desired_width(140.0));
                if focus {
                    response.request_focus();
                }
                if response.lost_focus() {
                    action = Some(if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        NodeAction::CancelRename
                    } else {
                        NodeAction::CommitRename(draft.clone())
                    });
                }
                return;
            }
            for item in &config.items {
                match item {
                    NodeMenuItem::Button(label, item_action) => {
                        let mut response = ui.button(*label);
                        if let Some(text) = item_action.hover_text() {
                            response = response.on_hover_text(text);
                        }
                        if response.clicked() {
                            action = Some(item_action.clone());
                        }
                    }
                    NodeMenuItem::Colors => {
                        if let Some(picked) = color_swatch_row(ui, color) {
                            action = Some(NodeAction::SetColor(picked));
                        }
                    }
                    NodeMenuItem::Separator => {
                        ui.separator();
                    }
                }
            }
        },
    );
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
                        ui.painter().circle_stroke(
                            rect.center(),
                            8.0,
                            egui::Stroke::new(2.0, ui.visuals().text_color()),
                        );
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
    use NodeAction as A;
    use NodeMenuItem::{Button, Colors, Separator};
    let file_items = || vec![Button("Open Containing Folder", A::Reveal), Button("Copy Path", A::CopyPath)];
    // Every menu ends in the Colors row, Remove from Group (when the node
    // is grouped), then the kind's delete items.
    let finish = |area_id, min_width, mut items: Vec<NodeMenuItem>, delete_items: Vec<NodeMenuItem>| {
        items.push(Colors);
        if view.has_group {
            items.extend([Separator, Button("Remove from Group", A::RemoveFromGroup)]);
        }
        items.push(Separator);
        items.extend(delete_items);
        NodeMenuConfig { area_id, min_width, items }
    };
    let delete = |label| vec![Button(label, A::Delete)];
    match view.kind {
        ItemNodeKind::Download => finish("download_context_menu", 170.0, file_items(), delete("Remove from List")),
        ItemNodeKind::Note => {
            finish("note_context_menu", 150.0, vec![Button("Copy Text", A::CopyText)], delete("Delete"))
        }
        ItemNodeKind::Image => finish("image_context_menu", 170.0, file_items(), delete("Delete")),
        ItemNodeKind::Pdf => finish("pdf_context_menu", 170.0, file_items(), delete("Delete")),
        ItemNodeKind::Json => {
            let mut items = file_items();
            items.push(Button("Expand as Graph", A::ExpandAsGraph));
            finish("json_context_menu", 170.0, items, delete("Delete"))
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
            items.push(Button("Fit Subtree", A::FitSubtree));
            finish("tab_context_menu", 150.0, items, delete("Close"))
        }
        ItemNodeKind::FailedTab => {
            let mut items = vec![Button("Retry", A::RetryMissingFile)];
            if view.other_default_engine {
                items.push(Button("Open in Default Engine", A::OpenInDefaultEngine));
            }
            items.push(Button("Copy URL", A::CopyUrl));
            finish("failed_tab_context_menu", 170.0, items, delete("Close"))
        }
        ItemNodeKind::Group => {
            let mut delete_items = vec![Button("Dissolve", A::Dissolve), Button("Close Group", A::Delete)];
            if !headless {
                delete_items.push(Button("Export Group...", A::ExportGroup));
            }
            finish("group_context_menu", 150.0, vec![Button("Rename", A::Rename)], delete_items)
        }
    }
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
    /// FailedTab only: the default engine isn't the one that failed, so
    /// the menu offers it.
    other_default_engine: bool,
    /// Group only: the rename draft, while this group is being renamed.
    renaming: Option<String>,
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
type DeferredOverlays = Vec<DeferredOverlay>;

/// Shows a native "Save As" dialog defaulting to `default_name`, filtered
/// to `.json`; if a path was picked, returns it along with the exact
/// "Saved <path>" toast text this app shows for every export (Session,
/// Bookmarks, one Group) on success. Doesn't write anything itself, and
/// deliberately isn't a method on `App` -- the caller still does its own
/// serialization (`self.export_session_to(&path)`, etc.)
/// and sets `self.save_toast`, which a `&mut self` helper here couldn't
/// do without a borrow conflict against a closure also wanting to touch
/// `self`. Previously this dialog-plus-toast tail was hand-copied three
/// times, differing only in the default filename and the filter's label.
fn save_dialog_with_toast(
    default_name: impl Into<String>,
    filter_label: &str,
) -> Option<(std::path::PathBuf, (String, std::time::Instant))> {
    let path =
        rfd::FileDialog::new().set_file_name(default_name.into()).add_filter(filter_label, &["json"]).save_file()?;
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
            egui::CollapsingHeader::new(format!("{key}  {{{}}}", map.len())).default_open(top_level).show(ui, |ui| {
                for (k, v) in map {
                    draw_json_value(ui, k, v, false);
                }
            });
        }
        serde_json::Value::Array(items) => {
            egui::CollapsingHeader::new(format!("{key}  [{}]", items.len())).default_open(top_level).show(ui, |ui| {
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
    let grandparent =
        nodes.get(id).and_then(|n| n.group.as_ref()).and_then(|group| nodes.get(group)).and_then(|g| g.group.clone());
    if let Some(node) = nodes.get_mut(id) {
        node.group = grandparent;
    }
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
    let dir = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default().join("Pictures");
    let _ = std::fs::create_dir_all(&dir);
    let timestamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    dir.join(format!("manifold-{timestamp}.png"))
}

/// A group card's border tints `DANGER_COLOR` once its member count exceeds
/// this threshold -- a cheap glanceable "this one's gotten big" cue. Only
/// applies while the group has no explicit tag color of its own (see its use
/// below) -- a deliberate user choice always wins over this automatic one,
/// same precedent tag colors already have over the plain default `ACCENT`
/// border.
const GROUP_OVERLOAD_THRESHOLD: usize = 8;

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
        ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font_id.clone(), egui::Color32::WHITE)).size().x
    };
    let result = if bucketed_width <= 0.0 {
        String::new()
    } else if width_of(text) <= bucketed_width {
        text.to_owned()
    } else {
        let chars: Vec<char> = text.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
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

/// Multiplicative step for one click of a zoom chip's −/+ (and, in Image
/// view, one `Ctrl+=`/`Ctrl+-`) -- shared so Graph and Image zoom feel the
/// same.
const ZOOM_STEP: f32 = 1.25;

/// What a click on `render_zoom_chip` asked for. The chip holds no zoom
/// state of its own -- each caller applies this to whatever "zoom" means
/// in its own view (Graph: the camera's `camera.rect`; Image view:
/// `ImageViewState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ZoomChipAction {
    Out,
    In,
    Reset100,
    Fit,
}

/// `rect` (a Graph camera's world-space `camera.rect`) resized about
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
            .add(egui::Button::new(format!("{}%", pct.round() as i32)).frame(false).min_size(egui::vec2(44.0, 0.0)))
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

/// A soft glow tinted the card's own border color, grown outward with
/// decreasing alpha: a plain black drop shadow was invisible against this
/// app's near-black background. `intensity` scales the alpha only, not the
/// spread, so a hover brightens it without resizing the card. Returned as
/// shapes rather than painted, so a caller can still put the glow visually
/// *behind* content laid out before the glow is known (the save toast: an
/// `egui::Frame` that auto-sizes to its own label; a graph card, whose
/// hover is only known after its body). `Painter::add` only ever appends
/// (there's no "insert behind" for shapes already painted), so such a
/// caller pre-reserves a placeholder slot with `Shape::Noop`, then fills it
/// in with these via `Painter::set` -- the standard egui pattern for this,
/// also how e.g. selection highlights behind not-yet-laid-out text are
/// done.
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

/// An in-progress eased pan/zoom from one `camera.rect` to another --
/// entering/leaving a group used to snap the camera to the new level's
/// `fit_all_nodes_scene_rect()` instantly; this animates the same jump over
/// `TRANSITION_DURATION` instead, same smoothstep easing as `Transition`
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
struct GraphCameraTween {
    from: egui::Rect,
    to: egui::Rect,
    start: std::time::Instant,
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
struct PanMomentum {
    initial_velocity: egui::Vec2,
    start: std::time::Instant,
    start_center: egui::Pos2,
}

/// Exponential decay rate (`k` in `v(t) = v0 * e^-kt`, per second) for
/// `PanMomentum` -- tuned for a glide that's clearly perceptible but
/// settles well under a second, not a floaty drift.
const PAN_MOMENTUM_DECAY: f32 = 3.5;
/// Minimum release speed (world-space units/second, from `GraphCamera::
/// drag_history`) for a released pan drag to start gliding at all -- below
/// this, an ordinary slow drag-and-let-go just stops, matching what a
/// user who wasn't trying to "flick" the canvas would expect.
const PAN_MOMENTUM_MIN_SPEED: f32 = 500.0;
/// Hard cap on how long a `PanMomentum` stays active, regardless of
/// `PAN_MOMENTUM_DECAY` -- at that rate, decay is already below 1% of
/// the initial speed well before this, so this is just a bound on the
/// `Option` field's own lifetime, not a value that shapes the motion.
const PAN_MOMENTUM_MAX_DURATION: std::time::Duration = std::time::Duration::from_millis(2000);
/// How many recent camera-center samples `GraphCamera::drag_history` keeps
/// while a background pan drag is held down -- spans roughly 100ms at a
/// steady frame rate, matching the same window egui's own `PointerState::
/// velocity()` uses internally for its analogous smoothing.
const PAN_HISTORY_LEN: usize = 6;

/// How long a "Saved <path>" toast stays on screen — see `App::save_toast`'s
/// doc comment.
const SAVE_TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(4);

/// One "Recently Closed" entry: a closed node and, if it's a group,
/// everything under it, as it was (see `closed_entry`). `nodes[0]` is the
/// node that was closed. `App::reopen_closed` rebuilds it with fresh ids.
/// Not cookies/localStorage/scroll position, the same "graph shape only"
/// limit session restore and an engine swap have.
struct ClosedEntry {
    nodes: Vec<ClosedNode>,
}

/// One node of a `ClosedEntry`, with its old id. `group`/`parent` are old
/// ids too: reopening remaps the ones inside the entry and keeps the rest
/// only if that node still exists.
struct ClosedNode {
    id: String,
    parent: Option<String>,
    group: Option<String>,
    pos: egui::Vec2,
    color: Option<egui::Color32>,
    piece: ClosedPiece,
}

/// The kind-specific part of a `ClosedNode`. A file-backed node keeps only
/// its path and is reloaded, so a changed or missing file reopens as it is
/// now; a note and an "Expand as Graph" leaf keep their content.
enum ClosedPiece {
    Tab { url: String, kind: EngineKind, pinned: bool, zoom_factor: f64 },
    Group { name: String },
    Note(String),
    Image(String),
    Pdf(String),
    Json(String),
    JsonLeaf { path: String, key: Option<String>, value: serde_json::Value },
}

/// How many entries `App::closed` keeps before dropping the oldest, like a
/// browser's own bounded "Recently closed" list.
const MAX_CLOSED: usize = 20;

/// What of `payload` is worth reopening. `None` for a Download (it has no
/// closed state) and a blank note (nothing to restore).
fn closed_piece(payload: &NodePayload) -> Option<ClosedPiece> {
    Some(match payload {
        NodePayload::Tab(tab) => ClosedPiece::Tab {
            url: tab.url.clone(),
            kind: tab.tab.kind,
            pinned: tab.pinned,
            zoom_factor: tab.zoom_factor,
        },
        NodePayload::FailedTab(tab) => {
            ClosedPiece::Tab { url: tab.url.clone(), kind: tab.kind, pinned: tab.pinned, zoom_factor: tab.zoom_factor }
        }
        NodePayload::Group(group) => ClosedPiece::Group { name: group.name.clone() },
        NodePayload::Note(note) if !note.text.trim().is_empty() => ClosedPiece::Note(note.text.clone()),
        NodePayload::Image(image) => ClosedPiece::Image(image.path.clone()),
        NodePayload::Pdf(pdf) => ClosedPiece::Pdf(pdf.path.clone()),
        NodePayload::Json(json) if json.key.is_none() => ClosedPiece::Json(json.path.clone()),
        NodePayload::Json(json) => {
            ClosedPiece::JsonLeaf { path: json.path.clone(), key: json.key.clone(), value: json.value.clone() }
        }
        NodePayload::Note(_) | NodePayload::Download(_) => return None,
    })
}

/// The `ClosedEntry` closing `id` should record: `id` and, for a group,
/// every node under it at any depth (in `order`, i.e. draw order), minus
/// what `closed_piece` skips. `None` when `id` itself isn't worth
/// reopening, which includes a lone "Expand as Graph" leaf: it's one of
/// many, and each would push a real entry out of `MAX_CLOSED`.
fn closed_entry(nodes: &NodeMap, order: &[String], id: &str) -> Option<ClosedEntry> {
    let snapshot = |id: &str, node: &GraphNode, piece| ClosedNode {
        id: id.to_string(),
        parent: node.parent.clone(),
        group: node.group.clone(),
        pos: node.pos,
        color: node.color,
        piece,
    };
    let root = nodes.get(id)?;
    let piece = closed_piece(&root.payload)?;
    if matches!(piece, ClosedPiece::JsonLeaf { .. }) {
        return None;
    }
    let mut entry = vec![snapshot(id, root, piece)];
    // Only a group has members; for anything else this finds none.
    let mut levels = vec![id.to_string()];
    while let Some(level) = levels.pop() {
        for (member_id, member) in order.iter().filter_map(|m| nodes.get(m).map(|n| (m, n))) {
            if member.group.as_deref() != Some(level.as_str()) {
                continue;
            }
            if member.as_group().is_some() {
                levels.push(member_id.clone());
            }
            if let Some(piece) = closed_piece(&member.payload) {
                entry.push(snapshot(member_id, member, piece));
            }
        }
    }
    Some(ClosedEntry { nodes: entry })
}

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

/// A `TextEdit` that a shortcut or click wants focused. The widget may not
/// exist until a later frame (a rename field replaces its button, the find
/// bar opens), so the request waits in `App::pending_focus` until the
/// widget is built and takes it with `take_focus`. One at a time, since
/// only one widget can hold focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FocusTarget {
    AddressBar,
    FindBar,
    QuickSwitcher,
    GroupRename,
    BookmarkFolder,
}

/// True, once, if `target` is the pending focus request: clears it so the
/// caller can `request_focus()` its widget.
fn take_focus(pending: &mut Option<FocusTarget>, target: FocusTarget) -> bool {
    if *pending == Some(target) {
        *pending = None;
        true
    } else {
        false
    }
}

/// The event loop's real user-event type: headless debug commands, plus
/// `TabSpawned` — see its own doc comment for why engine-host spawning needs
/// to be async in the first place.
enum UserEvent {
    Debug(DebugCommand),
    TabSpawned(Box<TabSpawnOutcome>),
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
    spawn: TabSpawn,
    /// `App::session_generation` when the spawn started -- see that field.
    generation: u64,
    result: std::io::Result<EngineTab>,
}

/// What `spawn_tab_async` needs to start a tab and, once it's up, to
/// insert its graph node: `TabSpawn::new` for a new tab (no parent or
/// group, at rest, nothing carried), `TabSpawn::respawn` for one that
/// already has a node.
struct TabSpawn {
    id: String,
    url: String,
    kind: EngineKind,
    parent: Option<String>,
    group: Option<String>,
    pos: egui::Vec2,
    vel: egui::Vec2,
    purpose: SpawnPurpose,
    carry: TabCarry,
    /// The engine to respawn with if this spawn fails: a swap's old
    /// engine, whose process `finish_swap` has already had to drop.
    fallback_kind: Option<EngineKind>,
}

/// What a tab keeps across a respawn (engine swap, crash recovery). A new
/// tab starts unpinned, untagged, at the default zoom. A new attribute
/// that must survive a respawn is one more field here.
struct TabCarry {
    /// See `TabPayload::pinned`.
    pinned: bool,
    /// See `GraphNode::color`.
    color: Option<egui::Color32>,
    /// See `TabPayload::zoom_factor`. A new tab takes
    /// `Settings::zoom_factor`, having no value of its own yet.
    zoom_factor: f64,
}

impl TabSpawn {
    fn new(
        id: String,
        url: String,
        kind: EngineKind,
        pos: egui::Vec2,
        purpose: SpawnPurpose,
        zoom_factor: f64,
    ) -> Self {
        TabSpawn {
            id,
            url,
            kind,
            parent: None,
            group: None,
            pos,
            vel: egui::Vec2::ZERO,
            purpose,
            carry: TabCarry { pinned: false, color: None, zoom_factor },
            fallback_kind: None,
        }
    }

    /// Tab `node` (`id`) respawned as `kind` at `url`, keeping its place in
    /// the graph and its `TabCarry`. `None` if `node` isn't a tab.
    fn respawn(id: &str, node: &GraphNode, kind: EngineKind, url: String) -> Option<Self> {
        let tab = node.as_tab()?;
        Some(TabSpawn {
            id: id.to_string(),
            url,
            kind,
            parent: node.parent.clone(),
            group: node.group.clone(),
            pos: node.pos,
            vel: node.vel,
            purpose: SpawnPurpose::Respawn,
            carry: TabCarry { pinned: tab.pinned, color: node.color, zoom_factor: tab.zoom_factor },
            fallback_kind: None,
        })
    }
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
    /// treatment `camera.rect`'s pan/zoom already gets.
    current_group: Option<String>,
    /// `(group_id, draft_name)` once renaming has started -- either the
    /// toolbar's group-name button (only shown while `current_group` is
    /// `Some`) or the top-level right-click context menu's own "Rename"
    /// (see `node_menu`) sets this the same way. A real
    /// `egui::TextEdit` replaces whichever plain button/menu row started
    /// it until Enter commits or Escape cancels. Transient UI state, not
    /// part of `GroupPayload` itself.
    renaming_group: Option<(String, String)>,
    /// `(node_id, screen_pos)` while a right-click context menu is showing
    /// for that node's card -- one field for every kind, so at most one
    /// menu is ever open by construction. Right-click is otherwise the
    /// rectangle-select gesture over the graph panel (see that gesture's
    /// own doc comment on `handle_rect_select`); a press landing on any card
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
    pending_import: Option<store::json::JsonDoc>,
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
    /// The session store (spec "Journal"): `None` before `open_store`, when
    /// `%APPDATA%` is unset, or when the file couldn't be opened (toasted).
    journal: Option<store::journal::Journal>,
    /// Ids for nodes made while there is no journal (unit tests, a failed
    /// open): this launch's own actor, so they parse as store ids.
    fallback_ids: (store::model::ActorId, u64),
    /// Tab spawns started by `spawn_tab_async` whose `TabSpawned` result
    /// hasn't been handled yet. Read by the headless `WAITTABS` command,
    /// whose waiting senders sit in `tabs_spawned_waiters` until this
    /// reaches zero.
    tab_spawns_in_flight: usize,
    /// Ids whose tab spawn is in flight (every `SpawnPurpose`): no live
    /// node yet, so `save_session` keeps them in the store rather than
    /// deleting them (contract "In-flight spawns").
    spawning: HashSet<String>,
    /// Other replicas' latest change to a node in `spawning`, applied when
    /// its spawn lands (`None`: deleted). The spawn itself carries only
    /// what it was started with.
    remote_in_flight: HashMap<String, Option<session_sync::RemoteNode>>,
    /// Explicit fields (`pos`, `url`) marked at user actions, for the next
    /// save to write (spec "Local edits become ops").
    explicit: std::collections::BTreeSet<(store::model::NodeId, u32)>,
    /// The shared `pos` each live node last had, so a remote change moves a
    /// node only when its `pos` changed (`materialize`).
    shared_pos: HashMap<String, egui::Vec2>,
    tabs_spawned_waiters: Vec<std::sync::mpsc::Sender<()>>,
    /// Bumped by `replace_running_session`. A tab spawn carries the value it
    /// started under, and `handle_tab_spawn_result` drops a result from an
    /// older one: it belongs to the replaced session, and inserting it would
    /// add a stray tab to the new scene, so a spawn from a replaced session
    /// never lands in it.
    session_generation: u64,
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
    /// Tab view's address bar and its autocomplete. See `AddressBar`.
    address_bar: AddressBar,
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
    /// Graph view's pan/zoom and its animations.
    camera: GraphCamera,
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
    /// (see `run_egui_frame`), so they take the same code paths and, like
    /// a real key, last one frame whether or not anything reads them.
    debug_input_events: Vec<egui::Event>,
    /// Text from a headless `ADDRESSBAR`, typed (select-all, then the text)
    /// once the address bar actually has egui focus: the command requests
    /// focus through `pending_focus`, which only takes hold in
    /// the frame after, so typing waits for a frame where
    /// `address_bar.focused` is true.
    debug_address_bar_typing: Option<String>,
    /// A focus request waiting for its `TextEdit` to be built (see
    /// `FocusTarget`): set by `Ctrl+L`/`Ctrl+F`/`Ctrl+K`, starting a
    /// rename, and headless `FOCUSADDRESS`/`ADDRESSBAR`/`OPENFINDBAR`.
    /// Unlike the headless key events above, this drives a real
    /// `request_focus()` on the real widget.
    pending_focus: Option<FocusTarget>,
    /// Set by a headless `QUIT` that arrived while debug input above (or
    /// `pending_screenshot`) was still waiting for a frame — e.g. `ENTER`+`QUIT`,
    /// or `SCREENSHOT`+`QUIT`, sent back-to-back with no delay. Exiting
    /// immediately in that case could drop the pending command/screenshot
    /// before any frame ever consumed it (`event_loop.exit()` gives no
    /// guarantee a `RedrawRequested` runs first). Holds how many more frames
    /// to wait: `headless_end_of_frame` exits once nothing is pending, or when
    /// this runs out (an `ADDRESSBAR` needs two frames, focus then typing;
    /// a screenshot on a stalled surface might never land).
    quit_after_redraws: Option<u8>,
    /// Set by a headless `SCREENSHOT` command; consumed on the next
    /// `RedrawRequested` and passed into `gpu::GpuState::render` (`render_frame`).
    pending_screenshot: Option<String>,
    /// The stdin reader waiting on `pending_screenshot` (see
    /// `DebugCommand::Screenshot`); sent `()` once it's been attempted.
    screenshot_waiter: Option<std::sync::mpsc::Sender<()>>,
    /// "Print to PDF": the target `.pdf` path, set alongside
    /// `pending_screenshot` (a temp PNG) so the underlying GPU capture
    /// mechanism doesn't need to know PDF exists at all -- once that
    /// screenshot actually completes (see `handle_screenshot_result`), the temp PNG is converted via
    /// `pdf::write_image_as_pdf` and deleted. `None` the rest of the time,
    /// including for an ordinary "Screenshot" button click.
    pending_print_to_pdf: Option<String>,

    /// "Recently Closed", oldest first: one entry per close, capped at
    /// `MAX_CLOSED`. The toolbar dropdown reopens any entry;
    /// `Ctrl+Shift+T`/headless `REOPENTAB` reopens the newest. See
    /// `ClosedEntry`.
    closed: Vec<ClosedEntry>,

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
    /// replace it, unlike `FindState::result`'s own "latest wins" (correct
    /// there -- a stale search count would be actively wrong, whereas a
    /// dropped one-time notification like "tab crashed" is just gone).
    save_toast: Option<(String, std::time::Instant)>,
    /// See `save_toast`'s own doc comment -- messages waiting to be shown,
    /// one at a time in order, each after the previous one's
    /// `SAVE_TOAST_DURATION` elapses. Pushed to by `queue_toast` (and
    /// directly by `DownloadCompleted`, for borrow reasons).
    save_toast_queue: std::collections::VecDeque<String>,

    /// When `save_session` last committed to the store -- shown in the
    /// settings popup's Sessions section next to a "Save Now" button, so a
    /// save that's actually continuous (every mutation calls
    /// `save_session`, not a timer) has some visible confirmation instead
    /// of being purely invisible. `None` until the first save this run.
    last_session_save: Option<std::time::Instant>,
    /// The last store error or repair notice toasted (`store_error`,
    /// `take_store_recovery`), so an identical one isn't repeated.
    last_store_notice: Option<String>,
    /// `settings::fingerprint()` as this process last saw it (set at load,
    /// updated after every successful save) -- see `save_settings`. `None`
    /// until the file has actually been loaded or saved once this run,
    /// which also means "skip the conflict check" the first time.
    settings_fingerprint: Option<u64>,

    /// Find-in-page, for the focused tab only. See `FindState`.
    find: FindState,

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
    /// `bookmarks.rs`. Listed by `App::build_bookmarks_popup`.
    bookmarks: Vec<bookmarks::Bookmark>,
    /// The bookmarks popup's own filter box text -- cleared each time the
    /// popup is opened, same "fresh state each open" reasoning
    /// `FindState::match_case` resetting on a new find session already has.
    bookmarks_filter: String,
    /// `(bookmark_url, draft)` while a bookmark's folder label in the
    /// popup is being renamed -- keyed by url (a bookmark's own natural
    /// identity, same as `toggle_bookmark`'s own match) rather than index,
    /// so it can't end up pointing at the wrong row if the list reorders.
    /// Same "real `TextEdit` replaces the label, Enter/blur commits,
    /// Escape cancels" shape `renaming_group` already has.
    renaming_bookmark_folder: Option<(String, String)>,
    /// Every distinct `(title, url)` this session has actually navigated
    /// to, most-recent-last, capped at `MAX_HISTORY_ENTRIES` (oldest
    /// dropped first) -- recorded once per real URL change in
    /// `EngineToHost::NavigationStateChanged`'s own handling, so it covers
    /// in-page navigation (clicked links) the same as address-bar entries,
    /// not just the latter. Session-only, cleared on restart -- unlike
    /// bookmarks, nothing here is ever written to disk.
    history: Vec<HistoryEntry>,
    /// The history popup's own filter box text -- same "cleared on each
    /// open" reasoning as `bookmarks_filter`.
    history_filter: String,
    /// Which popup anchored below a toolbar or Settings button is open, if
    /// any; one at a time. See `AnchoredPopup`, `toggle_popup` and
    /// `close_popup`.
    open_popup: Option<AnchoredPopup>,

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

    /// Failed tabs' cards (`FailedTabPayload`) whose `retry_failed_tab`
    /// spawn is in flight. Unlike a crash respawn, the card is still on
    /// the graph meanwhile, so it can be moved or closed: the outcome takes
    /// the card's placement as it is now, and is dropped if the card is
    /// gone. Set in `retry_failed_tab`, cleared by that outcome in
    /// `handle_tab_spawn_result`.
    failed_tab_retries: HashSet<String>,
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
            node_menu: None,
            node_menu_cache: None,
            json_viewer_node: None,
            quick_switcher: None,
            quick_switcher_cache: None,
            shortcuts_help_open: false,
            pending_import: None,
            session_name_draft: String::new(),
            graph_filter: String::new(),
            focused: None,
            viewed_image: None,
            image_view: ImageViewState::default(),
            split_tab: None,
            view_mode: ViewMode::Item,
            journal: None,
            fallback_ids: (store::model::ActorId::random(), 0),
            tab_spawns_in_flight: 0,
            spawning: HashSet::new(),
            remote_in_flight: HashMap::new(),
            explicit: std::collections::BTreeSet::new(),
            shared_pos: HashMap::new(),
            tabs_spawned_waiters: Vec::new(),
            session_generation: 0,
            commonmark_cache: egui_commonmark::CommonMarkCache::default(),
            cursor_pos: (0.0, 0.0),
            native_drag_tab: None,
            key_modifiers: 0,
            egui_ctx,
            egui_state: None,
            address_bar: AddressBar::new(settings.home_page.clone()),
            favicon_fetcher: FaviconFetcher::new(),
            pdf_fetcher: PdfFetcher::new(),
            redirect_fetcher: RedirectFetcher::new(),
            favicon_resolved: HashSet::new(),
            dragging: None,
            hovered: None,
            peek_suppressed: None,
            rect_select_start: None,
            selected_tabs: HashSet::new(),
            pending_new_node: None,
            camera: GraphCamera::new(),
            graph_lod_simplified: false,
            truncate_cache: HashMap::new(),
            favicon_origin_cache: HashMap::new(),
            graph_settled_key: None,
            headless: false,
            debug_input_events: Vec::new(),
            debug_address_bar_typing: None,
            pending_focus: None,
            quit_after_redraws: None,
            pending_screenshot: None,
            screenshot_waiter: None,
            pending_print_to_pdf: None,
            closed: Vec::new(),
            context_menu: None,
            context_menu_cache: None,
            save_toast: None,
            save_toast_queue: std::collections::VecDeque::new(),
            last_session_save: None,
            last_store_notice: None,
            settings_fingerprint,
            find: FindState::default(),
            tab_spawn_error: None,
            settings,
            settings_return_view: ViewMode::Item,
            bookmarks,
            bookmarks_filter: String::new(),
            renaming_bookmark_folder: None,
            history: Vec::new(),
            history_filter: String::new(),
            open_popup: None,
            transition: None,
            event_proxy: None,
            pending_respawn_focus: None,
            failed_tab_retries: HashSet::new(),
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
        let screen = self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
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
                self.focused.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|n| {
                    if n.title.is_empty() {
                        n.url.clone()
                    } else {
                        n.title.clone()
                    }
                })
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
        let full = self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
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
            node.tab.send(&HostToEngine::Resize { width, height, dpi_scale });
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

    /// Records what closing `id` loses (see `closed_entry`), dropping the
    /// oldest entry past `MAX_CLOSED`. Called before anything is removed.
    fn record_closed(&mut self, id: &str) {
        if let Some(entry) = closed_entry(&self.nodes, &self.node_order, id) {
            self.closed.push(entry);
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
    }

    /// Rebuilds a closed entry where it was: fresh ids, the old positions
    /// and tags, links between its own nodes remapped, and links outside it
    /// kept only if that node still exists (else the top level, or no
    /// parent). Tabs spawn as new tabs (`SpawnPurpose::New`, nothing
    /// focused). An image or JSON file that's gone since is toasted and
    /// skipped; a PDF reopens as its error card.
    fn reopen_closed(&mut self, entry: ClosedEntry) {
        let mut ids = std::collections::HashMap::new();
        for node in &entry.nodes {
            let id = match node.piece {
                ClosedPiece::Tab { .. } => self.alloc_node_id(),
                ClosedPiece::Group { .. } => self.alloc_node_id(),
                ClosedPiece::Note(_) => self.alloc_node_id(),
                ClosedPiece::Image(_) => self.alloc_node_id(),
                ClosedPiece::Pdf(_) => self.alloc_node_id(),
                ClosedPiece::Json(_) | ClosedPiece::JsonLeaf { .. } => self.alloc_node_id(),
            };
            ids.insert(node.id.clone(), id);
        }
        let relink = |old: &Option<String>| {
            let old = old.as_ref()?;
            ids.get(old).cloned().or_else(|| self.nodes.contains_key(old).then(|| old.clone()))
        };
        let placed: Vec<_> = entry.nodes.iter().map(|n| (relink(&n.parent), relink(&n.group))).collect();
        for (node, (parent, group)) in entry.nodes.into_iter().zip(placed) {
            let id = ids[&node.id].clone();
            let ClosedNode { pos, color, piece, .. } = node;
            // `None` once a helper has inserted the node itself.
            let payload = match piece {
                ClosedPiece::Tab { url, kind, pinned, zoom_factor } => {
                    self.tab_spawn_error = None;
                    let spawn = TabSpawn::new(id, url, kind, pos, SpawnPurpose::New, zoom_factor);
                    let carry = TabCarry { pinned, color, zoom_factor };
                    self.spawn_tab_async(TabSpawn { parent, group, carry, ..spawn });
                    continue;
                }
                ClosedPiece::Group { name } => Some(NodePayload::Group(GroupPayload { name, from_json: false })),
                ClosedPiece::Note(text) => Some(NodePayload::Note(NotePayload { text, editing: false })),
                ClosedPiece::JsonLeaf { path, key, value } => {
                    Some(NodePayload::Json(JsonPayload { path, value, key, error: None }))
                }
                ClosedPiece::Image(path) => {
                    let _ = self.insert_image_node_with_id(id.clone(), path, pos, group.clone());
                    None
                }
                ClosedPiece::Pdf(path) => {
                    self.insert_pdf_node_with_id(id.clone(), path, pos, group.clone());
                    None
                }
                ClosedPiece::Json(path) => {
                    let _ = self.insert_json_node_with_id(id.clone(), path, pos, group.clone());
                    None
                }
            };
            match payload {
                Some(payload) => {
                    self.nodes
                        .insert(id.clone(), GraphNode { parent, group, pos, vel: egui::Vec2::ZERO, color, payload });
                    self.node_order.push(id);
                }
                None => {
                    if let Some(node) = self.nodes.get_mut(&id) {
                        node.color = color;
                    }
                }
            }
        }
        self.save_session();
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
            node.tab.send(&HostToEngine::Navigate { url, referrer: None });
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
            .filter(|(_, node)| node.url.to_lowercase().contains(&query) || node.title.to_lowercase().contains(&query))
            .take(5)
            .map(|(id, node)| Suggestion::OpenTab {
                tab_id: id.clone(),
                url: node.url.clone(),
                title: node.title.clone(),
            })
            .collect()
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
        self.address_bar.dismiss();
        self.address_bar.focused = false;
    }

    /// Writes `settings.json`, unless another process (another
    /// `manifold.exe` on this profile) changed it since this one last read
    /// or wrote it: then the write goes to `settings::conflict_path()` and a
    /// toast says so, rather than clobbering theirs. The first save of a run
    /// (`settings_fingerprint` still `None`) always proceeds. Replaces every
    /// direct `self.settings.save()` call site.
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

    /// "Open Session" (settings popup). Named sessions move to the store in
    /// a later step of the session-store work; until then Open only says so.
    fn load_named_session(&mut self, _name: &str) {
        self.queue_toast("Named sessions are unavailable until the session store work lands".into());
    }

    /// Spawns the very first tab at startup: no parent, becomes focused,
    /// starts in Tab view (preserves "opens straight into a usable
    /// browser" from before M5 — Graph view is reached explicitly).
    fn spawn_root_tab(&mut self) {
        let id = self.alloc_node_id();
        let home_page = self.settings.home_page.clone();
        self.tab_spawn_error = None;
        let kind = self.settings.default_starting_engine;
        let zoom = self.settings.zoom_factor;
        self.spawn_tab_async(TabSpawn::new(id, home_page, kind, egui::Vec2::ZERO, SpawnPurpose::Root, zoom));
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

        let welcome_id = self.alloc_node_id();
        self.focused = Some(welcome_id.clone());
        self.view_mode = ViewMode::Graph;
        let welcome = "manifold://welcome".to_string();
        self.spawn_tab_async(TabSpawn::new(welcome_id, welcome, kind, egui::Vec2::ZERO, SpawnPurpose::Restore, zoom));

        let note_id = self.alloc_node_id();
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

        let group_id = self.alloc_node_id();
        let group_pos = egui::vec2(300.0, 0.0);
        self.nodes.insert(
            group_id.clone(),
            GraphNode {
                parent: None,
                group: None,
                pos: group_pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Group(GroupPayload { name: "Try grouping".to_string(), from_json: false }),
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
            let id = self.alloc_node_id();
            let pos = group_pos + offset;
            let spawn = TabSpawn::new(id, url.to_string(), kind, pos, SpawnPurpose::Restore, zoom);
            self.spawn_tab_async(TabSpawn { group: Some(group_id.clone()), ..spawn });
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
        let json_id = self.alloc_node_id();
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
    fn spawn_tab_async(&mut self, spawn: TabSpawn) {
        let proxy = self.event_proxy.clone().expect("event_proxy set in main before any tab spawn");
        self.tab_spawns_in_flight += 1;
        self.spawning.insert(spawn.id.clone());
        let generation = self.session_generation;
        std::thread::spawn(move || {
            let result = process_manager::spawn_tab(spawn.kind, &spawn.id, &spawn.url);
            let _ = proxy.send_event(UserEvent::TabSpawned(Box::new(TabSpawnOutcome { spawn, generation, result })));
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
            spawn: TabSpawn { id, url, kind, parent, group, pos, vel, purpose, carry, fallback_kind },
            generation,
            result,
        } = outcome;
        let TabCarry { pinned, color, zoom_factor } = carry;
        self.spawning.remove(&id);
        let remote = self.remote_in_flight.remove(&id);
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
        // same guard `reopen_closed`'s relinking applies for the same
        // reason, centralized here since every spawn path funnels through
        // this one function.
        let (parent, group, pos, color) = if self.failed_tab_retries.remove(&id) {
            match self.nodes.get(&id).filter(|n| n.as_failed_tab().is_some()) {
                Some(card) => (card.parent.clone(), card.group.clone(), card.pos, card.color),
                // Closed while retrying (see `failed_tab_retries`).
                None => return,
            }
        } else {
            (parent, group, pos, color)
        };
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
                    SpawnPurpose::Respawn | SpawnPurpose::Restore => self.focused.as_deref() == Some(id.as_str()),
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
                        self.address_bar.text = url;
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
                            if !self.address_bar.focused {
                                self.address_bar.text = url;
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
                            self.address_bar.text = url;
                            if visible {
                                self.send_resize_to(&id);
                            }
                        }
                    }
                }
                self.land_remote_in_flight(&id, remote);
                self.save_session();
            }
            Err(err) => {
                eprintln!("spawn tab failed for {id}: {err}");
                if let Some(fallback) = fallback_kind {
                    self.queue_toast(format!(
                        "Couldn't switch to {}: {err}. Kept {}.",
                        engine_label(kind),
                        engine_label(fallback)
                    ));
                    let carry = TabCarry { pinned, color, zoom_factor };
                    let spawn = TabSpawn {
                        id,
                        url,
                        kind: fallback,
                        parent,
                        group,
                        pos,
                        vel,
                        purpose,
                        carry,
                        fallback_kind: None,
                    };
                    if let Some(remote) = remote {
                        self.remote_in_flight.insert(spawn.id.clone(), remote);
                    }
                    self.respawn_tab(spawn);
                    return;
                }
                // A tab that already existed (a crash restart, a restored
                // session or group, a retry) keeps its place as a failed
                // card rather than disappearing; see `FailedTabPayload`.
                if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Restore) {
                    self.queue_toast(format!("Couldn't start {} for {url}: {err}", engine_label(kind)));
                    let error = err.to_string();
                    let failed = FailedTabPayload { url, kind, error, pinned, zoom_factor, retrying: false };
                    let payload = NodePayload::FailedTab(failed);
                    self.nodes.insert(id.clone(), GraphNode { parent, group, pos, vel, color, payload });
                    if !self.node_order.iter().any(|t| t == &id) {
                        self.node_order.push(id.clone());
                    }
                    self.land_remote_in_flight(&id, remote);
                    self.save_session();
                    return;
                }
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

    /// A fresh node id: the journal's counter (contract "Actors and ids"),
    /// or this launch's fallback actor without one.
    fn alloc_node_id(&mut self) -> String {
        match &mut self.journal {
            Some(journal) => journal.next_id().to_string(),
            None => {
                let (actor, counter) = &mut self.fallback_ids;
                let id = store::model::NodeId { actor: *actor, counter: *counter };
                *counter += 1;
                id.to_string()
            }
        }
    }

    /// Opens `session.mnf` under a fresh actor. A failure is toasted and
    /// leaves the app running without a store.
    fn open_store(&mut self) {
        let Some(path) = session::store_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match store::journal::Journal::open(&path, store::model::ActorId::random()) {
            Ok(journal) => self.journal = Some(journal),
            Err(err) => self.queue_toast(format!("Couldn't open the session file {}: {err}", path.display())),
        }
    }

    /// Removes a download or note node from the graph -- a download's file
    /// on disk is left untouched (this only forgets the node), and neither
    /// kind has anything else to persist/tear down on removal the way a
    /// tab's engine process or a group's membership does, so one plain
    /// `self.nodes`/`node_order` removal (plus the same stale-`dragging`/
    /// `hovered` guard `destroy_tab`/`finish_swap` also apply) covers both.
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
            NodePayload::Tab(_) | NodePayload::FailedTab(_) | NodePayload::Group(_) | NodePayload::Note(_) => None,
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

    /// A fresh group's default display name: "Group N" for the smallest N
    /// no group is named yet. Not the group count plus one, which repeats a
    /// name after a dissolve (three groups "Group 1/2/3", dissolve "Group
    /// 2", and the next new one is "Group 3" again), and not the id, which
    /// is a long store id. Manually *renaming* two groups to the identical
    /// string is left alone -- that's a deliberate user choice.
    fn default_group_name(&self) -> String {
        let taken: HashSet<&str> = self
            .nodes
            .values()
            .filter_map(|node| match &node.payload {
                NodePayload::Group(group) => Some(group.name.as_str()),
                _ => None,
            })
            .collect();
        (1..).map(|n| format!("Group {n}")).find(|name| !taken.contains(name.as_str())).expect("a free number")
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
            self.switch_group_level(parent);
        }
        self.forget_removed_group(id);
    }

    /// Drops the per-card state still naming a group card that's gone
    /// (dissolved or merged away): the released drag, the hover latch the
    /// peek fade polls, and a rename in progress.
    fn forget_removed_group(&mut self, id: &str) {
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
    /// `self.current_group` is currently viewing -- the id list
    /// `build_graph_ui`'s one card loop iterates for that kind, kept in stable
    /// insertion order (unlike filtering `self.nodes` directly, a
    /// `HashMap`'s iteration order isn't stable frame-to-frame). The kind
    /// filter matters even for one kind: a top-level group's own `group`
    /// is `None`, same as `current_group` at the top level, so a level
    /// check alone would leak group cards into every other kind's list.
    fn nodes_at_current_level(&self, kind: ItemNodeKind) -> Vec<String> {
        self.node_order
            .iter()
            .filter(|id| {
                self.nodes.get(*id).is_some_and(|n| n.group == self.current_group && item_node_kind(&n.payload) == kind)
            })
            .cloned()
            .collect()
    }

    /// Where the dragged card `id` would drop right now, and what that
    /// would do (`drop_action`). The back button is tested first, by
    /// pointer, since it's a screen-space toolbar widget. Then the card at
    /// this level containing the dragged card's center, a group card before
    /// a tab card. Feeds both the live ring and the release, so the two
    /// can't disagree.
    fn drop_target(
        &self,
        id: &str,
        pointer: Option<egui::Pos2>,
        leave_button: Option<egui::Rect>,
    ) -> Option<(DropEffect, Option<String>)> {
        let node = self.nodes.get(id)?;
        let dragged = item_node_kind(&node.payload);
        let inside_group = self.current_group.is_some();
        if leave_button.zip(pointer).is_some_and(|(button, pointer)| button.contains(pointer)) {
            return drop_action(dragged, DropTarget::BackButton, inside_group).map(|effect| (effect, None));
        }
        let center = egui::Pos2::new(node.pos.x, node.pos.y);
        [ItemNodeKind::Group, ItemNodeKind::Tab].into_iter().find_map(|kind| {
            let effect = drop_action(dragged, DropTarget::Card(kind), inside_group)?;
            let target = self.nodes_at_current_level(kind).into_iter().find(|other| {
                other != id && self.nodes.get(other).is_some_and(|n| node_card_rect(n).contains(center))
            })?;
            Some((effect, Some(target)))
        })
    }

    /// "Remove from Group" from any path (eject button, menu, a drop on
    /// the way back). The card leaves this level, so it leaves the
    /// rectangle selection too, which only ever holds this level's tabs
    /// (`switch_group_level` clears it): a later multi-select drag would
    /// otherwise move, or Join would pull back in, a tab no longer shown.
    fn eject(&mut self, id: &str) {
        remove_from_group(&mut self.nodes, id);
        self.selected_tabs.remove(id);
    }

    /// What a drop moves along with `id`: the whole rectangle selection
    /// when `id` is in it (it acts as one unit, as "Group Selected" does),
    /// which it empties, else just `id`.
    fn take_drag_unit(&mut self, id: &str) -> Vec<String> {
        if self.selected_tabs.contains(id) {
            self.selected_tabs.drain().collect()
        } else {
            vec![id.to_string()]
        }
    }

    /// A card released after a drag (`GraphAction::Drop`), with where it
    /// landed, if anywhere. Saves the session on every release, since the
    /// drag itself moved the card.
    fn apply_drop(&mut self, id: &str, target: Option<(DropEffect, Option<String>)>) {
        match target {
            Some((DropEffect::Eject, _)) => self.eject(id),
            Some((DropEffect::Join, Some(group))) => {
                for member in self.take_drag_unit(id) {
                    if let Some(node) = self.nodes.get_mut(&member) {
                        node.group = Some(group.clone());
                    }
                }
            }
            Some((DropEffect::GroupWith, Some(other))) => {
                let group = self.alloc_node_id();
                let pos = self.nodes.get(&other).map_or(egui::Vec2::ZERO, |n| n.pos);
                self.insert_group_at(group.clone(), pos);
                let unit = self.take_drag_unit(id);
                for member in std::iter::once(other).chain(unit) {
                    if let Some(node) = self.nodes.get_mut(&member) {
                        node.group = Some(group.clone());
                    }
                }
            }
            Some((DropEffect::Reparent, Some(parent))) => {
                if !self.would_create_cycle(id, &parent) {
                    if let Some(node) = self.nodes.get_mut(id) {
                        node.parent = Some(parent);
                    }
                }
            }
            Some((DropEffect::Merge, Some(target))) => self.merge_groups(id, &target),
            _ => {}
        }
        self.save_session();
    }

    /// `source`'s members move into `target`, and `source` goes: a group
    /// card dropped onto another. Refused if `target` is nested inside
    /// `source` (directly or transitively, walking the same `.group` chain
    /// `group_breadcrumb` does): reassigning source's own members to target
    /// while target is one of them would leave it (or a group between them)
    /// pointing at itself once source is removed, a self-reference cycle.
    fn merge_groups(&mut self, source: &str, target: &str) {
        let mut ancestor = Some(target.to_string());
        while let Some(cur) = ancestor {
            if cur == source {
                return;
            }
            ancestor = self.nodes.get(&cur).and_then(|n| n.group.clone());
        }
        for node in self.nodes.values_mut() {
            if node.group.as_deref() == Some(source) {
                node.group = Some(target.to_string());
            }
        }
        self.nodes.remove(source);
        self.node_order.retain(|g| g != source);
        self.forget_removed_group(source);
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
        // refuses a cycle at the source -- see `App::merge_groups`'s own
        // ancestor check), but this would hang the whole app with no
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
            self.node_order.iter().filter(|id| self.nodes.get(*id).is_some_and(|n| n.group == self.current_group)),
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
                if node.parent.as_deref() == Some(current.as_str()) && node.group == level_group && !result.contains(id)
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
        (self.current_group.clone(), GRAPH_DRAW_ORDER.iter().map(|&kind| self.nodes_at_current_level(kind).len()).sum())
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
            if best.as_ref().is_none_or(|(_, s)| score < *s) {
                best = Some((id, score));
            }
        }
        best.map(|(id, _)| id)
    }

    /// Moves `self.focused` to the nearest tab node in `dir` (see
    /// `nearest_tab_in_direction`), or -- if nothing at this level is
    /// currently focused -- just picks a starting node so the first arrow
    /// press has something to move from next time. Shared by
    /// `handle_graph_keys`' real-keyboard arrow handling and the headless
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
            let Some(node) = self.nodes.get(&id).filter(|n| n.group == self.current_group) else {
                return;
            };
            // A crashed focused tab stays focused as its failed card, which
            // has no Item view: Enter retries it, as its click does.
            if node.as_failed_tab().is_some() {
                self.retry_missing_file(&id);
            } else {
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
        // and fixed four separate times elsewhere -- `App::merge_groups`,
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
            graph_layout::step(&mut positions, &mut velocities, &edges, &pinned, GRAPH_WORLD_CENTER, SETTLE_DT);
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
        let Some((parent, group, url, kind)) = self
            .nodes
            .get(id)
            .and_then(|n| n.as_tab().map(|t| (n.parent.clone(), n.group.clone(), t.url.clone(), t.tab.kind)))
        else {
            return;
        };
        self.spawn_new_tab_with_kind(parent, group, &url, kind, SpawnPurpose::New);
    }

    fn spawn_new_tab(&mut self, parent: Option<String>, group: Option<String>, url: &str) {
        self.spawn_new_tab_with_kind(parent, group, url, self.settings.default_starting_engine, SpawnPurpose::New);
    }

    /// Shared by `spawn_new_tab` (always `default_starting_engine`),
    /// `duplicate_tab` (the original tab's own engine), and `start_split`
    /// (`SpawnPurpose::Split` instead of `New`, everything else identical)
    /// -- `purpose` is threaded through to `spawn_tab_async` rather than
    /// hardcoded, since those cases genuinely differ in what should happen once the
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
        let id = self.alloc_node_id();
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
        let spawn = TabSpawn::new(id.clone(), url.to_string(), kind, pos, purpose, self.settings.zoom_factor);
        self.spawn_tab_async(TabSpawn { parent, group, ..spawn });
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
            if !self.address_bar.focused {
                self.address_bar.text = node.url.clone();
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
                self.transition =
                    Some(Transition { node_id: id, kind: TransitionKind::ToGraph, start: std::time::Instant::now() });
            }
            // No focused tab and no viewed image (shouldn't normally
            // happen — Item view always has one — but nothing to animate
            // either way).
            None => {
                self.view_mode = ViewMode::Graph;
                self.save_session();
            }
        }
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
    /// closed tab was focused. Records nothing: `close_node` is the close
    /// that feeds "Recently Closed".
    fn destroy_tab(&mut self, id: &str) {
        self.remove_tab(id);
        self.save_session();
    }

    /// `destroy_tab` without the save: a tab another replica deleted
    /// (`remove_remotely_deleted`), whose removal is not a local edit.
    fn remove_tab(&mut self, id: &str) {
        let Some(parent) = self.nodes.get(id).filter(|n| n.as_tab().is_some()).map(|n| n.parent.clone()) else {
            return;
        };
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
        // `selected_tabs` -- harmless to the no-op `destroy_tab`/`nodes.get`
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
            // to send `FindStop` to, so only the UI side of the close.
            self.find.close();
        }
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
        let Some(spawn) = self.nodes.get(tab_id).and_then(|n| {
            let old = n.as_tab()?.tab.kind;
            Some(TabSpawn { fallback_kind: Some(old), ..TabSpawn::respawn(tab_id, n, target, url)? })
        }) else {
            return;
        };
        // Must drop (and thus Shutdown + kill) the old tab before spawning
        // the new one at the same tab_id — the new engine-host's named-pipe
        // listeners would collide with the old ones otherwise.
        self.nodes.remove(tab_id);
        // Same stale-state guard `destroy_tab` already applies: without it, a
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
        self.respawn_tab(spawn);
    }

    /// Shared by swap-finish and crash recovery: starts `spawn` (from
    /// `TabSpawn::respawn`, so the tab keeps its place in the graph and its
    /// `TabCarry`) and, if it's the focused tab, shows a spinner until it's
    /// back (only the focused tab is ever visible).
    fn respawn_tab(&mut self, spawn: TabSpawn) {
        if self.focused.as_deref() == Some(spawn.id.as_str()) {
            self.pending_respawn_focus = Some(spawn.id.clone());
        }
        self.spawn_tab_async(spawn);
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
            self.reopen_closed_shortcut();
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
            self.pending_focus = Some(FocusTarget::QuickSwitcher);
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
        self.pending_focus = Some(FocusTarget::GroupRename);
    }

    /// `Ctrl+Shift+G` / toolbar "+ New Group" / headless `NEWGROUP` —
    /// creates an empty group at whichever level is currently being
    /// viewed, nested under `current_group` if that's not the top level
    /// (see `GroupPayload`'s own doc comment on nesting).
    fn new_group_shortcut(&mut self) {
        let id = self.alloc_node_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        self.insert_group_at(id, pos);
        self.save_session();
    }

    /// Shared by `new_group_shortcut` (fresh id/jittered position) and the
    /// empty-canvas "New Tab/Note/Group/File" card (`pending_new_node`),
    /// which passes the exact position that was right-clicked instead.
    fn insert_group_at(&mut self, id: String, pos: egui::Vec2) {
        let name = self.default_group_name();
        self.nodes.insert(
            id.clone(),
            GraphNode {
                parent: None,
                group: self.current_group.clone(),
                pos,
                vel: egui::Vec2::ZERO,
                color: None,
                payload: NodePayload::Group(GroupPayload { name, from_json: false }),
            },
        );
        self.node_order.push(id);
    }

    /// The Graph toolbar's "+ Note" button -- a note spawns at whichever
    /// level is currently being viewed, same as "+ New Tab" and (now)
    /// "+ New Group" itself, since a note is a
    /// plain member of a level's subgraph like a tab or download is.
    fn new_note_shortcut(&mut self) {
        let id = self.alloc_node_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        self.insert_note_at(id, pos);
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
    /// if `tab_id` is gone. The store keeps only completed downloads
    /// (`DownloadCompleted` saves), so no save.
    fn insert_download_node(&mut self, tab_id: String, path: String, total_bytes: Option<u64>) {
        let Some((node_pos, node_group)) = self.nodes.get(&tab_id).map(|n| (n.pos, n.group.clone())) else {
            return;
        };
        let id = self.alloc_node_id();
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

    /// Inserts a node showing `payload` at `pos`, top level and uncolored:
    /// `materialize`'s way in for kinds with no insert helper of their own,
    /// and for an image/JSON card whose file couldn't be loaded (see
    /// `ImagePayload::error`).
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
        if let Some(kind) = self.nodes.get(id).and_then(|n| n.as_failed_tab()).map(|t| t.kind) {
            self.retry_failed_tab(id, kind);
            return;
        }
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

    /// Spawns a failed tab's card (`FailedTabPayload`) again, on `kind`.
    /// The card stays, showing a spinner, until the outcome replaces it:
    /// the live tab under the same id, or a fresh failed card with the new
    /// error.
    fn retry_failed_tab(&mut self, id: &str, kind: EngineKind) {
        let Some(node) = self.nodes.get_mut(id) else { return };
        let (parent, group, pos, color) = (node.parent.clone(), node.group.clone(), node.pos, node.color);
        let NodePayload::FailedTab(failed) = &mut node.payload else { return };
        if failed.retrying {
            return;
        }
        failed.retrying = true;
        self.failed_tab_retries.insert(id.to_string());
        let carry = TabCarry { pinned: failed.pinned, color, zoom_factor: failed.zoom_factor };
        let url = failed.url.clone();
        let vel = egui::Vec2::ZERO;
        let (id, purpose, fallback_kind) = (id.to_string(), SpawnPurpose::Respawn, None);
        self.respawn_tab(TabSpawn { id, url, kind, parent, group, pos, vel, purpose, carry, fallback_kind });
    }

    fn insert_image_node(&mut self, path: String) -> bool {
        let id = self.alloc_node_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let group = self.current_group.clone();
        self.insert_image_node_with_id(id, path, pos, group).is_ok()
    }

    /// The insert every `image` card goes through: the caller picks the id,
    /// position and group -- `insert_image_node` a fresh id at the current
    /// level, session restore the saved id and position with no group,
    /// "Recently Closed" the old position and group (`reopen_closed`), among others.
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
    /// same dragging/hovered/focused guards `destroy_tab` does -- but doesn't
    /// push to `closed_tabs`, since this isn't a user-initiated close to
    /// remember as reopenable.
    fn redirect_tab_to_image(&mut self, tab_id: &str, path: String) {
        let Some((parent, group, pos)) = self.nodes.get(tab_id).map(|n| (n.parent.clone(), n.group.clone(), n.pos))
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
            // `destroy_tab` -- see its own comment.
            if self.showing_tab_view() {
                self.view_mode = ViewMode::Graph;
            }
            self.find.close();
        }
        let id = self.alloc_node_id();
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
        let id = self.alloc_node_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let group = self.current_group.clone();
        self.insert_pdf_node_with_id(id, path, pos, group);
    }

    /// The insert every `pdf` card goes through: the caller picks the id,
    /// position and group -- `insert_pdf_node` a fresh id at the current
    /// level, session restore the saved id and position with no group,
    /// "Recently Closed" the old position and group (`reopen_closed`), among others.
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
        let id = self.alloc_node_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let group = self.current_group.clone();
        self.insert_json_node_with_id(id, path, pos, group).is_ok()
    }

    /// The insert every `json` card goes through: the caller picks the id,
    /// position and group -- `insert_json_node` a fresh id at the current
    /// level, session restore the saved id and position with no group,
    /// "Recently Closed" the old position and group (`reopen_closed`), among others.
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
    /// no new node kind exists for this). Read-only and one-way, per this
    /// feature's own design doc
    /// (`docs/superpowers/specs/2026-09-19-json-expand-as-graph-design.md`):
    /// there's no "collapse back". The groups and leaves are saved like any
    /// other node (`GroupPayload::from_json` marks the groups). The original
    /// card is replaced by the root group under a fresh id, keeping its
    /// `pos`/`group`/`color`, since a store node never changes kind: after
    /// this there's no compact card left to click back to, only
    /// "Add File..." to bring the file back as a fresh node. A bare scalar at the JSON's
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
            // A store node never changes kind, so the card becomes a group
            // under a fresh id, keeping its place, group, color and draw
            // order; the JSON node is deleted from the store.
            let Some(mut node) = self.nodes.remove(json_node_id) else { return };
            let root_id = self.alloc_node_id();
            node.payload =
                NodePayload::Group(GroupPayload { name: download_filename(&path).to_string(), from_json: true });
            self.nodes.insert(root_id.clone(), node);
            for id in self.node_order.iter_mut().filter(|id| *id == json_node_id) {
                *id = root_id.clone();
            }
            for node in self.nodes.values_mut().filter(|n| n.parent.as_deref() == Some(json_node_id)) {
                node.parent = Some(root_id.clone());
            }
            for slot in [&mut self.dragging, &mut self.hovered, &mut self.json_viewer_node] {
                if slot.as_deref() == Some(json_node_id) {
                    *slot = None;
                }
            }
            self.expand_json_entries(&root_id, &value, &path);
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
            serde_json::Value::Array(items) => items.iter().enumerate().map(|(i, v)| (format!("[{i}]"), v)).collect(),
            _ => return,
        };
        let parent_pos = self.nodes.get(parent_group_id).map(|n| n.pos).unwrap_or(GRAPH_WORLD_CENTER);
        for (key, entry_value) in entries {
            if matches!(entry_value, serde_json::Value::Object(_) | serde_json::Value::Array(_)) {
                let id = self.alloc_node_id();
                let pos = parent_pos + graph_layout::spawn_jitter(&id);
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        parent: None,
                        group: Some(parent_group_id.to_string()),
                        pos,
                        vel: egui::Vec2::ZERO,
                        color: None,
                        payload: NodePayload::Group(GroupPayload { name: key, from_json: true }),
                    },
                );
                self.node_order.push(id.clone());
                self.expand_json_entries(&id, entry_value, path);
            } else {
                let id = self.alloc_node_id();
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
            .add_filter("Image, PDF, or JSON", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "pdf", "json"])
            .pick_file()
        else {
            return;
        };
        let path = path.to_string_lossy().into_owned();
        let extension =
            std::path::Path::new(&path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
        let inserted = match (extension.as_deref(), pos) {
            (Some("pdf"), Some(pos)) => {
                let id = self.alloc_node_id();
                let group = self.current_group.clone();
                self.insert_pdf_node_with_id(id, path, pos, group);
                true
            }
            (Some("pdf"), None) => {
                self.insert_pdf_node(path);
                true
            }
            (Some("json"), Some(pos)) => {
                let id = self.alloc_node_id();
                let group = self.current_group.clone();
                self.insert_json_node_with_id(id, path, pos, group).is_ok()
            }
            (Some("json"), None) => self.insert_json_node(path),
            (Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"), Some(pos)) => {
                let id = self.alloc_node_id();
                let group = self.current_group.clone();
                self.insert_image_node_with_id(id, path, pos, group).is_ok()
            }
            (Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"), None) => self.insert_image_node(path),
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
                    self.show_level_of(&id);
                    self.focus_tab(&id);
                }
            }
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

    /// `Ctrl+Shift+T` / headless `REOPENTAB` — reopens the newest "Recently
    /// Closed" entry of any kind, where it was (`reopen_closed`); a no-op
    /// if there's none.
    fn reopen_closed_shortcut(&mut self) {
        if let Some(entry) = self.closed.pop() {
            self.reopen_closed(entry);
        }
    }

    /// `Ctrl+W` / headless `CLOSETAB` — closes the focused tab, a no-op if
    /// none is focused (Graph view with nothing selected).
    fn close_focused_tab_shortcut(&mut self) {
        if let Some(id) = self.focused.clone() {
            self.close_node(&id);
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
    /// normal `close_node` path, same "just that action run once per tab"
    /// shape `close_all_except` already has.
    fn close_all_at_current_level(&mut self) {
        for id in self.nodes_at_current_level(ItemNodeKind::Tab) {
            self.close_node(&id);
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
    /// about PDF -- `handle_screenshot_result` checks `pending_print_to_pdf` once that capture
    /// actually completes and does the PNG-to-PDF conversion there. Called
    /// by both the real toolbar button (after a real file-picker chooses
    /// `pdf_path`) and the headless `PRINTTOPDF` debug command (which
    /// supplies `pdf_path` directly, bypassing the picker).
    fn start_print_to_pdf(&mut self, pdf_path: String) {
        let timestamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
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
    /// `keep_id` through the normal `close_node` path (teardown, reparenting,
    /// Recently Closed all still apply -- this is just that same
    /// action run once per other tab, not a bulk shortcut around it).
    fn close_all_except(&mut self, keep_id: &str) {
        let other_ids: Vec<String> = self
            .node_order
            .iter()
            .filter(|id| id.as_str() != keep_id && self.nodes.get(id.as_str()).is_some_and(|n| n.as_tab().is_some()))
            .cloned()
            .collect();
        for id in other_ids {
            self.close_node(&id);
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
    /// since Graph view has no address bar to focus. Queues a focus request
    /// rather than setting `address_bar.focused` directly: real egui focus
    /// can only be requested on the widget, when `build_tab_view_ui` builds
    /// it (see `FocusTarget`).
    fn focus_address_bar_shortcut(&mut self) {
        if self.showing_tab_view() {
            self.pending_focus = Some(FocusTarget::AddressBar);
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
        self.find.open = true;
        self.pending_focus = Some(FocusTarget::FindBar);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if !self.find.text.is_empty() {
            self.send_find_start();
        }
    }

    /// The minimal right-click context menu: one "Open in new tab" item at
    /// `self.context_menu`'s position, if a menu is currently showing.
    /// Dismissed by clicking the item (and acting on it), a click anywhere
    /// outside the menu's own rect, or Escape (see the Escape-priority
    /// chain in `build_tab_view_ui`).
    fn build_context_menu(&mut self, ui: &mut egui::Ui) {
        let current = self.context_menu.clone();
        let Some((menu, fade)) =
            fade_popup_state(ui.ctx(), egui::Id::new("context_menu_fade"), &current, &mut self.context_menu_cache)
        else {
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
                popup_frame(ui).show(ui, |ui| {
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

    /// Destroys a group and everything in it, at any depth (`destroy_node`
    /// on each member), then the group itself. Records nothing: `close_node`
    /// has already recorded the whole subtree as one entry.
    fn destroy_group(&mut self, id: &str) {
        let member_ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.group.as_deref() == Some(id))
            .map(|(tid, _)| tid.clone())
            .collect();
        for tid in member_ids {
            self.destroy_node(&tid);
        }
        self.dissolve_group(id);
        // `destroy_node` doesn't save, and there's no save on exit -- without
        // this, a group whose members were only Downloads/Notes/Images/Pdfs/Jsons
        // (no Tab member whose `destroy_tab` happens to save) would have
        // those members reappear after a restart unless some unrelated
        // action saved first.
        self.save_session();
    }

    /// Closes one node: records it (and, for a group, everything under it)
    /// as one "Recently Closed" entry, then `destroy_node`s it. The single
    /// user-facing close: a card's hover ✕, its context-menu Delete/Close/
    /// "Remove from List", the Downloads popup's Remove, Ctrl+W, Close
    /// Selected and the rest. It doesn't save (callers decide:
    /// `close_node_and_save`); a tab's or group's destroy saves itself.
    fn close_node(&mut self, id: &str) {
        self.record_closed(id);
        self.destroy_node(id);
    }

    /// Removes one node with its kind's full cleanup, recording nothing: a
    /// Group recurses (`destroy_group`), a Tab goes through `destroy_tab`,
    /// and a Download/Note/Image/Pdf/Json frees its GPU texture if it's an
    /// Image/Pdf, clears `json_viewer_node` if it was showing this node,
    /// then `remove_node`s it.
    /// (Before this was shared, `destroy_tab`'s own no-op-on-non-tab guard
    /// left every non-tab, non-group member of a closed group merely
    /// reparented by `dissolve_group`, never actually destroyed -- a gap
    /// confirmed while building "Expand as Graph", whose whole output is
    /// `JsonPayload` leaves nested inside Groups.)
    fn destroy_node(&mut self, id: &str) {
        let Some(kind) = self.nodes.get(id).map(|node| item_node_kind(&node.payload)) else {
            return;
        };
        match kind {
            ItemNodeKind::Group => self.destroy_group(id),
            ItemNodeKind::Tab => self.destroy_tab(id),
            ItemNodeKind::FailedTab
            | ItemNodeKind::Download
            | ItemNodeKind::Note
            | ItemNodeKind::Image
            | ItemNodeKind::Pdf
            | ItemNodeKind::Json => {
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
    /// except for a tab or group, whose `destroy_tab`/`destroy_group`
    /// already save.
    fn close_node_and_save(&mut self, id: &str) {
        let persisted = self.nodes.get(id).is_some_and(|node| {
            matches!(
                item_node_kind(&node.payload),
                ItemNodeKind::FailedTab
                    | ItemNodeKind::Download
                    | ItemNodeKind::Note
                    | ItemNodeKind::Image
                    | ItemNodeKind::Pdf
                    | ItemNodeKind::Json
            )
        });
        self.close_node(id);
        if persisted {
            self.save_session();
        }
    }

    /// Builds an exportable snapshot of `group_id`'s own subtree -- its
    /// name, tag color, the groups nested in it at any depth, and every
    /// tab in any of them (url/engine/position), with `parent` links
    /// preserved only between those tabs (see `group_export::
    /// GroupExportTab`'s own doc comment). "Expand as Graph" groups
    /// (`GroupPayload::from_json`) are left out as in a session; their
    /// members go in the nearest exported group around them. `None` if the
    /// group doesn't exist.
    fn export_group(&self, group_id: &str) -> Option<group_export::GroupExport> {
        let group_node = self.nodes.get(group_id)?;
        let group = group_node.as_group()?;
        let group_color = group_node.color;
        // `group_id` and every group inside it, at any depth.
        let mut inside: HashSet<&str> = HashSet::from([group_id]);
        loop {
            let before = inside.len();
            for (id, node) in &self.nodes {
                if node.as_group().is_some() && node.group.as_deref().is_some_and(|g| inside.contains(g)) {
                    inside.insert(id.as_str());
                }
            }
            if inside.len() == before {
                break;
            }
        }
        // The exported group a node in `group` goes in: `None` for
        // `group_id` itself, else the nearest group that isn't `from_json`.
        let export_group = |group: Option<&str>| -> Option<String> {
            // Re-borrowed from `self.nodes` so the walk up can reassign it.
            let mut group = group.and_then(|g| self.nodes.get_key_value(g)).map(|(id, _)| id.as_str());
            while let Some(id) = group.filter(|&id| id != group_id) {
                let node = self.nodes.get(id)?;
                match node.as_group() {
                    Some(g) if !g.from_json => return Some(id.to_string()),
                    _ => group = node.group.as_deref(),
                }
            }
            None
        };
        let groups = self
            .node_order
            .iter()
            .filter(|id| id.as_str() != group_id && inside.contains(id.as_str()))
            .filter_map(|id| {
                let node = self.nodes.get(id)?;
                let nested = node.as_group().filter(|g| !g.from_json)?;
                Some(group_export::GroupExportGroup {
                    id: id.clone(),
                    name: nested.name.clone(),
                    color: node.color.map(|c| (c.r(), c.g(), c.b())),
                    pos_x: node.pos.x,
                    pos_y: node.pos.y,
                    group: export_group(node.group.as_deref()),
                })
            })
            .collect();
        let member_ids: HashSet<&str> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.group.as_deref().is_some_and(|g| inside.contains(g)))
            .map(|(id, _)| id.as_str())
            .collect();
        let tabs = self
            .nodes
            .iter()
            .filter(|(id, _)| member_ids.contains(id.as_str()))
            .filter_map(|(id, node)| {
                let (url, engine) = node.payload.saved_tab()?;
                Some(group_export::GroupExportTab {
                    id: id.clone(),
                    parent: node.parent.clone().filter(|p| member_ids.contains(p.as_str())),
                    url: url.to_string(),
                    engine,
                    pos_x: node.pos.x,
                    pos_y: node.pos.y,
                    group: export_group(node.group.as_deref()),
                })
            })
            .collect();
        Some(group_export::GroupExport {
            name: group.name.clone(),
            color: group_color.map(|c| (c.r(), c.g(), c.b())),
            tabs,
            groups,
        })
    }

    /// Recreates an exported group at the level being viewed -- a new group
    /// `GraphNode`, its nested groups, and each member tab, spawned via the
    /// normal async path. Every tab and group gets a freshly allocated id
    /// (`alloc_node_id`) rather than reusing the exported
    /// one, since importing into an already-running session can't assume
    /// the old ids are free; `parent` and `group` links are remapped
    /// through the old-id -> new-id maps built while allocating them. Uses
    /// `SpawnPurpose::Restore` so imported tabs never steal focus, same as
    /// startup session restore.
    fn import_group_export(&mut self, export: group_export::GroupExport) {
        let group_id = self.alloc_node_id();
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
                payload: NodePayload::Group(GroupPayload { name: export.name, from_json: false }),
            },
        );
        self.node_order.push(group_id.clone());

        let group_map: HashMap<String, String> =
            export.groups.iter().map(|g| (g.id.clone(), self.alloc_node_id())).collect();
        // A link to a group the file doesn't have lands in the imported group itself.
        let new_group = |old: Option<String>| {
            Some(old.and_then(|g| group_map.get(&g).cloned()).unwrap_or_else(|| group_id.clone()))
        };
        for nested in export.groups {
            let Some(new_id) = group_map.get(&nested.id).cloned() else { continue };
            let node = GraphNode {
                parent: None,
                group: new_group(nested.group),
                pos: egui::vec2(nested.pos_x, nested.pos_y),
                vel: egui::Vec2::ZERO,
                color: nested.color.map(|(r, g, b)| egui::Color32::from_rgb(r, g, b)),
                payload: NodePayload::Group(GroupPayload { name: nested.name, from_json: false }),
            };
            self.nodes.insert(new_id.clone(), node);
            self.node_order.push(new_id);
        }
        let id_map: HashMap<String, String> =
            export.tabs.iter().map(|tab| (tab.id.clone(), self.alloc_node_id())).collect();
        for tab in export.tabs {
            let Some(new_id) = id_map.get(&tab.id).cloned() else { continue };
            let parent = tab.parent.and_then(|p| id_map.get(&p).cloned());
            let pos = egui::vec2(tab.pos_x, tab.pos_y);
            let spawn =
                TabSpawn::new(new_id, tab.url, tab.engine, pos, SpawnPurpose::Restore, self.settings.zoom_factor);
            self.spawn_tab_async(TabSpawn { parent, group: new_group(tab.group), ..spawn });
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
        let current = self.node_menu.clone().and_then(|(id, pos)| self.node_menu_view(&id).map(|view| (id, pos, view)));
        let Some(((id, pos, view), fade)) =
            fade_popup_state(ui.ctx(), egui::Id::new("node_menu_fade"), &current, &mut self.node_menu_cache)
        else {
            return;
        };
        let live = current.is_some();
        let config = node_menu_config(&view, self.headless);
        let mut frozen_draft = view.renaming.clone();
        let rename = match &mut self.renaming_group {
            Some((gid, draft)) if live && *gid == id => {
                Some((draft, take_focus(&mut self.pending_focus, FocusTarget::GroupRename)))
            }
            _ => frozen_draft.as_mut().map(|draft| (draft, false)),
        };
        let (action, clicked_outside) = build_node_context_menu(ui, pos, fade, live, &config, view.color, rename);
        let close = match action {
            Some(NodeAction::Rename) => false,
            Some(_) => true,
            None => clicked_outside && view.renaming.is_none(),
        };
        if let Some(action) = action {
            self.apply_node_action(ui.ctx(), &id, action);
            self.save_session();
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
            other_default_engine: node
                .payload
                .as_failed_tab()
                .is_some_and(|t| t.kind != self.settings.default_starting_engine),
            renaming: self.renaming_group.as_ref().filter(|(gid, _)| gid == id).map(|(_, draft)| draft.clone()),
        })
    }

    /// Applies whichever item was clicked in a node's right-click menu.
    /// Each action only ever appears on kinds where it makes sense
    /// (`node_menu_config`), so none of these re-check the kind.
    fn apply_node_action(&mut self, ctx: &egui::Context, id: &str, action: NodeAction) {
        match action {
            NodeAction::Open => {
                if self.nodes.contains_key(id) {
                    self.selected_tabs.clear();
                    self.start_transition_to_item(id);
                }
            }
            NodeAction::CopyText => {
                if let Some(text) = self.nodes.get(id).and_then(|n| n.as_note()).map(|n| n.text.clone()) {
                    ctx.copy_text(text);
                }
            }
            NodeAction::Reveal => self.reveal_node(id),
            NodeAction::CopyPath => {
                if let Some(path) = self.node_file_path(id) {
                    ctx.copy_text(path);
                }
            }
            NodeAction::ExpandAsGraph => {
                self.expand_json_as_graph(id);
                self.save_session();
            }
            NodeAction::TogglePin => {
                if let Some(tab) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) {
                    tab.pinned = !tab.pinned;
                }
            }
            NodeAction::Duplicate => self.duplicate_tab(id),
            NodeAction::CopyUrl => {
                if let Some((url, _)) = self.nodes.get(id).and_then(|n| n.payload.saved_tab()) {
                    ctx.copy_text(url.to_string());
                }
            }
            NodeAction::ViewJson => self.json_viewer_node = Some(id.to_string()),
            NodeAction::FitSubtree => self.camera.jump_to(self.fit_subtree_scene_rect(id)),
            NodeAction::Rename => {
                let name = self.nodes.get(id).and_then(|n| n.as_group()).map(|g| g.name.clone()).unwrap_or_default();
                self.renaming_group = Some((id.to_string(), name));
                self.pending_focus = Some(FocusTarget::GroupRename);
            }
            NodeAction::CommitRename(name) => {
                self.renaming_group = None;
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    if let Some(group) = self.nodes.get_mut(id).and_then(|n| n.as_group_mut()) {
                        group.name = trimmed.to_string();
                    }
                }
            }
            NodeAction::CancelRename => self.renaming_group = None,
            NodeAction::SetColor(color) => {
                if let Some(node) = self.nodes.get_mut(id) {
                    node.color = color;
                }
            }
            NodeAction::CycleColor => {
                if let Some(node) = self.nodes.get_mut(id) {
                    node.color = next_tag_color(node.color);
                }
            }
            NodeAction::ToggleMute => self.toggle_muted(id),
            NodeAction::RetryMissingFile => self.retry_missing_file(id),
            NodeAction::OpenInDefaultEngine => self.retry_failed_tab(id, self.settings.default_starting_engine),
            NodeAction::RemoveFromGroup => self.eject(id),
            NodeAction::Dissolve => self.dissolve_group(id),
            NodeAction::ExportGroup => {
                if let Some(export) = self.export_group(id) {
                    if let Some((path, toast)) = save_dialog_with_toast(format!("{}.json", export.name), "Group") {
                        group_export::save_to(&export, &path);
                        self.queue_toast(toast.0);
                    }
                }
            }
            NodeAction::Delete => self.close_node_and_save(id),
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
                let response = popup_frame(ui).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
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
        let current = self.focused.as_ref().and_then(|id| tab_ids.iter().position(|t| *t == id));
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
        // The address bar is built only in Tab view, so leaving it by any
        // path ends the edit, as Escape does. Otherwise the dropdown and
        // typed text came back over whichever tab was shown next.
        if !self.showing_tab_view() && self.address_bar.editing() {
            self.address_bar.dismiss();
            self.address_bar.focused = false;
        }
        // Every view, so a toast queued in Graph view (a crash/respawn
        // notice, an unreadable session file) is seen where it happened.
        self.build_save_toast(ui);
        // A `Transition` (either direction) always renders Graph view's
        // node layout as the backdrop — it's the one screen both a
        // shrinking-away Tab view and an expanding-toward Tab view have a
        // coherent shared position in (the target/source node). The actual
        // animated content quad itself is drawn separately, in
        // `render_frame` — see `transition_content_rect_points`.
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

    /// The "Keyboard Shortcuts (F1)" half of `render_universal_chip`.
    /// Returns its own rect, which the shortcuts-help popup anchors to.
    fn build_help_button(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let help_response =
            chip_icon_button(ui, egui::Id::new("icon_btn_shortcuts_help"), egui_phosphor::light::QUESTION)
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
            let (help, settings) = in_reading_order(ui, self, Self::build_help_button, Self::build_settings_button);
            help_rect = help;
            help.union(settings)
        });
        help_rect
    }
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

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        headless,
        // Must be set before `run_app` — `resumed()` calls `spawn_root_tab`,
        // which needs this to hand its background spawn thread a way to report
        // back (see `TabSpawnOutcome`'s doc comment).
        event_proxy: Some(event_loop.create_proxy()),
        ..App::default()
    };
    if headless || debug_commands {
        debug_control::spawn_stdin_reader(event_loop.create_proxy());
    }

    event_loop.run_app(&mut app).expect("run event loop");
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // `Tab` is left out: `TabPayload` owns a live `EngineTab`, which
    // a unit test can't construct.
    #[test]
    fn closed_piece_skips_what_has_nothing_to_restore() {
        assert!(matches!(
            closed_piece(&NodePayload::Note(NotePayload { text: "n".into(), editing: false })),
            Some(ClosedPiece::Note(text)) if text == "n"
        ));
        // A blank note (never typed in, or only whitespace): nothing to restore.
        for text in [
            "", " 
	",
        ] {
            assert!(closed_piece(&NodePayload::Note(NotePayload { text: text.into(), editing: false })).is_none());
        }
        assert!(matches!(
            closed_piece(&NodePayload::Json(JsonPayload { path: "d.json".into(), value: serde_json::Value::Null, key: None, error: None })),
            Some(ClosedPiece::Json(path)) if path == "d.json"
        ));
        assert!(matches!(
            closed_piece(&NodePayload::Json(JsonPayload { path: "d.json".into(), value: 1.into(), key: Some("k".into()), error: None })),
            Some(ClosedPiece::JsonLeaf { key: Some(key), .. }) if key == "k"
        ));
        assert!(closed_piece(&NodePayload::Download(DownloadPayload {
            path: "file.bin".into(),
            bytes_received: 0,
            total_bytes: None,
            completed: true,
        }))
        .is_none());
        // A failed tab's card reopens as the tab it stands in for.
        assert!(matches!(
            closed_piece(&NodePayload::FailedTab(FailedTabPayload {
            url: "https://example.org/".into(),
            kind: EngineKind::Cef,
            error: "no binary".into(),
            pinned: true,
            zoom_factor: 1.5,
            retrying: false,
        })),
            Some(ClosedPiece::Tab { url, kind: EngineKind::Cef, pinned: true, zoom_factor })
                if url == "https://example.org/" && zoom_factor == 1.5
        ));
    }

    /// Closing a group records it and everything under it at any depth as
    /// one entry, root first, skipping blank notes and nothing outside it.
    #[test]
    fn closed_entry_snapshots_the_whole_subtree() {
        let node = |group: Option<&str>, payload| GraphNode {
            parent: None,
            group: group.map(str::to_string),
            pos: egui::Vec2::ZERO,
            vel: egui::Vec2::ZERO,
            color: None,
            payload,
        };
        let note = |text: &str| NodePayload::Note(NotePayload { text: text.into(), editing: false });
        let group = |name: &str| NodePayload::Group(GroupPayload { name: name.into(), from_json: false });
        let leaf = || {
            NodePayload::Json(JsonPayload {
                path: "d.json".into(),
                value: 1.into(),
                key: Some("k".into()),
                error: None,
            })
        };
        let image = NodePayload::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None });
        let mut nodes = NodeMap::default();
        let mut order = Vec::new();
        for (id, n) in [
            ("grp-0", node(None, group("Outer"))),
            ("note-0", node(Some("grp-0"), note("kept"))),
            ("note-1", node(Some("grp-0"), note(" "))),
            ("grp-1", node(Some("grp-0"), group("Inner"))),
            ("img-0", node(Some("grp-1"), image)),
            ("json-0", node(Some("grp-1"), leaf())),
            ("note-2", node(None, note("outside"))),
        ] {
            nodes.insert(id.to_string(), n);
            order.push(id.to_string());
        }
        let entry = closed_entry(&nodes, &order, "grp-0").unwrap();
        let ids: Vec<&str> = entry.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["grp-0", "note-0", "grp-1", "img-0", "json-0"]);
        assert_eq!(entry.nodes[3].group.as_deref(), Some("grp-1"));

        nodes.insert("json-1".into(), node(None, leaf()));
        assert!(closed_entry(&nodes, &order, "json-1").is_none());
        assert!(closed_entry(&nodes, &order, "note-1").is_none());
        assert_eq!(closed_entry(&nodes, &order, "note-2").unwrap().nodes.len(), 1);
    }

    // One row per kind, pinned to the exact menus each kind showed before
    // the per-kind builders were merged (see the node-menu spec), and
    // before the tab and group menus joined them.
    #[test]
    fn node_menu_config_matches_each_kinds_menu() {
        fn menu_of(view: &NodeMenuView, headless: bool) -> (&'static str, f32, String) {
            let config = node_menu_config(view, headless);
            let items: Vec<&str> = config
                .items
                .iter()
                .map(|item| match item {
                    NodeMenuItem::Button(label, _) => *label,
                    NodeMenuItem::Colors => "<colors>",
                    NodeMenuItem::Separator => "---",
                })
                .collect();
            (config.area_id, config.min_width, items.join(" | "))
        }
        fn menu(
            kind: ItemNodeKind,
            has_group: bool,
            pinned: bool,
            has_json: bool,
            headless: bool,
        ) -> (&'static str, f32, String) {
            let view = NodeMenuView {
                kind,
                has_group,
                color: None,
                pinned,
                has_json,
                other_default_engine: false,
                renaming: None,
            };
            menu_of(&view, headless)
        }
        for has_group in [false, true] {
            // Every kind can join a group (drag onto it), so every menu
            // offers "Remove from Group" when grouped.
            let g = if has_group { " | --- | Remove from Group" } else { "" };
            let menu = |kind, pinned, has_json| menu(kind, has_group, pinned, has_json, false);
            assert_eq!(
                menu(ItemNodeKind::Download, false, false),
                (
                    "download_context_menu",
                    170.0,
                    format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Remove from List")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Note, false, false),
                ("note_context_menu", 150.0, format!("Copy Text | <colors>{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Image, false, false),
                (
                    "image_context_menu",
                    170.0,
                    format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Delete")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Pdf, false, false),
                ("pdf_context_menu", 170.0, format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Json, false, false),
                (
                    "json_context_menu",
                    170.0,
                    format!("Open Containing Folder | Copy Path | Expand as Graph | <colors>{g} | --- | Delete")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Tab, false, false),
                (
                    "tab_context_menu",
                    150.0,
                    format!("Pin | Duplicate | Copy URL | Fit Subtree | <colors>{g} | --- | Close")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::Tab, true, true).2,
                format!("Unpin | Duplicate | Copy URL | View as JSON | Fit Subtree | <colors>{g} | --- | Close")
            );
            assert_eq!(
                menu(ItemNodeKind::Group, false, false),
                (
                    "group_context_menu",
                    150.0,
                    format!("Rename | <colors>{g} | --- | Dissolve | Close Group | Export Group...")
                )
            );
            assert_eq!(
                menu(ItemNodeKind::FailedTab, false, false),
                ("failed_tab_context_menu", 170.0, format!("Retry | Copy URL | <colors>{g} | --- | Close"))
            );
        }
        // A failed tab offers the default engine only when it isn't the
        // engine that failed.
        let failed = NodeMenuView {
            kind: ItemNodeKind::FailedTab,
            has_group: false,
            color: None,
            pinned: false,
            has_json: false,
            other_default_engine: true,
            renaming: None,
        };
        assert_eq!(menu_of(&failed, false).2, "Retry | Open in Default Engine | Copy URL | <colors> | --- | Close");
        // Export Group's file dialog can't run headless.
        assert_eq!(
            menu(ItemNodeKind::Group, false, false, false, true).2,
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
        let group = |name: &str| NodePayload::Group(GroupPayload { name: name.into(), from_json: false });
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
    fn node_ids_without_a_store_are_unique_store_ids() {
        let mut app = App::default();
        let a = app.alloc_node_id();
        let b = app.alloc_node_id();
        assert_ne!(a, b);
        let a: crate::store::model::NodeId = a.parse().expect("a store node id");
        let b: crate::store::model::NodeId = b.parse().unwrap();
        assert_eq!((a.actor, a.counter + 1), (b.actor, b.counter));
    }

    #[test]
    fn a_new_group_takes_the_smallest_free_number() {
        let mut app = App::default();
        let add = |app: &mut App, name: &str| {
            let payload = NodePayload::Group(GroupPayload { name: name.into(), from_json: false });
            let node = GraphNode {
                parent: None,
                group: None,
                pos: egui::Vec2::ZERO,
                vel: egui::Vec2::ZERO,
                color: None,
                payload,
            };
            let id = app.alloc_node_id();
            app.nodes.insert(id, node);
        };
        assert_eq!(app.default_group_name(), "Group 1");
        add(&mut app, "Group 1");
        add(&mut app, "Group 3");
        assert_eq!(app.default_group_name(), "Group 2", "a dissolved group's number is reused, never duplicated");
        add(&mut app, "Group 2");
        assert_eq!(app.default_group_name(), "Group 4");
    }
}
