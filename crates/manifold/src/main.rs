mod bookmarks;
mod card;
mod cef_pin;
mod cef_platform;
mod cli;
mod click_count;
mod closed;
mod content;
mod crash_report;
mod debug_control;
mod engine_info;
mod engine_install;
mod event_loop;
mod favicon_fetch;
mod fetched;
mod gpu;
mod graph_layout;
mod graph_nav;
mod graph_ui;
mod history;
mod idle_sleep;
mod image_view;
mod input;
mod jar;
mod jar_defs;
mod json_file;
mod json_level;
mod json_level_ui;
mod json_view;
mod level;
mod local_state;
mod names;
mod nav_history;
mod net;
mod node;
mod node_menu;
mod pdf;
mod pdf_render;
mod popup_gate;
mod popups;
mod process_manager;
mod profile;
mod redirect_fetch;
mod rgba;
mod session;
mod session_sync;
mod settings;
mod settings_page;
mod shortcuts;
mod sleep_state;
mod store;
mod suggestions;
mod tab_view;
mod tabs;
mod theme;
mod view_modes;
mod zoom_chip;

use closed::*;
use fetched::*;
use node_menu::*;
use view_modes::*;
use zoom_chip::*;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use favicon_fetch::FaviconFetcher;
use pdf_render::PdfRenderer;
use redirect_fetch::{FetchJob, FetchResult, FetchTarget, RedirectFetcher};
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
use json_level::{JsonEdit, JsonLevel};
use level::{EdgeCurve, LevelItem, LevelItemKind};
use node::*;
use popups::*;
use process_manager::{EngineKind, EngineTab};
use settings::{SearchEngine, Settings};
use store::model::field;
use tab_view::*;
use theme::*;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{
    DeviceId, ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode as WinitKeyCode, ModifiersState, PhysicalKey};
#[cfg(windows)]
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
const CHROME_HEIGHT_POINTS: f32 = 38.0;
/// Graph view's chrome is just the "+ New Tab" toolbar — no address bar,
/// no per-tab back/forward, since there's no single focused tab there.
/// Equal to `CHROME_HEIGHT_POINTS`, not independently tuned — a shorter
/// value here used to fit the old bundled Ubuntu-Light font's line height
/// but clipped button labels once `apply_fonts` switched to IBM Plex
/// Serif's taller metrics (confirmed live: "+ New Group"/"Reset View"
/// visibly cut off at the old 26.0).
const GRAPH_TOOLBAR_HEIGHT_POINTS: f32 = CHROME_HEIGHT_POINTS;

/// Strips a "Save Session As..." name down to characters safe to use
/// verbatim as a filename (`sessions/<name>.mnf`) -- alphanumeric, space,
/// hyphen, underscore only. No path separators or `.` survive, so there's
/// no path-traversal or extension-confusion risk from typed input, and no
/// separate validation step is needed before treating the result as a bare
/// file stem. Cleaned first and last by the name rules (`names::clean_name`), so a hidden character
/// never reaches the file name and the spaces the filter leaves behind collapse.
fn sanitize_session_name(raw: &str) -> String {
    let clean = crate::names::clean_name(raw);
    let safe: String = clean.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).collect();
    crate::names::clean_name(&safe).into_owned()
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

/// Writes a JSON snapshot (spec "JSON snapshots") to `path`, pretty-
/// printed, logging (not toasting) a failure -- shared by every Export
/// Group site (headless `EXPORTGROUP`, the node menu's own "Export
/// Group..." and the popup's), matching the old per-group export file's
/// own save, which likewise only logged. `what` names the file's kind for
/// the log line, matching `json_file::load`'s own `what`.
fn write_json_export(json: &store::json::JsonDoc, path: &std::path::Path, what: &str) {
    let written = serde_json::to_string_pretty(json).map_err(|e| e.to_string()).and_then(|text| {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, text).map_err(|e| e.to_string())
    });
    if let Err(err) = written {
        eprintln!("{what}: couldn't export to {}: {err}", path.display());
    }
}

/// The bare filename portion of a download's full path, for display --
/// shared by the graph card's own label and the "Downloads (N)" toolbar
/// dropdown so the two never show it differently. Falls back to the whole
/// path if it has no separable filename component (shouldn't normally
/// happen for a real downloaded file's path).
fn download_filename(path: &str) -> &str {
    std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path)
}

/// How long settings wait after the last change before they are written:
/// a window drag sends dozens of moves a second.
const SETTINGS_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// Where nodes' files are on this machine (spec "Where fetched files
/// live"): this machine's own fetch in the content dir (`<id>.<ext>`, the
/// newest if a Refresh changed the extension), which a Refresh replaces
/// without touching the shared `path`; else the stored path. A missing
/// stored path shows as missing, and `session_sync::materialize` refetches
/// it when the node has a URL. An unfinished `.tmp` save is never picked.
/// One directory read answers a whole restore or apply pass.
#[derive(Debug, Default)]
pub(crate) struct ContentIndex(HashMap<String, (std::time::SystemTime, std::path::PathBuf)>);

impl ContentIndex {
    pub(crate) fn scan(dir: Option<&std::path::Path>) -> Self {
        let mut newest: HashMap<String, (std::time::SystemTime, std::path::PathBuf)> = HashMap::new();
        let entries = dir.and_then(|dir| std::fs::read_dir(dir).ok()).into_iter().flatten().flatten();
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some((id, _)) = name.split_once('.') else { continue };
            if name.ends_with(".tmp") {
                continue;
            }
            let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            match newest.get(id) {
                Some((at, _)) if *at >= modified => {}
                _ => {
                    newest.insert(id.to_string(), (modified, e.path()));
                }
            }
        }
        Self(newest)
    }

    pub(crate) fn resolve(&self, id: &str, stored: &str) -> String {
        self.0.get(id).map_or_else(|| stored.to_string(), |(_, path)| path.to_string_lossy().into_owned())
    }
}

/// A one-line shape summary for a JSON node's own small graph card:
/// `{ N }`/`[ N ]` for a container,
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
/// World-space room `bounds_for_level` leaves around a level's cards, so
/// a fit doesn't leave one flush against the viewport edge.
const GRAPH_FIT_MARGIN: f32 = 60.0;
/// The graph's world-space center that the physics centering force pulls
/// toward — independent of screen/window coordinates now that the graph is
/// pannable/zoomable (`egui::Scene`), unlike pre-pan/zoom where node
/// positions doubled as screen coordinates directly.
pub(crate) const GRAPH_WORLD_CENTER: egui::Vec2 = egui::Vec2::ZERO;
/// Initial visible world-space rect for the graph's `egui::Scene`, before
/// any user panning/zooming — generous enough to show a handful of settled
/// nodes around `GRAPH_WORLD_CENTER` at 1:1 zoom.
const GRAPH_SCENE_DEFAULT_SIZE: egui::Vec2 = egui::vec2(900.0, 620.0);
const GRAPH_ZOOM_MIN: f32 = 0.15;
const GRAPH_ZOOM_MAX: f32 = 3.0;

fn default_graph_scene_rect() -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(0.0, 0.0), GRAPH_SCENE_DEFAULT_SIZE)
}

/// `profile::pictures_dir()`\manifold-<unix-seconds>.png, or the temp folder without one — same
/// "pick the obvious default, no save-as dialog" posture `downloads_dir()`
/// (in each engine-host's own download-handling code) already takes, just
/// on the manifold side for the toolbar's "Screenshot" button. The
/// timestamp is the simplest collision-free name that needs no extra
/// state; a real "Save As" dialog would be the alternative, but this
/// project doesn't build custom save UI anywhere else either.
fn screenshot_default_path() -> std::path::PathBuf {
    let dir = profile::pictures_dir().unwrap_or_else(std::env::temp_dir);
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

/// `truncate_core`'s cache key: a hash of the text (the text itself is in
/// the value, compared on a hit, so a lookup allocates nothing), the font
/// size as bits (`f32` isn't `Hash`/`Eq`), the width bucket, and which end
/// is cut. Every call site builds its `FontId` with `FontId::proportional`,
/// so the family is not in the key. See `truncate_to_width` for why the
/// width is bucketed rather than exact.
type TruncateCacheKey = (u64, u32, i32, bool);

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

/// Shortens `text` to the longest prefix (plus an ellipsis) that lays out
/// no wider than `max_width` at `font_id`, or returns it unchanged if it
/// already fits. Needed because graph-view node labels are now painted at
/// a fixed screen-space font size regardless of zoom (see
/// `build_graph_ui`'s `overlay_painter`/`project`) — at low zoom a node's
/// card shrinks well below the label's natural width, and unlike a normal
/// egui `Label` widget (which can `.truncate()` itself during layout),
/// text painted directly via `Painter::text` has no such built-in limit.
///
/// Memoized on `cache` -- each search step needs a real text-shaping call
/// (`ctx.fonts_mut(...).layout_no_wrap`), and the overwhelming common
/// case frame to frame is "this exact label, this exact width, again",
/// since neither a card's title nor its on-screen width usually change
/// between two consecutive frames. **Found on a general (not just
/// large-graph) performance audit**: this ran unconditionally, every
/// frame, for every visible labeled card, regardless of graph size.
///
/// **`max_width` is bucketed, not used exactly, for both the cache key
/// and the search bound.** Every call site derives it from
/// `zoom_scale`, which changes continuously (as a raw float) on every
/// frame during an active pan/zoom gesture -- keying on the exact width
/// meant this cache hit approximately never while the user was actually
/// zooming, the one time avoiding repeated font-shaping calls matters
/// most. Found live, following up on the general audit above.
fn truncate_to_width(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
) -> String {
    truncate_core(cache, ctx, text, font_id, max_width, false)
}

/// `text` cut to fit `max_width` with "…" at the cut: at the end, or at the
/// start when `from_start` (a path keeps its file name). Cuts fall on
/// grapheme boundaries, so a ZWJ emoji or a combining mark is never split,
/// and the width measured is the width drawn. A binary search over
/// grapheme boundaries rather than a shrink-by-one loop, since
/// `layout_no_wrap` (one text-shaping pass) is the expensive part and a
/// title could in principle be long. Memoized on `cache`.
fn truncate_core(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
    from_start: bool,
) -> String {
    use std::hash::{Hash, Hasher};
    use unicode_segmentation::UnicodeSegmentation;
    let bucketed_width = (max_width / TRUNCATE_WIDTH_BUCKET).floor() * TRUNCATE_WIDTH_BUCKET;
    let width_bucket = (max_width / TRUNCATE_WIDTH_BUCKET).floor() as i32;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let key: TruncateCacheKey = (hasher.finish(), font_id.size.to_bits(), width_bucket, from_start);
    if let Some((cached_text, cut)) = cache.get(&key) {
        if cached_text == text {
            return cut.clone();
        }
    }
    let width_of = |s: &str| -> f32 {
        ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font_id.clone(), egui::Color32::WHITE)).size().x
    };
    let result = if bucketed_width <= 0.0 {
        String::new()
    } else if width_of(text) <= bucketed_width {
        text.to_owned()
    } else {
        // Byte offsets of every grapheme boundary, both ends included.
        let mut bounds: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        bounds.push(text.len());
        let candidate = |k: usize| {
            if from_start {
                format!("\u{2026}{}", &text[bounds[bounds.len() - 1 - k]..])
            } else {
                format!("{}\u{2026}", &text[..bounds[k]])
            }
        };
        // The most graphemes `k` kept that still fit.
        let (mut lo, mut hi) = (0usize, bounds.len() - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if width_of(&candidate(mid)) <= bucketed_width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        candidate(lo)
    };
    if cache.len() >= TRUNCATE_CACHE_CAP {
        cache.clear();
    }
    cache.insert(key, (text.to_owned(), result.clone()));
    result
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

/// How long a "Saved <path>" toast stays on screen — see `App::save_toast`'s
/// doc comment.
const SAVE_TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(4);

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
    /// The tab menu's "New Jar..." name field (`App::naming_jar`).
    JarName,
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

/// Content on its way to becoming a node (`App::insert_content`).
pub(crate) enum ContentSource {
    /// A file on this machine. Sniffed; the node points at it where it is.
    Local(std::path::PathBuf),
    /// Checked content from `url` (see `redirect_fetch`).
    Web { url: String, body: WebBody },
}

pub(crate) enum WebBody {
    /// Saved in the content dir as `<id>.<ext>`; `id` is the node's id.
    File { kind: content::ContentKind, path: String },
    /// A note's text.
    Text(String),
}

/// The event loop's real user-event type: headless debug commands, plus
/// `TabSpawned` — see its own doc comment for why engine-host spawning needs
/// to be async in the first place.
enum UserEvent {
    Debug(DebugCommand),
    TabSpawned(Box<TabSpawnOutcome>),
    /// A screen reader connected, acted on a node, or left (`egui_winit::accesskit_winit`).
    Accessibility(egui_winit::accesskit_winit::Event),
}

impl From<egui_winit::accesskit_winit::Event> for UserEvent {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        UserEvent::Accessibility(event)
    }
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
    /// Startup session restore (see `App::restore_session`) — one of
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
    /// `App::wake_tab`: an unloaded tab's engine starting (a sleeping tab
    /// opened, a failed one retried). Lands like `Respawn`, decided at
    /// completion against `self.focused`, and is a host-started load
    /// (`note_host_load`), like a restore.
    Wake,
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
    result: Result<EngineTab, process_manager::SpawnError>,
    /// The cookies the host was seeded with, as its starting `jar::TabCookies` (empty for hosts that are not).
    seeded: jar::TabCookies,
    /// The jar the host was seeded from, resolved when the spawn started: its pulls merge into this one.
    jar: jar::JarKey,
    /// That jar's clear count when the seed was read (`jar::Snapshots::seed`); `None` if it never was.
    seed_clears: Option<u64>,
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

/// What a tab keeps across a respawn (engine swap, crash recovery) or a
/// wake. A new tab starts unpinned, untagged, at the default zoom, with no
/// title or mute of its own yet. A new attribute that must survive a
/// respawn or wake is one more field here.
struct TabCarry {
    /// See `TabPayload::pinned`.
    pinned: bool,
    /// See `GraphNode::color`.
    color: Option<egui::Color32>,
    /// See `TabPayload::zoom_factor`. A new tab takes
    /// `Settings::zoom_factor`, having no value of its own yet.
    zoom_factor: f64,
    /// See `TabPayload::title`: shown until the page reports its own.
    title: String,
    /// See `TabPayload::muted`. Sent to the new engine on landing.
    muted: bool,
    /// See `TabPayload::jar`.
    jar: crate::jar::JarChoice,
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
            carry: TabCarry {
                pinned: false,
                color: None,
                zoom_factor,
                title: String::new(),
                muted: false,
                jar: Default::default(),
            },
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
            carry: TabCarry {
                pinned: tab.pinned,
                color: node.color,
                zoom_factor: tab.zoom_factor,
                title: tab.title.clone(),
                muted: tab.muted,
                jar: tab.jar,
            },
            fallback_kind: None,
        })
    }
}

