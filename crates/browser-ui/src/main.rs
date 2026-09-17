mod bookmarks;
mod debug_control;
mod gpu;
mod graph_layout;
mod group_export;
mod pdf;
mod process_manager;
mod session;
mod settings;
mod suggestions;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
use winit::window::{Window, WindowId};

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
/// dragged tab is tested against on release — see `GroupNode`'s own doc
/// comment.
const GROUP_NODE_SIZE: egui::Vec2 = egui::vec2(150.0, 90.0);

/// Fixed tag-color choices a group or tab card's color swatch cycles
/// through (see `next_tag_color`) -- red/orange/yellow/green/cyan/purple,
/// chosen for mutual distinguishability against this app's dark chrome
/// rather than any particular meaning. Shared by `GroupNode::color` and
/// `TabNode::color`.
const TAG_COLOR_PALETTE: [egui::Color32; 6] = [
    egui::Color32::from_rgb(235, 87, 87),
    egui::Color32::from_rgb(242, 153, 74),
    egui::Color32::from_rgb(242, 201, 76),
    egui::Color32::from_rgb(111, 207, 151),
    egui::Color32::from_rgb(86, 204, 242),
    egui::Color32::from_rgb(187, 107, 217),
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

/// The `Area`/`Frame` shell every toolbar list-dropdown (Recently Closed,
/// Bookmarks, History, Downloads) wraps its own contents in -- pinned below
/// `anchor_rect`, same fill/stroke/corner-radius/margin, same min/max width.
/// A free function (not a method) so callers can still freely capture
/// `&mut self` in `add_contents` -- an `&mut self` method here would
/// conflict with that same borrow. Returns the drawn area's own rect, for
/// the caller's own `clicked_outside_popup` check. `fade` (0.0-1.0, from the
/// caller's own `ctx.animate_bool` on its own open flag) sets the whole
/// popup's opacity, so it fades in/out instead of popping instantly.
fn build_list_popup_frame(
    ctx: &egui::Context,
    id: &str,
    anchor_rect: egui::Rect,
    min_width: f32,
    max_width: f32,
    fade: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    egui::Area::new(egui::Id::new(id))
        .fixed_pos(anchor_rect.left_bottom())
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            egui::Frame::default()
                .fill(TOOLBAR_FILL)
                .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                .corner_radius(4u8)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_min_width(min_width);
                    ui.set_max_width(max_width);
                    add_contents(ui);
                });
        })
        .response
        .rect
}

/// The bare filename portion of a download's full path, for display --
/// shared by the graph card's own label and the "Downloads (N)" toolbar
/// dropdown so the two never show it differently. Falls back to the whole
/// path if it has no separable filename component (shouldn't normally
/// happen for a real downloaded file's path).
fn download_filename(path: &str) -> &str {
    std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path)
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
const TOOLBAR_FILL: egui::Color32 = egui::Color32::from_rgb(30, 31, 36);

fn default_graph_scene_rect() -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(0.0, 0.0), GRAPH_SCENE_DEFAULT_SIZE)
}

/// `%USERPROFILE%\Pictures\multiengine-browser-<unix-seconds>.png` — same
/// "pick the obvious default, no save-as dialog" posture `downloads_dir()`
/// (in each engine-host's own download-handling code) already takes, just
/// on the browser-ui side for the toolbar's "Screenshot" button. The
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
    dir.join(format!("multiengine-browser-{timestamp}.png"))
}

/// A single accent color used throughout — selection highlights, hyperlinks,
/// the focused-tab-strip fill, the focused graph node's border — so the
/// whole chrome reads as one coherent theme rather than egui's flat default
/// dark gray-on-gray.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(88, 166, 255);

/// A group card's border tints this color once its member count exceeds
/// `GROUP_OVERLOAD_THRESHOLD` -- a cheap glanceable "this one's gotten big"
/// cue, the same severity-red used nowhere else in this app's chrome so it
/// reads unambiguously as a warning rather than just another tag color.
/// Only applies while the group has no explicit tag color of its own (see
/// its use below) -- a deliberate user choice always wins over this
/// automatic one, same precedent tag colors already have over the plain
/// default `ACCENT` border.
const GROUP_OVERLOAD_COLOR: egui::Color32 = egui::Color32::from_rgb(220, 60, 60);
const GROUP_OVERLOAD_THRESHOLD: usize = 8;

/// The window/content-area base — behind every panel, the darkest layer.
/// Distinct from `TOOLBAR_FILL` on purpose: egui's own default has exactly
/// one dark gray for everything (panels, buttons, the app background all
/// read as the same flat plane), which is the single biggest reason this
/// app used to look like an unstyled egui demo rather than real chrome.
/// This + `TOOLBAR_FILL` + `SURFACE_RAISED` form a three-step depth ladder
/// (background < panel < raised widget) that egui's stock palette doesn't
/// have at all.
const BG: egui::Color32 = egui::Color32::from_rgb(16, 17, 20);
/// A button/field's own resting fill, one step up from `TOOLBAR_FILL` — the
/// panel it sits on. Also used for the graph card's engine-picker `Frame`.
/// Deliberately a wide jump from `TOOLBAR_FILL`, not a subtle one — the
/// point is a button reads as its own raised chip at a glance, not just
/// under close pixel inspection.
const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(54, 58, 68);
/// Hover state, one step up again — every interactive widget in this app's
/// toolbar/popups visibly lightens on hover, which stock egui's near-
/// identical inactive/hovered grays don't make obvious at a glance.
const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(72, 77, 90);
const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(230, 231, 235);
const TEXT_WEAK: egui::Color32 = egui::Color32::from_rgb(148, 151, 161);

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
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);

        let corner_radius: egui::CornerRadius = 3u8.into();
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

        style.visuals.window_corner_radius = 4u8.into();
        style.visuals.menu_corner_radius = 4u8.into();
        style.visuals.window_fill = TOOLBAR_FILL;
        style.visuals.panel_fill = BG;
        style.visuals.extreme_bg_color = BG;
        style.visuals.faint_bg_color = SURFACE_RAISED;
        style.visuals.window_stroke = egui::Stroke::new(1.0, SURFACE_RAISED);

        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke.color = egui::Color32::WHITE;
        style.visuals.hyperlink_color = ACCENT;
        style.visuals.override_text_color = None;
    });
}

/// Replaces egui's bundled default (`Ubuntu-Light`) with IBM Plex Serif,
/// inserted ahead of it in the proportional family rather than instead of
/// it — a glyph outside IBM Plex Serif's own coverage still falls back to
/// the bundled default (and from there to egui's bundled emoji/symbol
/// fonts) instead of tofu-boxing. Picked over the OS's own Segoe UI for
/// one concrete reason: `egui`/`ab_glyph` needs the actual font bytes at
/// build time, and there's no equivalent of a browser's "ask the OS for
/// this family" for a native GPU-rendered glyph atlas — bundling a real
/// file was required either way, so the choice came down to which file,
/// not whether to bundle one. A real static `Regular` weight (unlike
/// Nunito Sans, tried first, which only ships as a variable font) — the
/// serif reading was picked as a deliberate "tinge of something else"
/// out of the same side-by-side comparison Nunito Sans came from, not
/// treated as final.
fn apply_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "IBMPlexSerif".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/IBMPlexSerif-Regular.ttf"
        ))),
    );
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "IBMPlexSerif".to_owned());
    }
    // A real icon font, not a symbol glyph gamble -- IBM Plex Serif's own
    // doc comment above notes a theoretical fallback chain to egui's
    // bundled emoji/symbol font, but this project has repeatedly confirmed
    // live that several such glyphs still render as empty tofu boxes
    // regardless (see the pin/mute/audio-indicator dots, all painter-drawn
    // shapes specifically to dodge this). Phosphor's own codepoints live
    // in a private-use range no other font in this chain defines, so
    // there's no coverage ambiguity: every `egui_phosphor::regular::*`
    // constant used in this app's chrome is guaranteed to resolve here.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}

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
fn truncate_to_width(ctx: &egui::Context, text: &str, font_id: &egui::FontId, max_width: f32) -> String {
    let width_of = |s: &str| -> f32 {
        ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font_id.clone(), egui::Color32::WHITE))
            .size()
            .x
    };
    if max_width <= 0.0 {
        return String::new();
    }
    if width_of(text) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if width_of(&candidate) <= max_width {
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
    for (alpha, grow) in [(85u8, 3.5_f32), (38u8, 8.5)] {
        let alpha = ((alpha as f32) * intensity).min(255.0) as u8;
        painter.rect_filled(
            rect.expand(grow * inv_zoom),
            (base_corner_radius + grow) * inv_zoom,
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha),
        );
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
    Tab,
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
    /// Tab view's full-screen content quad shrinks down into `tab_id`'s
    /// graph-node thumbnail spot.
    ToGraph,
    /// `tab_id`'s graph-node thumbnail spot grows to fill the screen as
    /// Tab view's own content quad.
    ToTab,
}

/// An in-progress shrink/expand animation between Tab view and Graph view
/// for one specific tab. While this is `Some`, `build_ui` renders Graph
/// view's node layout as the backdrop (for *either* direction — it's the
/// one screen both endpoints have a coherent position in) with `tab_id`'s
/// live content quad drawn on top at an interpolated rect, rather than
/// switching `view_mode` instantly. See `App::content_rect_for_frame`.
struct Transition {
    tab_id: String,
    kind: TransitionKind,
    start: std::time::Instant,
}

const TRANSITION_DURATION: std::time::Duration = std::time::Duration::from_millis(220);

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

/// How long a "Saved <path>" toast stays on screen — see `App::save_toast`'s
/// doc comment.
const SAVE_TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(4);

/// One tab: its engine-host process/IPC handle, the "opened from" edge
/// (`parent`) that makes the tab collection a forest for the M5 graph view,
/// per-tab navigation/UI state, and force-directed layout state (`pos`/
/// `vel`) — see `graph_layout`.
struct TabNode {
    tab: EngineTab,
    parent: Option<String>,
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
    /// Graph view's pin toggle — a pinned node is excluded from the
    /// force-directed layout entirely (same `pinned` exclusion
    /// `graph_layout::step` already gives a node currently being dragged),
    /// so it stays exactly where the user put it instead of drifting under
    /// repulsion/centering forces as siblings come and go. Not persisted to
    /// `session.json` (unlike `pos`, which is) — resets to unpinned on
    /// every restart, a smaller scope than plumbing a new field through
    /// `SessionTab` warranted for a graph-view display preference.
    pinned: bool,
    /// Which group's subgraph this tab is shown in — `None` at the top
    /// level. See `GroupNode`'s own doc comment. Unlike `parent` (who
    /// opened this tab, used for the edge drawn to it), `group` is purely
    /// "which graph view is this node rendered in" — a tab's parent can
    /// point to a tab in a *different* group (or none at all) without
    /// anything breaking; the edge just isn't drawn unless both endpoints
    /// share the same `group` (see `build_graph_ui`'s level-filtering).
    /// Session-only, same scope decision as `pinned`.
    group: Option<String>,
    /// Set while a swap is in flight for *this* tab: the engine to swap
    /// *to*, once this tab's `GetState` reply (its `StateSnapshot`) comes
    /// back. Per-tab (not a flat `App` field) so a swap on one tab can't
    /// block or interfere with a concurrent swap/crash on another.
    swap_target: Option<EngineKind>,
    pos: egui::Vec2,
    vel: egui::Vec2,
    /// User-chosen tag color, cycled through `TAG_COLOR_PALETTE` by
    /// clicking the card's own color swatch (bottom-right corner -- the one
    /// corner of a tab card not already claimed by the pin dot, close "x",
    /// or (inside a group) the eject button). `None` keeps the card's plain
    /// default border. Session-only, same scope decision as `pinned`.
    color: Option<egui::Color32>,
}

/// A completed download shown as its own graph node, edged to the tab it
/// was downloaded from — same force-directed layout/edge machinery as
/// `TabNode` (see `graph_layout::step`), but no engine process/live content
/// behind it, just a static file reference. `parent` follows the same
/// reparent-on-close rule `close_tab` already applies to tabs (falls back
/// to the closed tab's own parent, or `None` if it was a root). Session-
/// only — not written to `session.json`: the downloaded file already
/// exists on disk regardless, so losing this node on restart costs nothing
/// but a "recent downloads" convenience.
struct DownloadNode {
    parent: Option<String>,
    /// Inherited from the parent tab's own `group` at creation time — see
    /// `TabNode::group`'s doc comment. Fixed at creation rather than
    /// tracking the parent tab live: a download node has no reason to
    /// follow its tab into a *different* group after the fact (nothing
    /// currently moves a tab between groups after creation either).
    group: Option<String>,
    path: String,
    pos: egui::Vec2,
    vel: egui::Vec2,
}