/// A named jar's synced definition (a `Kind::Jar` node). Not a graph node (spec "Data model").
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JarDef {
    pub(crate) name: String,
    pub(crate) color: Option<egui::Color32>,
    pub(crate) ephemeral: bool,
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
    /// The level of the Json node `current_group` names (spec "Level
    /// sources"); `None` on every other level.
    json_level: Option<JsonLevel>,
    /// The JSON outline panel shows (local state).
    json_outline_visible: bool,
    /// Its width in points, as last drawn (local state).
    json_outline_width: f32,
    /// Copy path's form on a JSON level (local state).
    json_accessor_paths: bool,
    /// `(group_id, draft_name)` once renaming has started -- either the
    /// toolbar's group-name button (only shown while `current_group` is
    /// `Some`) or the top-level right-click context menu's own "Rename"
    /// (see `node_menu`) sets this the same way. A real
    /// `egui::TextEdit` replaces whichever plain button/menu row started
    /// it until Enter commits or Escape cancels. Transient UI state, not
    /// part of `GroupPayload` itself.
    renaming_group: Option<(String, String)>,
    /// `(tab_id, draft_name)` while a tab's menu shows "New Jar..."'s name field in place of its items, the way
    /// `renaming_group` does for a group. Committing creates the jar and picks it for the tab (`NodeAction::NewJar`).
    naming_jar: Option<(String, String)>,
    /// A jar's "Clear Cookies" or "Delete" from Settings, waiting for its confirm dialog (`build_jar_confirm_popup`).
    pending_jar_confirm: Option<jar_defs::JarConfirm>,
    /// A named session's delete from Settings, waiting for its confirm dialog (`build_session_delete_popup`).
    pending_session_delete: Option<String>,
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
    /// When each live tab was last on screen, for idle auto-sleep.
    idle_clock: idle_sleep::IdleClock,
    /// Sleep requests awaiting the engine's `PageState` answer, by tab id.
    pending_sleeps: HashMap<String, sleep_state::PendingSleep>,
    /// The cookie jars WebView2 and CEF tabs share, by key (see `jar`). The default jar loads at startup; a named jar
    /// loads the first time a tab spawns with it (`ensure_jar_loaded`). An ephemeral jar lives only here.
    jars: BTreeMap<jar::JarKey, jar::Jar>,
    /// Per running host, the jar it was **spawned with** (spec "Live tabs when `jar` changes") and what its engine is
    /// believed to hold (`jar::TabCookies`): only a cookie the tab itself changed reaches that jar, and only a cookie
    /// the tab itself reported can be deleted by its absence.
    jar_tabs: HashMap<String, (jar::JarKey, jar::TabCookies)>,
    /// Hosts still holding cookies from a jar the user cleared (`clear_jar_cookies`): they switch (`will_switch`)
    /// although their key is unchanged, and their pulls are dropped so the old cookies cannot refill the jar. An id
    /// leaves when its host is replaced or closed (`clear_cookie_state`).
    jar_reseed: HashSet<String>,
    /// A copy of `jars` for spawn threads: the seed is read when the host connects, not when the spawn starts. Also
    /// counts each jar's clears, so a host seeded before a clear is caught when it lands (`note_spawned_jar`).
    jar_snapshots: Arc<std::sync::Mutex<jar::Snapshots>>,
    /// A sleep's `PageState` or a swap's `StateSnapshot` waiting for the tab's `Cookies` reply (or its deadline), so
    /// the host is not dropped before its last cookies arrive.
    cookie_holds: HashMap<String, (std::time::Instant, tabs::Held)>,
    /// When `jar_pull_all` last ran, for the 30 s pull in `about_to_wait`.
    jar_last_pull: Option<std::time::Instant>,
    /// Tabs with a `GetCookies` in flight, by tab id. A host may never answer, so an entry expires
    /// (`JAR_PULL_TIMEOUT`) instead of blocking the next pull.
    jar_pulls: HashMap<String, std::time::Instant>,
    /// Consecutive unanswered pulls per tab (`tabs::start_pull`).
    jar_silent: HashMap<String, u8>,
    /// Scroll positions of woken tabs, restored once their first load finishes.
    saved_scroll: HashMap<String, sleep_state::SavedScroll>,
    /// Each tab's back/forward list as the host saw it, by tab id: kept
    /// when the tab's engine is replaced (crash, wake, swap). See `nav_history`.
    nav_history: HashMap<String, nav_history::NavHistory>,
    /// Files dropped on the window since the last frame -- each one's
    /// placement (`drop_file`) is offset by `egui::vec2(24.0, 24.0) *
    /// drop_batch`, so several dropped in the same frame fan out instead of
    /// stacking exactly on top of each other. Reset to 0 at the top of
    /// `render_frame`.
    drop_batch: u32,
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
    /// Ids whose tab spawn is in flight (every `SpawnPurpose`), each with
    /// the tab it was started as (`session_sync::spawn_shared`). No live
    /// node yet, so `save_session` keeps them in the store rather than
    /// deleting them (contract "In-flight spawns"), and creates one the
    /// store doesn't hold yet (a new, imported or reopened tab), so a quit
    /// before it lands doesn't lose it.
    spawning: HashMap<String, session_sync::Shared>,
    /// Other replicas' latest change to a node in `spawning`, applied when
    /// its spawn lands (`None`: deleted). The spawn itself carries only
    /// what it was started with.
    remote_in_flight: HashMap<String, Option<session_sync::RemoteNode>>,
    /// Tabs loading a url the host gave them, with when each was last seen
    /// loading; their url changes aren't shared meanwhile
    /// (`App::shares_url_change`).
    host_loads: HashMap<String, std::time::Instant>,
    /// Tabs `navigate_tab` just moved on, with the url each left, when, and
    /// whether the engine has reported loading since (`App::reported_url`).
    navigated_from: HashMap<String, (String, std::time::Instant, bool)>,
    /// Explicit fields (`pos`, `url`) marked at user actions, for the next
    /// save to write (spec "Local edits become ops").
    explicit: std::collections::BTreeSet<(store::model::NodeId, u32)>,
    /// What the store held for each node when this replica last read or
    /// committed it. `materialize` applies a field only where the store
    /// now differs, so a remote change to one field doesn't overwrite
    /// another field's unsaved local value.
    last_shared: HashMap<String, session_sync::Shared>,
    /// Named jars' synced definitions (spec "Data model"). A jar is a `Kind::Jar` store node but never a graph node:
    /// `session_sync` bridges this map to the document.
    jar_defs: BTreeMap<store::model::NodeId, JarDef>,
    /// Personal state (spec "Personal state") last written to
    /// `session.mnf.local`, or read at startup: `maybe_write_local` skips
    /// writing when the current state compares equal, so an idle session
    /// never rewrites the file.
    local_written: Option<local_state::LocalState>,
    /// Set while startup couldn't read `session.mnf.local` (another
    /// window held the lock, or the read failed): nothing writes it, so
    /// this profile's saved state isn't replaced by defaults, and
    /// `poll_store` retries the read.
    local_unread: Option<session_sync::LocalUnread>,
    /// When `local_written` was last written (or, at startup, `Instant::now`
    /// so the first `EVERY` window starts from launch, not from `UNIX_EPOCH`).
    local_written_at: std::time::Instant,
    /// Set when Recently Closed changes (Task 8); `maybe_write_local` writes
    /// `AFTER_CLOSED` after this, instead of waiting for the `EVERY` timer.
    closed_changed_at: Option<std::time::Instant>,
    /// Personal-state entries for nodes `apply_local` found still spawning
    /// (no live node yet): applied once each one's node is inserted
    /// (`handle_tab_spawn_result`), then removed. Only `nodes` is ever
    /// populated here -- the view fields are resolved once, in `apply_local`
    /// itself, since spawning is enough for those to take effect.
    pending_local: Option<local_state::LocalState>,
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
    /// Click counts for page presses (`InputEvent::MouseDown::click_count`).
    click_counter: click_count::ClickCounter,
    /// Physical keys of a shortcut chord pressed in Tab view, whose release (and typed text) must not
    /// reach the page either (`shortcuts::claims_chord`).
    swallowed_keys: std::collections::HashSet<WinitKeyCode>,

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
    pdf_renderer: PdfRenderer,
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
    /// Every node the current drag is moving (a lone card, or the whole
    /// selection when the dragged card is selected — see `take_drag_unit`
    /// and the `dragged()` branch that fills this in). Read and cleared at
    /// release, where each one's `pos` is marked exactly once (spec "Local
    /// edits become ops"): marking every frame of the drag instead would
    /// let an intermediate position reach the store, since a save can fire
    /// mid-drag from an unrelated event (`poll_store`, or a url change
    /// batched through `drain_tab_events`), independent of the gesture.
    drag_moved: Vec<String>,
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
    truncate_cache: HashMap<TruncateCacheKey, (String, String)>,
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
    /// Whether the last `about_to_wait` found the window minimized: the
    /// loop then ticks at `MINIMIZED_TICK` instead of redrawing, and on the
    /// way back resumes redrawing.
    was_minimized: bool,
    /// The last input, engine message burst or egui repaint request: after
    /// `IDLE_AFTER` without one the loop slows to `IDLE_FRAME` (see
    /// `about_to_wait`).
    last_activity: std::time::Instant,
    /// Engine frames seen in the current second (a burst is activity).
    frame_burst: (std::time::Instant, u32),
    /// When an idle loop next draws.
    next_idle_frame: std::time::Instant,
    /// The wait between idle frames: `IDLE_FRAME`, or less when egui asked
    /// for a repaint sooner (`request_repaint_after`).
    idle_interval: std::time::Duration,
    /// Tabs `SetVisible(false)` was sent to on minimize, to be shown again
    /// on restore (`hide_tabs_for_minimize`).
    hidden_for_minimize: Vec<String>,
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
    /// "Print to PDF": (temp PNG, PDF): the capture `start_print_to_pdf`
    /// asked for, and where its PDF goes. Paired so an unrelated screenshot
    /// can't take the PDF target. Set alongside `pending_screenshot` so the
    /// GPU capture needn't know PDF exists; once that capture completes
    /// (`handle_screenshot_result`), the temp PNG is converted via
    /// `pdf::write_image_as_pdf` and deleted. `None` the rest of the time,
    /// including for an ordinary "Screenshot" button click.
    pending_print_to_pdf: Option<(String, String)>,

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
    /// Admits or drops page-opened tabs (`EngineToHost::OpenTabRequested`)
    /// per source tab: a user gesture and a rate budget.
    popup_gate: popup_gate::PopupGate,
    /// Recent engine crash times per tab id (`tabs::crashes_too_often`): a
    /// tab crashing too often is left as a failed card, not respawned.
    tab_crashes: HashMap<String, Vec<std::time::Instant>>,

    /// When `save_session` last committed to the store -- shown in the
    /// settings popup's Sessions section next to a "Save Now" button, so a
    /// save that's actually continuous (every mutation calls
    /// `save_session`, not a timer) has some visible confirmation instead
    /// of being purely invisible. `None` until the first save this run.
    last_session_save: Option<std::time::Instant>,
    /// When a save that didn't commit (another instance kept writing, or
    /// the write failed) is tried again by `poll_store`; `None` when
    /// nothing is owed. Without it an edit could sit unsaved until the next
    /// local action or remote write.
    save_retry_at: Option<std::time::Instant>,
    /// Store errors and repair notices toasted since the last commit
    /// (`store_error`, `take_store_recovery`), each shown once: a read-only
    /// file with a damaged tail reports both at every reload.
    store_notices: std::collections::BTreeSet<String>,
    /// Set once the store reports a repair (`take_store_recovery`): what
    /// was loaded may be missing nodes, so `prune_content_dir` doesn't run.
    store_recovered: bool,
    /// `settings::fingerprint()` as this process last saw it (set at load,
    /// updated after every successful save) -- see `save_settings`. `None`
    /// until the file has actually been loaded or saved once this run,
    /// which also means "skip the conflict check" the first time.
    settings_fingerprint: Option<u64>,
    /// When settings changed by a window move or resize are due to be
    /// written (`note_settings_changed`, `flush_settings_if_due`); `None`
    /// when nothing is waiting.
    settings_save_due: Option<std::time::Instant>,
    /// Set for the length of one restore or apply pass, so each node's file
    /// lookup doesn't read the content dir again.
    content_index: Option<ContentIndex>,

    /// Find-in-page, for the focused tab only. See `FindState`.
    find: FindState,

    /// Engine runtime installs, at most one per engine (see `engine_install::InstallJob`).
    engine_installs: HashMap<EngineKind, engine_install::InstallJob>,
    /// The last install's failure message per engine, until the next attempt starts.
    engine_failures: HashMap<EngineKind, String>,
    /// An engine download or removal waiting for its confirm dialog (`build_engine_confirm_popup`).
    pending_engine_confirm: Option<EngineConfirm>,
    /// `engine_state` per engine for the current frame: it stats the disk, and cards, pickers, menus
    /// and Settings all ask. Cleared at the start of each frame and whenever an install changes.
    engine_state_cache: std::cell::RefCell<HashMap<EngineKind, engine_install::EngineState>>,
    /// The bring-your-own CEF directory's last validation, with the path it was for (`custom_cef_dir`).
    cef_dir_check: std::cell::RefCell<Option<engine_install::DirCheck>>,
    /// Settings' text field for that directory while it is being edited; committed on Enter or focus loss.
    cef_dir_draft: Option<String>,
    /// The Home page field's text while it's being edited; Escape drops it (`build_settings_page`).
    home_page_draft: Option<String>,

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
    /// to, most-recent-last, capped at `history::MAX_HISTORY_ENTRIES` (oldest
    /// dropped first) -- recorded once per real URL change in
    /// `EngineToHost::NavigationStateChanged`'s own handling, so it covers
    /// in-page navigation (clicked links) the same as address-bar entries,
    /// not just the latter. Persisted to `history.json` whenever saved.
    history: Vec<history::HistoryEntry>,
    /// Set whenever an entry is pushed to `history` or when history is cleared.
    /// Saved by `save_session` or `close_store` when `true`.
    history_dirty: bool,
    /// The history popup's own filter box text -- same "cleared on each
    /// open" reasoning as `bookmarks_filter`.
    history_filter: String,
    /// The History popup's filter, the history length it was computed at, and its matches: recomputed only when
    /// either changes (or when `history_dirty` is set, which also covers an in-place title update).
    history_matches_cache: Option<(String, usize, Vec<usize>)>,
    /// Which popup anchored below a toolbar or Settings button is open, if
    /// any; one at a time. See `AnchoredPopup`, `toggle_popup` and
    /// `close_popup`.
    open_popup: Option<AnchoredPopup>,
    /// A graph card or one of its controls had keyboard focus in the last card pass: the cards
    /// show their Tab-order numbers (`CardPass::show_order`).
    card_nav_active: bool,
    /// Ctrl has been held on its own long enough to reveal key hints (`ctrl_reveal_step`).
    hints_revealed: bool,
    /// The card holding keyboard focus this frame: its kind and screen rect, for the key-hint strip
    /// (`build_card_hint`).
    card_hint: Option<(ItemNodeKind, egui::Rect)>,

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

    /// Unloaded tabs' cards whose `wake_tab` spawn is in flight. Unlike a
    /// crash respawn, the card is still on the graph meanwhile, so it can
    /// be moved or closed: the outcome takes the card's placement as it is
    /// now, and is dropped if the card is gone. Set in `wake_tab`, cleared
    /// by that outcome in `handle_tab_spawn_result`.
    wakes_in_flight: HashSet<String>,

    /// Tabs that stay tabs whatever they load, each with whether that load
    /// has settled (reported `loading == false`): one opened by "Open as
    /// tab", and one loading another replica's url (`materialize`), which
    /// that replica converts itself. Every `RedirectableContent`/`JsonBody`
    /// report is skipped while a tab is in here. It stays until a real url
    /// change after the load settles (an HTTP redirect during it doesn't
    /// count; see `skip_convert_after`) or the tab closes (`remove_tab`).
    /// Not persisted (spec "Left open").
    skip_convert: HashMap<String, bool>,

    /// Windowless tests have no event loop to spawn on: `spawn_tab_async`
    /// parks each spawn here (with its `session_generation`) for the test
    /// to hand `handle_tab_spawn_result` an outcome itself.
    #[cfg(test)]
    test_spawns: Vec<(TabSpawn, u64)>,
    /// A test's jar directory, read only by `jars_dir`. `None`: no jar files at all (`profile::jars_dir()` is `None`
    /// under test).
    #[cfg(test)]
    jars_dir_override: Option<std::path::PathBuf>,
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
        let (history, history_err) = history::load();
        // Before constructing anything that might make its own first real
        // HTTPS request (search suggestions, favicons) -- see `suggestions::
        // warm_up_https_client`'s own doc comment for why this needs to run
        // exactly once, centrally, rather than each network consumer doing
        // its own redundant warm-up (or, worse, one racing a real request
        // against another's not-yet-finished warm-up).
        suggestions::warm_up_https_client();
        let jars: BTreeMap<jar::JarKey, jar::Jar> = profile::jars_dir()
            .map(|dir| (jar::JarKey::Default, jar::Jar::load(&dir, jar::JarKey::Default)))
            .into_iter()
            .collect();
        let jar_snapshots = Arc::new(std::sync::Mutex::new(jar::Snapshots::new(jars.clone())));
        let mut app = Self {
            window: None,
            last_window_title: "manifold".to_string(),
            gpu: None,
            nodes: NodeMap::default(),
            node_order: Vec::new(),
            groups_collapsed: false,
            current_group: None,
            json_level: None,
            json_outline_visible: true,
            json_outline_width: 260.0,
            json_accessor_paths: false,
            renaming_group: None,
            naming_jar: None,
            pending_jar_confirm: None,
            pending_session_delete: None,
            node_menu: None,
            node_menu_cache: None,
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
            idle_clock: idle_sleep::IdleClock::default(),
            pending_sleeps: HashMap::new(),
            jars,
            jar_tabs: HashMap::new(),
            jar_reseed: HashSet::new(),
            jar_snapshots,
            cookie_holds: HashMap::new(),
            jar_last_pull: None,
            jar_pulls: HashMap::new(),
            jar_silent: HashMap::new(),
            saved_scroll: HashMap::new(),
            nav_history: nav_history::load(),
            drop_batch: 0,
            view_mode: ViewMode::Item,
            journal: None,
            fallback_ids: (store::model::ActorId::random(), 0),
            tab_spawns_in_flight: 0,
            spawning: HashMap::new(),
            remote_in_flight: HashMap::new(),
            host_loads: HashMap::new(),
            navigated_from: HashMap::new(),
            explicit: std::collections::BTreeSet::new(),
            last_shared: HashMap::new(),
            jar_defs: BTreeMap::new(),
            local_written: None,
            local_unread: None,
            local_written_at: std::time::Instant::now(),
            closed_changed_at: None,
            pending_local: None,
            tabs_spawned_waiters: Vec::new(),
            session_generation: 0,
            commonmark_cache: egui_commonmark::CommonMarkCache::default(),
            cursor_pos: (0.0, 0.0),
            native_drag_tab: None,
            key_modifiers: 0,
            click_counter: Default::default(),
            swallowed_keys: Default::default(),
            egui_ctx,
            egui_state: None,
            address_bar: AddressBar::new(settings.home_page.clone()),
            favicon_fetcher: FaviconFetcher::new(profile::favicon_cache_dir()),
            pdf_renderer: PdfRenderer::new(),
            redirect_fetcher: RedirectFetcher::new(profile::content_dir()),
            favicon_resolved: HashSet::new(),
            dragging: None,
            drag_moved: Vec::new(),
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
            was_minimized: false,
            last_activity: std::time::Instant::now(),
            frame_burst: (std::time::Instant::now(), 0),
            next_idle_frame: std::time::Instant::now(),
            idle_interval: event_loop::IDLE_FRAME,
            hidden_for_minimize: Vec::new(),
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
            popup_gate: popup_gate::PopupGate::default(),
            tab_crashes: HashMap::new(),
            last_session_save: None,
            save_retry_at: None,
            store_notices: std::collections::BTreeSet::new(),
            store_recovered: false,
            settings_fingerprint,
            settings_save_due: None,
            content_index: None,
            find: FindState::default(),
            engine_installs: HashMap::new(),
            engine_failures: HashMap::new(),
            pending_engine_confirm: None,
            engine_state_cache: Default::default(),
            cef_dir_check: Default::default(),
            cef_dir_draft: None,
            home_page_draft: None,
            tab_spawn_error: None,
            settings,
            settings_return_view: ViewMode::Item,
            bookmarks,
            bookmarks_filter: String::new(),
            renaming_bookmark_folder: None,
            history,
            history_dirty: false,
            history_filter: String::new(),
            history_matches_cache: None,
            open_popup: None,
            card_nav_active: false,
            hints_revealed: false,
            card_hint: None,
            transition: None,
            event_proxy: None,
            pending_respawn_focus: None,
            wakes_in_flight: HashSet::new(),
            skip_convert: HashMap::new(),
            #[cfg(test)]
            test_spawns: Vec::new(),
            #[cfg(test)]
            jars_dir_override: None,
        };
        for err in [settings_err, bookmarks_err, history_err].into_iter().flatten() {
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
                self.history.push(history::HistoryEntry { title: String::new(), url: url.clone() });
                if self.history.len() > history::MAX_HISTORY_ENTRIES {
                    self.history.remove(0);
                }
                self.history_dirty = true;
                self.history_matches_cache = None;
            }
        }
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            if node.url != url {
                self.navigated_from.insert(tab_id.to_string(), (node.url.clone(), std::time::Instant::now(), false));
            }
            node.url = url.clone();
            node.tab.send(&HostToEngine::Navigate { url, referrer: None });
            // Until the engine labels the navigation, a report can still
            // carry the old url (CEF reported loading before the new address,
            // unlabelled): that isn't the user's either.
            self.note_host_load(tab_id);
            self.mark(tab_id, field::URL);
            // Now, while `node.url` is still the typed url: a save writes the
            // live value, and the page's own redirect or script navigation
            // (never shared) would otherwise be what the mark writes. Only a
            // save that doesn't commit leaves the mark for a later one.
            self.save_session();
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
            .filter_map(|id| self.nodes.get(id).and_then(|n| n.payload.listed_tab()).map(|tab| (id, tab)))
            .filter(|(_, (title, url))| url.to_lowercase().contains(&query) || title.to_lowercase().contains(&query))
            .take(5)
            .map(|(id, (title, url))| Suggestion::OpenTab {
                tab_id: id.clone(),
                url: url.to_string(),
                title: title.to_string(),
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
    /// direct `self.settings.save()` call site. It writes the window
    /// geometry too, so a debounced save still waiting is done by it.
    fn save_settings(&mut self) {
        self.settings_save_due = None;
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

    /// Settings changed (window size or position): written once things
    /// are quiet for `SETTINGS_SAVE_DELAY`, or at exit.
    pub(crate) fn note_settings_changed(&mut self) {
        self.settings_save_due = Some(std::time::Instant::now() + SETTINGS_SAVE_DELAY);
    }

    /// Writes settings if a change is due by `now`. Returns whether it wrote.
    pub(crate) fn flush_settings_if_due(&mut self, now: std::time::Instant) -> bool {
        match self.settings_save_due {
            Some(due) if due <= now => {
                self.settings_save_due = None;
                self.save_settings();
                true
            }
            _ => false,
        }
    }

    /// At exit: writes a debounced settings change still waiting.
    pub(crate) fn flush_pending_settings(&mut self) {
        if self.settings_save_due.is_some() {
            self.save_settings();
        }
    }

    /// `session::named_session_path`, toasting why there is none: "Not a session name" for a name that can't be a
    /// file (`session_name_is_safe`; a device name such as `CON` survives `sanitize_session_name`), "No session
    /// file" without a profile.
    fn named_session_path_or_toast(&mut self, name: &str) -> Option<std::path::PathBuf> {
        let path = session::named_session_path(name);
        if path.is_none() {
            self.queue_toast(if session::session_name_is_safe(name) {
                "No session file".into()
            } else {
                let shown = crate::names::to_plain(&crate::names::display_name(name, crate::names::UNTITLED));
                format!("Not a session name: {shown}")
            });
        }
        path
    }

    /// "Save Session As..." (settings popup) / headless `SAVESESSIONAS` --
    /// a full compacted copy of the running store (spec "Named sessions"),
    /// with this profile's own personal state (`local_state_for_copy`), saved to `sessions/<name>
    /// .mnf`. Saves the running session first so the copy isn't missing
    /// recent edits; a failure there or in the copy itself is toasted, not
    /// fatal (the running session is untouched either way). `raw` is sanitised here (`sanitize_session_name`), for
    /// Settings and the headless command alike; nothing left of it saves nothing.
    fn save_named_session(&mut self, raw: &str) {
        let name = sanitize_session_name(raw);
        if name.is_empty() {
            return;
        }
        // The Settings page lists saves and sizes: read them again next frame, whatever happens below.
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
        let Some(path) = self.named_session_path_or_toast(&name) else { return };
        if !self.save_session() {
            self.queue_toast("The saved copy may miss recent edits: the session couldn't be saved first".into());
        }
        let local = self.local_state_for_copy();
        let Some(journal) = self.journal.as_ref() else { return };
        if let Err(err) = journal.save_copy(&path, local.as_deref()) {
            let shown = crate::names::to_plain(&crate::names::display_name(&name, crate::names::UNTITLED));
            self.queue_toast(format!("Couldn't save as {shown}: {err}"));
        }
    }

    /// The personal state Save As gives its copy: this profile's, or none while startup's is still unread
    /// (`App::local_unread`). The live view then lacks what the file holds (its Recently Closed, say); a copy
    /// with none opens with defaults rather than a partial state passed off as this profile's.
    fn local_state_for_copy(&self) -> Option<Vec<u8>> {
        self.local_unread.is_none().then(|| self.local_state().to_bytes())
    }

    /// "Open Session" (settings popup) / headless `OPENSESSION` -- reads a
    /// named save's store file and personal state, then replaces the
    /// running session with it via `replace_running_session` (same path
    /// Import Session uses). A read error, including one from a file
    /// written by a newer manifold (`JournalError::ReadOnly` -- `Journal::
    /// read` still reads it, just says so in its `Display`), is toasted
    /// and leaves the running session untouched.
    fn load_named_session(&mut self, name: &str) {
        let shown = crate::names::to_plain(&crate::names::display_name(name, crate::names::UNTITLED));
        let Some(path) = self.named_session_path_or_toast(name) else { return };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.queue_toast(format!("Couldn't open {shown}: {err}"));
                return;
            }
        };
        let doc = match store::journal::Journal::read(&bytes) {
            Ok(doc) => doc,
            Err(err) => {
                self.queue_toast(format!("Couldn't open {shown}: {err}"));
                return;
            }
        };
        let local = store::local::read(&path, doc.id)
            .ok()
            .flatten()
            .and_then(|bytes| local_state::LocalState::from_bytes(&bytes));
        let mut json = store::json::doc_to_json(&doc);
        json.local = local.map(|l| serde_json::to_value(l).expect("personal state serializes"));
        self.replace_running_session(&json, store::json::Trust::Own);
    }

    /// Whether the first-launch demo can render here: its `manifold://` pages need WebView2.
    fn demo_available(&self) -> bool {
        !matches!(self.engine_state(EngineKind::Webview2), engine_install::EngineState::Unavailable(_))
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
    ///
    /// Always WebView2, whatever the default engine: it is the only engine that serves `manifold://` (see
    /// `docs/superpowers/specs/2026-09-19-manifold-scheme-design.md`). Where WebView2 is unavailable the caller opens
    /// a normal root tab instead (`demo_available`).
    fn spawn_first_launch_demo(&mut self) {
        let kind = EngineKind::Webview2;
        let zoom = self.settings.zoom_factor;

        let welcome_id = self.alloc_node_id();
        self.focused = Some(welcome_id.clone());
        self.view_mode = ViewMode::Graph;
        let welcome = "manifold://welcome".to_string();
        self.spawn_tab_async(TabSpawn::new(welcome_id, welcome, kind, egui::Vec2::ZERO, SpawnPurpose::Restore, zoom));

        let note = NodePayload::Note(NotePayload {
            text: "This is a Note -- drag it anywhere, or right-click to delete it \
                           (undo via \"Recently Closed\" if you change your mind)."
                .to_string(),
            editing: false,
            url: None,
        });
        self.place_node(None, note, Placement { group: None, pos: egui::vec2(-300.0, 0.0) });

        let group_pos = egui::vec2(300.0, 0.0);
        let group = NodePayload::Group(GroupPayload::named("Try grouping".to_string()));
        let group_id = self.place_node(None, group, Placement { group: None, pos: group_pos });
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

        // Deliberately left as a plain card, not entered -- the welcome
        // page's own "Try it" section already tells the user to
        // double-click this exact kind of node themselves, so entering its
        // level here would rob them of the discovery the copy is pointing
        // at. Synthetic, not file-backed (`path` is a display-only label, not a
        // real path -- see `JsonPayload`'s own doc comment on `key` for the
        // same "inherited path, no real ownership" reasoning `key: Some`
        // leaves use, though this node uses `key: None` since it's a
        // genuine standalone JSON node, not an exploded leaf).
        let json = NodePayload::Json(JsonPayload {
            path: "(demo).json".to_string(),
            value: std::sync::Arc::new(serde_json::json!({
                "project": "Manifold",
                "tags": ["browser", "graph-ui"],
                "version": {"major": 0, "minor": 1}
            })),
            key: None,
            error: None,
            url: None,
            view: json_view::JsonView::default(),
        });
        self.place_node(None, json, Placement { group: None, pos: egui::vec2(0.0, -260.0) });
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

    /// Opens `session.mnf` under a fresh actor. A file that isn't a session
    /// file at all is set aside (`Journal::open_or_set_aside`) and a new one
    /// started, toasting where the old one went; that launch then counts as
    /// recovered (`store_recovered`), so nothing is pruned against the new,
    /// empty session. Any other failure is toasted
    /// and leaves the app running without a store.
    fn open_store(&mut self) {
        if let Some(path) = session::store_path() {
            self.open_store_at(&path);
        }
    }

    /// `open_store` for the store at `path`.
    fn open_store_at(&mut self, path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match store::journal::Journal::open_or_set_aside(path, store::model::ActorId::random()) {
            Ok((journal, aside)) => {
                self.journal = Some(journal);
                if let Some(aside) = aside {
                    // The new, empty session names none of the old one's fetched content or jars: pruning
                    // against it this launch would delete what the set-aside file still needs.
                    self.store_recovered = true;
                    self.queue_toast(format!(
                        "The session file wasn't readable; it was kept (with its .prev/.local) as {} and a new one \
                         started",
                        aside.display()
                    ));
                }
            }
            Err(err) => self.queue_toast(format!("Couldn't open the session file {}: {err}", path.display())),
        }
    }

    /// Removes a download or note node from the graph -- a download's file
    /// on disk is left untouched (this only forgets the node), and neither
    /// kind has anything else to persist/tear down on removal the way a
    /// tab's engine process or a group's membership does, so one plain
    /// `self.nodes`/`node_order` removal (plus the same stale-`dragging`/
    /// `hovered` guard `destroy_tab`/`finish_swap` also apply) covers both.
    ///
    /// Removing the Json node whose level is being viewed (another replica
    /// deleted it, or its group was closed) leaves to that node's group,
    /// as `dissolve_group` does for a group: `current_group` must never
    /// name a node that's gone, or `insertion_group` would orphan new nodes.
    ///
    /// What it opened (a tab from Open as Tab) is then opened from its own
    /// `parent`, as `remove_tab` does for a tab's children.
    fn remove_node(&mut self, id: &str) {
        let removed = self.nodes.remove(id);
        let parent = removed.as_ref().and_then(|n| n.parent.clone());
        for node in self.nodes.values_mut().filter(|n| n.parent.as_deref() == Some(id)) {
            node.parent = parent.clone();
        }
        if removed.as_ref().is_some_and(|n| n.as_pdf().is_some()) {
            self.pdf_renderer.cancel(id);
        }
        if self.current_group.as_deref() == Some(id) {
            self.switch_group_level(removed.and_then(|n| n.group));
        }
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
        // Off Windows a download's path is already native (WebKit is not run through WSL there).
        if cfg!(not(windows)) {
            return Some(download.path.clone());
        }
        Some(if let Some(posix) = download.path.strip_prefix('/') {
            format!("\\\\wsl.localhost\\Ubuntu\\{}", posix.replace('/', "\\"))
        } else {
            download.path.clone()
        })
    }

    /// Opens `path`'s containing folder with the file itself pre-selected --
    /// the mechanism behind `reveal_node`.
    #[cfg(windows)]
    fn reveal_in_explorer(path: &str) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        if path.is_empty() {
            return Err("This download has no file path".to_string());
        }
        // `raw_arg`, not `arg`: `arg` would escape the quotes `explorer_select_arg` adds.
        std::process::Command::new("explorer.exe")
            .raw_arg(explorer_select_arg(path))
            .spawn()
            .map(drop)
            .map_err(|e| format!("Couldn't open Explorer: {e}"))
    }
    /// Elsewhere there is no portable "select this file", so the containing folder is opened with the desktop's
    /// own handler (`xdg-open` on Linux, `open` on macOS).
    #[cfg(not(windows))]
    fn reveal_in_explorer(path: &str) -> Result<(), String> {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let folder = std::path::Path::new(path).parent().ok_or_else(|| format!("{path} has no containing folder"))?;
        std::process::Command::new(opener)
            .arg(folder)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't open {}: {opener}: {e}", folder.display()))
    }

    /// The real file behind a Download/Image/Pdf/Json node, in the form
    /// Explorer and a pasted clipboard path understand -- what both
    /// "Open Containing Folder" (`reveal_node`) and "Copy Path" use. A
    /// download's path may be a WSL-side POSIX path (WebKit downloads), so
    /// it goes through `windows_path_for_download`; an image/PDF/JSON path
    /// always came from a real Windows file picker, so it's used as-is.
    /// `None` for kinds with no backing file (Tab, Group, Note), and for an
    /// empty path: "no file" (a Download from a `Trust::Foreign` import has
    /// its machine-local path dropped).
    fn node_file_path(&self, id: &str) -> Option<String> {
        let path = match &self.nodes.get(id)?.payload {
            NodePayload::Download(_) => self.windows_path_for_download(id),
            NodePayload::Image(image) => Some(image.path.clone()),
            NodePayload::Pdf(pdf) => Some(pdf.path.clone()),
            NodePayload::Json(json) => Some(json.path.clone()),
            NodePayload::Tab(_) | NodePayload::Unloaded(_) | NodePayload::Group(_) | NodePayload::Note(_) => None,
        };
        path.filter(|p| !p.is_empty())
    }

    /// Opens a node's file's containing folder with the file pre-selected
    /// (a plain click on a download card does this too -- same "just show
    /// the obvious thing, no custom UI" posture as the download itself, see
    /// `EngineToHost::DownloadCompleted`'s doc comment). No-op for kinds
    /// with no file; a file node with no path (`node_file_path`) is toasted.
    fn reveal_node(&mut self, id: &str) {
        let Some(node) = self.nodes.get(id) else { return };
        let has_file = matches!(
            node.payload,
            NodePayload::Download(_) | NodePayload::Image(_) | NodePayload::Pdf(_) | NodePayload::Json(_)
        );
        match self.node_file_path(id) {
            Some(path) => {
                if let Err(e) = Self::reveal_in_explorer(&path) {
                    self.queue_toast(e);
                }
            }
            None if has_file => self.queue_toast("This file's path isn't known on this machine".to_string()),
            None => {}
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
                NodePayload::Group(group) => Some(group.name.as_str()), // name rule: compares stored names
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
    ///
    /// A local dissolve: unset tabs among the members re-snapshot their jar from the new level (deviation 12).
    fn dissolve_group(&mut self, id: &str) {
        for member in self.dissolve_group_quietly(id) {
            self.refollow(&member);
        }
    }

    /// `dissolve_group` without the jar re-snapshot, for a remote delete: the remote device's own refollow arrives as
    /// ordinary tab edits (spec "Inheritance"). Returns the members moved.
    fn dissolve_group_quietly(&mut self, id: &str) -> Vec<String> {
        let parent = self.nodes.get(id).and_then(|n| n.group.clone());
        let mut members = Vec::new();
        for (member, node) in self.nodes.iter_mut() {
            if node.group.as_deref() == Some(id) {
                node.group = parent.clone();
                members.push(member.clone());
            }
        }
        self.nodes.remove(id);
        self.node_order.retain(|g| g != id);
        if self.current_group.as_deref() == Some(id) {
            self.switch_group_level(parent);
        }
        self.forget_removed_group(id);
        members
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

    /// The shared fold behind both of the above (and `level_items`'
    /// world bounds more generally): every rect's union, expanded by a
    /// flat margin, or `default_graph_scene_rect()` when there's nothing
    /// to fit.
    pub(crate) fn bounds_for_rects(&self, rects: impl IntoIterator<Item = egui::Rect>) -> egui::Rect {
        let bounds = rects.into_iter().fold(egui::Rect::NOTHING, egui::Rect::union);
        if !bounds.is_finite() {
            return default_graph_scene_rect();
        }
        bounds.expand(GRAPH_FIT_MARGIN)
    }

    /// Graph view's own `Escape` handling / headless `DISMISSPEEK` —
    /// dismisses the currently-hovered node's peek preview early, without
    /// needing the pointer to move away first. See `peek_suppressed`'s own
    /// doc comment for why this can't just clear `hovered` directly.
    fn dismiss_hover_peek(&mut self) {
        self.peek_suppressed = self.hovered.clone();
    }

    /// Shared by `new_group_shortcut` (`pos` `None`: a jittered spawn) and
    /// the empty-canvas "New Tab/Note/Group/File" card (`pending_new_node`),
    /// which passes the exact position that was right-clicked instead.
    /// Doesn't save; the caller does.
    fn insert_group_at(&mut self, id: String, pos: Option<egui::Vec2>) {
        let name = self.default_group_name();
        let at = self.placement(&id, pos);
        self.place_node(Some(id), NodePayload::Group(GroupPayload::named(name)), at);
    }

    /// Like `insert_group_at`, for `new_note_shortcut` and the card's note.
    /// Doesn't save; the caller does.
    fn insert_note_at(&mut self, id: String, pos: Option<egui::Vec2>) {
        let at = self.placement(&id, pos);
        let note = NotePayload { text: String::new(), editing: true, url: None };
        self.place_node(Some(id), NodePayload::Note(note), at);
    }

    /// Starts an unloaded tab's engine (`UnloadedTabPayload`) on `kind`.
    /// The card stays, showing a spinner, until the outcome replaces it:
    /// the live tab under the same id, or a failed card with the new
    /// error. Ignored while a wake is already in flight.
    pub(crate) fn wake_tab(&mut self, id: &str, kind: EngineKind) {
        let Some(node) = self.nodes.get_mut(id) else { return };
        let (parent, group, pos, color) = (node.parent.clone(), node.group.clone(), node.pos, node.color);
        let NodePayload::Unloaded(tab) = &mut node.payload else { return };
        if tab.state == UnloadedState::Waking {
            return;
        }
        tab.state = UnloadedState::Waking;
        self.wakes_in_flight.insert(id.to_string());
        let carry = TabCarry {
            pinned: tab.pinned,
            color,
            zoom_factor: tab.zoom_factor,
            title: tab.title.clone(),
            muted: tab.muted,
            jar: tab.jar,
        };
        let url = tab.url.clone();
        let vel = egui::Vec2::ZERO;
        let (id, purpose, fallback_kind) = (id.to_string(), SpawnPurpose::Wake, None);
        self.respawn_tab(TabSpawn { id, url, kind, parent, group, pos, vel, purpose, carry, fallback_kind });
    }

    /// `Ctrl+Shift+W` / headless `CLOSEALLLEVEL` — closes every tab
    /// `nodes_at_current_level(Tab)` returns (top level: every ungrouped tab,
    /// leaving groups and their members untouched; inside a group: every
    /// tab member: the tabs that group's own "Close Group and Contents"
    /// context-menu entry would close, though that entry also closes the
    /// group and its other members). Each closes through the
    /// normal `close_node` path, same "just that action run once per tab"
    /// shape `close_all_except` already has.
    fn close_all_at_current_level(&mut self) {
        for id in self.tabs_at_current_level() {
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
        let png = temp_png.to_string_lossy().into_owned();
        self.pending_screenshot = Some(png.clone());
        self.pending_print_to_pdf = Some((png, pdf_path));
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
    /// for anything already muted. Includes sleeping and waking tabs (no
    /// engine to send `SetMuted` to, but a wake's landing already sends it
    /// from the card's own `muted`, and personal state saves it either
    /// way -- see `UnloadedTabPayload::muted`).
    fn mute_all_except(&mut self, keep_id: &str) {
        let other_ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|(id, n)| id.as_str() != keep_id && n.payload.saved_tab().is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in other_ids {
            let Some(node) = self.nodes.get_mut(&id) else { continue };
            match &mut node.payload {
                NodePayload::Tab(tab) if !tab.muted => {
                    tab.muted = true;
                    tab.tab.send(&HostToEngine::SetMuted { muted: true });
                }
                NodePayload::Unloaded(tab) => tab.muted = true,
                _ => {}
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
            .filter(|id| {
                id.as_str() != keep_id && self.nodes.get(id.as_str()).is_some_and(|n| n.payload.saved_tab().is_some())
            })
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
        for pinned in self.nodes.values_mut().filter_map(|n| n.payload.pinned_mut()) {
            *pinned = pin;
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
    /// Image/Pdf, then `remove_node`s it.
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
            ItemNodeKind::Tab | ItemNodeKind::Unloaded => self.destroy_tab(id),
            ItemNodeKind::Download
            | ItemNodeKind::Note
            | ItemNodeKind::Image
            | ItemNodeKind::Pdf
            | ItemNodeKind::Json => {
                if matches!(kind, ItemNodeKind::Image | ItemNodeKind::Pdf) {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.free_image(id);
                    }
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
                ItemNodeKind::Download
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

    /// A brief bottom-left toast, for `self.save_toast` — cleared once
    /// `SAVE_TOAST_DURATION` elapses. No dismiss interaction (unlike the
    /// context menu): it just times out, and clicks pass through it to the
    /// card or page underneath. Fades in over its first
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

    /// `url` short enough for a toast: its first `TOAST_URL_CHARS` characters and an ellipsis. A toast sizes to its
    /// text, and a `data:` URL can be megabytes.
    fn toast_url(url: &str) -> String {
        const TOAST_URL_CHARS: usize = 80;
        match url.char_indices().nth(TOAST_URL_CHARS) {
            Some((at, _)) => format!("{}\u{2026}", &url[..at]),
            None => url.to_string(),
        }
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
        ui.ctx().request_repaint();
        let fade_in = (elapsed.as_secs_f32() / TOAST_FADE_IN.as_secs_f32()).min(1.0);
        let remaining = SAVE_TOAST_DURATION.saturating_sub(elapsed);
        let fade_out = (remaining.as_secs_f32() / TOAST_FADE_OUT.as_secs_f32()).min(1.0);
        let opacity = fade_in.min(fade_out);
        let message = message.clone();
        egui::Area::new(egui::Id::new("save_toast"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
            .order(egui::Order::Foreground)
            .interactable(false)
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
                    control_font(ui);
                    ui.label(message);
                });
                let shapes = card_glow_shapes(response.response.rect, 3.0, ACCENT, 1.0, 1.0);
                for (slot, shape) in glow_slots.into_iter().zip(shapes) {
                    ui.painter().set(slot, shape);
                }
            });
    }

    /// One frame of UI. An open anchored popup takes Escape (the views skip theirs while one is
    /// open, `popup_open`), and when it closes from the keyboard focus goes back to its opening button.
    fn build_ui(&mut self, ui: &mut egui::Ui) {
        self.engine_state_cache.borrow_mut().clear();
        let open_at_start = self.open_popup;
        let escape = open_at_start.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape));
        self.build_ui_inner(ui);
        if escape && self.open_popup == open_at_start {
            self.open_popup = None;
        }
        if let Some(which) = open_at_start.filter(|_| self.open_popup.is_none()) {
            if !ui.input(|i| i.pointer.any_click()) {
                if let Some(button) = ui.data(|d| d.get_temp::<egui::Id>(App::popup_opener_key(which))) {
                    ui.memory_mut(|m| m.request_focus(button));
                }
            }
        }
    }

    fn build_ui_inner(&mut self, ui: &mut egui::Ui) {
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
        // `render_frame` — see `transition_content_rects_points`.
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
                    .image(tex_id, rect, crate::card::FULL_UV, egui::Color32::WHITE);
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
        self.build_engine_confirm_popup(ui);
        self.build_jar_confirm_popup(ui);
        self.build_session_delete_popup(ui);
    }

    /// The "Keyboard Shortcuts (F1)" half of `render_universal_chip`.
    /// Returns its own rect, which the shortcuts-help popup anchors to.
    fn build_help_button(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let help_response =
            chip_icon_button(ui, egui::Id::new("icon_btn_shortcuts_help"), egui_phosphor::light::QUESTION)
                .named_tip("Keyboard Shortcuts (F1)");
        if help_response.clicked() {
            self.shortcuts_help_open = !self.shortcuts_help_open;
        }
        help_response.rect
    }

    /// The Settings-gear half of `render_universal_chip`.
    fn build_settings_button(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let response = chip_icon_button(ui, egui::Id::new("icon_btn_settings_gear"), egui_phosphor::light::GEAR)
            .named_tip("Settings");
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
    crash_report::install();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }

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
    // line up with where the content actually is on screen. Windows-only:
    // winit already reports the real scale factor on X11/Wayland.
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
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
    std::thread::spawn(process_manager::sweep_cef_cache);
    std::thread::spawn(process_manager::sweep_webview2_profiles);
    std::thread::spawn(process_manager::sweep_frame_files);

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
    drop(app);
    process_manager::finish_cache_removals();
}

/// Explorer's `/select,` argument for `path`, quoted: Explorer splits an
/// unquoted argument at commas, so `a,b.pdf` opened the wrong folder.
#[cfg(windows)]
fn explorer_select_arg(path: &str) -> String {
    format!("/select,\"{path}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = r#"[{"name":"sid","value":"1","domain":"a.com","path":"/","expires":null,"secure":false,"http_only":true,"same_site":"Lax"}]"#;

    #[cfg(windows)]
    #[test]
    fn explorer_select_quotes_the_path() {
        assert_eq!(explorer_select_arg(r"C:\d\a,b.pdf"), r#"/select,"C:\d\a,b.pdf""#);
    }

    fn with_fonts(f: impl FnOnce(&egui::Context)) {
        let ctx = egui::Context::default();
        // `run_ui` takes an `FnMut`; the test body runs once.
        let mut f = Some(f);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                f(ui.ctx());
            }
        });
        // The font atlas upload is never applied here; dropping it unapplied is a debug assertion.
        output.textures_delta.clear();
    }

    #[test]
    fn a_cut_from_the_start_keeps_a_true_suffix_on_grapheme_boundaries() {
        use unicode_segmentation::UnicodeSegmentation;
        with_fonts(|ctx| {
            let mut cache = HashMap::new();
            let font = egui::FontId::proportional(14.0);
            let text = format!("{}e\u{301}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}tail.jpg", "dir\\".repeat(30));
            let cut = truncate_start_to_width(&mut cache, ctx, &text, &font, 80.0);
            let kept = cut.strip_prefix('\u{2026}').expect("cut at the start");
            assert!(text.ends_with(kept), "a true suffix, not a re-reversed one: {cut}");
            let start = text.len() - kept.len();
            assert!(text.grapheme_indices(true).any(|(i, _)| i == start), "starts on a grapheme boundary");
        });
    }

    #[test]
    fn a_cut_from_the_end_never_splits_a_grapheme() {
        use unicode_segmentation::UnicodeSegmentation;
        with_fonts(|ctx| {
            let mut cache = HashMap::new();
            let font = egui::FontId::proportional(14.0);
            // A ZWJ family, and a base letter with a combining mark; every width, so some cut lands
            // inside a sequence if the search counts chars.
            for text in ["\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".repeat(40), "e\u{301}".repeat(60)] {
                for width in (8..=200).step_by(4) {
                    let cut = truncate_to_width(&mut cache, ctx, &text, &font, width as f32);
                    let kept = cut.strip_suffix('\u{2026}').expect("cut at the end");
                    assert!(
                        text.grapheme_indices(true).any(|(i, _)| i == kept.len()) || kept.is_empty(),
                        "width {width}: {kept:?}"
                    );
                }
            }
        });
    }

    #[test]
    fn a_repeated_cut_is_one_cache_entry() {
        with_fonts(|ctx| {
            let mut cache = HashMap::new();
            let font = egui::FontId::proportional(14.0);
            let a = truncate_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
            let b = truncate_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
            assert_eq!((a, cache.len()), (b, 1));
            truncate_start_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
            assert_eq!(cache.len(), 2, "start and end cuts are cached apart");
        });
    }

    #[test]
    fn window_geometry_is_saved_after_a_quiet_spell_not_per_event() {
        let mut app = App::default();
        let t0 = std::time::Instant::now();
        app.note_settings_changed();
        assert!(!app.flush_settings_if_due(t0), "not at once");
        app.note_settings_changed();
        assert!(app.flush_settings_if_due(t0 + SETTINGS_SAVE_DELAY * 2));
        assert!(!app.flush_settings_if_due(t0 + SETTINGS_SAVE_DELAY * 3), "flushed once");
        app.note_settings_changed();
        app.save_settings();
        assert!(!app.flush_settings_if_due(t0 + SETTINGS_SAVE_DELAY * 4), "an immediate save covers a pending one");
    }

    fn sid_key() -> jar::Key {
        ("sid".into(), "a.com".into(), "/".into())
    }

    fn on_default(app: &mut App, tab: &str) {
        app.jar_tabs.insert(tab.into(), (jar::JarKey::Default, jar::TabCookies::default()));
    }

    fn default_jar(app: &App) -> &jar::Jar {
        &app.jars[&jar::JarKey::Default]
    }

    #[test]
    fn pulled_cookies_reach_the_jar_and_a_logout_removes_them() {
        let mut app = App::default();
        on_default(&mut app, "a");
        assert!(app.apply_pulled_cookies("a", SID));
        assert_eq!(default_jar(&app).cookies.len(), 1);
        assert!(app.apply_pulled_cookies("a", "[]"), "sid was known to a, so its absence is a logout");
        assert!(default_jar(&app).cookies.is_empty());
        assert!(!app.apply_pulled_cookies("a", "not json"), "a garbled pull changes nothing");
    }

    #[test]
    fn a_tab_that_never_saw_a_cookie_cannot_delete_it() {
        let mut app = App::default();
        // Tab b was opened before a logged in, so it was seeded with nothing.
        on_default(&mut app, "a");
        on_default(&mut app, "b");
        assert!(app.apply_pulled_cookies("a", SID));
        assert!(!app.apply_pulled_cookies("b", "[]"), "b never saw sid: its pull must not delete it");
        assert_eq!(default_jar(&app).cookies.len(), 1);
        assert!(app.apply_pulled_cookies("a", "[]"), "a logout seen by a (sid in a's known) does delete it");
        assert!(default_jar(&app).cookies.is_empty());
    }

    #[test]
    fn a_seeded_tab_deletes_a_cookie_only_once_it_has_reported_it() {
        let mut app = App::default();
        on_default(&mut app, "a");
        app.apply_pulled_cookies("a", SID);
        let seeded = jar::TabCookies::seeded(&default_jar(&app).cookies);
        app.jar_tabs.insert("b".into(), (jar::JarKey::Default, seeded));
        assert!(!app.apply_pulled_cookies("b", "[]"), "the seed may have failed: an empty first pull is no logout");
        assert_eq!(default_jar(&app).cookies.len(), 1);
        assert!(!app.apply_pulled_cookies("b", SID), "b now holds sid, unchanged from its seed");
        assert!(app.jar_tabs["b"].1.reported.contains(&sid_key()));
        assert!(app.apply_pulled_cookies("b", "[]"), "b held sid and lost it: a logout");
        assert!(default_jar(&app).cookies.is_empty());
    }

    #[test]
    fn a_pull_merges_into_the_jar_the_tab_was_spawned_with() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        app.ensure_jar_loaded(jar::JarKey::Named(work));
        app.jar_tabs.insert("a".into(), (jar::JarKey::Named(work), jar::TabCookies::default()));
        assert!(app.apply_pulled_cookies("a", SID));
        assert_eq!(app.jars[&jar::JarKey::Named(work)].cookies.len(), 1);
        assert!(app.jars.get(&jar::JarKey::Default).is_none_or(|j| j.cookies.is_empty()), "default untouched");
    }

    #[test]
    fn a_pull_with_no_entry_or_a_deleted_jar_is_dropped() {
        let mut app = App::default();
        assert!(!app.apply_pulled_cookies("nobody", SID), "no jar_tabs entry: dropped (deviation 5)");
        let work = app.create_jar("Work");
        app.jar_tabs.insert("a".into(), (jar::JarKey::Named(work), jar::TabCookies::default()));
        app.forget_jar(work);
        assert!(!app.apply_pulled_cookies("a", SID), "its jar was deleted while it ran: dropped");
        assert!(app.jars.values().all(|j| j.cookies.is_empty()));
    }

    #[test]
    fn resolved_jar_falls_back_to_default_and_flags_a_missing_jar() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let gone = store::model::NodeId { actor: store::model::ActorId(5), counter: 5 };
        let pick = |jar| jar::JarChoice { jar, set: true };
        assert_eq!(app.resolved_jar(&pick(None)), (jar::JarKey::Default, false));
        assert_eq!(app.resolved_jar(&pick(Some(work))), (jar::JarKey::Named(work), false));
        assert_eq!(app.resolved_jar(&pick(Some(gone))), (jar::JarKey::Default, true));
    }

    #[test]
    fn an_ephemeral_jar_never_writes_a_file() {
        let dir = store::testutil::temp_dir("jar-ephemeral");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let tmp = app.create_jar("Throwaway");
        app.set_jar_ephemeral(tmp, true);
        app.ensure_jar_loaded(jar::JarKey::Named(tmp));
        app.jar_tabs.insert("a".into(), (jar::JarKey::Named(tmp), jar::TabCookies::default()));
        assert!(app.apply_pulled_cookies("a", SID));
        assert!(!jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists());
        app.set_jar_ephemeral(tmp, false);
        assert!(jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists(), "turned off: saved (deviation 10)");
        app.set_jar_ephemeral(tmp, true);
        assert!(!jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists(), "turned on: file deleted");
        assert_eq!(app.jars[&jar::JarKey::Named(tmp)].cookies.len(), 1, "cookies kept in memory until quit");
    }

    #[test]
    fn deleting_a_jar_deletes_its_file_and_clearing_empties_it() {
        let dir = store::testutil::temp_dir("jar-delete-clear");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let work = app.create_jar("Work");
        let key = jar::JarKey::Named(work);
        app.ensure_jar_loaded(key);
        app.jar_tabs.insert("a".into(), (key, jar::TabCookies::default()));
        app.apply_pulled_cookies("a", SID);
        assert!(jar::Jar::path(&dir, key).exists());
        app.clear_jar_cookies(key);
        assert!(app.jars[&key].cookies.is_empty());
        assert!(jar::Jar::load(&dir, key).cookies.is_empty(), "cleared on disk too");
        app.delete_jar(work);
        assert!(!jar::Jar::path(&dir, key).exists());
        assert!(!app.jars.contains_key(&key));
    }

    /// A jar `Work` whose file holds one cookie, saved by an earlier run: this run has not loaded it.
    fn jar_saved_earlier(name: &str) -> (std::path::PathBuf, App, store::model::NodeId) {
        let dir = store::testutil::temp_dir(name);
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let work = app.create_jar("Work");
        let mut jar = jar::Jar::default();
        jar.cookies = jar::from_json(SID).unwrap();
        jar.save(&dir, jar::JarKey::Named(work)).unwrap();
        (dir, app, work)
    }

    /// Final review I1: ephemeral on, then off, for a jar not loaded this run must not lose its saved logins: on
    /// loads them first (they stay in memory until quit), off saves them back.
    #[test]
    fn ephemeral_on_then_off_keeps_an_unloaded_jars_cookies() {
        let (dir, mut app, work) = jar_saved_earlier("jar-ephemeral-unloaded");
        let key = jar::JarKey::Named(work);
        app.set_jar_ephemeral(work, true);
        assert!(!jar::Jar::path(&dir, key).exists(), "on: the file goes");
        assert_eq!(app.jars[&key].cookies.len(), 1, "but its cookies were loaded first");
        assert_eq!(app.jar_snapshots.lock().unwrap().seed(key).0.len(), 1, "and seed tabs");
        app.set_jar_ephemeral(work, false);
        assert_eq!(jar::Jar::load(&dir, key).cookies.len(), 1, "off: saved back");
    }

    /// Final review I1: a locked jar's unreadable file (and its earlier copies) is never touched, ephemeral or not.
    #[test]
    fn ephemeral_on_leaves_a_locked_jars_files_alone() {
        let dir = store::testutil::temp_dir("jar-ephemeral-locked");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let work = app.create_jar("Work");
        let key = jar::JarKey::Named(work);
        // A directory where the file should be: reading it fails, so the jar loads locked.
        std::fs::create_dir(jar::Jar::path(&dir, key)).unwrap();
        let aside = dir.join(format!("{}.json.unreadable-100", key.file_stem()));
        std::fs::write(&aside, "x").unwrap();
        app.set_jar_ephemeral(work, true);
        assert!(app.jar_defs[&work].ephemeral);
        assert!(jar::Jar::path(&dir, key).is_dir() && aside.exists());
        app.set_jar_ephemeral(work, false);
        assert!(jar::Jar::path(&dir, key).is_dir() && aside.exists(), "off saves nothing over it");
    }

    /// Final review I1: Settings asks before making a jar with saved logins ephemeral; one without, or turning it
    /// off, acts at once.
    #[test]
    fn ephemeral_on_asks_first_only_when_the_jar_has_a_saved_file() {
        let (dir, mut app, work) = jar_saved_earlier("jar-ephemeral-confirm");
        app.request_jar_ephemeral(work, true);
        assert_eq!(app.pending_jar_confirm, Some(jar_defs::JarConfirm::Ephemeral(work)));
        assert!(!app.jar_defs[&work].ephemeral, "nothing happens until confirmed");
        assert!(jar::Jar::path(&dir, jar::JarKey::Named(work)).exists());
        let (title, detail, button) = app.jar_confirm_text(jar_defs::JarConfirm::Ephemeral(work)).unwrap();
        assert_eq!((title.as_str(), button), ("Make Work ephemeral?", "Make Ephemeral"));
        assert!(detail.contains("deleted from disk now") && detail.contains("until Manifold quits"), "{detail}");
        app.pending_jar_confirm = None;
        let home = app.create_jar("Home");
        app.request_jar_ephemeral(home, true);
        assert!(app.pending_jar_confirm.is_none() && app.jar_defs[&home].ephemeral, "no file: nothing to lose");
        app.request_jar_ephemeral(home, false);
        assert!(!app.jar_defs[&home].ephemeral);
        app.set_jar_ephemeral(work, true);
        assert!(app.jar_confirm_text(jar_defs::JarConfirm::Ephemeral(work)).is_none(), "already ephemeral");
    }

    /// Final review I2: an unreadable copy or a crashed save's temp file is a full copy of the jar's credentials, so
    /// deleting the jar takes them too. A live process's temp file and other jars' files stay.
    #[test]
    fn deleting_a_jar_removes_its_unreadable_copies_and_dead_temp_files() {
        let dir = store::testutil::temp_dir("jar-delete-asides");
        let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
        let work = app.create_jar("Work");
        let home = app.create_jar("Home");
        let (w, h) = (jar::JarKey::Named(work).file_stem(), jar::JarKey::Named(home).file_stem());
        let live = format!("{w}.json.{}.tmp", std::process::id());
        let gone = [format!("{w}.json"), format!("{w}.json.unreadable-100"), format!("{w}.json.4294967295.tmp")];
        let kept = [live, format!("{h}.json"), format!("{h}.json.unreadable-100"), "default.json".to_string()];
        for name in gone.iter().chain(&kept) {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        app.delete_jar(work);
        for name in &gone {
            assert!(!dir.join(name).exists(), "{name} should be deleted");
        }
        for name in &kept {
            assert!(dir.join(name).exists(), "{name} should be kept");
        }
    }

    #[test]
    fn a_sleeping_tab_never_needs_a_switch_and_a_deleted_jar_switches_to_default() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let t = crate::session_sync::tests::add(&mut app, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        // A stale entry from before it slept, on a key other than the one it now resolves to.
        app.jar_tabs.insert(t.clone(), (jar::JarKey::Default, jar::TabCookies::default()));
        app.set_tab_jar(&t, Some(work));
        assert!(!app.will_switch(&t), "asleep: it spawns into the new jar");
        app.jar_reseed.insert(t.clone());
        assert!(!app.will_switch(&t), "asleep: a reseed spawns into the cleared jar anyway");
        assert!(app.pending_sleeps.is_empty(), "the sweeps after each jar change asked nothing of it");
        // A host spawned with Work whose jar is then deleted must head for Default.
        app.jar_tabs.insert(t.clone(), (jar::JarKey::Named(work), jar::TabCookies::default()));
        app.forget_jar(work);
        assert_eq!(app.resolved_jar(&app.tab_jar(&t).unwrap()).0, jar::JarKey::Default);
    }

    #[test]
    fn clearing_a_jar_marks_its_running_tabs_for_a_reseed() {
        let mut app = App::default();
        app.jar_tabs.insert("a".into(), (jar::JarKey::Default, jar::TabCookies::default()));
        let work = app.create_jar("Work");
        app.jar_tabs.insert("b".into(), (jar::JarKey::Named(work), jar::TabCookies::default()));
        app.clear_jar_cookies(jar::JarKey::Default);
        assert!(app.jar_reseed.contains("a"));
        assert!(!app.jar_reseed.contains("b"), "another jar's tab keeps its host");
        app.clear_cookie_state("a");
        assert!(!app.jar_reseed.contains("a"));
    }

    /// Final review I3: a Clear Cookies landing after a spawning host read its seed, but before the host arrived, is
    /// honoured: the host holds the old cookies, so it is marked to switch like the hosts running at the clear, even
    /// though the landing first wipes its id's old cookie state.
    #[test]
    fn a_clear_during_a_spawn_marks_the_host_when_it_lands() {
        let key = jar::JarKey::Default;
        let mut app = App::default();
        on_default(&mut app, "a");
        app.apply_pulled_cookies("a", SID);
        app.clear_cookie_state("a");
        let read_seed = |app: &App| {
            let (cookies, clears) = app.jar_snapshots.lock().unwrap().seed(key);
            (jar::TabCookies::seeded(&cookies), Some(clears))
        };
        let (old_seed, before) = read_seed(&app);
        assert_eq!(old_seed.baseline.len(), 1);
        app.clear_jar_cookies(key);
        app.note_spawned_jar("t", key, old_seed.clone(), before);
        assert!(app.jar_reseed.contains("t"), "seeded before the clear: it holds the old cookies");
        assert!(!app.apply_pulled_cookies("t", SID), "so its pulls cannot refill the cleared jar");
        assert!(default_jar(&app).cookies.is_empty());

        let (empty_seed, after) = read_seed(&app);
        app.jar_reseed.insert("u".into()); // an earlier host's mark: the landing wipes it
        app.note_spawned_jar("u", key, empty_seed.clone(), after);
        assert!(!app.jar_reseed.contains("u"), "seeded after the clear: the empty jar");
        app.clear_jar_cookies(key);
        app.note_spawned_jar("v", key, empty_seed, after);
        assert!(!app.jar_reseed.contains("v"), "an empty seed holds nothing the clear took");
        app.note_spawned_jar("w", key, old_seed.clone(), None);
        assert!(!app.jar_reseed.contains("w"), "never seeded (its engine ignores jars)");
        let work = app.create_jar("Work");
        app.clear_jar_cookies(jar::JarKey::Named(work));
        app.note_spawned_jar("x", key, old_seed, after.map(|n| n + 1));
        assert!(!app.jar_reseed.contains("x"), "only its own jar's clears count");
    }

    /// The pull `request_sleep` (or a timed pull, or one already in flight) sends a reseed tab still returns the
    /// cookies its engine was seeded with; they must not refill the jar the user just cleared.
    #[test]
    fn a_cleared_jar_is_not_refilled_by_its_old_tabs_pulls() {
        let mut app = App::default();
        on_default(&mut app, "a");
        app.clear_jar_cookies(jar::JarKey::Default);
        assert!(!app.apply_pulled_cookies("a", SID), "a host still holding the old cookies: dropped");
        assert!(default_jar(&app).cookies.is_empty());
        app.clear_cookie_state("a"); // its replacement host landed
        on_default(&mut app, "a");
        assert!(app.apply_pulled_cookies("a", SID), "the new host's pulls count again");
    }

    #[test]
    fn a_pending_pull_blocks_another_until_it_times_out() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let pulls: HashMap<String, Instant> = [("t".to_string(), now)].into();
        assert!(tabs::pull_pending(&pulls, "t", now + Duration::from_secs(1)));
        assert!(!tabs::pull_pending(&pulls, "t", now + Duration::from_secs(11)), "a silent host is asked again");
        assert!(!tabs::pull_pending(&pulls, "other", now));
    }

    fn sleep_answer() -> tabs::Held {
        tabs::Held::PageState(ipc_protocol::PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false })
    }

    fn swap_answer() -> tabs::Held {
        tabs::Held::Snapshot(process_manager::EngineKind::Cef, "http://x/".into())
    }

    #[test]
    fn a_second_hold_cancels_the_first_so_the_latest_request_wins() {
        let mut app = App::default();
        app.jar_pulls.insert("t".into(), std::time::Instant::now());
        // Sleep, then swap: only the swap stays parked, and the sleep's pending entry is gone (no timeout sleep).
        app.pending_sleeps.insert(
            "t".into(),
            sleep_state::PendingSleep { why: sleep_state::SleepWhy::Manual, sent: std::time::Instant::now() },
        );
        assert!(app.hold_for_cookies("t", sleep_answer()).is_none());
        assert!(app.hold_for_cookies("t", swap_answer()).is_none());
        assert!(matches!(app.cookie_holds["t"].1, tabs::Held::Snapshot(..)));
        assert!(app.pending_sleeps.is_empty(), "the cancelled sleep left nothing behind");
        // Swap, then sleep: only the sleep stays parked.
        assert!(app.hold_for_cookies("t", sleep_answer()).is_none());
        assert!(matches!(app.cookie_holds["t"].1, tabs::Held::PageState(..)));
        assert_eq!(app.cookie_holds.len(), 1);
    }

    #[test]
    fn a_respawn_drops_a_held_answer_meant_for_the_old_host() {
        let mut app = App::default();
        app.jar_pulls.insert("t".into(), std::time::Instant::now());
        app.pending_sleeps.insert(
            "t".into(),
            sleep_state::PendingSleep { why: sleep_state::SleepWhy::Manual, sent: std::time::Instant::now() },
        );
        assert!(app.hold_for_cookies("t", sleep_answer()).is_none());
        app.clear_cookie_state("t"); // what a spawn result for the same id does first
        assert!(app.cookie_holds.is_empty() && app.jar_pulls.is_empty() && app.pending_sleeps.is_empty());
        app.release_expired_cookie_holds();
        app.release_cookie_hold("t"); // nothing left to run against the new host
    }

    #[test]
    fn closing_a_tab_clears_its_cookie_bookkeeping() {
        use crate::session_sync::tests::{add, sleeping_tab};
        let mut app = App::default();
        let tab = add(&mut app, sleeping_tab("https://x.test/", "A"));
        app.jar_tabs.insert(tab.clone(), (jar::JarKey::Default, jar::TabCookies::default()));
        app.jar_pulls.insert(tab.clone(), std::time::Instant::now());
        app.jar_silent.insert(tab.clone(), 2);
        app.cookie_holds.insert(
            tab.clone(),
            (std::time::Instant::now(), tabs::Held::Snapshot(process_manager::EngineKind::Cef, String::new())),
        );
        app.remove_tab(&tab);
        assert!(
            !app.jar_tabs.contains_key(&tab)
                && !app.jar_pulls.contains_key(&tab)
                && !app.jar_silent.contains_key(&tab)
                && !app.cookie_holds.contains_key(&tab)
        );
    }

    #[test]
    fn a_host_that_never_answers_is_given_up_on_after_three_pulls() {
        use std::time::{Duration, Instant};
        let (mut pulls, mut silent) = (HashMap::new(), HashMap::new());
        let t0 = Instant::now();
        assert!(tabs::start_pull(&mut pulls, &mut silent, "t", t0));
        assert!(!tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(1)), "pending");
        assert!(tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(30)), "second ask");
        assert!(tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(60)), "third ask");
        assert!(!tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(90)), "three silences: stop");
        assert!(!pulls.contains_key("t"), "so no hold waits on it");
        assert!(!tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(300)));
        silent.clear(); // a reply (or a respawn) resets it
        assert!(tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(400)));
    }

    #[test]
    fn a_sleep_answer_is_held_for_a_pending_pull_and_released_by_it() {
        let mut app = App::default();
        let state = || ipc_protocol::PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false };
        assert!(app.hold_for_cookies("t", tabs::Held::PageState(state())).is_some(), "no pull pending: act now");
        app.jar_pulls.insert("t".into(), std::time::Instant::now());
        assert!(app.hold_for_cookies("t", tabs::Held::PageState(state())).is_none());
        assert!(app.cookie_holds.contains_key("t"));
        app.release_cookie_hold("t");
        assert!(app.cookie_holds.is_empty());
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
        let group = |name: &str| NodePayload::Group(GroupPayload::named(name));
        let note = || NodePayload::Note(NotePayload { text: String::new(), editing: false, url: None });
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
            let payload = NodePayload::Group(GroupPayload::named(name));
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

    /// Name rules: a session name is cleaned, made file-safe, then cleaned again (spaces the filter leaves collapse).
    #[test]
    fn a_session_name_is_cleaned_then_made_file_safe() {
        assert_eq!(sanitize_session_name("  Trip\u{200B} to\u{00A0}\u{00A0}Rome: day 1 "), "Trip to Rome day 1");
        assert_eq!(sanitize_session_name("a . b"), "a b");
        assert_eq!(sanitize_session_name("\u{202E}..\\"), "");
        assert_eq!(sanitize_session_name(&"é".repeat(80)).chars().count(), 64);
    }

    /// Save As (Settings or headless `SAVESESSIONAS`) with nothing left of the name does nothing; before, the headless
    /// path skipped `sanitize_session_name`.
    #[test]
    fn save_as_with_nothing_left_of_the_name_does_nothing() {
        let mut app = App::default();
        app.save_named_session("\u{200B} ..");
        assert!(app.save_toast_queue.is_empty(), "{:?}", app.save_toast_queue);
        app.save_named_session("Trip");
        assert_eq!(app.save_toast_queue.len(), 1, "under test there is no session file");
    }

    /// A toast shows a long URL's start and an ellipsis, cut on a character boundary.
    #[test]
    fn a_toast_url_is_cut_to_eighty_characters() {
        assert_eq!(App::toast_url("https://a/"), "https://a/");
        let long = format!("data:text/plain,{}", "é".repeat(500));
        let shown = App::toast_url(&long);
        assert_eq!(shown.chars().count(), 81);
        assert!(shown.starts_with("data:text/plain,é") && shown.ends_with("é\u{2026}"), "{shown}");
        assert_eq!(App::toast_url(&"x".repeat(80)), "x".repeat(80));
    }

    /// Save As while startup's personal state is unread carries none into the copy; once read, it carries this
    /// profile's.
    #[test]
    fn save_as_while_personal_state_is_unread_carries_none() {
        let mut app = App::default();
        assert!(app.local_state_for_copy().is_some());
        app.local_unread = Some(session_sync::LocalUnread { retry_at: std::time::Instant::now(), fresh: false });
        assert_eq!(app.local_state_for_copy(), None);
    }

    /// Review fix: a Windows device name survives `sanitize_session_name` but can't be a file; Save As says so
    /// instead of "No session file".
    #[test]
    fn save_as_a_device_name_says_why() {
        let mut app = App::default();
        app.save_named_session("Con");
        assert_eq!(Vec::from(app.save_toast_queue.clone()), ["Not a session name: Con"]);
    }
}