/// A tab group: a container node shown at the top level of Graph view that,
/// double-clicked, drills into its own subgraph of member tabs/downloads
/// (`App::current_group`) in place of the top level's — the same "swap
/// which set of nodes this view renders" idea `ViewMode::Tab`/`Graph`
/// already is, just one level deeper, and reusing that same flat
/// `graph_layout::step` simulation rather than a second nested one running
/// inside the group's own node. No nesting in this first cut — a group
/// can't contain another group, only tabs/downloads (`TabNode::group`/
/// `DownloadNode::group` are a single `Option<String>`, not a path).
/// Entering/leaving eases the camera via `GraphCameraTween` rather than
/// `Transition` — see that type's own doc comment for why. Groups (and
/// membership) are session-only, same scope decision as `TabNode::pinned`
/// — not written to `session.json`.
struct GroupNode {
    name: String,
    pos: egui::Vec2,
    vel: egui::Vec2,
    /// User-chosen tag color, cycled through `TAG_COLOR_PALETTE` by
    /// clicking the card's own color swatch (bottom-left corner, same spot
    /// a tab's pin dot occupies) -- `None` keeps the plain default `ACCENT`
    /// border every group card had before this existed.
    color: Option<egui::Color32>,
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
    /// tab stays pinned — see `TabNode::pinned`'s own doc comment. Always
    /// `false` for a genuinely new tab (`SpawnPurpose::New`/`Root`).
    pinned: bool,
    /// Same "carried through respawn, `None` for a genuinely new tab"
    /// treatment as `pinned` above, but for `TabNode::color`.
    color: Option<egui::Color32>,
    url: String,
    pos: egui::Vec2,
    vel: egui::Vec2,
    purpose: SpawnPurpose,
    result: std::io::Result<EngineTab>,
}

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    tabs: HashMap<String, TabNode>,
    /// Stable iteration/z-order for graph rendering and hit-testing — a
    /// `HashMap`'s own iteration order is unspecified and can change
    /// frame-to-frame, which would make node click/drag targeting flaky.
    tab_order: Vec<String>,
    /// Completed downloads shown as graph nodes — see `DownloadNode`'s own
    /// doc comment.
    downloads: HashMap<String, DownloadNode>,
    download_order: Vec<String>,
    /// Tab groups shown as top-level graph nodes — see `GroupNode`'s own
    /// doc comment.
    groups: HashMap<String, GroupNode>,
    group_order: Vec<String>,
    /// Toggled by the Graph toolbar's "Collapse Groups"/"Expand Groups"
    /// button -- while `true`, every non-empty group card falls back to
    /// the plain centered name+count layout an empty group already uses,
    /// skipping the 2x2 member-thumbnail grid. A denser overview for a
    /// graph with many/large groups, purely a rendering toggle -- doesn't
    /// touch `groups`/membership at all. Session-only, resets on restart.
    groups_collapsed: bool,
    /// `None` while viewing the top level; `Some(group_id)` while a
    /// group's own subgraph has been drilled into (see `GroupNode`'s doc
    /// comment) — only meaningful when `view_mode == Graph`, but not reset
    /// on leaving Graph view, so returning to Graph resumes wherever the
    /// user last drilled into, same "graph view remembers its own state"
    /// treatment `graph_scene_rect`'s pan/zoom already gets.
    current_group: Option<String>,
    /// `(group_id, draft_name)` once renaming has started -- either the
    /// toolbar's group-name button (only shown while `current_group` is
    /// `Some`) or the top-level right-click context menu's own "Rename"
    /// (see `group_context_menu`) sets this the same way. A real
    /// `egui::TextEdit` replaces whichever plain button/menu row started
    /// it until Enter commits or Escape cancels. Transient UI state, not
    /// part of `GroupNode` itself.
    renaming_group: Option<(String, String)>,
    /// Set for exactly one frame when `renaming_group` is first set, same
    /// one-shot `response.request_focus()` pattern as
    /// `pending_focus_address_bar`.
    pending_focus_rename: bool,
    /// `(group_id, screen_pos)` while a top-level right-click context menu
    /// is showing for that group's card -- Rename/color swatches/Dissolve
    /// in one place, see `App::build_group_context_menu`. Right-click is
    /// otherwise the rectangle-select gesture over the graph panel (see
    /// that gesture's own doc comment in `build_graph_ui`); a press landing
    /// on a group's card specifically opens this instead of starting a
    /// marquee.
    group_context_menu: Option<(String, egui::Pos2)>,
    /// `(download_id, screen_pos)` while a right-click context menu is
    /// showing for that download's card -- Open Containing Folder/Copy
    /// Path/Remove from List, same "consolidate scattered card actions
    /// into one menu" idea `group_context_menu` already applies to groups.
    /// A press landing on a download's card opens this instead of starting
    /// a marquee, same reasoning as `group_context_menu`'s own doc comment.
    download_context_menu: Option<(String, egui::Pos2)>,
    /// `(tab_id, screen_pos)` while a right-click context menu is showing
    /// for that tab's card -- Pin/color/Remove from Group/Close in one
    /// place, same "consolidate scattered card actions" idea
    /// `group_context_menu`/`download_context_menu` already apply. A press
    /// landing on a tab's card opens this instead of starting a marquee,
    /// same reasoning as those two.
    tab_context_menu: Option<(String, egui::Pos2)>,
    /// `Ctrl+K` quick-switcher, if currently open — see `QuickSwitcher`'s
    /// own doc comment and `App::build_quick_switcher`.
    quick_switcher: Option<QuickSwitcher>,
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
    view_mode: ViewMode,
    next_tab_id: u64,
    next_download_id: u64,
    next_group_id: u64,
    cursor_pos: (f64, f64),
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
    /// Index into `combined_suggestions()` the user has moved to via
    /// Up/Down — `None` means no row is highlighted (Enter falls through
    /// to a plain navigate of the typed text). Reset whenever the
    /// suggestion list itself changes or the dropdown closes, so a stale
    /// index can never point at an unrelated row — see `build_tab_ui`'s
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
    /// The graph view's current pan/zoom state (visible world-space rect),
    /// persisted across frames per `egui::Scene::show`'s own contract —
    /// mutated by the user dragging (pan) or scrolling (zoom).
    graph_scene_rect: egui::Rect,
    /// `Some` while entering/leaving a group is easing the camera toward
    /// the new level's framing instead of having already snapped there --
    /// see `GraphCameraTween`'s own doc comment.
    graph_camera_tween: Option<GraphCameraTween>,
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
    /// Set by a headless `ESCAPE`/`ENTER`/`ARROWDOWN`/`ARROWUP` command,
    /// consumed (and reset) the next time `build_tab_ui` checks the
    /// corresponding real-keypress condition — see the comments at each
    /// check site.
    pending_debug_escape: bool,
    pending_debug_enter: bool,
    pending_debug_arrow_down: bool,
    pending_debug_arrow_up: bool,
    /// Set by `Ctrl+L`/headless `FOCUSADDRESS` (see
    /// `App::focus_address_bar_shortcut`), consumed the next time
    /// `build_tab_ui` builds the address bar's `TextEdit` — unlike the
    /// `pending_debug_*` flags above, this drives a *real* `request_focus()`
    /// call on the real widget (this is the one shortcut where a real user
    /// genuinely wants real, persistent egui focus, not just the app-state
    /// approximation `ADDRESSBAR`'s `debug_address_bar_pin` settles for),
    /// so it needs a frame to actually exist as a widget to focus.
    pending_focus_address_bar: bool,
    /// Set by a headless `QUIT` that arrived while a `pending_debug_*` flag
    /// above (or `pending_screenshot`) was still set — e.g. `ENTER`+`QUIT`,
    /// or `SCREENSHOT`+`QUIT`, sent back-to-back with no delay. Exiting
    /// immediately in that case could drop the pending command/screenshot
    /// before any frame ever consumed it (`event_loop.exit()` gives no
    /// guarantee a `RedrawRequested` runs first). Deferring one frame lets
    /// `build_tab_ui`/etc. consume the pending flag, or lets `RedrawRequested`
    /// actually capture and save the pending screenshot, normally;
    /// `RedrawRequested`'s handler checks this and exits right after.
    quit_after_next_redraw: bool,
    /// Set by a headless `ADDRESSBAR` command, cleared by `ENTER`/`ESCAPE`/
    /// accepting a suggestion. A real click gives the `TextEdit` real,
    /// *persistent* egui focus, so `response.has_focus()` keeps reporting
    /// `true` frame after frame on its own — headless mode has no way to
    /// synthesize that (see `debug_control`'s doc comment on why keyboard/
    /// text commands are direct calls, not synthetic events), so
    /// `ADDRESSBAR` sets `address_bar_focused` for only the one frame it's
    /// processed on; the very next frame, the real (never actually
    /// clicked) widget's honest `response.has_focus() == false` overwrites
    /// it right back. Without this pin, `editing_address` (see
    /// `build_tab_ui`) goes false again within ~1 frame for any URL-like
    /// input (nothing populates suggestions to keep it true the way a
    /// search query's does), and the very next frame's resync-to-tab-URL
    /// clobbers whatever `ADDRESSBAR` just set — confirmed live: a
    /// `SCREENSHOT` taken 1 second after `ADDRESSBAR https://github.com`
    /// showed the address bar already back to the tab's real URL, for
    /// every engine tried (not engine-specific). This pin makes
    /// `address_bar_focused` stay true across frames the same way real
    /// focus would, until something that should actually end editing
    /// clears it.
    debug_address_bar_pin: bool,
    /// Set by a headless `SCREENSHOT` command; consumed on the next
    /// `RedrawRequested` and passed into `gpu::GpuState::render`.
    pending_screenshot: Option<String>,
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

    /// The minimal right-click context menu, if currently showing — see
    /// `ContextMenuState`'s own doc comment.
    context_menu: Option<ContextMenuState>,

    /// A brief "Saved <path>" toast for the most recent completed download
    /// or screenshot save, and when it was set — cleared once
    /// `SAVE_TOAST_DURATION` has passed. No queue: a second save finishing
    /// while one toast is showing just replaces it, same "latest wins"
    /// simplicity as `find_result`.
    save_toast: Option<(String, std::time::Instant)>,

    /// When the auto-restore `session.json` was last written -- shown in
    /// the settings popup's Sessions section next to a "Save Now" button,
    /// so a save that's actually continuous (every mutation calls
    /// `save_session`, not a timer) has some visible confirmation instead
    /// of being purely invisible. `None` until the first save this run.
    last_session_save: Option<std::time::Instant>,

    find_bar_open: bool,
    find_text: String,
    /// Resets to `false` on every `close_find_bar` — a fresh find session
    /// (new tab, or reopening after closing) starts case-insensitive by
    /// default, matching a real browser's find bar.
    find_match_case: bool,
    find_result: Option<(u32, u32)>,
    /// Set by `Ctrl+F`/headless `OPENFINDBAR`, consumed the next time
    /// `build_tab_ui` builds the find bar's `TextEdit` — same "needs a
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
    /// `%APPDATA%\multiengine-browser\settings.json` on every change — see
    /// `settings.rs`.
    settings: Settings,
    /// Which view `ViewMode::Settings` should return to (its own Back
    /// button, `Escape`, or `Ctrl+G`) -- set once when the settings page is
    /// entered, from whichever of Tab/Graph was showing at that moment.
    settings_return_view: ViewMode,
    /// Flat `(title, url)` list, loaded once at startup and persisted to
    /// `%APPDATA%\multiengine-browser\bookmarks.json` on every change — see
    /// `bookmarks.rs`. `bookmarks_open` is the Tab-view toolbar dropdown's
    /// own visibility, see `App::build_bookmarks_popup`.
    bookmarks: Vec<bookmarks::Bookmark>,
    bookmarks_open: bool,
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
    /// the tab the user is looking at. `build_tab_ui`'s existing "focused
    /// tab missing from `self.tabs`" guard (a pre-existing one-frame safety
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
        let settings = Settings::load();
        let bookmarks = bookmarks::load();
        Self {
            window: None,
            gpu: None,
            tabs: HashMap::new(),
            tab_order: Vec::new(),
            downloads: HashMap::new(),
            download_order: Vec::new(),
            groups: HashMap::new(),
            group_order: Vec::new(),
            groups_collapsed: false,
            current_group: None,
            renaming_group: None,
            pending_focus_rename: false,
            group_context_menu: None,
            download_context_menu: None,
            tab_context_menu: None,
            quick_switcher: None,
            shortcuts_help_open: false,
            pending_import: None,
            session_name_draft: String::new(),
            graph_filter: String::new(),
            graph_zoom_pct: 100.0,
            pending_focus_quick_switcher: false,
            focused: None,
            view_mode: ViewMode::Tab,
            next_tab_id: 0,
            next_download_id: 0,
            next_group_id: 0,
            cursor_pos: (0.0, 0.0),
            key_modifiers: 0,
            egui_ctx,
            egui_state: None,
            address_bar_text: settings.home_page.clone(),
            address_bar_focused: false,
            open_tab_suggestions: Vec::new(),
            search_suggestions: Vec::new(),
            suggestion_fetcher: SuggestionFetcher::new(),
            dropdown_highlighted: None,
            dragging: None,
            hovered: None,
            peek_suppressed: None,
            rect_select_start: None,
            selected_tabs: HashSet::new(),
            graph_scene_rect: default_graph_scene_rect(),
            graph_camera_tween: None,
            graph_settled_key: None,
            headless: false,
            pending_debug_escape: false,
            pending_debug_enter: false,
            pending_debug_arrow_down: false,
            pending_debug_arrow_up: false,
            pending_focus_address_bar: false,
            quit_after_next_redraw: false,
            debug_address_bar_pin: false,
            pending_screenshot: None,
            pending_print_to_pdf: None,
            closed_tabs: Vec::new(),
            context_menu: None,
            save_toast: None,
            last_session_save: None,
            find_bar_open: false,
            find_text: String::new(),
            find_match_case: false,
            find_result: None,
            pending_focus_find_bar: false,
            tab_spawn_error: None,
            settings,
            settings_return_view: ViewMode::Tab,
            bookmarks,
            bookmarks_open: false,
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
        }
    }
}

impl App {
    fn chrome_height_points(&self) -> f32 {
        match self.view_mode {
            ViewMode::Tab => CHROME_HEIGHT_POINTS,
            ViewMode::Graph | ViewMode::Settings => GRAPH_TOOLBAR_HEIGHT_POINTS,
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
    /// it, visibly squash the page.
    fn content_size_px(&self) -> (u32, u32) {
        let full = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        (full.width, full.height.saturating_sub(self.chrome_height_px()))
    }

    fn send_resize_to(&mut self, tab_id: &str) {
        let (width, height) = self.content_size_px();
        let dpi_scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        if let Some(node) = self.tabs.get_mut(tab_id) {
            node.tab.send(&HostToEngine::Resize {
                width,
                height,
                dpi_scale,
            });
        }
    }

    /// Applies `self.settings.zoom_factor` to one tab — called right after
    /// spawning it (new tabs pick up whatever the default is *at spawn
    /// time*; this never retroactively revisits already-open tabs, matching
    /// every other "default" setting here) and again by the Settings
    /// page's zoom control for the currently focused tab specifically
    /// (`build_settings_page`), since seeing the zoom control move with no
    /// visible effect on the tab you're looking at would be confusing.
    fn send_zoom_to(&mut self, tab_id: &str) {
        let factor = self.settings.zoom_factor;
        if let Some(node) = self.tabs.get_mut(tab_id) {
            node.tab.send(&HostToEngine::SetZoom { factor });
        }
    }

    /// Persists the current zoom setting and immediately applies it to
    /// whichever tab is focused right now — otherwise moving the zoom
    /// control would have no visible effect until the *next* tab spawn,
    /// which reads as broken rather than "this is a default for new tabs."
    fn apply_zoom_change(&mut self) {
        self.settings.save();
        if let Some(id) = self.focused.clone() {
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

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_settings_page();
        }

        egui::Panel::top("settings_toolbar")
            .exact_size(CHROME_HEIGHT_POINTS)
            .frame(
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if ui
                        .button(format!("{} Back", egui_phosphor::regular::CARET_LEFT))
                        .clicked()
                    {
                        self.close_settings_page();
                    }
                    ui.label(egui::RichText::new("Settings").strong().color(TEXT_PRIMARY));
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
                                self.settings.save();
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
                egui::ComboBox::from_id_salt("settings_starting_engine")
                    .selected_text(engine_label(self.settings.default_starting_engine))
                    .show_ui(ui, |ui| {
                        for kind in [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit, EngineKind::Webview2] {
                            let selected = self.settings.default_starting_engine == kind;
                            if ui.selectable_label(selected, engine_label(kind)).clicked() && !selected {
                                self.settings.default_starting_engine = kind;
                                self.settings.save();
                            }
                        }
                    });

                ui.add_space(8.0);
                ui.label("Zoom (new tabs; also applies to the current tab now)");
                ui.horizontal(|ui| {
                    if ui.button(egui_phosphor::regular::MINUS).clicked() {
                        self.settings.zoom_factor = (self.settings.zoom_factor - 0.1).max(0.5);
                        self.apply_zoom_change();
                    }
                    ui.label(format!("{:.0}%", self.settings.zoom_factor * 100.0));
                    if ui.button(egui_phosphor::regular::PLUS).clicked() {
                        self.settings.zoom_factor = (self.settings.zoom_factor + 0.1).min(2.0);
                        self.apply_zoom_change();
                    }
                    if ui.button("Reset").clicked() {
                        self.settings.zoom_factor = 1.0;
                        self.apply_zoom_change();
                    }
                });

                ui.add_space(8.0);
                ui.label("Home page (new tabs)");
                if ui
                    .add(egui::TextEdit::singleline(&mut self.settings.home_page).desired_width(f32::INFINITY))
                    .lost_focus()
                {
                    self.settings.save();
                }

                ui.add_space(10.0);
                ui.separator();
                ui.label(egui::RichText::new("Sessions").strong().color(TEXT_PRIMARY));
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
                            .small_button(egui_phosphor::regular::X)
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
                ui.label(egui::RichText::new("Bookmarks").strong().color(TEXT_PRIMARY));
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
                self.save_toast = Some((format!("session '{name}'"), std::time::Instant::now()));
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
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name("session.json")
                .add_filter("Session", &["json"])
                .save_file()
            {
                self.build_session_snapshot().save_to(&path);
                self.save_toast = Some((path.display().to_string(), std::time::Instant::now()));
            }
        }
        if import_requested {
            if let Some(path) =
                rfd::FileDialog::new().add_filter("Session", &["json"]).pick_file()
            {
                if let Some(session) = session::Session::load_from(&path) {
                    self.pending_import = Some(session);
                    self.close_settings_page();
                }
            }
        }
        if export_bookmarks_requested {
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name("bookmarks.json")
                .add_filter("Bookmarks", &["json"])
                .save_file()
            {
                bookmarks::save_to(&self.bookmarks, &path);
                self.save_toast = Some((path.display().to_string(), std::time::Instant::now()));
            }
        }
        if import_bookmarks_requested {
            if let Some(path) =
                rfd::FileDialog::new().add_filter("Bookmarks", &["json"]).pick_file()
            {
                if let Some(imported) = bookmarks::load_from(&path) {
                    self.bookmarks = imported;
                    bookmarks::save(&self.bookmarks);
                }
                self.close_settings_page();
            }
        }
    }

    /// The "Recently Closed" toolbar dropdown (Graph view, top level or
    /// not -- unlike groups, this isn't level-scoped): every entry in
    /// `self.closed_tabs`, most recently closed first, click one to respawn
    /// exactly that tab and remove it from the list. `Ctrl+Shift+T`/
    /// `REOPENTAB` only ever reaches the *last* entry; this is the way to
    /// reopen an older one without reopening everything after it first.
    fn build_recently_closed_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut reopen_index: Option<usize> = None;
        let area_rect =
            build_list_popup_frame(ui.ctx(), "recently_closed_popup", anchor_rect, 220.0, 320.0, fade, |ui| {
            for (index, closed) in self.closed_tabs.iter().enumerate().rev() {
                ui.horizontal(|ui| {
                    engine_dot(ui, closed.kind, 8.0);
                    if ui.add(egui::Label::new(&closed.url).truncate().sense(egui::Sense::click())).clicked() {
                        reopen_index = Some(index);
                    }
                });
            }
        });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some(index) = reopen_index {
            let closed = self.closed_tabs.remove(index);
            self.respawn_closed_tab(closed);
            close = true;
        }
        if clicked_outside {
            close = true;
        }
        if close || self.closed_tabs.is_empty() {
            self.recently_closed_open = false;
        }
    }

    fn navigate_tab(&mut self, tab_id: &str, url: String) {
        if let Some(node) = self.tabs.get_mut(tab_id) {
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
        if let Some(node) = self.tabs.get_mut(tab_id) {
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
        self.tab_order
            .iter()
            .filter(|id| id.as_str() != exclude)
            .filter_map(|id| self.tabs.get(id).map(|node| (id, node)))
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
        self.debug_address_bar_pin = false;
        self.dropdown_highlighted = None;
    }

    /// Snapshots the current tab graph (shape, url/engine/position per tab,
    /// focused id, view mode) and writes it to disk — called at every point
    /// that shape actually changes (a tab spawns, closes, navigates, or
    /// focus/view changes), not once per frame; see `session::Session`'s
    /// own doc comment for what is/isn't portable across a restart.
    /// Fire-and-forget, same as `Settings::save` — a failed write is
    /// logged, never fatal.
    fn save_session(&mut self) {
        self.build_session_snapshot().save();
        self.last_session_save = Some(std::time::Instant::now());
    }

    /// The `session::Session` value both the auto-restore `save_session`
    /// and "Save Session As..." serialize -- factored out so the two never
    /// drift into building this differently.
    fn build_session_snapshot(&self) -> session::Session {
        let tabs = self
            .tab_order
            .iter()
            .filter_map(|id| {
                let node = self.tabs.get(id)?;
                Some(session::SessionTab {
                    id: id.clone(),
                    parent: node.parent.clone(),
                    url: node.url.clone(),
                    engine: node.tab.kind,
                    pos_x: node.pos.x,
                    pos_y: node.pos.y,
                })
            })
            .collect();
        session::Session {
            tabs,
            focused: self.focused.clone(),
            view_mode_tab: matches!(self.view_mode, ViewMode::Tab),
            next_tab_id: self.next_tab_id,
        }
    }

    /// Startup: replays a saved `session::Session` instead of spawning one
    /// fresh root tab — returns `false` (having done nothing) if there's no
    /// session file, it's corrupt, or it has zero tabs, so the caller can
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
        let Some(session) = session::Session::load() else {
            return false;
        };
        if session.tabs.is_empty() {
            return false;
        }
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
        let valid_focus = session
            .focused
            .as_ref()
            .filter(|id| session.tabs.iter().any(|t| &t.id == *id));
        self.focused = valid_focus
            .cloned()
            .or_else(|| session.tabs.first().map(|t| t.id.clone()));
        self.view_mode = if session.view_mode_tab { ViewMode::Tab } else { ViewMode::Graph };
        if matches!(self.view_mode, ViewMode::Tab) {
            // The focused tab hasn't actually spawned yet — this is the
            // exact same "still starting up" gap crash recovery has, so
            // reuse its own spinner-instead-of-snapping-to-Graph path
            // (see `build_tab_ui`) rather than inventing a second one.
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
        let Some(session) = session::Session::load_named(name) else { return };
        self.replace_running_session(session);
    }

    /// Tears down every currently-open tab/download/group -- each
    /// `TabNode`'s own `Drop` handles process shutdown+kill, the same
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
        self.tabs.clear();
        self.tab_order.clear();
        self.downloads.clear();
        self.download_order.clear();
        self.groups.clear();
        self.group_order.clear();
        self.focused = None;
        self.current_group = None;
        self.dragging = None;
        self.hovered = None;
        self.selected_tabs.clear();
        self.rect_select_start = None;
        self.graph_settled_key = None;
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
        self.spawn_tab_async(id, None, None, false, None, home_page, egui::Vec2::ZERO, egui::Vec2::ZERO, SpawnPurpose::Root, kind);
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
        std::thread::spawn(move || {
            let result = process_manager::spawn_tab(kind, &id, &url);
            let _ = proxy.send_event(UserEvent::TabSpawned(TabSpawnOutcome {
                id,
                parent,
                group,
                pinned,
                color,
                url,
                pos,
                vel,
                purpose,
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
        let TabSpawnOutcome { id, parent, group, pinned, color, url, pos, vel, purpose, result } = outcome;
        // A group dissolved while this spawn was still in flight (spawning
        // is async, see this fn's own doc comment) would otherwise leave
        // the new tab pointing at a group id nothing can navigate into —
        // same guard `reopen_closed_tab_shortcut` applies for the same
        // reason, centralized here since every spawn path funnels through
        // this one function.
        let group = group.filter(|g| self.groups.contains_key(g));
        if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Restore)
            && self.pending_respawn_focus.as_deref() == Some(id.as_str())
        {
            self.pending_respawn_focus = None;
        }
        match result {
            Ok(mut tab) => {
                let is_focused = match purpose {
                    SpawnPurpose::Root => true,
                    SpawnPurpose::New => false,
                    SpawnPurpose::Respawn | SpawnPurpose::Restore => {
                        self.focused.as_deref() == Some(id.as_str())
                    }
                };
                tab.set_visible(is_focused);
                self.tabs.insert(
                    id.clone(),
                    TabNode {
                        tab,
                        parent,
                        url: url.clone(),
                        title: String::new(),
                        can_go_back: false,
                        can_go_forward: false,
                        loading: false,
                        audio_playing: false,
                        muted: false,
                        pinned,
                        group,
                        swap_target: None,
                        pos,
                        vel,
                        color,
                    },
                );
                if !self.tab_order.iter().any(|t| t == &id) {
                    self.tab_order.push(id.clone());
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
                    SpawnPurpose::Respawn => {
                        if is_focused {
                            if !self.address_bar_focused && !self.debug_address_bar_pin {
                                self.address_bar_text = url;
                            }
                            self.send_resize_to(&id);
                        }
                    }
                    SpawnPurpose::Restore => {
                        self.send_zoom_to(&id);
                        if is_focused {
                            self.address_bar_text = url;
                            self.send_resize_to(&id);
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
                    SpawnPurpose::Respawn => format!("Couldn't restart tab: {err}"),
                    SpawnPurpose::Restore => format!("Couldn't restore tab: {err}"),
                });
            }
        }
    }

    fn alloc_tab_id(&mut self) -> String {
        let id = format!("tab-{}", self.next_tab_id);
        self.next_tab_id += 1;
        id
    }

    /// Distinct prefix from `alloc_tab_id` so a download node's id can
    /// never collide with a tab's — both share the same graph/edge id
    /// space (`graph_layout::step` sees them as one flat set of nodes).
    fn alloc_download_id(&mut self) -> String {
        let id = format!("dl-{}", self.next_download_id);
        self.next_download_id += 1;
        id
    }

    /// Removes a download node from the graph — the file on disk is left
    /// untouched, this only forgets the node. Session-only state, so
    /// there's nothing to persist after removal either.
    fn remove_download_node(&mut self, id: &str) {
        self.downloads.remove(id);
        self.download_order.retain(|d| d != id);
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
    /// engine-host. Shared by `reveal_download` and the download context
    /// menu's "Copy Path".
    fn windows_path_for_download(&self, id: &str) -> Option<String> {
        let node = self.downloads.get(id)?;
        Some(if let Some(posix) = node.path.strip_prefix('/') {
            format!("\\\\wsl.localhost\\Ubuntu\\{}", posix.replace('/', "\\"))
        } else {
            node.path.clone()
        })
    }

    /// Graph node click on a download: opens the containing folder with
    /// the file pre-selected, same "just show the obvious thing, no custom
    /// UI" posture as the download itself (see `EngineToHost::
    /// DownloadCompleted`'s doc comment).
    fn reveal_download(&self, id: &str) {
        let Some(path) = self.windows_path_for_download(id) else { return };
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{path}"))
            .spawn();
    }

    /// Distinct prefix from `alloc_tab_id`/`alloc_download_id` — group ids
    /// share the same flat id space at the top level (`graph_layout::step`
    /// again sees ids only, not node kinds).
    fn alloc_group_id(&mut self) -> String {
        let id = format!("grp-{}", self.next_group_id);
        self.next_group_id += 1;
        id
    }

    /// Dissolves a group: members become top-level tabs again (never
    /// closed — grouping is purely a display grouping, not an ownership
    /// one), and the group card itself is removed. `current_group` resets
    /// to top level if it happened to be the group just dissolved (should
    /// only actually matter defensively — the close button that calls
    /// this only ever renders at the top level, where the group being
    /// closed can't simultaneously be the one being viewed).
    fn dissolve_group(&mut self, id: &str) {
        for node in self.tabs.values_mut() {
            if node.group.as_deref() == Some(id) {
                node.group = None;
            }
        }
        for node in self.downloads.values_mut() {
            if node.group.as_deref() == Some(id) {
                node.group = None;
            }
        }
        self.groups.remove(id);
        self.group_order.retain(|g| g != id);
        if self.current_group.as_deref() == Some(id) {
            self.current_group = None;
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
        id.as_ref().and_then(|id| self.tabs.get(id)).and_then(|n| n.group.clone())
    }

    /// `tab_order`/`download_order`, filtered to whichever level
    /// `self.current_group` is currently viewing — the id lists
    /// `build_graph_ui`'s node-rendering loops iterate, kept in their
    /// stable insertion order (unlike filtering `self.tabs`/`self.downloads`
    /// directly, a `HashMap`'s iteration order isn't stable frame-to-frame).
    fn tabs_at_current_level(&self) -> Vec<String> {
        self.tab_order
            .iter()
            .filter(|id| self.tabs.get(*id).is_some_and(|n| n.group == self.current_group))
            .cloned()
            .collect()
    }

    fn downloads_at_current_level(&self) -> Vec<String> {
        self.download_order
            .iter()
            .filter(|id| self.downloads.get(*id).is_some_and(|n| n.group == self.current_group))
            .cloned()
            .collect()
    }

    /// "Reset View"'s scene rect: bounds every node currently visible at
    /// this level (tabs/downloads, plus group cards at the top level —
    /// same level rules `tabs_at_current_level`/`downloads_at_current_level`
    /// and `build_graph_ui`'s own `self.current_group.is_none()` group-card
    /// gate already use) instead of always framing the same fixed
    /// world-space rect regardless of how many nodes exist or where
    /// they've drifted to. Each node contributes its own half-size (a
    /// node's *card*, not just its center point, must fit on screen) plus
    /// a flat margin so a node isn't left flush against the viewport edge.
    /// Falls back to `default_graph_scene_rect()` when there's nothing to
    /// fit (an empty group, or before any tab has spawned) — an empty
    /// bounding box has no sensible center/size to frame.
    fn fit_all_nodes_scene_rect(&self) -> egui::Rect {
        const MARGIN: f32 = 60.0;
        let mut bounds = egui::Rect::NOTHING;
        for id in self.tabs_at_current_level() {
            if let Some(node) = self.tabs.get(&id) {
                let center = egui::Pos2::new(node.pos.x, node.pos.y);
                bounds = bounds.union(egui::Rect::from_center_size(center, GRAPH_NODE_SIZE));
            }
        }
        for id in self.downloads_at_current_level() {
            if let Some(node) = self.downloads.get(&id) {
                let center = egui::Pos2::new(node.pos.x, node.pos.y);
                bounds = bounds.union(egui::Rect::from_center_size(center, DOWNLOAD_NODE_SIZE));
            }
        }
        if self.current_group.is_none() {
            for node in self.groups.values() {
                let center = egui::Pos2::new(node.pos.x, node.pos.y);
                bounds = bounds.union(egui::Rect::from_center_size(center, GROUP_NODE_SIZE));
            }
        }
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
        let Some(level_group) = self.tabs.get(root_id).map(|n| n.group.clone()) else {
            return Vec::new();
        };
        let mut result = vec![root_id.to_string()];
        let mut frontier = vec![root_id.to_string()];
        while let Some(current) = frontier.pop() {
            for (id, node) in &self.tabs {
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

    /// The tab context menu's "Fit Subtree" entry -- same margin/fallback
    /// shape as `fit_all_nodes_scene_rect`, just scoped to `subtree_ids`
    /// instead of the whole level.
    fn fit_subtree_scene_rect(&self, root_id: &str) -> egui::Rect {
        const MARGIN: f32 = 60.0;
        let mut bounds = egui::Rect::NOTHING;
        for id in self.subtree_ids(root_id) {
            if let Some(node) = self.tabs.get(&id) {
                let center = egui::Pos2::new(node.pos.x, node.pos.y);
                bounds = bounds.union(egui::Rect::from_center_size(center, GRAPH_NODE_SIZE));
            }
        }
        if !bounds.is_finite() {
            return default_graph_scene_rect();
        }
        bounds.expand(MARGIN)
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
            self.tabs_at_current_level().len()
                + self.downloads_at_current_level().len()
                + if self.current_group.is_none() { self.groups.len() } else { 0 },
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
        for id in self.tabs_at_current_level() {
            let Some(node) = self.tabs.get(&id) else { continue };
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
            .filter(|id| self.tabs.get(id.as_str()).is_some_and(|n| n.group == self.current_group))
            .and_then(|id| self.tabs.get(id))
            .map(|n| n.pos);
        match from_at_level {
            Some(from) => {
                if let Some(next) = self.nearest_tab_in_direction(from, dir) {
                    self.focused = Some(next);
                }
            }
            None => {
                if let Some(first) = self.tabs_at_current_level().into_iter().next() {
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
            if self.tabs.get(&id).is_some_and(|n| n.group == self.current_group) {
                self.start_transition_to_tab(&id);
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

        let mut positions: HashMap<String, egui::Vec2> = self
            .tabs
            .iter()
            .filter(|(_, n)| n.group == self.current_group)
            .map(|(id, n)| (id.clone(), n.pos))
            .chain(
                self.downloads
                    .iter()
                    .filter(|(_, n)| n.group == self.current_group)
                    .map(|(id, n)| (id.clone(), n.pos)),
            )
            .chain(
                self.current_group
                    .is_none()
                    .then(|| self.groups.iter().map(|(id, n)| (id.clone(), n.pos)))
                    .into_iter()
                    .flatten(),
            )
            .collect();
        let mut velocities: HashMap<String, egui::Vec2> = self
            .tabs
            .iter()
            .filter(|(_, n)| n.group == self.current_group)
            .map(|(id, n)| (id.clone(), n.vel))
            .chain(
                self.downloads
                    .iter()
                    .filter(|(_, n)| n.group == self.current_group)
                    .map(|(id, n)| (id.clone(), n.vel)),
            )
            .chain(
                self.current_group
                    .is_none()
                    .then(|| self.groups.iter().map(|(id, n)| (id.clone(), n.vel)))
                    .into_iter()
                    .flatten(),
            )
            .collect();
        let edges: Vec<(String, String)> = self
            .tabs
            .iter()
            .filter(|(_, n)| n.group == self.current_group)
            .filter_map(|(id, n)| n.parent.clone().map(|p| (p, id.clone())))
            .chain(
                self.downloads
                    .iter()
                    .filter(|(_, n)| n.group == self.current_group)
                    .filter_map(|(id, n)| n.parent.clone().map(|p| (p, id.clone()))),
            )
            .filter(|(p, c)| positions.contains_key(p) && positions.contains_key(c))
            .collect();
        let pinned: HashSet<String> = self
            .tabs
            .iter()
            .filter(|(_, n)| n.group == self.current_group && n.pinned)
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

        for (id, node) in self.tabs.iter_mut() {
            if let Some(p) = positions.get(id) {
                node.pos = *p;
            }
            if let Some(v) = velocities.get(id) {
                node.vel = *v;
            }
        }
        for (id, node) in self.downloads.iter_mut() {
            if let Some(p) = positions.get(id) {
                node.pos = *p;
            }
            if let Some(v) = velocities.get(id) {
                node.vel = *v;
            }
        }
        for (id, node) in self.groups.iter_mut() {
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
        let Some((parent, group, url, kind)) =
            self.tabs.get(id).map(|n| (n.parent.clone(), n.group.clone(), n.url.clone(), n.tab.kind))
        else {
            return;
        };
        self.spawn_new_tab_with_kind(parent, group, &url, kind);
    }

    fn spawn_new_tab(&mut self, parent: Option<String>, group: Option<String>, url: &str) {
        self.spawn_new_tab_with_kind(parent, group, url, self.settings.default_starting_engine);
    }

    /// Shared by `spawn_new_tab` (always `default_starting_engine`) and
    /// `reopen_closed_tab_shortcut` (the closed tab's own engine, not
    /// necessarily today's default).
    fn spawn_new_tab_with_kind(
        &mut self,
        parent: Option<String>,
        group: Option<String>,
        url: &str,
        kind: EngineKind,
    ) {
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
            .and_then(|p| self.tabs.get(p))
            .filter(|n| n.group == group)
            .map(|n| n.pos)
            .or_else(|| self.tabs.values().find(|n| n.group == group).map(|n| n.pos))
            .unwrap_or(egui::Vec2::ZERO);
        let pos = parent_pos + graph_layout::spawn_jitter(&id);
        self.tab_spawn_error = None;
        self.spawn_tab_async(id, parent, group, false, None, url.to_string(), pos, egui::Vec2::ZERO, SpawnPurpose::New, kind);
    }

    /// Peek-click commits focus: the tab losing focus (if any, and if
    /// different) is told to stop rendering, the newly focused one is told
    /// to start, and the view switches to Tab. At most one tab is ever
    /// visible at a time.
    fn focus_tab(&mut self, id: &str) {
        if self.focused.as_deref() != Some(id) {
            self.close_find_bar();
            self.context_menu = None;
            if let Some(old_id) = self.focused.clone() {
                if let Some(old) = self.tabs.get_mut(&old_id) {
                    old.tab.set_visible(false);
                }
            }
        }
        if let Some(node) = self.tabs.get_mut(id) {
            node.tab.set_visible(true);
            if !self.address_bar_focused && !self.debug_address_bar_pin {
                self.address_bar_text = node.url.clone();
            }
        }
        self.focused = Some(id.to_string());
        self.view_mode = ViewMode::Tab;
        self.send_resize_to(id);
        self.save_session();
    }

    /// Leaves Tab view for Graph view: starts a shrink animation (see
    /// `Transition`) rather than switching instantly. The tab keeps
    /// rendering (`set_visible(true)`, unchanged) for the animation's
    /// whole duration, so the thumbnail it shrinks into shows real,
    /// current content instead of whatever frame happened to be on screen
    /// when the animation started — only actually suspended once the
    /// animation finishes (`finish_transition_if_done`).
    fn leave_to_graph(&mut self) {
        self.context_menu = None;
        match self.focused.clone() {
            Some(id) => {
                self.transition = Some(Transition {
                    tab_id: id,
                    kind: TransitionKind::ToGraph,
                    start: std::time::Instant::now(),
                });
            }
            // No focused tab at all (shouldn't normally happen — Tab view
            // always has one — but nothing to animate either way).
            None => {
                self.view_mode = ViewMode::Graph;
                self.save_session();
            }
        }
        self.debug_address_bar_pin = false;
        self.dropdown_highlighted = None;
    }

    /// Graph view's node-click equivalent of `focus_tab` — starts an
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
    fn start_transition_to_tab(&mut self, id: &str) {
        if self.transition.is_some() {
            return;
        }
        self.focus_tab(id);
        // `focus_tab` already set `view_mode = Tab`, but `build_ui` renders
        // Graph view as the backdrop for as long as `self.transition` is
        // `Some` regardless of `view_mode` (see its own doc comment) —
        // `finish_transition_if_done` is what actually matters once the
        // animation completes.
        self.transition = Some(Transition {
            tab_id: id.to_string(),
            kind: TransitionKind::ToTab,
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
        let tab_id = transition.tab_id.clone();
        let kind = transition.kind;
        self.transition = None;
        match kind {
            TransitionKind::ToGraph => {
                self.view_mode = ViewMode::Graph;
                if let Some(node) = self.tabs.get_mut(&tab_id) {
                    node.tab.set_visible(false);
                }
            }
            TransitionKind::ToTab => {
                self.view_mode = ViewMode::Tab;
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

    /// The rect (in egui points, screen space) a `Transition` should draw
    /// its tab's live content quad at *this frame* — eased interpolation
    /// between Tab view's full content area and the tab's graph-node
    /// thumbnail spot, direction depending on `transition.kind`.
    fn transition_content_rect_points(&self, transition: &Transition) -> egui::Rect {
        let screen = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let screen_points = egui::vec2(screen.width as f32 / ppp, screen.height as f32 / ppp);

        let full_content_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, CHROME_HEIGHT_POINTS),
            egui::vec2(screen_points.x, (screen_points.y - CHROME_HEIGHT_POINTS).max(0.0)),
        );

        let node_rect = self
            .tabs
            .get(&transition.tab_id)
            .map(|node| {
                let scene_outer_rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, GRAPH_TOOLBAR_HEIGHT_POINTS),
                    egui::vec2(screen_points.x, (screen_points.y - GRAPH_TOOLBAR_HEIGHT_POINTS).max(0.0)),
                );
                let (scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
                let world_rect =
                    egui::Rect::from_center_size(egui::pos2(node.pos.x, node.pos.y), GRAPH_NODE_SIZE)
                        .shrink(GRAPH_THUMB_MARGIN);
                egui::Rect::from_min_max(
                    egui::pos2(
                        world_rect.min.x * scale + translation.x,
                        world_rect.min.y * scale + translation.y,
                    ),
                    egui::pos2(
                        world_rect.max.x * scale + translation.x,
                        world_rect.max.y * scale + translation.y,
                    ),
                )
            })
            .unwrap_or(full_content_rect);

        let (from, to) = match transition.kind {
            TransitionKind::ToGraph => (full_content_rect, node_rect),
            TransitionKind::ToTab => (node_rect, full_content_rect),
        };
        let t = (transition.start.elapsed().as_secs_f32() / TRANSITION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
        // Smoothstep: eases in and out, rather than the constant-velocity
        // (and slightly mechanical-looking) feel of plain linear `lerp`.
        let eased = t * t * (3.0 - 2.0 * t);
        egui::Rect::from_min_max(
            from.min + (to.min - from.min) * eased,
            from.max + (to.max - from.max) * eased,
        )
    }

    /// Closes a tab: tears down its process (via `EngineTab`'s `Drop` —
    /// `Shutdown` + kill), frees its graph-view thumbnail, and reparents
    /// its children to its own parent so the forest stays connected rather
    /// than orphaning that whole subtree. Falls back to Graph view if the
    /// closed tab was focused.
    fn close_tab(&mut self, id: &str) {
        let Some((parent, url, kind, group)) = self
            .tabs
            .get(id)
            .map(|n| (n.parent.clone(), n.url.clone(), n.tab.kind, n.group.clone()))
        else {
            return;
        };
        self.closed_tabs.push(ClosedTab { url, kind, parent: parent.clone(), group });
        if self.closed_tabs.len() > MAX_CLOSED_TABS {
            self.closed_tabs.remove(0);
        }
        for node in self.tabs.values_mut() {
            if node.parent.as_deref() == Some(id) {
                node.parent = parent.clone();
            }
        }
        for node in self.downloads.values_mut() {
            if node.parent.as_deref() == Some(id) {
                node.parent = parent.clone();
            }
        }
        self.tabs.remove(id);
        self.tab_order.retain(|t| t != id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
        }
        if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
        }
        if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
            self.view_mode = ViewMode::Graph;
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
    /// focused) -> old engine-host dropped when its `TabNode` is replaced.
    fn start_swap_to(&mut self, tab_id: &str, target: EngineKind) {
        let Some(node) = self.tabs.get_mut(tab_id) else {
            return;
        };
        if node.tab.kind == target || node.swap_target.is_some() {
            return; // no-op swap, or one swap at a time for this tab
        }
        node.swap_target = Some(target);
        node.tab.send(&HostToEngine::GetState);
    }

    fn finish_swap(&mut self, tab_id: &str, target: EngineKind, url: String) {
        let Some((parent, group, pinned, color, pos, vel)) = self
            .tabs
            .get(tab_id)
            .map(|n| (n.parent.clone(), n.group.clone(), n.pinned, n.color, n.pos, n.vel))
        else {
            return;
        };
        // Must drop (and thus Shutdown + kill) the old tab before spawning
        // the new one at the same tab_id — the new engine-host's named-pipe
        // listeners would collide with the old ones otherwise.
        self.tabs.remove(tab_id);
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
        self.respawn_tab(tab_id, target, url, parent, group, pinned, color, pos, vel);
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
            url,
            pos,
            vel,
            SpawnPurpose::Respawn,
            kind,
        );
    }

    /// Browser-chrome keyboard shortcuts — checked via egui's own input
    /// polling (same mechanism `build_tab_ui`'s own `Escape` check uses),
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
        if let Some(node) = self.tabs.get_mut(&id) {
            node.pinned = !node.pinned;
        }
    }

    /// `Ctrl+R`/`F5` / headless `RELOAD` — reloads the focused tab. Every
    /// engine exposes a plain no-argument reload (see `HostToEngine::
    /// Reload`'s own doc comment), so there's no can-reload guard the way
    /// `GoBack`/`GoForward` need one.
    fn reload_focused_tab_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        if let Some(node) = self.tabs.get_mut(&id) {
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
        let Some(name) = self.groups.get(&group_id).map(|g| g.name.clone()) else { return };
        self.renaming_group = Some((group_id, name));
        self.pending_focus_rename = true;
    }

    /// `Ctrl+Shift+G` / toolbar "+ New Group" / headless `NEWGROUP` —
    /// creates an empty group at the top level. No-ops while viewing a
    /// group's own subgraph, same "no nesting" restriction "+ New Group"'s
    /// own button already enforces by only rendering there.
    fn new_group_shortcut(&mut self) {
        if self.current_group.is_some() {
            return;
        }
        let id = self.alloc_group_id();
        let pos = GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id);
        let name = format!("Group {}", self.groups.len() + 1);
        self.groups.insert(id.clone(), GroupNode { name, pos, vel: egui::Vec2::ZERO, color: None });
        self.group_order.push(id);
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
            ViewMode::Tab => self.leave_to_graph(),
            ViewMode::Settings => self.close_settings_page(),
            ViewMode::Graph => {
                let Some(id) = self.focused.clone() else { return };
                if self.tabs.contains_key(&id) {
                    self.current_group = self.tabs.get(&id).and_then(|n| n.group.clone());
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

    /// `Ctrl+=`/`Ctrl++` / headless `ZOOMIN` — same step/cap the settings
    /// popup's own "+" button uses.
    fn zoom_in_shortcut(&mut self) {
        self.settings.zoom_factor = (self.settings.zoom_factor + 0.1).min(2.0);
        self.apply_zoom_change();
    }

    /// `Ctrl+-` / headless `ZOOMOUT` — same step/floor the settings popup's
    /// own "−" button uses.
    fn zoom_out_shortcut(&mut self) {
        self.settings.zoom_factor = (self.settings.zoom_factor - 0.1).max(0.5);
        self.apply_zoom_change();
    }

    /// `Ctrl+0` / headless `ZOOMRESET` — same as the settings popup's own
    /// "Reset" button.
    fn zoom_reset_shortcut(&mut self) {
        self.settings.zoom_factor = 1.0;
        self.apply_zoom_change();
    }

    /// `F12` / headless `OPENDEVTOOLS` — opens the focused tab's engine's
    /// own native DevTools UI (a separate real window the engine manages
    /// itself; nothing comes back to `browser-ui` for this, so there's no
    /// state to track here beyond sending the message). See
    /// `HostToEngine::OpenDevTools`'s doc comment for per-engine support.
    fn open_devtools_shortcut(&mut self) {
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.tabs.get_mut(&id) else { return };
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
        self.spawn_new_tab_with_kind(closed.parent, closed.group, &closed.url, closed.kind);
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
    /// `tabs_at_current_level` returns (top level: every ungrouped tab,
    /// leaving groups and their members untouched; inside a group: every
    /// member, same effect as that group's own "Close Group" context-menu
    /// entry, just reachable without opening it). Each closes through the
    /// normal `close_tab` path, same "just that action run once per tab"
    /// shape `close_all_except`/`close_group` already have.
    fn close_all_at_current_level(&mut self) {
        for id in self.tabs_at_current_level() {
            self.close_tab(&id);
        }
    }

    /// Toggles `id`'s mute state and sends the real `SetMuted` — shared by
    /// Tab view's "Muted" toolbar button and Graph view's own audio-dot
    /// click (see `TabNode::muted`'s doc comment for the WebView2/WebKit-
    /// only caveat).
    fn toggle_muted(&mut self, id: &str) {
        if let Some(node) = self.tabs.get_mut(id) {
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
        let temp_png = std::env::temp_dir().join(format!("multiengine-browser-print-{timestamp}.png"));
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
        let Some(node) = self.tabs.get(id) else { return };
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
            build_list_popup_frame(ui.ctx(), "bookmarks_popup", anchor_rect, 220.0, 320.0, fade, |ui| {
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
                                .clicked()
                            {
                                navigate_url = Some(bookmark.url.clone());
                            }
                            if ui.small_button(egui_phosphor::regular::X).clicked() {
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
        if close || self.bookmarks.is_empty() {
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
            build_list_popup_frame(ui.ctx(), "history_popup", anchor_rect, 220.0, 320.0, fade, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.history_filter)
                    .desired_width(f32::INFINITY)
                    .hint_text("Filter..."),
            );
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for entry in self.history.iter().rev() {
                    let label = if entry.title.is_empty() { &entry.url } else { &entry.title };
                    if !filter_matches(&self.history_filter, &[&entry.title, &entry.url]) {
                        continue;
                    }
                    if ui.add(egui::Label::new(label).truncate().sense(egui::Sense::click())).clicked() {
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
        if close || self.history.is_empty() {
            self.history_open = false;
        }
    }

    /// Tab view's "Downloads (N)" dropdown -- every entry in
    /// `self.downloads` (the same data the graph view's own download
    /// nodes/context menu show, see `App::download_context_menu`), most-
    /// recently-downloaded first, click a filename to open its containing
    /// folder (same as `reveal_download`), "x" to remove it from the list
    /// (same as `remove_download_node`). Exists so a completed download can
    /// be found without first switching to Graph view.
    fn build_downloads_popup(&mut self, ui: &mut egui::Ui, anchor_rect: egui::Rect, fade: f32) {
        let mut close = false;
        let mut reveal_id: Option<String> = None;
        let mut remove_id: Option<String> = None;
        let area_rect =
            build_list_popup_frame(ui.ctx(), "downloads_popup", anchor_rect, 220.0, 320.0, fade, |ui| {
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for id in self.download_order.iter().rev() {
                    let Some(node) = self.downloads.get(id) else { continue };
                    ui.horizontal(|ui| {
                        let filename = download_filename(&node.path);
                        if ui
                            .add(egui::Label::new(filename).truncate().sense(egui::Sense::click()))
                            .clicked()
                        {
                            reveal_id = Some(id.clone());
                        }
                        if ui.small_button(egui_phosphor::regular::X).clicked() {
                            remove_id = Some(id.clone());
                        }
                    });
                }
            });
        });

        let clicked_outside = clicked_outside_popup(ui, area_rect, Some(anchor_rect));

        if let Some(id) = reveal_id {
            self.reveal_download(&id);
            close = true;
        }
        if let Some(id) = remove_id {
            self.remove_download_node(&id);
        }
        if clicked_outside {
            close = true;
        }
        if close || self.downloads.is_empty() {
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
        let all_pinned = !self.tabs.is_empty() && self.tabs.values().all(|n| n.pinned);
        let area = egui::Area::new(egui::Id::new("tab_actions_popup"))
            .fixed_pos(anchor_rect.left_bottom())
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(4u8)
                    .inner_margin(egui::Margin::same(6))
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
        let other_ids: Vec<String> =
            self.tabs.keys().filter(|id| id.as_str() != keep_id).cloned().collect();
        for id in other_ids {
            if let Some(node) = self.tabs.get_mut(&id) {
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
        let other_ids: Vec<String> =
            self.tab_order.iter().filter(|id| id.as_str() != keep_id).cloned().collect();
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
        for node in self.tabs.values_mut() {
            node.pinned = pin;
        }
    }

    /// `Ctrl+L` / headless `FOCUSADDRESS` — only meaningful in Tab view,
    /// since Graph view has no address bar to focus. Sets a pending flag
    /// rather than `address_bar_focused` directly: the address bar's
    /// `TextEdit` doesn't exist as a widget until `build_tab_ui` builds it,
    /// so actually moving real egui focus onto it needs to happen there,
    /// via `response.request_focus()` — see `pending_focus_address_bar`'s
    /// own doc comment.
    fn focus_address_bar_shortcut(&mut self) {
        if matches!(self.view_mode, ViewMode::Tab) {
            self.pending_focus_address_bar = true;
            // `request_focus()` only takes hold starting the *next* frame
            // after it's called (immediate-mode: the widget this frame
            // already painted before `build_tab_ui` gets to call it) — a
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
        if !matches!(self.view_mode, ViewMode::Tab) {
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
        let Some(node) = self.tabs.get_mut(&id) else { return };
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
            if let Some(node) = self.tabs.get_mut(&id) {
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
        let Some(node) = self.tabs.get_mut(&id) else { return };
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
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(4u8)
                    .inner_margin(egui::Margin::symmetric(10, 6))
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
                            if ui.button(egui_phosphor::regular::X).clicked() {
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

    /// `self.tab_order`, filtered by a case-insensitive substring match
    /// against title-or-url — factored out so the quick-switcher's render
    /// function and its Enter-key jump always agree on the exact same list
    /// (computed once per frame, not separately in each place that needs
    /// it).
    fn quick_switcher_matches(&self, query: &str) -> Vec<String> {
        let query = query.to_lowercase();
        self.tab_order
            .iter()
            .filter(|id| {
                self.tabs.get(id.as_str()).is_some_and(|n| {
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
            let group = self.tabs.get(&id).and_then(|n| n.group.clone());
            self.current_group = group;
            self.focus_tab(&id);
        }
        self.quick_switcher = None;
    }

    /// `Ctrl+K`'s tab quick-switcher: a centered search box plus a
    /// scrollable list of matches (see `quick_switcher_matches`), arrow
    /// keys move the selection, Enter jumps to it, Escape closes. Called
    /// from `build_ui` itself rather than `build_tab_ui`/`build_graph_ui`
    /// so it works the same from either view.
    fn build_quick_switcher(&mut self, ui: &mut egui::Ui) {
        if self.quick_switcher.is_none() {
            return;
        }

        // Read once, outside the `Area`, so arrow/Enter/Escape work
        // regardless of whether the search field inside currently has
        // egui focus — same reasoning the address-bar dropdown's own
        // arrow-key handling already documents.
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
        let Some(switcher) = &self.quick_switcher else { return };
        let query = switcher.query.clone();
        let selected_index = switcher.selected;
        let matches = self.quick_switcher_matches(&query);
        let mut jump_to: Option<String> = None;

        egui::Area::new(egui::Id::new("quick_switcher"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(6u8)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(360.0);
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
                        ui.separator();
                        if matches.is_empty() {
                            ui.label("No matching tabs");
                        }
                        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                            for (i, id) in matches.iter().enumerate() {
                                let Some(node) = self.tabs.get(id) else { continue };
                                let label = if node.title.is_empty() {
                                    node.url.clone()
                                } else {
                                    node.title.clone()
                                };
                                if ui.selectable_label(i == selected_index, label).clicked() {
                                    jump_to = Some(id.clone());
                                }
                            }
                        });
                    });
            });

        if let Some(id) = jump_to {
            let group = self.tabs.get(&id).and_then(|n| n.group.clone());
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
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(6u8)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(360.0);
                        ui.label(egui::RichText::new("Keyboard Shortcuts").strong().color(TEXT_PRIMARY));
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
    /// chain in `build_tab_ui`).
    fn build_context_menu(&mut self, ui: &mut egui::Ui) {
        let Some(menu) = self.context_menu.clone() else { return };
        let mut open_tab = false;
        let mut close = false;

        let area = egui::Area::new(egui::Id::new("context_menu"))
            .fixed_pos(egui::pos2(menu.x, menu.y))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(3u8)
                    .inner_margin(egui::Margin::symmetric(6, 4))
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

    /// Top-level right-click context menu for a group's card -- Rename,
    /// a color swatch row, and Dissolve in one place, rather than the
    /// click-name-to-rename/always-visible-swatch/hover-"x" trio spread
    /// across the card itself. Reuses `self.renaming_group` for the Rename
    /// flow: clicking "Rename" swaps this same menu's contents to the
    /// `TextEdit`, exactly like the toolbar's own in-subgraph rename does,
    /// rather than opening a second, separate text field somewhere else.
    fn build_group_context_menu(&mut self, ui: &mut egui::Ui) {
        let Some((id, pos)) = self.group_context_menu.clone() else { return };
        if !self.groups.contains_key(&id) {
            self.group_context_menu = None;
            return;
        }
        let mut close = false;
        let mut dissolve = false;
        let mut close_group = false;
        let mut export_group_requested = false;
        let is_renaming = self.renaming_group.as_ref().is_some_and(|(gid, _)| gid == &id);

        let area = egui::Area::new(egui::Id::new("group_context_menu"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(4u8)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(150.0);
                        if is_renaming {
                            if let Some((_, draft)) = &mut self.renaming_group {
                                let response =
                                    ui.add(egui::TextEdit::singleline(draft).desired_width(140.0));
                                if std::mem::take(&mut self.pending_focus_rename) {
                                    response.request_focus();
                                }
                                if response.lost_focus() {
                                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        self.renaming_group = None;
                                    } else if let Some((gid, name)) = self.renaming_group.take() {
                                        let trimmed = name.trim();
                                        if !trimmed.is_empty() {
                                            if let Some(node) = self.groups.get_mut(&gid) {
                                                node.name = trimmed.to_string();
                                            }
                                        }
                                    }
                                    close = true;
                                }
                            }
                        } else {
                            if ui.button("Rename").clicked() {
                                let name = self.groups.get(&id).map(|g| g.name.clone()).unwrap_or_default();
                                self.renaming_group = Some((id.clone(), name));
                                self.pending_focus_rename = true;
                            }
                            ui.add_space(4.0);
                            ui.label("Color");
                            ui.horizontal(|ui| {
                                let current = self.groups.get(&id).and_then(|g| g.color);
                                for &swatch in &TAG_COLOR_PALETTE {
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(18.0, 18.0),
                                        egui::Sense::click(),
                                    );
                                    ui.painter().circle_filled(rect.center(), 8.0, swatch);
                                    if current == Some(swatch) {
                                        ui.painter().circle_stroke(
                                            rect.center(),
                                            8.0,
                                            egui::Stroke::new(2.0, ui.visuals().text_color()),
                                        );
                                    }
                                    if response.clicked() {
                                        if let Some(node) = self.groups.get_mut(&id) {
                                            node.color = Some(swatch);
                                        }
                                        close = true;
                                    }
                                }
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(18.0, 18.0),
                                    egui::Sense::click(),
                                );
                                ui.painter().circle_stroke(
                                    rect.center(),
                                    8.0,
                                    egui::Stroke::new(1.5, ui.visuals().weak_text_color()),
                                );
                                if response.clicked() {
                                    if let Some(node) = self.groups.get_mut(&id) {
                                        node.color = None;
                                    }
                                    close = true;
                                }
                            });
                            ui.add_space(4.0);
                            ui.separator();
                            if ui.button("Dissolve").clicked() {
                                dissolve = true;
                                close = true;
                            }
                            if ui.button("Close Group").clicked() {
                                close_group = true;
                                close = true;
                            }
                            // Real modal file dialog blocks on OS/human
                            // input neither exists in headless mode, same
                            // reasoning Export/Import Session and Bookmarks
                            // already have -- `EXPORTGROUP <path>` calls the
                            // same underlying save directly instead, against
                            // whichever group's context menu is open.
                            if !self.headless && ui.button("Export Group...").clicked() {
                                export_group_requested = true;
                                close = true;
                            }
                        }
                    });
            });

        let clicked_outside = clicked_outside_popup(ui, area.response.rect, None);
        if clicked_outside && !is_renaming {
            close = true;
        }

        if dissolve {
            self.dissolve_group(&id);
        }
        if close_group {
            self.close_group(&id);
        }
        if export_group_requested {
            if let Some(export) = self.export_group(&id) {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(format!("{}.json", export.name))
                    .add_filter("Group", &["json"])
                    .save_file()
                {
                    group_export::save_to(&export, &path);
                    self.save_toast = Some((path.display().to_string(), std::time::Instant::now()));
                }
            }
        }
        if close {
            self.group_context_menu = None;
        }
    }

    /// Closes every tab member of a group (each through the normal
    /// `close_tab` path -- teardown, reparenting, the `closed_tabs` stack
    /// all still apply, same as `close_all_except`/`close_selected`), then
    /// dissolves the now-tabless group itself. A download member is never
    /// closed by this (downloads have no "closed" state of their own,
    /// see `App::close_tab`'s own scope) -- `dissolve_group` returns it to
    /// the top level exactly as a plain Dissolve already would.
    fn close_group(&mut self, id: &str) {
        let member_ids: Vec<String> = self
            .tabs
            .iter()
            .filter(|(_, node)| node.group.as_deref() == Some(id))
            .map(|(tid, _)| tid.clone())
            .collect();
        for tid in member_ids {
            self.close_tab(&tid);
        }
        self.dissolve_group(id);
    }

    /// Builds an exportable snapshot of `group_id`'s own subtree -- its
    /// name, tag color, and every member tab's url/engine/position, with
    /// `parent` links preserved only between members of this same group
    /// (see `group_export::GroupExportTab`'s own doc comment). `None` if
    /// the group doesn't exist.
    fn export_group(&self, group_id: &str) -> Option<group_export::GroupExport> {
        let group = self.groups.get(group_id)?;
        let member_ids: std::collections::HashSet<&str> = self
            .tabs
            .iter()
            .filter(|(_, n)| n.group.as_deref() == Some(group_id))
            .map(|(id, _)| id.as_str())
            .collect();
        let tabs = self
            .tabs
            .iter()
            .filter(|(id, _)| member_ids.contains(id.as_str()))
            .map(|(id, node)| group_export::GroupExportTab {
                id: id.clone(),
                parent: node.parent.clone().filter(|p| member_ids.contains(p.as_str())),
                url: node.url.clone(),
                engine: node.tab.kind,
                pos_x: node.pos.x,
                pos_y: node.pos.y,
            })
            .collect();
        Some(group_export::GroupExport {
            name: group.name.clone(),
            color: group.color.map(|c| (c.r(), c.g(), c.b())),
            tabs,
        })
    }

    /// Recreates an exported group at the top level -- a new `GroupNode`
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
        self.groups.insert(
            group_id.clone(),
            GroupNode { name: export.name, pos, vel: egui::Vec2::ZERO, color },
        );
        self.group_order.push(group_id.clone());

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
                tab.url,
                pos,
                egui::Vec2::ZERO,
                SpawnPurpose::Restore,
                tab.engine,
            );
        }
    }

    /// Right-click menu for a download's card -- Open Containing Folder
    /// (same effect as a plain click, see `reveal_download`), Copy Path,
    /// Remove from List (same effect as the card's own close "x", see
    /// `remove_download_node`). See `App::build_group_context_menu` for the
    /// identical pattern this mirrors.
    fn build_download_context_menu(&mut self, ui: &mut egui::Ui) {
        let Some((id, pos)) = self.download_context_menu.clone() else { return };
        if !self.downloads.contains_key(&id) {
            self.download_context_menu = None;
            return;
        }
        let mut close = false;
        let mut reveal = false;
        let mut remove = false;

        let area = egui::Area::new(egui::Id::new("download_context_menu"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(4u8)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(170.0);
                        if ui.button("Open Containing Folder").clicked() {
                            reveal = true;
                            close = true;
                        }
                        if ui.button("Copy Path").clicked() {
                            if let Some(path) = self.windows_path_for_download(&id) {
                                ui.ctx().copy_text(path);
                            }
                            close = true;
                        }
                        ui.separator();
                        if ui.button("Remove from List").clicked() {
                            remove = true;
                            close = true;
                        }
                    });
            });

        let clicked_outside = clicked_outside_popup(ui, area.response.rect, None);
        if clicked_outside {
            close = true;
        }

        if reveal {
            self.reveal_download(&id);
        }
        if remove {
            self.remove_download_node(&id);
        }
        if close {
            self.download_context_menu = None;
        }
    }

    /// Right-click menu for a tab's card -- Pin/Unpin, the same 6-color
    /// swatch row groups already have, Remove from Group (only while
    /// actually viewing a group's own subgraph, same scope as the card's
    /// own hover-revealed eject button), and Close. See
    /// `App::build_group_context_menu` for the identical pattern this
    /// mirrors.
    fn build_tab_context_menu(&mut self, ui: &mut egui::Ui) {
        let Some((id, pos)) = self.tab_context_menu.clone() else { return };
        let Some((pinned, color, group)) =
            self.tabs.get(&id).map(|n| (n.pinned, n.color, n.group.clone()))
        else {
            self.tab_context_menu = None;
            return;
        };
        let mut close = false;
        let mut toggle_pin = false;
        let mut remove_from_group = false;
        let mut close_tab_requested = false;
        let mut duplicate_requested = false;
        let mut fit_subtree_requested = false;
        let mut set_color: Option<Option<egui::Color32>> = None;

        let area = egui::Area::new(egui::Id::new("tab_context_menu"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(4u8)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(150.0);
                        if ui.button(if pinned { "Unpin" } else { "Pin" }).clicked() {
                            toggle_pin = true;
                            close = true;
                        }
                        if ui.button("Duplicate").clicked() {
                            duplicate_requested = true;
                            close = true;
                        }
                        if ui.button("Copy URL").clicked() {
                            if let Some(url) = self.tabs.get(&id).map(|n| n.url.clone()) {
                                ui.ctx().copy_text(url);
                            }
                            close = true;
                        }
                        if ui
                            .button("Fit Subtree")
                            .on_hover_text("Zoom the graph to frame just this tab and its children")
                            .clicked()
                        {
                            fit_subtree_requested = true;
                            close = true;
                        }
                        ui.add_space(4.0);
                        ui.label("Color");
                        ui.horizontal(|ui| {
                            for &swatch in &TAG_COLOR_PALETTE {
                                let (rect, response) =
                                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                                ui.painter().circle_filled(rect.center(), 8.0, swatch);
                                if color == Some(swatch) {
                                    ui.painter().circle_stroke(
                                        rect.center(),
                                        8.0,
                                        egui::Stroke::new(2.0, ui.visuals().text_color()),
                                    );
                                }
                                if response.clicked() {
                                    set_color = Some(Some(swatch));
                                    close = true;
                                }
                            }
                            let (rect, response) =
                                ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                            ui.painter().circle_stroke(
                                rect.center(),
                                8.0,
                                egui::Stroke::new(1.5, ui.visuals().weak_text_color()),
                            );
                            if response.clicked() {
                                set_color = Some(None);
                                close = true;
                            }
                        });
                        if group.is_some() {
                            ui.add_space(4.0);
                            ui.separator();
                            if ui.button("Remove from Group").clicked() {
                                remove_from_group = true;
                                close = true;
                            }
                        }
                        ui.add_space(4.0);
                        ui.separator();
                        if ui.button("Close").clicked() {
                            close_tab_requested = true;
                            close = true;
                        }
                    });
            });

        let clicked_outside = clicked_outside_popup(ui, area.response.rect, None);
        if clicked_outside {
            close = true;
        }

        if toggle_pin {
            if let Some(node) = self.tabs.get_mut(&id) {
                node.pinned = !node.pinned;
            }
        }
        if let Some(color) = set_color {
            if let Some(node) = self.tabs.get_mut(&id) {
                node.color = color;
            }
        }
        if remove_from_group {
            if let Some(node) = self.tabs.get_mut(&id) {
                node.group = None;
            }
        }
        if duplicate_requested {
            self.duplicate_tab(&id);
        }
        if fit_subtree_requested {
            self.graph_scene_rect = self.fit_subtree_scene_rect(&id);
        }
        // Closing the tab already makes its context menu meaningless, but
        // `close_tab` doesn't know about this menu -- clear it explicitly
        // rather than leaving a menu open for an id that no longer exists.
        if close_tab_requested {
            self.close_tab(&id);
        }
        if close {
            self.tab_context_menu = None;
        }
    }

    /// A brief "Saved <path>" toast, bottom-left, for `self.save_toast` —
    /// cleared once `SAVE_TOAST_DURATION` elapses. No dismiss interaction
    /// (unlike the context menu): it just times out. Fades in over its
    /// first `TOAST_FADE_IN` and out over its last `TOAST_FADE_OUT` rather
    /// than popping on and cutting off instantly -- driven directly off
    /// `shown_at.elapsed()` (this app's winit loop runs `ControlFlow::Poll`,
    /// so it's already redrawing every frame regardless; no extra
    /// `request_repaint` needed for a time-based fade to animate smoothly),
    /// not `ctx.animate_bool` -- unlike the toggle-bool dropdown popups,
    /// this already has a real wall-clock lifetime to derive opacity from
    /// directly.
    fn build_save_toast(&mut self, ui: &mut egui::Ui) {
        const TOAST_FADE_IN: std::time::Duration = std::time::Duration::from_millis(150);
        const TOAST_FADE_OUT: std::time::Duration = std::time::Duration::from_millis(400);
        let Some((path, shown_at)) = &self.save_toast else { return };
        let elapsed = shown_at.elapsed();
        if elapsed >= SAVE_TOAST_DURATION {
            self.save_toast = None;
            return;
        }
        let fade_in = (elapsed.as_secs_f32() / TOAST_FADE_IN.as_secs_f32()).min(1.0);
        let remaining = SAVE_TOAST_DURATION.saturating_sub(elapsed);
        let fade_out = (remaining.as_secs_f32() / TOAST_FADE_OUT.as_secs_f32()).min(1.0);
        let opacity = fade_in.min(fade_out);
        let path = path.clone();
        egui::Area::new(egui::Id::new("save_toast"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(opacity);
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(3u8)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.label(format!("Saved {path}"));
                    });
            });
    }

    /// `Ctrl+Tab`/`Ctrl+Shift+Tab`: focuses the next/previous tab in
    /// `tab_order` (insertion order — this graph-based UI has no tab strip
    /// to define a more meaningful order), wrapping at both ends. A no-op
    /// with zero or one tab open.
    fn cycle_focused_tab(&mut self, forward: bool) {
        if self.tab_order.len() < 2 {
            return;
        }
        let current = self
            .focused
            .as_ref()
            .and_then(|id| self.tab_order.iter().position(|t| t == id));
        let len = self.tab_order.len() as isize;
        let next = match current {
            Some(i) => {
                let delta = if forward { 1 } else { -1 };
                (i as isize + delta).rem_euclid(len) as usize
            }
            None => 0,
        };
        let id = self.tab_order[next].clone();
        self.focus_tab(&id);
    }

    fn build_ui(&mut self, ui: &mut egui::Ui) {
        self.handle_global_shortcuts(ui);
        // A `Transition` (either direction) always renders Graph view's
        // node layout as the backdrop — it's the one screen both a
        // shrinking-away Tab view and an expanding-toward Tab view have a
        // coherent shared position in (the target/source node). The actual
        // animated content quad itself is drawn separately, in
        // `RedrawRequested` — see `transition_content_rect_points`.
        if self.transition.is_some() {
            self.build_graph_ui(ui);
            return;
        }
        match self.view_mode {
            ViewMode::Tab => self.build_tab_ui(ui),
            ViewMode::Graph => self.build_graph_ui(ui),
            ViewMode::Settings => self.build_settings_page(ui),
        }
        self.build_quick_switcher(ui);
        self.build_import_confirm_popup(ui);
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
        let current = self.tabs.len();
        let mut confirm = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::new("import_confirm_popup"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                    .corner_radius(6u8)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_min_width(240.0);
                        ui.label(egui::RichText::new("Import Session").strong().color(TEXT_PRIMARY));
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

    fn build_tab_ui(&mut self, ui: &mut egui::Ui) {
        let Some(focused_id) = self.focused.clone() else {
            self.view_mode = ViewMode::Graph;
            return;
        };
        if !self.tabs.contains_key(&focused_id) {
            if self.pending_respawn_focus.as_deref() == Some(focused_id.as_str()) {
                // An engine swap or crash recovery is respawning the tab
                // the user is currently looking at — stay in Tab view
                // (don't bounce to Graph, see `pending_respawn_focus`'s doc
                // comment) and show a lightweight placeholder (with just
                // enough chrome to still escape to Graph) instead of
                // nothing until it reappears in `self.tabs`.
                let mut back_to_graph = false;
                egui::Panel::top("tab_strip_respawning")
                    .exact_size(CHROME_HEIGHT_POINTS)
                    .frame(
                        egui::Frame::default()
                            .fill(TOOLBAR_FILL)
                            .inner_margin(egui::Margin::symmetric(10, 0)),
                    )
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            if ui.button("Graph").clicked() {
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
            || !self.search_suggestions.is_empty()
            || self.debug_address_bar_pin;
        if !editing_address {
            self.address_bar_text = self.tabs[&focused_id].url.clone();
        }
        let current_kind = self.tabs.get(&focused_id).map(|n| n.tab.kind);

        let mut go_back = false;
        let mut go_forward = false;
        let mut reload_requested = false;
        let mut home_requested = false;
        let mut swap_to = None;
        let mut navigate = None;
        let mut back_to_graph = false;
        let mut mute_toggle = false;
        let mut toggle_bookmark_requested = false;
        let mut tab_actions_button_rect: Option<egui::Rect> = None;
        let mut bookmarks_button_rect: Option<egui::Rect> = None;
        let mut history_button_rect: Option<egui::Rect> = None;
        let mut downloads_button_rect: Option<egui::Rect> = None;
        let mut screenshot_requested = false;
        let mut print_to_pdf_requested = false;
        let mut address_bar_rect: Option<egui::Rect> = None;
        let mut shortcuts_help_button_rect: Option<egui::Rect> = None;
        // Whether Enter fired this frame (real keypress or headless `ENTER`)
        // — deferred here rather than acted on immediately inside the
        // toolbar closure below, since deciding what Enter *does* (accept
        // the highlighted dropdown row vs. plain-navigate) needs `combined`
        // (the suggestion list), which isn't computed until after this
        // closure returns.
        let mut enter_pressed = false;

        egui::Panel::top("toolbar")
            .exact_size(CHROME_HEIGHT_POINTS)
            .frame(
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| {
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
                    if ui.button("Graph").clicked() {
                        back_to_graph = true;
                    }
                    let node = &self.tabs[&focused_id];
                    // `◀`/`▶` were already a proven-safe Unicode gamble
                    // (confirmed live elsewhere in this app), not a real
                    // problem -- migrated to `egui_phosphor::CARET_LEFT`/
                    // `CARET_RIGHT` anyway once the icon font was already
                    // in place, for the same visual consistency the other
                    // icon migrations this session already have.
                    if ui
                        .add_enabled(node.can_go_back, egui::Button::new(egui_phosphor::regular::CARET_LEFT))
                        .clicked()
                    {
                        go_back = true;
                    }
                    if ui
                        .add_enabled(
                            node.can_go_forward,
                            egui::Button::new(egui_phosphor::regular::CARET_RIGHT),
                        )
                        .clicked()
                    {
                        go_forward = true;
                    }
                    // Was plain text ("Reload") to avoid a refresh-arrows
                    // glyph gamble in this app's own font -- now a real
                    // `egui_phosphor` icon instead, same reasoning as every
                    // other icon migrated once the icon font was already
                    // wired in.
                    if ui
                        .button(egui_phosphor::regular::ARROW_CLOCKWISE)
                        .on_hover_text("Reload")
                        .clicked()
                    {
                        reload_requested = true;
                    }
                    if ui.button(egui_phosphor::regular::HOUSE).on_hover_text("Go to home page").clicked() {
                        home_requested = true;
                    }

                    // Tab identity (was a separate strip above the toolbar,
                    // merged into this single bar): a small loading spinner
                    // or the fetched favicon, right before the engine picker.
                    if node.loading {
                        ui.add(egui::Spinner::new().size(12.0));
                    } else if let Some(tex_id) =
                        self.gpu.as_ref().and_then(|g| g.favicon_id(&focused_id))
                    {
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(
                            tex_id,
                            FAVICON_SIZE,
                        )));
                    }

                    let combo_label = current_kind.map(engine_label).unwrap_or("?");
                    let combo_text = match current_kind {
                        Some(kind) => egui::RichText::new(combo_label).color(engine_color(kind)),
                        None => egui::RichText::new(combo_label),
                    };
                    egui::ComboBox::from_id_salt("engine_picker")
                        .selected_text(combo_text)
                        .show_ui(ui, |ui| {
                            for kind in [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit, EngineKind::Webview2] {
                                let selected = current_kind == Some(kind);
                                ui.horizontal(|ui| {
                                    engine_dot(ui, kind, 8.0);
                                    if ui
                                        .selectable_label(selected, engine_label(kind))
                                        .clicked()
                                        && !selected
                                    {
                                        swap_to = Some(kind);
                                    }
                                });
                            }
                        });

                    // WebView2/WebKit only — see `HostToEngine::SetMuted`'s
                    // doc comment for CEF/Servo's gap. `selectable_label`
                    // gives a pressed look while muted, same minimal
                    // indicator shape the engine picker's own selected row
                    // uses. Icon swaps `SPEAKER_HIGH`/`SPEAKER_X` by state
                    // -- same icon the graph-card audio indicator's own
                    // muted case already uses, so the toolbar toggle and
                    // the graph decoration read as the same concept.
                    // `selectable_label`'s own selected-state styling
                    // (accent background, dark foreground) handles the
                    // icon's contrast automatically, same as any other
                    // selected label here.
                    let node_muted = self.tabs.get(&focused_id).is_some_and(|n| n.muted);
                    let mute_icon = if node_muted {
                        egui_phosphor::regular::SPEAKER_X
                    } else {
                        egui_phosphor::regular::SPEAKER_HIGH
                    };
                    if ui.selectable_label(node_muted, mute_icon).on_hover_text("Muted").clicked() {
                        mute_toggle = true;
                    }
                    // "Mute Others"/"Close Others"/"Pin All"/"Unpin All"
                    // folded into one dropdown -- a later session pass
                    // flagged the toolbar as cramped with all three as
                    // always-visible buttons (confirmed live: text touching
                    // the window edge even at 1300pt wide). Same anchored
                    // Area/Frame popup pattern "Bookmarks (N)"/"History (N)"
                    // already use.
                    let tab_actions_response = ui.button("Tab actions");
                    if tab_actions_response.clicked() {
                        self.tab_actions_open = !self.tab_actions_open;
                    }
                    tab_actions_button_rect = Some(tab_actions_response.rect);

                    // `selectable_label`, same reasoning as `Muted` above --
                    // a pressed look for "already bookmarked", now a real
                    // `BOOKMARK_SIMPLE` icon instead of the plain-text
                    // fallback a star glyph's tofu risk used to require.
                    let is_bookmarked = self
                        .tabs
                        .get(&focused_id)
                        .is_some_and(|n| self.bookmarks.iter().any(|b| b.url == n.url));
                    if ui
                        .selectable_label(is_bookmarked, egui_phosphor::regular::BOOKMARK_SIMPLE)
                        .on_hover_text("Bookmark")
                        .clicked()
                    {
                        toggle_bookmark_requested = true;
                    }
                    if !self.bookmarks.is_empty() {
                        let label = format!("Bookmarks ({})", self.bookmarks.len());
                        let response = ui.button(label);
                        if response.clicked() {
                            self.bookmarks_open = !self.bookmarks_open;
                            if self.bookmarks_open {
                                self.bookmarks_filter.clear();
                            }
                        }
                        bookmarks_button_rect = Some(response.rect);
                    }
                    if !self.history.is_empty() {
                        let label = format!("History ({})", self.history.len());
                        let response = ui.button(label);
                        if response.clicked() {
                            self.history_open = !self.history_open;
                            if self.history_open {
                                self.history_filter.clear();
                            }
                        }
                        history_button_rect = Some(response.rect);
                    }
                    if !self.downloads.is_empty() {
                        let label = format!("Downloads ({})", self.downloads.len());
                        let response = ui.button(label);
                        if response.clicked() {
                            self.downloads_open = !self.downloads_open;
                        }
                        downloads_button_rect = Some(response.rect);
                    }

                    // Full window (chrome + content), same composited
                    // picture the headless `SCREENSHOT` debug command
                    // already captures via `GpuState::render`'s
                    // `screenshot_path` -- this button is that same
                    // mechanism wired to a real default save location
                    // instead of a test-supplied path.
                    if ui
                        .button(egui_phosphor::regular::CAMERA)
                        .on_hover_text("Screenshot")
                        .clicked()
                    {
                        screenshot_requested = true;
                    }
                    // Hidden in headless mode, same reason Export/Import
                    // Session are: a real native file dialog blocks on
                    // OS/human input neither exists there. `PRINTTOPDF
                    // <path>` bypasses it directly for headless testing.
                    if !self.headless
                        && ui.button(egui_phosphor::regular::PRINTER).on_hover_text("Print to PDF").clicked()
                    {
                        print_to_pdf_requested = true;
                    }

                    // Reserve room for the gear button after the address
                    // bar — `desired_width(f32::INFINITY)` would otherwise
                    // claim the whole remaining row, leaving nothing for a
                    // widget added after it in the same horizontal layout.
                    let gear_reserve = 32.0;
                    let text_width = (ui.available_width() - gear_reserve).max(40.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.address_bar_text)
                            .desired_width(text_width)
                            // Match `style.spacing.button_padding` (set in
                            // `apply_style`) so the address bar renders the
                            // same height as the back/forward buttons and
                            // engine picker next to it — `TextEdit`'s own
                            // default margin (`Margin::symmetric(4, 2)`)
                            // doesn't share that style value, so left unset
                            // the row's tallest item (the text field) stood
                            // out with visibly more height than its
                            // neighbors instead of a level toolbar.
                            .margin(egui::Margin::symmetric(8, 4)),
                    );
                    if std::mem::take(&mut self.pending_focus_address_bar) {
                        response.request_focus();
                    }
                    self.address_bar_focused = response.has_focus();
                    address_bar_rect = Some(response.rect);
                    let help_response = ui
                        .button(egui_phosphor::regular::QUESTION)
                        .on_hover_text("Keyboard Shortcuts (F1)");
                    if help_response.clicked() {
                        self.shortcuts_help_open = !self.shortcuts_help_open;
                    }
                    shortcuts_help_button_rect = Some(help_response.rect);
                    if ui.button(egui_phosphor::regular::GEAR).on_hover_text("Settings").clicked() {
                        self.toggle_settings_page();
                    }
                    if response.changed() {
                        self.open_tab_suggestions =
                            self.compute_open_tab_suggestions(&self.address_bar_text, &focused_id);
                        self.suggestion_fetcher.note_text_changed(&self.address_bar_text);
                        // A changed query invalidates whatever row was
                        // highlighted for the *previous* suggestion list —
                        // keeping a stale index could silently highlight
                        // (and Enter-accept) an unrelated row.
                        self.dropdown_highlighted = None;
                    }
                    // `pending_debug_enter` (headless mode's `ENTER` command,
                    // see `debug_control`) doesn't gate on `lost_focus()`:
                    // headless "focus" is set directly by the `ADDRESSBAR`
                    // command, not via a real click-driven egui focus
                    // transfer, so `response.lost_focus()` would never fire
                    // for it.
                    if std::mem::take(&mut self.pending_debug_enter)
                        || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        enter_pressed = true;
                    }
                });
            });

        self.build_shortcuts_help(ui, shortcuts_help_button_rect);

        if self.find_bar_open {
            self.build_find_bar(ui);
        }

        self.build_context_menu(ui);
        self.build_save_toast(ui);

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
            let arrow_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown))
                || std::mem::take(&mut self.pending_debug_arrow_down);
            let arrow_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp))
                || std::mem::take(&mut self.pending_debug_arrow_up);
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
                        egui::Frame::default()
                            .fill(TOOLBAR_FILL)
                            .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
                            .corner_radius(4u8)
                            .inner_margin(egui::Margin::same(4))
                            .show(ui, |ui| {
                                ui.set_width(rect.width());
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
        // `pending_debug_escape` (headless mode's `ESCAPE` command) folds
        // into the same real-Escape check rather than getting its own
        // branch, so headless testing exercises the exact same dismiss-vs-
        // leave-to-graph logic a real keypress would.
        // Neutralized while the quick-switcher is open: it does its own,
        // separate Escape check in `build_quick_switcher` to close itself,
        // and `key_pressed` is a non-consuming peek -- without this, the
        // same Escape press would also fire this chain (leaving to Graph,
        // or dismissing the address-bar dropdown) underneath it.
        let escape_pressed = (ui.input(|i| i.key_pressed(egui::Key::Escape))
            || std::mem::take(&mut self.pending_debug_escape))
            && self.quick_switcher.is_none();
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
            if let Some(node) = self.tabs.get_mut(&focused_id) {
                node.tab.send(&HostToEngine::GoBack);
            }
        }
        if go_forward {
            if let Some(node) = self.tabs.get_mut(&focused_id) {
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
        if let Some(kind) = swap_to {
            self.start_swap_to(&focused_id, kind);
        }
        if mute_toggle {
            self.toggle_muted(&focused_id);
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
        // gap while closed having gone untracked. The underlying data
        // (self.bookmarks/history/downloads, the toolbar buttons) all
        // outlive the `_open` flag going false, so drawing continues
        // correctly through the fade-out too, not just fade-in.
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
        let tab_actions_fade =
            ui.ctx().animate_bool(egui::Id::new("tab_actions_popup_fade"), self.tab_actions_open);
        if tab_actions_fade > 0.0 {
            if let Some(rect) = tab_actions_button_rect {
                self.build_tab_actions_popup(ui, rect, &focused_id, tab_actions_fade);
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
        if let Some(text) = navigate {
            let url = resolve_navigation_url(&text, self.settings.default_search_engine);
            self.navigate_tab(&focused_id, url);
            self.open_tab_suggestions.clear();
            self.search_suggestions.clear();
            self.debug_address_bar_pin = false;
            self.dropdown_highlighted = None;
        }
    }

    fn build_graph_ui(&mut self, ui: &mut egui::Ui) {
        let mut new_tab_parent: Option<Option<String>> = None;
        let mut clicked_focus: Option<String> = None;
        let mut close_requested: Option<String> = None;
        let mut download_close_requested: Option<String> = None;
        let mut download_clicked: Option<String> = None;
        let mut pin_toggle_requested: Option<String> = None;
        let mut remove_from_group_requested: Option<String> = None;
        let mut tab_color_cycle_requested: Option<String> = None;
        let mut mute_toggle_requested: Option<String> = None;
        let mut new_group_requested = false;
        let mut leave_group_requested = false;
        let mut group_close_requested: Option<String> = None;
        let mut color_cycle_requested: Option<String> = None;
        let mut group_merge_requested: Option<(String, String)> = None;
        let mut group_entered: Option<String> = None;
        let mut rename_start: Option<(String, String)> = None;
        let mut rename_commit: Option<String> = None;
        let mut rename_cancel = false;

        let mut reset_view = false;
        let mut group_selected_requested = false;
        let mut close_selected_requested = false;
        let mut ungroup_all_requested = false;
        let mut import_group_requested = false;
        let mut shortcuts_help_button_rect: Option<egui::Rect> = None;
        let mut recently_closed_button_rect: Option<egui::Rect> = None;
        let mut leave_group_button_rect: Option<egui::Rect> = None;
        egui::Panel::top("graph_toolbar")
            .exact_size(GRAPH_TOOLBAR_HEIGHT_POINTS)
            .frame(
                egui::Frame::default()
                    .fill(TOOLBAR_FILL)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if let Some(group_id) = &self.current_group {
                        // Inside a group's own subgraph: a way back takes
                        // the place of "+ New Group" (no nesting in this
                        // first cut — see `GroupNode`'s doc comment), and
                        // the group's name replaces the empty space "+ New
                        // Tab"/"Reset View" already fill at the top level.
                        let leave_group_response = ui.button("\u{25c0} Groups");
                        if leave_group_response.clicked() {
                            leave_group_requested = true;
                        }
                        leave_group_button_rect = Some(leave_group_response.rect);
                        let is_renaming =
                            self.renaming_group.as_ref().is_some_and(|(id, _)| id == group_id);
                        if is_renaming {
                            if let Some((_, draft)) = &mut self.renaming_group {
                                let response = ui
                                    .add(egui::TextEdit::singleline(draft).desired_width(140.0));
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
                        } else {
                            let name = self
                                .groups
                                .get(group_id)
                                .map(|g| g.name.clone())
                                .unwrap_or_default();
                            if ui.button(&name).on_hover_text("Click to rename").clicked() {
                                rename_start = Some((group_id.clone(), name));
                            }
                        }
                    }
                    if ui.button("+ New Tab").clicked() {
                        new_tab_parent = Some(self.focused.clone());
                    }
                    // These four buttons used to carry full text labels
                    // ("+ New Group", "Import Group...", "Ungroup All",
                    // "Collapse Groups"/"Expand Groups") and, together with
                    // "+ New Tab"/"Reset View", made this toolbar cramped
                    // enough to confirm live (text touching the window
                    // edge at 1300pt wide). Switched to `egui_phosphor`
                    // icons instead of folding into a dropdown -- each
                    // stays a one-click action, just narrower, with the
                    // old label preserved as a hover tooltip so the action
                    // is still discoverable.
                    if self.current_group.is_none()
                        && ui
                            .button(egui_phosphor::regular::FOLDER_PLUS)
                            .on_hover_text("New Group")
                            .clicked()
                    {
                        new_group_requested = true;
                    }
                    // Same "real file dialog blocks on OS/human input
                    // neither exists in headless mode" reasoning as every
                    // other Export/Import button -- `IMPORTGROUP <path>`
                    // calls the same underlying import directly instead.
                    if self.current_group.is_none()
                        && !self.headless
                        && ui
                            .button(egui_phosphor::regular::UPLOAD_SIMPLE)
                            .on_hover_text("Import Group...")
                            .clicked()
                    {
                        import_group_requested = true;
                    }
                    if self.current_group.is_none()
                        && !self.group_order.is_empty()
                        && ui
                            .button(egui_phosphor::regular::FOLDER_MINUS)
                            .on_hover_text("Ungroup All")
                            .clicked()
                    {
                        ungroup_all_requested = true;
                    }
                    if self.current_group.is_none() && !self.group_order.is_empty() {
                        let (icon, hover) = if self.groups_collapsed {
                            (egui_phosphor::regular::CORNERS_OUT, "Expand Groups")
                        } else {
                            (egui_phosphor::regular::CORNERS_IN, "Collapse Groups")
                        };
                        if ui.button(icon).on_hover_text(hover).clicked() {
                            self.groups_collapsed = !self.groups_collapsed;
                        }
                    }
                    if ui.button("Reset View").clicked() {
                        reset_view = true;
                    }
                    ui.label(format!("{}%", self.graph_zoom_pct.round() as i32));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.graph_filter)
                            .desired_width(120.0)
                            .hint_text("Filter..."),
                    );
                    if !self.closed_tabs.is_empty() {
                        let label = format!("Recently Closed ({})", self.closed_tabs.len());
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
                        // Matches "+ New Group" above: no nesting in this
                        // first cut, so grouping is a top-level-only action.
                        // Rectangle-select itself and closing a selection
                        // both still work fine inside a group's own subgraph.
                        if self.current_group.is_none()
                            && ui.button(format!("Group Selected ({})", self.selected_tabs.len())).clicked()
                        {
                            group_selected_requested = true;
                        }
                        if ui
                            .button(format!("Close Selected ({})", self.selected_tabs.len()))
                            .clicked()
                        {
                            close_selected_requested = true;
                        }
                    }
                    if let Some(err) = &self.tab_spawn_error {
                        ui.colored_label(egui::Color32::from_rgb(220, 60, 60), err);
                    }
                    let help_response = ui
                        .button(egui_phosphor::regular::QUESTION)
                        .on_hover_text("Keyboard Shortcuts (F1)");
                    if help_response.clicked() {
                        self.shortcuts_help_open = !self.shortcuts_help_open;
                    }
                    shortcuts_help_button_rect = Some(help_response.rect);
                    if ui.button(egui_phosphor::regular::GEAR).on_hover_text("Settings").clicked() {
                        self.toggle_settings_page();
                    }
                });
            });
        self.build_shortcuts_help(ui, shortcuts_help_button_rect);
        let recently_closed_fade = ui
            .ctx()
            .animate_bool(egui::Id::new("recently_closed_popup_fade"), self.recently_closed_open);
        if recently_closed_fade > 0.0 {
            if let Some(rect) = recently_closed_button_rect {
                self.build_recently_closed_popup(ui, rect, recently_closed_fade);
            }
        }
        self.build_group_context_menu(ui);
        self.build_download_context_menu(ui);
        self.build_tab_context_menu(ui);
        if reset_view {
            self.graph_scene_rect = self.fit_all_nodes_scene_rect();
        }

        if self.tabs.is_empty() {
            ui.centered_and_justified(|ui| {
                match &self.tab_spawn_error {
                    Some(err) => ui.colored_label(
                        egui::Color32::from_rgb(220, 60, 60),
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
            let current_key = self.current_level_settle_key();
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
            // Escape clears a rectangle-select selection, dismisses a
            // group's context menu, and dismisses a hover-peek preview
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
                self.group_context_menu = None;
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
            if let Some(pos) = pointer_pos.filter(|p| scene_outer_rect.contains(*p)) {
                // A press landing on a group's own card opens its context
                // menu instead of starting a marquee -- groups aren't
                // marquee-selectable (only tabs are, see
                // `nearest_tab_in_direction`'s own doc comment on why
                // `self.focused` stays tab-only), so a right-click there
                // would otherwise just compute an empty selection.
                let world_pos = ((pos.to_vec2() - translation) / zoom_scale).to_pos2();
                let hit_group = self.current_group.is_none()
                    .then(|| {
                        self.groups.iter().find_map(|(gid, g)| {
                            egui::Rect::from_center_size(egui::Pos2::new(g.pos.x, g.pos.y), GROUP_NODE_SIZE)
                                .contains(world_pos)
                                .then(|| gid.clone())
                        })
                    })
                    .flatten();
                let hit_download = hit_group.is_none()
                    .then(|| {
                        self.downloads_at_current_level().into_iter().find(|did| {
                            self.downloads.get(did).is_some_and(|d| {
                                egui::Rect::from_center_size(egui::Pos2::new(d.pos.x, d.pos.y), DOWNLOAD_NODE_SIZE)
                                    .contains(world_pos)
                            })
                        })
                    })
                    .flatten();
                let hit_tab = (hit_group.is_none() && hit_download.is_none())
                    .then(|| {
                        self.tabs_at_current_level().into_iter().find(|tid| {
                            self.tabs.get(tid).is_some_and(|n| {
                                egui::Rect::from_center_size(egui::Pos2::new(n.pos.x, n.pos.y), GRAPH_NODE_SIZE)
                                    .contains(world_pos)
                            })
                        })
                    })
                    .flatten();
                if let Some(gid) = hit_group {
                    self.group_context_menu = Some((gid, pos));
                    self.download_context_menu = None;
                    self.tab_context_menu = None;
                    self.rect_select_start = None;
                } else if let Some(did) = hit_download {
                    self.download_context_menu = Some((did, pos));
                    self.group_context_menu = None;
                    self.tab_context_menu = None;
                    self.rect_select_start = None;
                } else if let Some(tid) = hit_tab {
                    self.tab_context_menu = Some((tid, pos));
                    self.group_context_menu = None;
                    self.download_context_menu = None;
                    self.rect_select_start = None;
                } else {
                    self.group_context_menu = None;
                    self.download_context_menu = None;
                    self.tab_context_menu = None;
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
                let world_rect = egui::Rect::from_two_pos(
                    ((screen_rect.min.to_vec2() - translation) / zoom_scale).to_pos2(),
                    ((screen_rect.max.to_vec2() - translation) / zoom_scale).to_pos2(),
                );
                self.selected_tabs = self
                    .tabs_at_current_level()
                    .into_iter()
                    .filter(|id| {
                        self.tabs.get(id).is_some_and(|node| {
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
        let world_bounds = self.fit_all_nodes_scene_rect();
        let minimap_scale = (minimap_rect.width() / world_bounds.width().max(1.0))
            .min(minimap_rect.height() / world_bounds.height().max(1.0));
        let world_to_minimap = |p: egui::Pos2| -> egui::Pos2 {
            minimap_rect.center() + (p - world_bounds.center()) * minimap_scale
        };
        overlay_painter.rect_filled(minimap_rect, 4.0, egui::Color32::from_black_alpha(160));
        for id in self.tabs_at_current_level() {
            if let Some(node) = self.tabs.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 2.0, ui.visuals().text_color());
            }
        }
        for id in self.downloads_at_current_level() {
            if let Some(node) = self.downloads.get(&id) {
                let p = world_to_minimap(egui::Pos2::new(node.pos.x, node.pos.y));
                overlay_painter.circle_filled(p, 2.0, ui.visuals().weak_text_color());
            }
        }
        if self.current_group.is_none() {
            for node in self.groups.values() {
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
        let mut scene_rect = self.graph_scene_rect;
        egui::Scene::new()
            .zoom_range(GRAPH_ZOOM_MIN..=GRAPH_ZOOM_MAX)
            .drag_pan_buttons(egui::DragPanButtons::PRIMARY)
            .show(ui, &mut scene_rect, |ui| {
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
                for (id, node) in &self.tabs {
                    if node.group != self.current_group {
                        continue;
                    }
                    let Some(parent_id) = &node.parent else {
                        continue;
                    };
                    let Some(parent) = self.tabs.get(parent_id).filter(|p| p.group == self.current_group) else {
                        continue;
                    };
                    let _ = id;
                    draw_graph_edge(
                        ui.painter(),
                        egui::Pos2::new(parent.pos.x, parent.pos.y),
                        egui::Pos2::new(node.pos.x, node.pos.y),
                        egui::Stroke::new(2.0 * inv_zoom, ui.visuals().weak_text_color()),
                    );
                }
                // Download nodes are always parented to a tab (never to
                // another download), so the same `self.tabs` lookup works.
                for node in self.downloads.values() {
                    if node.group != self.current_group {
                        continue;
                    }
                    let Some(parent_id) = &node.parent else {
                        continue;
                    };
                    let Some(parent) = self.tabs.get(parent_id).filter(|p| p.group == self.current_group) else {
                        continue;
                    };
                    draw_graph_edge(
                        ui.painter(),
                        egui::Pos2::new(parent.pos.x, parent.pos.y),
                        egui::Pos2::new(node.pos.x, node.pos.y),
                        egui::Stroke::new(2.0 * inv_zoom, ui.visuals().weak_text_color()),
                    );
                }

                for id in self.tabs_at_current_level() {
                    let Some(node_pos) = self.tabs.get(&id).map(|n| n.pos) else {
                        continue;
                    };
                    let center_pos = egui::Pos2::new(node_pos.x, node_pos.y);
                    let rect = egui::Rect::from_center_size(center_pos, GRAPH_NODE_SIZE);
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("graph_node", id.as_str())),
                        egui::Sense::click_and_drag(),
                    );

                    let is_focused = self.focused.as_deref() == Some(id.as_str());
                    let is_hovered = self.hovered.as_deref() == Some(id.as_str()) || response.hovered();
                    let is_selected = self.selected_tabs.contains(&id);
                    let tag_color = self.tabs.get(&id).and_then(|n| n.color);

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
                    draw_card_glow(ui.painter(), rect, 6.0, border_color, inv_zoom, 1.0 + hover_t * 0.8);
                    ui.painter().rect_filled(rect, 6.0 * inv_zoom, card_fill);
                    ui.painter().rect_stroke(
                        rect,
                        6.0 * inv_zoom,
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
                            6.0 * inv_zoom,
                            egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 45),
                        );
                    }

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

                    if let Some(node) = self.tabs.get(&id) {
                        let label = if node.title.is_empty() {
                            node.url.as_str()
                        } else {
                            node.title.as_str()
                        };
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
                            engine_color(node.tab.kind),
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
                            let label = truncate_to_width(ui.ctx(), label, &label_font, label_max_width);
                            overlay_painter.text(
                                project(egui::Pos2::new(title_strip.left(), title_strip.center().y))
                                    + egui::vec2(16.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                label,
                                label_font,
                                egui::Color32::WHITE,
                            );
                        }
                    }
                    if let Some(tex_id) = self.gpu.as_ref().and_then(|g| g.favicon_id(&id)) {
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
                    // Audio-playing/muted indicator: a plain colored dot
                    // (like the engine-identity one below the thumbnail),
                    // not a speaker glyph — confirmed elsewhere in this
                    // project that several Unicode symbol glyphs render as
                    // empty boxes in this app's font, so a painter-drawn
                    // shape is the reliable choice, not text. `audio_playing`
                    // is only ever true for WebView2/WebKit tabs — see
                    // `TabNode::audio_playing`'s doc comment — but the dot
                    // also shows (and stays clickable) whenever `muted` is
                    // set, even after audio stops, so there's always a way
                    // to see and undo a mute from Graph view without first
                    // switching to Tab view.
                    let node_muted = self.tabs.get(&id).is_some_and(|n| n.muted);
                    let node_audio_playing = self.tabs.get(&id).is_some_and(|n| n.audio_playing);
                    let mut audio_clicked = false;
                    if node_audio_playing || node_muted {
                        let audio_rect = egui::Rect::from_center_size(
                            rect.right_top() + egui::vec2(-8.0, 8.0) * inv_zoom,
                            egui::vec2(16.0, 16.0) * inv_zoom,
                        );
                        let audio_response = ui.interact(
                            audio_rect,
                            egui::Id::new(("graph_node_audio", id.as_str())),
                            egui::Sense::click(),
                        );
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
                            overlay_painter.text(
                                project(audio_rect.center()),
                                egui::Align2::CENTER_CENTER,
                                egui_phosphor::regular::SPEAKER_X,
                                egui::FontId::proportional(12.0),
                                egui::Color32::from_rgb(220, 60, 60),
                            );
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
                            let waveform_color = egui::Color32::from_rgb(80, 200, 120);
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
                    // see `TabNode::pinned`'s own doc comment. A real
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
                    let node_pinned = self.tabs.get(&id).is_some_and(|n| n.pinned);
                    let pin_rect =
                        egui::Rect::from_center_size(rect.left_bottom(), egui::vec2(16.0, 16.0) * inv_zoom);
                    let pin_response = ui.interact(
                        pin_rect,
                        egui::Id::new(("graph_node_pin", id.as_str())),
                        egui::Sense::click(),
                    );
                    let pin_color = if node_pinned {
                        ACCENT
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    overlay_painter.text(
                        project(pin_rect.center()),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::regular::PUSH_PIN,
                        egui::FontId::proportional(14.0),
                        pin_color,
                    );
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
                        let close_response = ui.interact(
                            close_rect,
                            egui::Id::new(("graph_node_close", id.as_str())),
                            egui::Sense::click(),
                        );
                        let close_fill = if close_response.hovered() {
                            egui::Color32::from_rgb(220, 60, 60)
                        } else {
                            ui.visuals().extreme_bg_color
                        };
                        ui.painter().rect_filled(close_rect, 4.0 * inv_zoom, close_fill);
                        ui.painter().rect_stroke(
                            close_rect,
                            4.0 * inv_zoom,
                            egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color()),
                            egui::StrokeKind::Inside,
                        );
                        overlay_painter.text(
                            project(close_rect.center()),
                            egui::Align2::CENTER_CENTER,
                            egui_phosphor::regular::X,
                            egui::FontId::proportional(14.0),
                            ui.visuals().text_color(),
                        );
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
                        let eject_response = ui.interact(
                            eject_rect,
                            egui::Id::new(("graph_node_eject", id.as_str())),
                            egui::Sense::click(),
                        );
                        let eject_fill = if eject_response.hovered() {
                            ACCENT
                        } else {
                            ui.visuals().extreme_bg_color
                        };
                        ui.painter().rect_filled(eject_rect, 4.0 * inv_zoom, eject_fill);
                        ui.painter().rect_stroke(
                            eject_rect,
                            4.0 * inv_zoom,
                            egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color()),
                            egui::StrokeKind::Inside,
                        );
                        // `SIGN_OUT` rather than the bare "^" this used to
                        // be -- migrated for the same reason the pin dot
                        // was: a caret is genuinely ambiguous here (reads
                        // as "expand" as easily as "leave"), where a real
                        // "step out" icon states its own meaning.
                        overlay_painter.text(
                            project(eject_rect.center()),
                            egui::Align2::CENTER_CENTER,
                            egui_phosphor::regular::SIGN_OUT,
                            egui::FontId::proportional(14.0),
                            ui.visuals().text_color(),
                        );
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
                    let swatch_response = ui.interact(
                        swatch_rect,
                        egui::Id::new(("graph_node_swatch", id.as_str())),
                        egui::Sense::click(),
                    );
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
                        if let Some(node) = self.tabs.get_mut(&id) {
                            node.pos += response.drag_delta();
                        }
                    } else if self.dragging.as_deref() == Some(id.as_str()) {
                        self.dragging = None;
                        // Dropping a tab onto a group's card joins that
                        // group -- only meaningful at the top level, where
                        // tabs and groups are rendered (and can overlap)
                        // together. See `GroupNode`'s own doc comment.
                        if self.current_group.is_none() {
                            if let Some(tab_pos) = self.tabs.get(&id).map(|n| n.pos) {
                                let dropped_on_group = self.groups.iter().find_map(|(gid, g)| {
                                    egui::Rect::from_center_size(
                                        egui::Pos2::new(g.pos.x, g.pos.y),
                                        GROUP_NODE_SIZE,
                                    )
                                    .contains(egui::Pos2::new(tab_pos.x, tab_pos.y))
                                    .then(|| gid.clone())
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
                                            if let Some(node) = self.tabs.get_mut(&tab_id) {
                                                node.group = Some(group_id.clone());
                                            }
                                        }
                                    } else if let Some(node) = self.tabs.get_mut(&id) {
                                        node.group = Some(group_id);
                                    }
                                } else {
                                    // Not dropped on a group -- check
                                    // another ungrouped tab's card instead,
                                    // creating a new group with both.
                                    // `tabs_at_current_level()` only ever
                                    // returns ungrouped tabs at the top
                                    // level (its own filter is `n.group ==
                                    // self.current_group`, i.e. `None`
                                    // here), so anything it finds is
                                    // already a valid drop target with no
                                    // extra check needed.
                                    let dropped_on_tab =
                                        self.tabs_at_current_level().into_iter().find(|other_id| {
                                            other_id != &id
                                                && self.tabs.get(other_id).is_some_and(|other| {
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
                                            self.tabs.get(&other_id).map(|n| n.pos).unwrap_or(tab_pos);
                                        let name = format!("Group {}", self.groups.len() + 1);
                                        self.groups.insert(
                                            group_id.clone(),
                                            GroupNode { name, pos, vel: egui::Vec2::ZERO, color: None },
                                        );
                                        self.group_order.push(group_id.clone());
                                        if let Some(node) = self.tabs.get_mut(&other_id) {
                                            node.group = Some(group_id.clone());
                                        }
                                        if let Some(node) = self.tabs.get_mut(&id) {
                                            node.group = Some(group_id);
                                        }
                                    }
                                }
                            }
                        } else if leave_group_button_rect
                            .is_some_and(|rect| ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| rect.contains(p)))
                        {
                            // Dragging a group member onto the "◀ Groups"
                            // back button ejects it to the top level --
                            // the button's rect is screen-space (it's a
                            // toolbar widget), so this checks the raw
                            // pointer position directly rather than the
                            // node's world-space `pos` the top-level
                            // group-card/tab-card drop targets above use.
                            // Same effect as the hover-revealed eject
                            // button, just reached by dragging the card
                            // itself out through the way back instead.
                            remove_from_group_requested = Some(id.clone());
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

                    // Graph toolbar filter: painted last (on the same
                    // untransformed foreground layer the text above used,
                    // so it covers that too, not just the thumbnail/
                    // borders below it) rather than threading a dim factor
                    // through every draw call above -- one extra scrim
                    // covers the whole card in one shot.
                    if let Some(node) = self.tabs.get(&id) {
                        if graph_filter_dims(&self.graph_filter, &[&node.title, &node.url]) {
                            overlay_painter.rect_filled(
                                egui::Rect::from_min_max(project(rect.min), project(rect.max)),
                                6.0,
                                egui::Color32::from_black_alpha(190),
                            );
                        }
                    }
                }

                // Download nodes: no thumbnail/title-strip/audio-dot
                // machinery (no live content behind them), just a card with
                // the filename and a click-to-reveal/close affordance,
                // reusing the same interact/hover/drag pattern as tab nodes.
                for id in self.downloads_at_current_level() {
                    let Some(node_pos) = self.downloads.get(&id).map(|n| n.pos) else {
                        continue;
                    };
                    let center_pos = egui::Pos2::new(node_pos.x, node_pos.y);
                    let rect = egui::Rect::from_center_size(center_pos, DOWNLOAD_NODE_SIZE);
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("graph_download_node", id.as_str())),
                        egui::Sense::click_and_drag(),
                    );

                    let is_hovered = self.hovered.as_deref() == Some(id.as_str()) || response.hovered();
                    let card_fill = if is_hovered {
                        ui.visuals().extreme_bg_color.gamma_multiply(1.35)
                    } else {
                        ui.visuals().extreme_bg_color
                    };
                    ui.painter().rect_filled(rect, 6.0 * inv_zoom, card_fill);
                    let (border_width, border_color) = if is_hovered {
                        (1.5, ui.visuals().text_color())
                    } else {
                        (1.0, ui.visuals().weak_text_color())
                    };
                    ui.painter().rect_stroke(
                        rect,
                        6.0 * inv_zoom,
                        egui::Stroke::new(border_width * inv_zoom, border_color),
                        egui::StrokeKind::Inside,
                    );

                    if let Some(node) = self.downloads.get(&id) {
                        let filename = download_filename(&node.path);
                        // `egui_phosphor::DOWNLOAD_SIMPLE` -- was a raw
                        // Unicode arrow-to-bar glyph (`\u{2913}`) that
                        // happened to render, despite this comment's own
                        // claim of avoiding exactly that gamble; migrated
                        // to a real icon for consistency once the icon
                        // font was already in place for everything else.
                        overlay_painter.text(
                            project(rect.center_top()) + egui::vec2(0.0, 10.0),
                            egui::Align2::CENTER_TOP,
                            egui_phosphor::regular::DOWNLOAD_SIMPLE,
                            egui::FontId::proportional(18.0),
                            ui.visuals().text_color(),
                        );
                        // Below `MIN_LABEL_CARD_SCREEN_WIDTH`, skip the
                        // filename entirely rather than truncating to a
                        // near-illegible sliver -- the download-icon glyph
                        // above still marks the node as a download on its
                        // own.
                        if rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                            let filename_font = egui::FontId::proportional(14.0);
                            let filename_max_width = (rect.width() * zoom_scale - 12.0).max(0.0);
                            let filename =
                                truncate_to_width(ui.ctx(), filename, &filename_font, filename_max_width);
                            overlay_painter.text(
                                project(rect.center_bottom()) + egui::vec2(0.0, -8.0),
                                egui::Align2::CENTER_BOTTOM,
                                filename,
                                filename_font,
                                ui.visuals().text_color(),
                            );
                        }
                    }

                    if is_hovered {
                        let close_rect =
                            egui::Rect::from_center_size(rect.right_top(), egui::vec2(18.0, 18.0) * inv_zoom);
                        let close_response = ui.interact(
                            close_rect,
                            egui::Id::new(("graph_download_node_close", id.as_str())),
                            egui::Sense::click(),
                        );
                        let close_fill = if close_response.hovered() {
                            egui::Color32::from_rgb(220, 60, 60)
                        } else {
                            ui.visuals().extreme_bg_color
                        };
                        ui.painter().rect_filled(close_rect, 4.0 * inv_zoom, close_fill);
                        ui.painter().rect_stroke(
                            close_rect,
                            4.0 * inv_zoom,
                            egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color()),
                            egui::StrokeKind::Inside,
                        );
                        overlay_painter.text(
                            project(close_rect.center()),
                            egui::Align2::CENTER_CENTER,
                            egui_phosphor::regular::X,
                            egui::FontId::proportional(14.0),
                            ui.visuals().text_color(),
                        );
                        if close_response.clicked() {
                            download_close_requested = Some(id.clone());
                        }
                    }

                    if response.dragged() {
                        self.dragging = Some(id.clone());
                        if let Some(node) = self.downloads.get_mut(&id) {
                            node.pos += response.drag_delta();
                        }
                    } else if self.dragging.as_deref() == Some(id.as_str()) {
                        self.dragging = None;
                    }
                    if response.hovered() {
                        self.hovered = Some(id.clone());
                    } else if self.hovered.as_deref() == Some(id.as_str()) {
                        self.hovered = None;
                    }
                    if response.clicked() {
                        download_clicked = Some(id.clone());
                    }

                    if let Some(node) = self.downloads.get(&id) {
                        if graph_filter_dims(&self.graph_filter, &[&node.path]) {
                            overlay_painter.rect_filled(
                                egui::Rect::from_min_max(project(rect.min), project(rect.max)),
                                6.0,
                                egui::Color32::from_black_alpha(190),
                            );
                        }
                    }
                }

                // Group nodes: top level only (no nesting in this first
                // cut — see `GroupNode`'s own doc comment). Double-click
                // drills into the group's own subgraph; a dragged tab
                // dropped onto one joins it (handled above, in the tab
                // loop, since that's where the drag-release fires).
                if self.current_group.is_none() {
                    for id in self.group_order.clone() {
                        let Some(node_pos) = self.groups.get(&id).map(|g| g.pos) else {
                            continue;
                        };
                        let center_pos = egui::Pos2::new(node_pos.x, node_pos.y);
                        let rect = egui::Rect::from_center_size(center_pos, GROUP_NODE_SIZE);
                        let response = ui.interact(
                            rect,
                            egui::Id::new(("graph_group_node", id.as_str())),
                            egui::Sense::click_and_drag(),
                        );

                        let is_hovered = self.hovered.as_deref() == Some(id.as_str()) || response.hovered();
                        let tag_color = self.groups.get(&id).and_then(|g| g.color);
                        let member_ids: Vec<String> = self
                            .tabs
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
                            tag_color.unwrap_or(if overloaded { GROUP_OVERLOAD_COLOR } else { ACCENT });
                        draw_card_glow(ui.painter(), rect, 6.0, border_color, inv_zoom, 1.0 + hover_t * 0.8);
                        ui.painter().rect_filled(rect, 6.0 * inv_zoom, card_fill);
                        ui.painter().rect_stroke(
                            rect,
                            6.0 * inv_zoom,
                            egui::Stroke::new((1.0 + hover_t * 0.5) * inv_zoom, border_color),
                            egui::StrokeKind::Inside,
                        );

                        if let Some(node) = self.groups.get(&id) {
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
                                        overlay_painter.text(
                                            project(grid_rect.right_bottom()) + egui::vec2(-3.0, -3.0),
                                            egui::Align2::RIGHT_BOTTOM,
                                            format!("+{}", member_count - 4),
                                            egui::FontId::proportional(12.0),
                                            egui::Color32::WHITE,
                                        );
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
                                    let name =
                                        truncate_to_width(ui.ctx(), &node.name, &name_font, name_max_width);
                                    overlay_painter.text(
                                        project(title_strip.center()),
                                        egui::Align2::CENTER_CENTER,
                                        name,
                                        name_font,
                                        egui::Color32::WHITE,
                                    );
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
                                let name =
                                    truncate_to_width(ui.ctx(), &node.name, &name_font, name_max_width);
                                overlay_painter.text(
                                    project(rect.center_top()) + egui::vec2(0.0, 14.0),
                                    egui::Align2::CENTER_TOP,
                                    name,
                                    name_font,
                                    ui.visuals().text_color(),
                                );
                                let count_label = format!(
                                    "{} tab{}",
                                    member_count,
                                    if member_count == 1 { "" } else { "s" }
                                );
                                overlay_painter.text(
                                    project(rect.center_bottom()) + egui::vec2(0.0, -10.0),
                                    egui::Align2::CENTER_BOTTOM,
                                    count_label,
                                    egui::FontId::proportional(14.0),
                                    ui.visuals().weak_text_color(),
                                );
                            }
                        }

                        if is_hovered {
                            let close_rect = egui::Rect::from_center_size(
                                rect.right_top(),
                                egui::vec2(18.0, 18.0) * inv_zoom,
                            );
                            let close_response = ui.interact(
                                close_rect,
                                egui::Id::new(("graph_group_node_close", id.as_str())),
                                egui::Sense::click(),
                            );
                            let close_fill = if close_response.hovered() {
                                egui::Color32::from_rgb(220, 60, 60)
                            } else {
                                ui.visuals().extreme_bg_color
                            };
                            ui.painter().rect_filled(close_rect, 4.0 * inv_zoom, close_fill);
                            ui.painter().rect_stroke(
                                close_rect,
                                4.0 * inv_zoom,
                                egui::Stroke::new(1.0 * inv_zoom, ui.visuals().weak_text_color()),
                                egui::StrokeKind::Inside,
                            );
                            overlay_painter.text(
                                project(close_rect.center()),
                                egui::Align2::CENTER_CENTER,
                                egui_phosphor::regular::X,
                                egui::FontId::proportional(14.0),
                                ui.visuals().text_color(),
                            );
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
                        let swatch_response = ui.interact(
                            swatch_rect,
                            egui::Id::new(("graph_group_node_swatch", id.as_str())),
                            egui::Sense::click(),
                        );
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
                            if let Some(node) = self.groups.get_mut(&id) {
                                node.pos += response.drag_delta();
                            }
                        } else if self.dragging.as_deref() == Some(id.as_str()) {
                            self.dragging = None;
                            // Dropping one group's card onto another merges
                            // the dragged group's members into the target
                            // and removes the dragged group -- same
                            // "drop onto a card" language a tab already
                            // uses to join a group.
                            if let Some(dragged_pos) = self.groups.get(&id).map(|g| g.pos) {
                                let dropped_on_group = self.groups.iter().find_map(|(other_id, other)| {
                                    if other_id == &id {
                                        return None;
                                    }
                                    egui::Rect::from_center_size(
                                        egui::Pos2::new(other.pos.x, other.pos.y),
                                        GROUP_NODE_SIZE,
                                    )
                                    .contains(egui::Pos2::new(dragged_pos.x, dragged_pos.y))
                                    .then(|| other_id.clone())
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

                        if let Some(node) = self.groups.get(&id) {
                            if graph_filter_dims(&self.graph_filter, &[&node.name]) {
                                overlay_painter.rect_filled(
                                    egui::Rect::from_min_max(project(rect.min), project(rect.max)),
                                    6.0,
                                    egui::Color32::from_black_alpha(190),
                                );
                            }
                        }
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
                        let node_pos = self.tabs.get(&hovered_id).map(|n| n.pos);
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
            });
        self.graph_scene_rect = scene_rect;

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
            if self.tabs.contains_key(&id) {
                self.selected_tabs.clear();
                self.start_transition_to_tab(&id);
            }
        }
        if let Some(id) = download_close_requested {
            self.remove_download_node(&id);
        }
        if let Some(id) = download_clicked {
            self.reveal_download(&id);
        }
        if let Some(id) = pin_toggle_requested {
            if let Some(node) = self.tabs.get_mut(&id) {
                node.pinned = !node.pinned;
            }
        }
        if let Some(id) = remove_from_group_requested {
            if let Some(node) = self.tabs.get_mut(&id) {
                node.group = None;
            }
        }
        if let Some(id) = tab_color_cycle_requested {
            if let Some(node) = self.tabs.get_mut(&id) {
                node.color = next_tag_color(node.color);
            }
        }
        if let Some(id) = mute_toggle_requested {
            self.toggle_muted(&id);
        }
        if new_group_requested {
            self.new_group_shortcut();
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
                .filter_map(|id| self.tabs.get(id))
                .fold(egui::Vec2::ZERO, |acc, node| acc + node.pos);
            let pos = sum / count;
            let name = format!("Group {}", self.groups.len() + 1);
            self.groups.insert(id.clone(), GroupNode { name, pos, vel: egui::Vec2::ZERO, color: None });
            self.group_order.push(id.clone());
            for tab_id in self.selected_tabs.drain() {
                if let Some(node) = self.tabs.get_mut(&tab_id) {
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
            for id in std::mem::take(&mut self.group_order) {
                self.dissolve_group(&id);
            }
        }
        if import_group_requested {
            if let Some(path) = rfd::FileDialog::new().add_filter("Group", &["json"]).pick_file() {
                if let Some(export) = group_export::load_from(&path) {
                    self.import_group_export(export);
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
                    if let Some(node) = self.groups.get_mut(&id) {
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
            self.download_context_menu = None;
            self.tab_context_menu = None;
        }
        if leave_group_requested {
            self.current_group = None;
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
            self.download_context_menu = None;
            self.tab_context_menu = None;
        }
        if let Some(id) = group_close_requested {
            self.dissolve_group(&id);
        }
        if let Some(id) = color_cycle_requested {
            if let Some(node) = self.groups.get_mut(&id) {
                node.color = next_tag_color(node.color);
            }
        }
        if let Some((source_id, target_id)) = group_merge_requested {
            for node in self.tabs.values_mut() {
                if node.group.as_deref() == Some(source_id.as_str()) {
                    node.group = Some(target_id.clone());
                }
            }
            for node in self.downloads.values_mut() {
                if node.group.as_deref() == Some(source_id.as_str()) {
                    node.group = Some(target_id.clone());
                }
            }
            self.groups.remove(&source_id);
            self.group_order.retain(|g| g != &source_id);
        }
        if let Some((id, draft)) = rename_start {
            self.renaming_group = Some((id, draft));
            self.pending_focus_rename = true;
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes().with_title("multiengine-browser");
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
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
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
            self.spawn_root_tab();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Debug(cmd) => self.handle_debug_command(event_loop, cmd),
            UserEvent::TabSpawned(outcome) => self.handle_tab_spawn_result(outcome),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed_by_egui = match (&mut self.egui_state, &self.window) {
            (Some(state), Some(window)) => state.on_window_event(window.as_ref(), &event).consumed,
            _ => false,
        };
        let in_tab_view = matches!(self.view_mode, ViewMode::Tab);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                if let Some(id) = self.focused.clone() {
                    self.send_resize_to(&id);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                let chrome_height = self.chrome_height_px() as f64;
                if in_tab_view && !consumed_by_egui && position.y >= chrome_height {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.tabs.get_mut(&id) {
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
                // the right button specifically so a right-click on tab
                // content still reaches `InputEvent::MouseDown`/`MouseUp`
                // the same way every left-click already does.
                let egui_wants_it = consumed_by_egui && button != WinitMouseButton::Right;
                if in_tab_view && !egui_wants_it && y >= chrome_height {
                    if let Some(button) = translate_mouse_button(button) {
                        if let Some(id) = self.focused.clone() {
                            if let Some(node) = self.tabs.get_mut(&id) {
                                let event = match state {
                                    ElementState::Pressed => InputEvent::MouseDown {
                                        x,
                                        y: y - chrome_height,
                                        button,
                                        modifiers: self.key_modifiers,
                                    },
                                    ElementState::Released => InputEvent::MouseUp {
                                        x,
                                        y: y - chrome_height,
                                        button,
                                        modifiers: self.key_modifiers,
                                    },
                                };
                                node.tab.send(&HostToEngine::Input(event));
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let chrome_height = self.chrome_height_px() as f64;
                let (x, y) = self.cursor_pos;
                if in_tab_view && !consumed_by_egui && y >= chrome_height {
                    if let Some(id) = self.focused.clone() {
                        if let Some(node) = self.tabs.get_mut(&id) {
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
                        if let Some(node) = self.tabs.get_mut(&id) {
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

                // The content quad's rect (points, screen space) and which
                // tab's texture to draw there — a `Transition` overrides
                // both to the animated in-between rect and that tab's id,
                // regardless of `view_mode`; otherwise the normal
                // full-content-area-in-Tab-view/nothing-in-Graph-view rule.
                let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
                let (content_rect_pts, focused_for_content) = if let Some(transition) = &self.transition {
                    (
                        self.transition_content_rect_points(transition),
                        Some(transition.tab_id.clone()),
                    )
                } else if self.view_mode == ViewMode::Tab {
                    let screen = self
                        .window
                        .as_ref()
                        .map(|w| w.inner_size())
                        .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
                    let top = self.chrome_height_points();
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(0.0, top),
                        egui::vec2(screen.width as f32 / ppp, (screen.height as f32 / ppp - top).max(0.0)),
                    );
                    (rect, self.focused.clone())
                } else {
                    (egui::Rect::ZERO, None)
                };
                let content_rect_px = (
                    content_rect_pts.min.x * ppp,
                    content_rect_pts.min.y * ppp,
                    content_rect_pts.width() * ppp,
                    content_rect_pts.height() * ppp,
                );

                let content_on_top = self.transition.is_some();
                // Not taken yet -- a transient surface-acquire failure
                // (Outdated/Occluded/...) means the screenshot was never
                // attempted this frame, and the request must survive to
                // retry on a later one instead of being silently dropped.
                let screenshot_path = self.pending_screenshot.clone();
                if let Some(gpu) = &mut self.gpu {
                    let outcome = match egui_output {
                        Some(output) => gpu.render(
                            content_rect_px,
                            focused_for_content.as_deref(),
                            content_on_top,
                            Some((&self.egui_ctx, output)),
                            screenshot_path.as_deref(),
                        ),
                        None => gpu.render(
                            content_rect_px,
                            focused_for_content.as_deref(),
                            content_on_top,
                            None,
                            screenshot_path.as_deref(),
                        ),
                    };
                    if outcome.surface_ok {
                        self.pending_screenshot = None;
                        match outcome.screenshot_result {
                            Some(Ok(())) => {
                                if let Some(path) = screenshot_path {
                                    if let Some(pdf_path) = self.pending_print_to_pdf.take() {
                                        match pdf::write_image_as_pdf(
                                            std::path::Path::new(&path),
                                            std::path::Path::new(&pdf_path),
                                        ) {
                                            Ok(()) => {
                                                self.save_toast =
                                                    Some((pdf_path, std::time::Instant::now()));
                                            }
                                            Err(err) => eprintln!("print to pdf: {err}"),
                                        }
                                        let _ = std::fs::remove_file(&path);
                                    } else {
                                        self.save_toast = Some((path, std::time::Instant::now()));
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

                // See `quit_after_next_redraw`'s doc comment — this frame's
                // `build_ui` call above (already run, via `ctx.run_ui`) is
                // what actually consumes any pending debug flag that was
                // still set when `DebugCommand::Quit` deferred the exit;
                // safe to exit now that it's had its one guaranteed frame.
                if std::mem::take(&mut self.quit_after_next_redraw) {
                    event_loop.exit();
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

    /// Applies one headless `DebugCommand` (see `debug_control`'s doc
    /// comment for the overall design: mouse commands are real synthetic
    /// `WindowEvent`s, keyboard/text commands call the equivalent effect
    /// directly since `winit::event::KeyEvent` has no public constructor).
    fn handle_debug_command(&mut self, event_loop: &ActiveEventLoop, cmd: DebugCommand) {
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
            DebugCommand::AddressBar(text) => {
                // Mirrors the real `TextEdit`'s `response.changed()` arm
                // (see `build_tab_ui`) — setting `address_bar_focused`
                // directly stands in for a real click-driven egui focus
                // transfer, which headless mode has no way to synthesize.
                // `debug_address_bar_pin` (see its own doc comment) is what
                // actually makes this stick across the frames between now
                // and whenever `ENTER`/`ESCAPE`/a suggestion click ends it —
                // `address_bar_focused` alone only lasts the one frame this
                // command is processed on.
                self.address_bar_focused = true;
                self.debug_address_bar_pin = true;
                let exclude = self.focused.clone().unwrap_or_default();
                self.open_tab_suggestions = self.compute_open_tab_suggestions(&text, &exclude);
                self.suggestion_fetcher.note_text_changed(&text);
                self.address_bar_text = text;
            }
            // Only `build_tab_ui` ever consumes these four (`std::mem::take`,
            // same as a real key press), and it only runs while genuinely in
            // Tab view with no transition animating -- setting the flag
            // outside that window left it stuck `true` indefinitely (a real
            // key press instead just evaporates unconsumed at end of frame,
            // egui's `InputState` isn't a sticky latch the way this is).
            // Found live: an `ESCAPE` sent while already in Graph view (a
            // common "make sure we're in Graph view" test-setup step) stayed
            // pending and fired the moment ANY later command opened a tab,
            // silently bouncing straight back out to Graph.
            DebugCommand::Enter if self.view_mode == ViewMode::Tab && self.transition.is_none() => {
                self.pending_debug_enter = true;
            }
            DebugCommand::Escape if self.view_mode == ViewMode::Tab && self.transition.is_none() => {
                self.pending_debug_escape = true;
            }
            DebugCommand::ArrowDown if self.view_mode == ViewMode::Tab && self.transition.is_none() => {
                self.pending_debug_arrow_down = true;
            }
            DebugCommand::ArrowUp if self.view_mode == ViewMode::Tab && self.transition.is_none() => {
                self.pending_debug_arrow_up = true;
            }
            DebugCommand::Enter | DebugCommand::Escape | DebugCommand::ArrowDown | DebugCommand::ArrowUp => {}
            DebugCommand::GraphFocusLeft => self.move_graph_focus(egui::vec2(-1.0, 0.0)),
            DebugCommand::GraphFocusRight => self.move_graph_focus(egui::vec2(1.0, 0.0)),
            DebugCommand::GraphFocusUp => self.move_graph_focus(egui::vec2(0.0, -1.0)),
            DebugCommand::GraphFocusDown => self.move_graph_focus(egui::vec2(0.0, 1.0)),
            DebugCommand::GraphOpenFocused => self.open_focused_graph_node(),
            DebugCommand::TypeIntoPage(text) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.tabs.get_mut(&id) {
                        for ch in text.chars() {
                            node.tab
                                .send(&HostToEngine::Input(InputEvent::Char { codepoint: ch as u32 }));
                        }
                    }
                }
            }
            DebugCommand::KeyPage(key) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.tabs.get_mut(&id) {
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
                    if let Some(node) = self.tabs.get_mut(&id) {
                        node.tab.set_visible(visible);
                    }
                }
            }
            DebugCommand::Screenshot(path) => {
                self.pending_screenshot = Some(path);
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
            DebugCommand::ExportSession(path) => {
                self.build_session_snapshot().save_to(std::path::Path::new(&path));
            }
            DebugCommand::ImportSession(path) => {
                if let Some(session) = session::Session::load_from(std::path::Path::new(&path)) {
                    self.pending_import = Some(session);
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
                if let Some((group_id, _)) = self.group_context_menu.clone() {
                    if let Some(export) = self.export_group(&group_id) {
                        group_export::save_to(&export, std::path::Path::new(&path));
                    }
                }
            }
            DebugCommand::ImportGroup(path) => {
                if let Some(export) = group_export::load_from(std::path::Path::new(&path)) {
                    self.import_group_export(export);
                }
            }
            DebugCommand::DuplicateTab => self.duplicate_focused_tab_shortcut(),
            DebugCommand::ExportBookmarks(path) => {
                bookmarks::save_to(&self.bookmarks, std::path::Path::new(&path));
            }
            DebugCommand::ImportBookmarks(path) => {
                if let Some(imported) = bookmarks::load_from(std::path::Path::new(&path)) {
                    self.bookmarks = imported;
                    bookmarks::save(&self.bookmarks);
                }
            }
            DebugCommand::PrintToPdf(path) => self.start_print_to_pdf(path),
            DebugCommand::TogglePin => self.pin_focused_tab_shortcut(),
            DebugCommand::Reload => self.reload_focused_tab_shortcut(),
            DebugCommand::NewGroup => self.new_group_shortcut(),
            DebugCommand::CloseAllLevel => self.close_all_at_current_level(),
            DebugCommand::ToggleShortcutsHelp => {
                self.shortcuts_help_open = !self.shortcuts_help_open;
            }
            DebugCommand::Quit => {
                // See `quit_after_next_redraw`'s doc comment: exiting right
                // now could drop a pending Enter/Escape/arrow command, or a
                // just-requested Screenshot that hasn't been captured yet
                // (`pending_screenshot` is only consumed inside a
                // `RedrawRequested` — a `Screenshot` immediately followed by
                // `Quit`, with no frame in between, would otherwise silently
                // lose the screenshot the same way an unconsumed Enter
                // could).
                if self.pending_debug_enter
                    || self.pending_debug_escape
                    || self.pending_debug_arrow_down
                    || self.pending_debug_arrow_up
                    || self.pending_focus_address_bar
                    || self.pending_screenshot.is_some()
                {
                    self.quit_after_next_redraw = true;
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
    /// the very `TabNode` a queued `FrameReady` refers to — applying to a
    /// since-removed tab_id is harmless (checked at upload time via `gpu`
    /// only, no `self.tabs` lookup needed there) but there's no reason to
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

        let tab_ids: Vec<String> = self.tab_order.clone();
        let mut frame_updates: Vec<FrameUpdate> = Vec::new();
        let mut swap_finishes: Vec<(String, EngineKind, String)> = Vec::new();
        let mut crashes: Vec<(String, String)> = Vec::new();
        let mut open_tab_requests: Vec<(String, String)> = Vec::new();
        let mut favicon_updates: Vec<(String, Vec<u8>, u32, u32)> = Vec::new();
        let mut download_events: Vec<(String, String)> = Vec::new();
        // Whether any tab's real (settled) url changed this pass — used to
        // trigger one `save_session()` after the loop below rather than one
        // per `NavigationStateChanged` (title/history-only changes and a
        // same-url reload both leave this `false`; url actually changing is
        // the one thing worth persisting here).
        let mut nav_url_changed = false;

        for tab_id in &tab_ids {
            let Some(node) = self.tabs.get_mut(tab_id) else {
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
                        frame_updates.push(FrameUpdate {
                            tab_id: tab_id.clone(),
                            socket_id: node.tab.socket_id.clone(),
                            kind: node.tab.kind,
                            width,
                            height,
                            pixels,
                        });
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
                            self.history.push(HistoryEntry { title: title.clone(), url: url.clone() });
                            if self.history.len() > MAX_HISTORY_ENTRIES {
                                self.history.remove(0);
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
                    EngineToHost::FaviconChanged { rgba, width, height } => {
                        favicon_updates.push((tab_id.clone(), rgba, width, height));
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
                    EngineToHost::ContextMenuLink { x, y, href } => {
                        // Content-area coordinates -> window point space:
                        // Tab view's content starts below the toolbar (see
                        // `CHROME_HEIGHT_POINTS`'s other call sites). Only
                        // meaningful for the tab currently on screen.
                        if self.focused.as_deref() == Some(tab_id.as_str()) {
                            self.context_menu = Some(ContextMenuState {
                                x: x as f32,
                                y: y as f32 + CHROME_HEIGHT_POINTS,
                                href,
                            });
                        }
                    }
                    EngineToHost::DownloadCompleted { path } => {
                        self.save_toast = Some((path.clone(), std::time::Instant::now()));
                        download_events.push((tab_id.clone(), path));
                    }
                }
            }
        }
        if nav_url_changed {
            self.save_session();
        }

        for (tab_id, reason) in crashes {
            let Some((kind, url, parent, group, pinned, color, pos, vel)) = self.tabs.get(&tab_id).map(|n| {
                (
                    n.tab.kind,
                    n.url.clone(),
                    n.parent.clone(),
                    n.group.clone(),
                    n.pinned,
                    n.color,
                    n.pos,
                    n.vel,
                )
            }) else {
                continue;
            };
            eprintln!("engine-host crashed ({reason}) for {tab_id}, restarting at {url}");
            self.tabs.remove(&tab_id);
            // Same stale-state guard `close_tab`/`finish_swap` apply — see
            // `finish_swap`'s own comment on this.
            if self.dragging.as_deref() == Some(tab_id.as_str()) {
                self.dragging = None;
            }
            if self.hovered.as_deref() == Some(tab_id.as_str()) {
                self.hovered = None;
            }
            self.respawn_tab(&tab_id, kind, url, parent, group, pinned, color, pos, vel);
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

        // A completed download becomes its own graph node, child of the
        // tab it came from — same spawn-jitter placement a new tab gets
        // relative to its parent.
        for (tab_id, path) in download_events {
            let id = self.alloc_download_id();
            let group = self.tabs.get(&tab_id).and_then(|n| n.group.clone());
            let parent_pos = self.tabs.get(&tab_id).map(|n| n.pos).unwrap_or(egui::Vec2::ZERO);
            let pos = parent_pos + graph_layout::spawn_jitter(&id);
            self.downloads.insert(
                id.clone(),
                DownloadNode { parent: Some(tab_id), group, path, pos, vel: egui::Vec2::ZERO },
            );
            self.download_order.push(id);
        }

        for (tab_id, rgba, width, height) in favicon_updates {
            if let Some(gpu) = &mut self.gpu {
                gpu.upload_favicon(&tab_id, &rgba, width, height);
            }
        }

        for update in frame_updates {
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
    let headless = std::env::args().any(|arg| arg == "--headless");

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
    if headless {
        debug_control::spawn_stdin_reader(event_loop.create_proxy());
    }

    event_loop.run_app(&mut app).expect("run event loop");
}
