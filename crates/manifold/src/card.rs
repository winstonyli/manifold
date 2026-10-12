//! Graph cards: the shared frame (`draw_graph_card`) with its sizes,
//! styles, corner controls and drop rules, and each kind's body painter.

use super::*;
use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP, TYPE_IMAGE, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB};

/// Below this on-screen card width (in points), a node's label is hidden
/// entirely rather than truncated — a card this small can't fit more than
/// one or two letters plus "…" anyway (see `truncate_to_width`), and a
/// near-illegible sliver of text reads as noise, not information, at the
/// zoom levels this app's `GRAPH_ZOOM_MIN` allows.
pub(crate) const MIN_LABEL_CARD_SCREEN_WIDTH: f32 = 40.0;

pub(crate) const GRAPH_THUMB_MARGIN: f32 = 6.0;

pub(crate) const FAVICON_SIZE: egui::Vec2 = egui::vec2(16.0, 16.0);

/// A group card's border tints `DANGER_COLOR` once its member count exceeds
/// this threshold -- a cheap glanceable "this one's gotten big" cue. Only
/// applies while the group has no explicit tag color of its own (see its use
/// below) -- a deliberate user choice always wins over this automatic one,
/// same precedent tag colors already have over the plain default `ACCENT`
/// border.
pub(crate) const GROUP_OVERLOAD_THRESHOLD: usize = 8;

pub(crate) const GRAPH_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 110.0);

/// What a card's header says (`App::paint_card_header`).
pub(crate) struct CardHeader<'a> {
    /// `None`: the caller draws its own icon in the glyph's place (a live tab's favicon or engine dot).
    pub(crate) glyph: Option<&'static str>,
    /// The glyph's color; the dim text color if `None`.
    pub(crate) glyph_color: Option<egui::Color32>,
    /// Drawn in the glyph's place, if the card has one (a favicon).
    pub(crate) icon_image: Option<egui::TextureId>,
    /// A title from a page or a file, drawn as it is. Unused when `user_name` is set.
    pub(crate) name: String,
    /// A stored group name, already through the display rule (`names::display_name`), so a hidden character shows
    /// as a placeholder. The caller keeps the segments, so the rule runs once a frame for the tooltip and the header.
    pub(crate) user_name: Option<&'a [crate::names::NameSeg<'a>]>,
    pub(crate) source: String,
    /// `source` is a local path, so it is elided at the start (a web address, at the end).
    pub(crate) source_is_path: bool,
    /// Over a picture: a dark scrim behind light text. Otherwise the card's own fill.
    pub(crate) scrim: bool,
    /// Screen points the fallback strip leaves free at its right.
    pub(crate) right_margin: f32,
}

/// The file name and source line of a card that has a `path` and maybe a web `url`: the `url` if
/// there is one, else the `path`. The flag is whether the source is the path.
pub(crate) fn file_header_text(path: &str, url: Option<&str>) -> (String, String, bool) {
    let name = crate::content::file_name(path).unwrap_or_else(|| path.to_owned());
    (name, url.unwrap_or(path).to_owned(), url.is_none())
}

/// `text` cut to `max_width` screen points at the start ("…\\Trips\\a.jpg"), so a path's file name
/// stays visible. Shares `truncate_to_width`'s cache.
pub(crate) fn truncate_start_to_width(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
) -> String {
    truncate_core(cache, ctx, text, font_id, max_width, true)
}

/// Height of a card's header F (glyph or favicon, title, source line), in screen points.
pub(crate) const TAB_HEADER_POINTS: f32 = 40.0;
/// The strip a card too short for the header (`tab_has_header`) gets along its bottom instead, in screen points.
pub(crate) const TAB_STRIP_POINTS: f32 = 18.0;
/// Header F's layout, in screen points from the card's top-left. `paint_card_header` and the JSON
/// preview's own header (`json_level_ui::paint_json_content`) both place their text from these.
pub(crate) const HEADER_GLYPH_X: f32 = 10.0;
/// The line the glyph sits on, and the title's when there is no source line.
pub(crate) const HEADER_CENTER_Y: f32 = 20.0;
pub(crate) const HEADER_TEXT_X: f32 = 28.0;
pub(crate) const HEADER_TITLE_Y: f32 = 12.0;
pub(crate) const HEADER_SOURCE_Y: f32 = 28.0;
pub(crate) const HEADER_TITLE_SIZE: f32 = 13.0;
pub(crate) const HEADER_SOURCE_SIZE: f32 = 11.0;
/// Below this card height (screen points) the header gives way to the old strip over the thumbnail.
const TAB_HEADER_MIN_CARD_POINTS: f32 = 80.0;
/// Kept clear at a header's right edge, mirroring `HEADER_TEXT_X` at its left: the corner controls
/// (the audio indicator, the hover-revealed close button) sit there.
const HEADER_RIGHT_RESERVE: f32 = HEADER_TEXT_X;

/// The whole texture: UVs for an image drawn uncropped.
pub(crate) const FULL_UV: egui::Rect = egui::Rect { min: egui::Pos2::ZERO, max: egui::pos2(1.0, 1.0) };

/// Behind a header's text drawn over a picture: dark enough to read light text on any image.
const IMAGE_TEXT_SCRIM: egui::Color32 = egui::Color32::from_black_alpha(190);

/// Behind a short card's title strip along its bottom: a lighter scrim, the strip is small.
const TITLE_STRIP_SCRIM: egui::Color32 = egui::Color32::from_black_alpha(160);

/// Over a card the Graph toolbar filter doesn't match (`dim_if_filtered_out`).
const FILTER_DIM_SCRIM: egui::Color32 = egui::Color32::from_black_alpha(190);

/// An audio card control's waveform while the tab plays. A warm sage rather than a minty green:
/// still unambiguously "active/live" (the universal recording-light color family), just shifted to
/// sit with the rest of this app's warm palette instead of against it.
const WAVEFORM_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 190, 90);

/// A note's text on its pale paper.
const NOTE_TEXT: egui::Color32 = egui::Color32::from_black_alpha(220);

/// Whether a Tab card of `rect` (world units, `inv_zoom` world units per screen point) is tall
/// enough on screen to show its header.
pub(crate) fn tab_has_header(rect: egui::Rect, inv_zoom: f32) -> bool {
    rect.height() / inv_zoom >= TAB_HEADER_MIN_CARD_POINTS
}

/// The body of a card whose header (when `tall`) takes the top `TAB_HEADER_POINTS`: the whole
/// card when it has only the bottom strip.
pub(crate) fn card_body_rect(rect: egui::Rect, tall: bool, inv_zoom: f32) -> egui::Rect {
    let top = if tall { rect.top() + TAB_HEADER_POINTS * inv_zoom } else { rect.top() };
    egui::Rect::from_min_max(egui::pos2(rect.left(), top), rect.right_bottom())
}

/// Queues `text` at `size` screen points, centred on `pos` (world units).
fn push_centered_text(
    deferred_overlays: &mut DeferredOverlays,
    pos: egui::Pos2,
    text: impl Into<String>,
    size: f32,
    color: egui::Color32,
) {
    let text = text.into();
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        overlay_painter.text(project(pos), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(size), color);
    }));
}

/// A jar chip's height in screen points.
const JAR_CHIP_HEIGHT: f32 = 16.0;

/// A jar chip's dot radius, and its padding: before the first dot, after the name (or the last dot, with no name).
const JAR_CHIP_R: f32 = 4.0;
const JAR_CHIP_PAD: f32 = 5.0;
const JAR_CHIP_NAME_END: f32 = 6.0;
/// The gap between the jar's dot and its name.
const JAR_CHIP_NAME_GAP: f32 = 4.0;
/// The chip's black scrim, so its light name reads on a white page. The frame blends in linear light, which makes a
/// scrim look far thinner than its alpha suggests: measured on a white thumbnail (2026-10-03), alpha 210 came out
/// sRGB grey 123 (about 3.7:1 against the name), so it is 240 (about grey 69, 8:1).
const JAR_CHIP_SCRIM_ALPHA: u8 = 240;
/// Narrower than this, a chip's name (about one letter and "…") is left out rather than squeezed.
const JAR_CHIP_MIN_NAME: f32 = 14.0;

/// The gap between a jar's name and its marker (`JarLabel::marker_text`).
const JAR_CHIP_MARKER_GAP: f32 = 4.0;

/// What of a jar chip fits `max` screen points (`fit_jar_chip`): whether the old jar's dot and arrow are drawn
/// (`lead`), the width the name gets (`None`: no name), and whether the marker is drawn (always whole, with the name).
#[derive(Debug, PartialEq)]
struct ChipFit {
    lead: bool,
    name: Option<f32>,
    marker: bool,
}

/// Fits a chip whose lead is `lead`, name `name` and marker `marker` points wide (0: no marker) into `max`: whole if it
/// fits, else the name ellipsized (never the marker), else just the dots, else just the jar's dot (a waiting switch's
/// cue goes last but one), else nothing.
fn fit_jar_chip(max: f32, lead: f32, name: f32, marker: f32) -> Option<ChipFit> {
    let dots = |lead: f32| JAR_CHIP_PAD + lead + 2.0 * JAR_CHIP_R;
    let tail = if marker > 0.0 { JAR_CHIP_MARKER_GAP + marker } else { 0.0 };
    let room = max - dots(lead) - JAR_CHIP_NAME_GAP - tail - JAR_CHIP_NAME_END;
    if room >= name.min(JAR_CHIP_MIN_NAME) {
        Some(ChipFit { lead: lead > 0.0, name: Some(name.min(room)), marker: marker > 0.0 })
    } else if dots(lead) + JAR_CHIP_PAD <= max {
        Some(ChipFit { lead: lead > 0.0, name: None, marker: false })
    } else if dots(0.0) + JAR_CHIP_PAD <= max {
        Some(ChipFit { lead: false, name: None, marker: false })
    } else {
        None
    }
}

/// A jar chip with its top-left at `at` (screen), at most `max_width` wide (`fit_jar_chip`): a dot (outlined when
/// following the group, `warn`-coloured for a missing jar), the name and its marker on a dark scrim, after the old
/// jar's dot and an arrow while the tab waits to switch. The marker is styled as everywhere else
/// (`JarLabel::marker_format`), in `text` dimmed by the theme's weak text alpha: the scrim is always near-black, so
/// the panel's own weak colour would not suit it. Its hover text goes through `CardBody::tooltip`. Returns the scrim
/// drawn.
pub(crate) fn paint_jar_chip(
    painter: &egui::Painter,
    at: egui::Pos2,
    max_width: f32,
    chip: &crate::jar_defs::JarChip,
    font: egui::FontId,
    text: egui::Color32,
    warn: egui::Color32,
) -> Option<egui::Rect> {
    const R: f32 = JAR_CHIP_R;
    let cy = at.y + JAR_CHIP_HEIGHT / 2.0;
    let arrow = jar_chip_arrow(painter, chip, font.clone(), text);
    let weak = text.gamma_multiply(painter.ctx().global_style().visuals.weak_text_alpha);
    let (segs, name_format) = (chip.name.display(), chip.name.name_format(font.clone()));
    let name_job = |wrap: Option<f32>| {
        let mut job = egui::text::LayoutJob::default();
        crate::names::append_name(&mut job, &segs, &name_format, weak);
        if let Some(room) = wrap {
            job.wrap = egui::text::TextWrapping {
                max_width: room,
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
        }
        painter.layout_job(job)
    };
    let name = name_job(None);
    let marker = chip.name.marker_text().map(|m| {
        painter.layout_job(egui::text::LayoutJob::single_section(
            m,
            crate::jar_defs::JarLabel::marker_format(font.size, weak),
        ))
    });
    let lead = jar_dots_width(arrow.as_deref()) - 2.0 * R;
    let fit = fit_jar_chip(max_width, lead, name.size().x, marker.as_ref().map_or(0.0, |m| m.size().x))?;
    // name rule: a fitted width, not text
    let name = fit.name.map(|room| if room >= name.size().x { name } else { name_job(Some(room)) });
    let marker = marker.filter(|_| fit.marker);
    let lead = if fit.lead { lead } else { 0.0 };
    let tail = marker.as_ref().map_or(0.0, |m| JAR_CHIP_MARKER_GAP + m.size().x);
    let width = match &name {
        Some(name) => JAR_CHIP_PAD + lead + 2.0 * R + JAR_CHIP_NAME_GAP + name.size().x + tail + JAR_CHIP_NAME_END,
        None => JAR_CHIP_PAD + lead + 2.0 * R + JAR_CHIP_PAD,
    };
    let scrim = egui::Rect::from_min_size(at, egui::vec2(width, JAR_CHIP_HEIGHT));
    painter.rect_filled(scrim, JAR_CHIP_HEIGHT / 2.0, egui::Color32::from_black_alpha(JAR_CHIP_SCRIM_ALPHA));
    let x = paint_jar_dots(painter, at.x + JAR_CHIP_PAD, cy, chip, arrow.filter(|_| fit.lead), text, warn);
    if let Some(name) = name {
        let x = x + JAR_CHIP_NAME_GAP;
        let size = name.size();
        painter.galley(egui::pos2(x, cy - size.y / 2.0), name, text);
        if let Some(marker) = marker {
            let m = marker.size();
            painter.galley(egui::pos2(x + size.x + JAR_CHIP_MARKER_GAP, cy - m.y / 2.0), marker, text);
        }
    }
    Some(scrim)
}

/// The arrow between a waiting switch's two dots (`paint_jar_dots`); `None` unless the tab waits to switch.
pub(crate) fn jar_chip_arrow(
    painter: &egui::Painter,
    chip: &crate::jar_defs::JarChip,
    font: egui::FontId,
    text: egui::Color32,
) -> Option<std::sync::Arc<egui::Galley>> {
    chip.will_switch.then(|| painter.layout_no_wrap(egui_phosphor::light::ARROW_RIGHT.into(), font, text))
}

/// How wide `paint_jar_dots` paints, with `arrow` (`jar_chip_arrow`) or without.
pub(crate) fn jar_dots_width(arrow: Option<&egui::Galley>) -> f32 {
    arrow.map_or(0.0, |a| 2.0 * JAR_CHIP_R + 3.0 + a.size().x + 3.0) + 2.0 * JAR_CHIP_R
}

/// A jar's dots from `x` (screen) on the line `cy`, shared by the card chip and Tab view: with `arrow`, the jar the
/// live tab still runs on (`was_color`) and the arrow first; then its jar's dot, outlined when following the group and
/// `warn`-coloured for a missing jar. Returns the x after the last dot.
pub(crate) fn paint_jar_dots(
    painter: &egui::Painter,
    mut x: f32,
    cy: f32,
    chip: &crate::jar_defs::JarChip,
    arrow: Option<std::sync::Arc<egui::Galley>>,
    text: egui::Color32,
    warn: egui::Color32,
) -> f32 {
    const R: f32 = JAR_CHIP_R;
    if let Some(arrow) = arrow {
        painter.circle_filled(egui::pos2(x + R, cy), R, chip.was_color);
        x += 2.0 * R + 3.0;
        let size = arrow.size();
        painter.galley(egui::pos2(x, cy - size.y / 2.0), arrow, text);
        x += size.x + 3.0;
    }
    let color = if chip.missing { warn } else { chip.color };
    if chip.following {
        painter.circle_stroke(egui::pos2(x + R, cy), R - 0.75, egui::Stroke::new(1.5, color));
    } else {
        painter.circle_filled(egui::pos2(x + R, cy), R, color);
    }
    x + 2.0 * R
}

/// Queues tab `chip` at world point `world` (the card's corner it hangs from), moved by `offset` screen points, at
/// most `max_width` screen points wide: the card's width less the chip's inset on both sides.
fn push_jar_chip(
    deferred_overlays: &mut DeferredOverlays,
    world: egui::Pos2,
    offset: egui::Vec2,
    max_width: f32,
    chip: crate::jar_defs::JarChip,
    warn: egui::Color32,
) {
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        let font = egui::FontId::proportional(11.0);
        let text = egui::Color32::from_gray(240);
        paint_jar_chip(overlay_painter, project(world) + offset, max_width, &chip, font, text, warn);
    }));
}

/// `tooltip`, with `more` on a line of its own when both exist.
fn chain_tooltip(tooltip: Option<String>, more: Option<String>) -> Option<String> {
    match (tooltip, more) {
        (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
        (a, b) => a.or(b),
    }
}

/// The part of a `src`-sized texture (as a uv rect) that fills a `dst`-shaped rect without
/// stretching: the full width or height, cropped on the other axis around the top-center, so a
/// page's header stays in view.
pub(crate) fn cover_uv(src: (u32, u32), dst: egui::Vec2) -> egui::Rect {
    if src.0 == 0 || src.1 == 0 || dst.x <= 0.0 || dst.y <= 0.0 {
        return FULL_UV;
    }
    let src_aspect = src.0 as f32 / src.1 as f32;
    let dst_aspect = dst.x / dst.y;
    if src_aspect > dst_aspect {
        // Source is wider: crop the sides, centered.
        let w = dst_aspect / src_aspect;
        egui::Rect::from_min_max(egui::pos2((1.0 - w) / 2.0, 0.0), egui::pos2((1.0 + w) / 2.0, 1.0))
    } else {
        // Source is taller: crop the bottom.
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, src_aspect / dst_aspect))
    }
}

/// Where a Tab card's thumbnail lands: the card inset by `GRAPH_THUMB_MARGIN`, minus the header
/// when `tab_has_header`. World units: the margin scales with the card, the header does not.
pub(crate) fn tab_thumb_rect(rect: egui::Rect, inv_zoom: f32) -> egui::Rect {
    let mut thumb = rect.shrink(crate::GRAPH_THUMB_MARGIN);
    if tab_has_header(rect, inv_zoom) {
        thumb.min.y = rect.top() + TAB_HEADER_POINTS * inv_zoom;
    }
    thumb
}

/// A download node has no live thumbnail, but its header (see `CardHeader`) and body need the
/// same footprint a tab card has: a 60-high card had no room for the 40-point header.
pub(crate) const DOWNLOAD_NODE_SIZE: egui::Vec2 = GRAPH_NODE_SIZE;

/// A group card's footprint: a tab card's, for the same header-and-body layout. Also the
/// drop-target rect a dragged tab is tested against on release — see `App::current_group`'s own doc
/// comment.
pub(crate) const GROUP_NODE_SIZE: egui::Vec2 = GRAPH_NODE_SIZE;

/// A note card's footprint -- taller than the other non-thumbnail kinds
/// since its whole body is an editable multi-line text area, not just a
/// title strip.
pub(crate) const NOTE_NODE_SIZE: egui::Vec2 = egui::vec2(180.0, 140.0);

/// The largest an image node's card gets, in either dimension -- unlike
/// every other kind, an image card's actual footprint is derived per-node
/// from `ImagePayload::width`/`height`'s own aspect ratio (see
/// `image_node_card_size`), scaled down to fit inside this box. Never
/// scaled *up* past the image's real pixel size, so a small picture isn't
/// blurrily stretched to fill it.
pub(crate) const IMAGE_NODE_MAX_SIZE: egui::Vec2 = egui::vec2(220.0, 220.0);

/// A PDF node's card size while its background render is still in flight
/// (`PdfPayload::outcome` is `None`) or failed (`Some(Err(_))`) -- the real
/// page's own aspect ratio isn't known yet in either case, so this is a
/// plain fixed box, same idea `DOWNLOAD_NODE_SIZE` already has for "no live
/// content to size around." Once rendered, the card switches to
/// `image_node_card_size` like an `Image` node does, sized from the real
/// page dimensions `PdfOutcome` carries.
pub(crate) const PDF_PENDING_NODE_SIZE: egui::Vec2 = egui::vec2(150.0, 90.0);

/// A JSON error card's fixed box -- there's no root to preview, just the
/// missing-file card. A file-backed Json card is sized by
/// `json_view::preview_size` instead (see `node_card_size`).
pub(crate) const JSON_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 90.0);

/// Graph toolbar filter: when `filter` doesn't match any of `haystacks`,
/// dims the card at `rect` with one black scrim, pushed as a deferred
/// overlay so it paints last (on the same untransformed foreground layer
/// card text uses, so it covers that too, not just the thumbnail/borders
/// below it) rather than threading a dim factor through every draw call.
/// `rounding` matches the card's own corner radius (screen space).
pub(crate) fn dim_if_filtered_out(
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
                FILTER_DIM_SCRIM,
            );
        }));
    }
}

/// The per-kind background, colors and filter-dim rounding a graph
/// card's shared frame (`App::draw_graph_card`) paints with.
pub(crate) struct CardStyle {
    /// The card's background, under its body (bodies paint content only).
    pub(crate) fill: egui::Color32,
    pub(crate) border_hover: egui::Color32,
    pub(crate) border_idle: egui::Color32,
    pub(crate) close_fill: egui::Color32,
    pub(crate) close_border: egui::Color32,
    pub(crate) close_icon: egui::Color32,
    pub(crate) dim_rounding: f32,
}

pub(crate) fn card_style(kind: CardKind, visuals: &egui::Visuals) -> CardStyle {
    let themed = CardStyle {
        fill: visuals.extreme_bg_color,
        border_hover: visuals.text_color(),
        border_idle: visuals.weak_text_color(),
        close_fill: visuals.extreme_bg_color,
        close_border: visuals.weak_text_color(),
        close_icon: visuals.text_color(),
        dim_rounding: 4.0,
    };
    // Presentation matches the type; behaviour reads the container table (`is_container`). See the plan 2b-6
    // Outcome, "Presentation arms".
    match kind.ty {
        TYPE_NOTE | TYPE_DOWNLOAD | TYPE_TAB => CardStyle { dim_rounding: 6.0, ..themed },
        // ACCENT at rest, as group cards have always been.
        TYPE_GROUP => CardStyle { border_idle: ACCENT, dim_rounding: 6.0, ..themed },
        _ => themed,
    }
}

/// What a card's body (`App::paint_card_body`) tells the shared frame:
/// which part of the card is the click/drag handle (the whole card,
/// except a Note being edited, where only its header is, so clicks in
/// the text go to the `TextEdit`), and an optional hover tooltip (a
/// failed PDF render's error).
pub(crate) struct CardBody {
    pub(crate) interact_rect: egui::Rect,
    pub(crate) tooltip: Option<String>,
}

/// What `App::draw_graph_card` hands back to `build_graph_ui`, which
/// applies each kind's own click action unless a control took the click.
pub(crate) struct CardOutput {
    pub(crate) response: egui::Response,
    /// The action of the control clicked this frame (a middle-click on
    /// the card counts as its ✕).
    pub(crate) control: Option<NodeAction>,
}

/// What a graph card's frame (`App::draw_graph_card`) reads from its
/// node, snapshotted once up front (`App::card_view`) so nothing holds a
/// node borrow across the frame's `&mut self` calls.
pub(crate) struct CardView {
    pub(crate) kind: CardKind,
    pub(crate) color: Option<egui::Color32>,
    /// In a group: its cards show the eject button.
    pub(crate) in_group: bool,
    /// A group over `GROUP_OVERLOAD_THRESHOLD` members: its border warns.
    pub(crate) overloaded: bool,
    /// Group only: its members' ids, for its thumbnail grid and count.
    pub(crate) members: Vec<String>,
    /// The open tab: its border is ACCENT.
    pub(crate) focused: bool,
    /// Tab only.
    pub(crate) pinned: bool,
    /// Tab only.
    pub(crate) muted: bool,
    /// Tab only.
    pub(crate) audio_playing: bool,
    /// In the rectangle selection (tabs only): washed in ACCENT, and
    /// dragged as one unit.
    pub(crate) selected: bool,
}

/// A card's border width and color at hover progress `hover_t` (0 at
/// rest, 1 hovered): the tag color at rest, else DANGER_COLOR for an
/// overloaded group, else the kind's idle color, easing toward its hover
/// color, from width 1 to 1.5. A focused tab's is ACCENT at 2.5.
pub(crate) fn card_border(view: &CardView, style: &CardStyle, hover_t: f32) -> (f32, egui::Color32) {
    // The strongest, least ambiguous signal: not eased, and over any tag.
    if view.focused {
        return (2.5, ACCENT);
    }
    let resting = view.color.unwrap_or(if view.overloaded { DANGER_COLOR } else { style.border_idle });
    (1.0 + 0.5 * hover_t, lerp_color32(resting, style.border_hover, hover_t))
}

/// An on-card button of a graph card's frame. `card_controls` lists them
/// in registration order, so where two overlap the later one takes the
/// pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CardControl {
    Swatch,
    Pin,
    Audio,
    Eject,
    Close,
}

impl CardControl {
    /// Revealed on hover, rather than always shown.
    pub(crate) fn hover_only(self) -> bool {
        matches!(self, CardControl::Eject | CardControl::Close)
    }

    pub(crate) fn action(self, kind: CardKind) -> NodeAction {
        match self {
            CardControl::Swatch => NodeAction::CycleColor,
            CardControl::Pin => NodeAction::TogglePin,
            CardControl::Audio => NodeAction::ToggleMute,
            CardControl::Eject => NodeAction::RemoveFromGroup,
            CardControl::Close if is_container(kind.ty) => NodeAction::Dissolve,
            CardControl::Close => NodeAction::Delete,
        }
    }

    /// Its hit and paint rect, centered on one of the card's corners.
    pub(crate) fn rect(self, card: egui::Rect, inv_zoom: f32) -> egui::Rect {
        let (anchor, size) = match self {
            CardControl::Swatch => (card.right_bottom(), 16.0),
            CardControl::Pin => (card.left_bottom(), 16.0),
            CardControl::Audio => (card.right_top() + egui::vec2(-8.0, 8.0) * inv_zoom, 16.0),
            CardControl::Eject => (card.left_top(), 18.0),
            CardControl::Close => (card.right_top(), 18.0),
        };
        egui::Rect::from_center_size(anchor, egui::Vec2::splat(size) * inv_zoom)
    }
}

/// What Del (the card's Close) does to a `kind` of card, for the focused card's key hint: the first word of the
/// node menu's row hinted Del.
pub(crate) fn del_verb(kind: CardKind) -> &'static str {
    if is_container(kind.ty) {
        return "dissolve";
    }
    match kind.ty {
        TYPE_TAB => "close",
        TYPE_DOWNLOAD => "remove",
        _ => "delete",
    }
}

/// What a single click (or Enter) on a `kind` of card does, for the kinds whose click is a `NodeAction`
/// (`App::draw_graph_cards`; a card whose file is missing retries it instead):
/// - Tab, Unloaded: opens it into Tab view (an unloaded one wakes, `App::wake_tab`).
/// - Image: maximizes it into Item view, the gesture a tab uses.
/// - Download, Pdf: reveals the file. A PDF has no Item view, so it gets the other file card's click.
///
/// A Note's click edits it in place and a Group or Json opens on double-click, in the caller.
pub(crate) fn click_action(kind: CardKind) -> Option<NodeAction> {
    match kind.ty {
        TYPE_TAB | TYPE_IMAGE => Some(NodeAction::Open),
        TYPE_DOWNLOAD | TYPE_PDF => Some(NodeAction::Reveal),
        _ => None,
    }
}

/// The buttons `view`'s card shows, in registration order (see
/// `CardControl`): the swatch and the pin on every card, and a live tab's audio indicator while playing or muted, eject on
/// a grouped one, and the ✕ last, so it wins over anything under it.
pub(crate) fn card_controls(view: &CardView) -> Vec<CardControl> {
    let mut controls = vec![CardControl::Swatch, CardControl::Pin];
    if view.kind.ty == TYPE_TAB {
        // Also while muted after audio stops, so a mute can always be
        // seen and undone from Graph view. An unloaded card has no engine
        // to be playing or muted, so it never gets this control.
        if view.kind.loaded() && (view.audio_playing || view.muted) {
            controls.push(CardControl::Audio);
        }
    }
    if view.in_group {
        controls.push(CardControl::Eject);
    }
    controls.push(CardControl::Close);
    controls
}

/// What a dragged card was released over (see `App::drop_target`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DropTarget {
    /// The toolbar's back button, inside a group.
    BackButton,
    Card(CardKind),
}

/// What a drop does (`App::apply_drop`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DropEffect {
    /// Out of its group, one level up.
    Eject,
    /// A group onto a group: its members move into the target, and it goes.
    Merge,
    /// Into the target group.
    Join,
    /// A tab onto a tab at the top level: a new group of both.
    GroupWith,
    /// A tab onto a sibling tab inside a group: the target becomes its
    /// parent.
    Reparent,
}

/// Every drag-and-drop rule in Graph view, for every kind. A group dropped on a group merges into it; the
/// node menu's "Move into <group>" (`NodeAction::MoveToGroup`) is always a Join, so it nests one instead.
pub(crate) fn drop_action(dragged: CardKind, target: DropTarget, inside_group: bool) -> Option<DropEffect> {
    // A tab, live or sleeping, on both sides of a tab-onto-tab drop.
    let tabs = |onto: CardKind| dragged.ty == TYPE_TAB && onto.ty == TYPE_TAB;
    match (target, inside_group) {
        (DropTarget::BackButton, true) => Some(DropEffect::Eject),
        (DropTarget::Card(onto), _) if is_container(dragged.ty) && is_container(onto.ty) => Some(DropEffect::Merge),
        (DropTarget::Card(onto), _) if can_contain(dragged, onto) => Some(DropEffect::Join),
        (DropTarget::Card(onto), false) if tabs(onto) => Some(DropEffect::GroupWith),
        (DropTarget::Card(onto), true) if tabs(onto) => Some(DropEffect::Reparent),
        _ => None,
    }
}

/// What every card in one Graph-view card pass shares
/// (`App::draw_graph_card`).
pub(crate) struct CardPass<'a> {
    pub(crate) deferred_overlays: &'a mut DeferredOverlays,
    pub(crate) actions: &'a mut Vec<GraphAction>,
    pub(crate) inv_zoom: f32,
    pub(crate) zoom_scale: f32,
    /// Where the card being dragged would drop right now
    /// (`App::drop_target`), computed once before the pass.
    pub(crate) drop_preview: &'a Option<(DropEffect, Option<String>)>,
    /// Pick-target mode's valid targets (`App::valid_targets`), each ringed; empty outside the mode.
    pub(crate) pick_targets: &'a [String],
    /// During a connect-drag, the card under the pointer (`App::connect_drag_end`): its target ring is drawn bright,
    /// as a hovered one is in pick-target mode.
    pub(crate) connect_target: Option<&'a str>,
    /// While a link's end is dragged (`App::retarget_preview`), the ring colour for the outcome of releasing on
    /// `connect_target`, which is then ringed even where it is no valid target (refused, its own card), so a ring
    /// never promises a retarget the release will not do. `None` otherwise: valid targets keep the accent ring.
    pub(crate) connect_ring: Option<egui::Color32>,
    /// Keyboard navigation is on (a card or control had focus last frame): each card shows its
    /// Tab-order number.
    pub(crate) show_order: bool,
    /// The level's card ids in Tab order (creation order, `App::card_tab_order`). Only the first is
    /// a Tab stop from outside the graph; `App::handle_card_tab` moves between the rest.
    pub(crate) order: &'a [String],
    /// A card or one of its controls has keyboard focus this frame.
    pub(crate) any_focused: bool,
    /// Every group's members (`App::group_members`), built once before the pass.
    pub(crate) members: &'a HashMap<String, Vec<String>>,
}

/// An outline on a keyboard-focused card or control, white so it reads differently from the gold
/// border of the active tab. Outside the rect, in world units (`inv_zoom` keeps it a constant size).
pub(crate) fn card_focus_ring(ui: &egui::Ui, rect: egui::Rect, inv_zoom: f32) {
    ui.painter().rect_stroke(
        rect.expand(3.0 * inv_zoom),
        3.0 * inv_zoom,
        egui::Stroke::new(2.0 * inv_zoom, TEXT_PRIMARY),
        egui::StrokeKind::Outside,
    );
}

/// One of a card's `card_controls`, hit-tested and drawn at its corner.
/// Returns its response: the frame turns a click into
/// `control.action(kind)` and counts its hover as the card's. Icons are
/// painted on the overlay layer (see `DeferredOverlays`), like card text.
#[allow(
    clippy::too_many_arguments,
    reason = "free function: callers hold no CardPass, and a struct for one call site would only rename the arguments"
)]
pub(crate) fn paint_card_control(
    ui: &egui::Ui,
    deferred_overlays: &mut DeferredOverlays,
    control: CardControl,
    card_rect: egui::Rect,
    id: &str,
    view: &CardView,
    style: &CardStyle,
    inv_zoom: f32,
    lod_simplified: bool,
) -> egui::Response {
    let rect = control.rect(card_rect, inv_zoom);
    let response = ui
        // Not focusable: Tab moves between cards only, and the keyboard reaches these actions
        // through Delete and the node menu.
        .interact(rect, egui::Id::new(("graph_card_control", control, id)), egui::Sense::CLICK)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    match control {
        CardControl::Swatch => {
            match view.color {
                Some(color) => {
                    ui.painter().circle_filled(rect.center(), 6.0 * inv_zoom, color);
                }
                // A faint ring, so an uncolored card still shows where to
                // click.
                None => {
                    ui.painter().circle_stroke(
                        rect.center(),
                        6.0 * inv_zoom,
                        egui::Stroke::new(1.5 * inv_zoom, style.border_idle),
                    );
                }
            }
        }
        // A `PUSH_PIN` icon, ACCENT when pinned. A pinned node is left out
        // of the force-directed layout (see `Mirror::pinned`).
        CardControl::Pin => {
            let pin_color = if view.pinned { ACCENT } else { ui.visuals().weak_text_color() };
            if !lod_simplified {
                push_centered_text(deferred_overlays, rect.center(), egui_phosphor::light::PUSH_PIN, 14.0, pin_color);
            }
        }
        // Audio-playing/muted indicator: shown while playing so audio
        // isn't a surprise, and kept showing while muted (even after
        // audio stops) so a mute can always be seen and undone from
        // Graph view without first switching to Tab view.
        // `audio_playing` is only ever true for WebView2/WebKit tabs —
        // see `TabPayload::audio_playing`'s doc comment. Muted draws a
        // real `SPEAKER_X` icon; actually playing draws a small
        // painter-drawn waveform instead -- see its own rationale below.
        CardControl::Audio => {
            if view.muted {
                // Muted stays static -- nothing is actually
                // playing to animate. A real `SPEAKER_X` icon
                // rather than a plain red dot (migrated
                // alongside the pin/eject icons): a bare dot
                // only meant "muted" once you'd learned the
                // convention, where the icon states it. Text,
                // so drawn on `overlay_painter` like the pin/
                // eject icons, not `ui.painter()` like the
                // waveform's own vector bars below.
                if !lod_simplified {
                    push_centered_text(
                        deferred_overlays,
                        rect.center(),
                        egui_phosphor::light::SPEAKER_X,
                        12.0,
                        DANGER_COLOR,
                    );
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
                // An idle window draws at a lower rate than the bars need.
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                let t = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                let bar_w = 2.5 * inv_zoom;
                let gap = 1.5 * inv_zoom;
                let max_h = 10.0 * inv_zoom;
                let min_h = 3.0 * inv_zoom;
                let total_w = bar_w * 3.0 + gap * 2.0;
                let base_x = rect.center().x - total_w / 2.0;
                let base_y = rect.center().y + max_h / 2.0;
                for i in 0..3 {
                    let phase = i as f64 * 1.7;
                    let h = min_h + (max_h - min_h) * 0.5 * (1.0 + (t * 6.0 + phase).sin() as f32);
                    let x = base_x + i as f32 * (bar_w + gap);
                    let bar_rect =
                        egui::Rect::from_min_max(egui::Pos2::new(x, base_y - h), egui::Pos2::new(x + bar_w, base_y));
                    ui.painter().rect_filled(bar_rect, 1.0 * inv_zoom, WAVEFORM_COLOR);
                }
            }
        }
        CardControl::Eject => {
            paint_control_square(
                ui,
                deferred_overlays,
                rect,
                response.hovered(),
                ACCENT,
                egui_phosphor::light::SIGN_OUT,
                style,
                inv_zoom,
            );
        }
        CardControl::Close => {
            paint_control_square(
                ui,
                deferred_overlays,
                rect,
                response.hovered(),
                DANGER_COLOR,
                egui_phosphor::light::X,
                style,
                inv_zoom,
            );
        }
    }
    response
}

/// The ✕ and eject buttons' look: a small square in the card's close
/// colors (`card_style`), filled `hover_fill` while hovered.
#[allow(
    clippy::too_many_arguments,
    reason = "free function: callers hold no CardPass, and a struct for one call site would only rename the arguments"
)]
pub(crate) fn paint_control_square(
    ui: &egui::Ui,
    deferred_overlays: &mut DeferredOverlays,
    rect: egui::Rect,
    hovered: bool,
    hover_fill: egui::Color32,
    icon: &'static str,
    style: &CardStyle,
    inv_zoom: f32,
) {
    ui.painter().rect_filled(rect, 2.0 * inv_zoom, if hovered { hover_fill } else { style.close_fill });
    ui.painter().rect_stroke(
        rect,
        2.0 * inv_zoom,
        egui::Stroke::new(1.0 * inv_zoom, style.close_border),
        egui::StrokeKind::Inside,
    );
    push_centered_text(deferred_overlays, rect.center(), icon, 14.0, style.close_icon);
}

/// An image node's own card size: `width`x`height` scaled down (never up)
/// to fit inside `IMAGE_NODE_MAX_SIZE` while preserving aspect ratio --
/// shared by the graph card's own layout and `fit_all_nodes_scene_rect`'s
/// bounds union so the two never disagree about how big a given image's
/// card actually is.
pub(crate) fn image_node_card_size(width: u32, height: u32) -> egui::Vec2 {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (IMAGE_NODE_MAX_SIZE.x / width).min(IMAGE_NODE_MAX_SIZE.y / height).min(1.0);
    egui::vec2(width * scale, height * scale)
}

/// A PDF node's own card size: `PDF_PENDING_NODE_SIZE` while still loading
/// or on a failed render (the real page dimensions aren't known in either
/// case), otherwise the rendered page's own size via `image_node_card_size`
/// -- shared by `fit_all_nodes_scene_rect`'s bounds union, the right-click
/// hit-test, and the graph card's own layout, so all three never disagree.
pub(crate) fn pdf_card_size(pdf: &PdfPayload) -> egui::Vec2 {
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
pub(crate) fn node_card_size(runtime: &Runtime) -> egui::Vec2 {
    match runtime {
        Runtime::Tab(_) | Runtime::Unloaded(_) => GRAPH_NODE_SIZE,
        Runtime::Download(_) => DOWNLOAD_NODE_SIZE,
        Runtime::Note(_) => NOTE_NODE_SIZE,
        // A placeholder has no real size: the same box a failed PDF gets.
        Runtime::Image(image) if image.error.is_some() => PDF_PENDING_NODE_SIZE,
        Runtime::Image(image) => image_node_card_size(image.width, image.height),
        Runtime::Pdf(pdf) => pdf_card_size(pdf),
        // A file's own card previews its root (spec "The card at the
        // parent level"); an error card keeps the small box.
        Runtime::Json(json) if json.error.is_none() => crate::json_view::preview_size(&json.value),
        Runtime::Json(_) => JSON_NODE_SIZE,
        // A group is the one kind with no runtime (2b-6 Ruling 8: no unknown-type card yet).
        Runtime::None => GROUP_NODE_SIZE,
    }
}

/// A card in world space: `node_card_size` centered on `pos`, its layout position (`App::card_rect`).
pub(crate) fn node_card_rect(pos: egui::Vec2, runtime: &Runtime) -> egui::Rect {
    egui::Rect::from_center_size(pos.to_pos2(), node_card_size(runtime))
}

/// What the Graph filter box matches a card against: a tab's title and
/// URL, a download's full path, a note's text, a file node's filename, a group's name (`text`, `App::own_text`, for
/// a note or a group).
pub(crate) fn card_filter_text<'a>(runtime: &'a Runtime, text: &'a str) -> Vec<&'a str> {
    match runtime {
        Runtime::Tab(tab) => vec![&tab.title, &tab.url],
        Runtime::Unloaded(tab) => vec![&tab.title, &tab.url],
        Runtime::Download(download) => vec![&download.path],
        Runtime::Note(_) => vec![text],
        Runtime::Image(image) => vec![download_filename(&image.path)],
        Runtime::Pdf(pdf) => vec![download_filename(&pdf.path)],
        Runtime::Json(json) => vec![download_filename(&json.path)],
        // A group is the one kind with no runtime (2b-6 Ruling 8: no unknown-type card yet); it matches on its name, `text`.
        Runtime::None => vec![text],
    }
}

/// The Sleeping/Waking half of an unloaded card's tooltip -- Failed's
/// tooltip needs the engine and error message too, so `paint_unloaded_tab_body`
/// builds that one itself rather than through this helper.
pub(crate) fn sleeping_or_waking_tooltip(state: &UnloadedState) -> &'static str {
    match state {
        UnloadedState::Waking => "Loading…",
        _ => "Sleeping — click to load",
    }
}

/// How a failed card whose engine can't run names why: (tooltip phrase, card-body phrase), after the
/// engine's label. A runtime that is there but out of date needs an update; anything else is not installed.
fn missing_engine_phrases(state: &crate::engine_install::EngineState) -> (&'static str, &'static str) {
    match state {
        crate::engine_install::EngineState::UpdateNeeded { .. } => ("needs an update", "needs an update"),
        _ => ("is not installed", "not installed"),
    }
}

impl App {
    /// Ends every note edit (`end_note_edit`), saving if one was on. A note's TextEdit that
    /// stops being laid out (its level left, the view changed) loses focus
    /// without `lost_focus()` ever firing, so `editing` would stay on.
    pub(crate) fn end_note_edits(&mut self) -> bool {
        let editing: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.as_note().is_some_and(|note| note.editing))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &editing {
            self.end_note_edit(id);
        }
        if !editing.is_empty() {
            self.save_session();
        }
        !editing.is_empty()
    }

    /// Every group's members, in `node_order`: built once per card pass.
    pub(crate) fn group_members(&self) -> HashMap<String, Vec<String>> {
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        for id in &self.node_order {
            if let Some(group) = self.nodes.get(id).and(self.mirror.group(id)) {
                members.entry(group.to_owned()).or_default().push(id.clone());
            }
        }
        members
    }

    /// `id`'s `CardView`, or `None` when the node is gone. `members` is
    /// `group_members`, built once for the whole pass.
    pub(crate) fn card_view(&self, id: &str, members: &HashMap<String, Vec<String>>) -> Option<CardView> {
        let node = self.nodes.get(id)?;
        let kind = self.card_kind(id)?;
        let members = if is_container(kind.ty) { members.get(id).cloned().unwrap_or_default() } else { Vec::new() };
        let tab = node.as_tab();
        Some(CardView {
            kind,
            color: self.mirror.color(id),
            in_group: self.mirror.group(id).is_some(),
            overloaded: members.len() > GROUP_OVERLOAD_THRESHOLD,
            members,
            focused: self.focused.as_deref() == Some(id),
            pinned: self.mirror.pinned(id),
            muted: tab.is_some_and(|t| t.muted),
            audio_playing: tab.is_some_and(|t| t.audio_playing),
            selected: self.interaction.selected.contains(id),
        })
    }

    /// One card of any kind on the graph: everything the kinds share
    /// (cull, click/drag and hover interacts, border, controls, selection
    /// wash, drag and drop through `drop_target`, filter dim), with the
    /// per-kind content painted by `paint_card_body`, plus the frame's own
    /// background, glow and eased hover. `None` when the node is gone or
    /// culled. The caller applies the kind's own click action from the
    /// returned response.
    pub(crate) fn draw_graph_card(&mut self, ui: &mut egui::Ui, pass: &mut CardPass, id: &str) -> Option<CardOutput> {
        let inv_zoom = pass.inv_zoom;
        let rect = self.card_rect(id)?;
        // Viewport culling: skip every bit of interact/paint
        // work below for a node that isn't actually visible --
        // the single highest-leverage fix for a graph with many
        // more nodes than fit on screen at once. Never culls
        // the node currently being dragged, even once it's
        // moved outside the viewport -- `ui.interact`'s own
        // drag tracking is what keeps a drag alive frame to
        // frame, so skipping it mid-drag would freeze the node
        // in place the instant it crossed the visible edge.
        // The Tab entry (first card) and the card holding keyboard focus stay registered too, so
        // Tab can reach them and focus survives a pan that hasn't brought them into view yet, and
        // a note being edited, whose editor must stay laid out to keep its focus.
        let keeps_keyboard = pass.order.first().is_some_and(|first| first == id)
            || ui.memory(|m| m.has_focus(egui::Id::new(("graph_card", id))))
            || ui.memory(|m| m.has_focus(egui::Id::new(("note_edit", id))));
        if self.interaction.dragging.as_deref() != Some(id) && !keeps_keyboard && !rect.intersects(self.camera.rect) {
            return None;
        }
        let view = self.card_view(id, pass.members)?;
        // Glow and fill go under the body, but hover is only known after
        // the interacts, which must follow the body (below). This slot is
        // filled in once it is.
        let background = ui.painter().add(egui::Shape::Noop);
        let body = self.paint_card_body(ui, pass, id, &view, rect);
        let deferred_overlays = &mut *pass.deferred_overlays;

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
        // Focusable only as the Tab entry (first in `pass.order`) or while holding focus, so egui's
        // own traversal can't walk the cards in draw order.
        let card_id = egui::Id::new(("graph_card", id));
        let slot = pass.order.iter().position(|o| o == id);
        let sense = if slot == Some(0) || ui.memory(|m| m.has_focus(card_id)) {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::CLICK | egui::Sense::DRAG
        };
        // Space on a focused card toggles it in the selection, as the marquee would select it. Taken
        // before the interact, which would otherwise read it as a click and open the tab.
        if ui.memory(|m| m.has_focus(card_id))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space))
        {
            self.toggle_selected(id);
        }
        let response = ui.interact(body.interact_rect, card_id, sense);
        // Not while a card's menu is open: the pointer is usually still on
        // the card it right-clicked, and the tooltip would cover the menu.
        let response = match body.tooltip {
            Some(tooltip) if self.menus.node_menu.is_none() => response.on_hover_text(tooltip),
            _ => response,
        };
        // Screen readers: the card is a button named by its kind and title (egui announces it on focus).
        if let Some(node) = self.nodes.get(id) {
            let (text, source) = (self.own_text(id), self.source_url(id));
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), node.accessible_name(text, source))
            });
        }
        // Keyboard: a focus ring, the camera follows focus, and the Tab-order number while navigating.
        pass.any_focused |= response.has_focus();
        if response.has_focus() {
            card_focus_ring(ui, rect, inv_zoom);
            let to_screen = ui.ctx().layer_transform_to_global(ui.layer_id());
            self.card_hint = Some((view.kind, to_screen.map_or(rect, |t| t * rect)));
        }
        if response.gained_focus() {
            self.pan_to_card(rect);
        }
        if pass.show_order {
            let (number, corner) = (slot.map_or(0, |s| s + 1), rect.left_top());
            deferred_overlays.push(Box::new(move |painter, project| {
                // Kept on screen: a card at the edge would clip its badge.
                let center = project(corner);
                let center = center.clamp(painter.clip_rect().shrink(10.0).min, painter.clip_rect().shrink(10.0).max);
                // A keycap: the number is the key that jumps to this card.
                let galley = painter.layout_no_wrap(number.to_string(), egui::FontId::monospace(11.0), TEXT_PRIMARY);
                let cap = egui::Rect::from_center_size(center, galley.size() + egui::vec2(8.0, 4.0));
                painter.rect_filled(cap, 3.0, BG);
                painter.rect_stroke(cap, 3.0, egui::Stroke::new(1.0, TEXT_PRIMARY), egui::StrokeKind::Inside);
                painter.galley(cap.center() - galley.size() / 2.0, galley, TEXT_PRIMARY);
            }));
        }
        let hover = ui.interact(rect, egui::Id::new(("graph_card_hover", id)), egui::Sense::hover());
        let pointer_on_card = hover.hovered() || response.hovered();
        // Last frame's hover too, which keeps the ✕ drawn while the
        // pointer is on it: registered last, it takes the hover from both
        // interacts above.
        let is_hovered = self.interaction.hovered.as_deref() == Some(id) || pointer_on_card;

        // One eased hover drives the glow, the fill's brightening and the
        // border, so the three move together.
        let hover_t = ui.ctx().animate_bool_responsive(egui::Id::new(("graph_card_hover_t", id)), is_hovered);
        let style = card_style(view.kind, ui.visuals());
        let (border_width, border_color) = card_border(&view, &style, hover_t);
        let mut background_shapes = card_glow_shapes(rect, 2.0, border_color, inv_zoom, 1.0 + hover_t * 0.8).to_vec();
        background_shapes.push(egui::Shape::rect_filled(
            rect,
            2.0 * inv_zoom,
            style.fill.gamma_multiply(1.0 + hover_t * 0.35),
        ));
        // Rectangle-select's highlight: a translucent ACCENT wash, distinct
        // from the focus border (an outline only), so a selected tab still
        // reads differently from the focused one when both apply. In the
        // background slot, so it tints the card but not its thumbnail.
        if view.selected {
            background_shapes.push(egui::Shape::rect_filled(
                rect,
                2.0 * inv_zoom,
                egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 45),
            ));
        }
        ui.painter().set(background, egui::Shape::Vec(background_shapes));
        // An inside stroke painted over the body; every body keeps its
        // content inset from the edge.
        ui.painter().rect_stroke(
            rect,
            2.0 * inv_zoom,
            egui::Stroke::new(border_width * inv_zoom, border_color),
            egui::StrokeKind::Inside,
        );

        // The card the dragged one would drop onto: a bold ring, expanded
        // well past this card so it still shows around a larger dragged
        // card on top of it. Tracks the pointer directly, not eased.
        if pass.drop_preview.as_ref().and_then(|(_, target)| target.as_deref()) == Some(id) {
            ui.painter().rect_stroke(
                rect.expand(16.0 * inv_zoom),
                10.0 * inv_zoom,
                egui::Stroke::new(4.0 * inv_zoom, ACCENT),
                egui::StrokeKind::Outside,
            );
        }
        // Pick-target mode: a valid target gets a thinner accent ring, closer in than the drop ring, and brighter
        // while hovered, where a click would connect.
        let reached = pass.connect_target == Some(id);
        if pass.pick_targets.iter().any(|t| t == id) || (reached && pass.connect_ring.is_some()) {
            let alpha = if pointer_on_card || reached { 255 } else { 160 };
            let ring = pass.connect_ring.filter(|_| reached).unwrap_or(ACCENT);
            ui.painter().rect_stroke(
                rect.expand(6.0 * inv_zoom),
                6.0 * inv_zoom,
                egui::Stroke::new(
                    2.5 * inv_zoom,
                    egui::Color32::from_rgba_unmultiplied(ring.r(), ring.g(), ring.b(), alpha),
                ),
                egui::StrokeKind::Outside,
            );
        }
        // Registered after both interacts above, so they take the pointer.
        let delete = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Delete));
        // Shift+F10 on a focused card opens its menu, anchored at the card's bottom-left.
        if response.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F10)) {
            let corner = rect.left_bottom();
            let at = ui.ctx().layer_transform_to_global(ui.layer_id()).map_or(corner, |t| t * corner);
            self.menus.node_menu = Some((id.to_string(), at));
        }
        let mut control =
            (response.clicked_by(egui::PointerButton::Middle) || delete).then(|| CardControl::Close.action(view.kind));
        let mut control_hovered = false;
        for card_control in card_controls(&view) {
            if card_control.hover_only() && !is_hovered {
                continue;
            }
            let control_response = paint_card_control(
                ui,
                deferred_overlays,
                card_control,
                rect,
                id,
                &view,
                &style,
                inv_zoom,
                self.graph_lod_simplified,
            );
            control_hovered |= control_response.hovered();
            // A right-click on the swatch opens the menu (the right-press already did) on its Color page.
            if card_control == CardControl::Swatch && control_response.secondary_clicked() {
                if let Some((menu_id, pos)) = self.menus.node_menu.clone().filter(|(menu_id, _)| menu_id == id) {
                    self.menus.node_menu_page = Some((menu_id, pos, crate::node_menu::MenuPage::Color));
                    ui.ctx().request_repaint();
                }
            }
            if control_response.clicked() && control.is_none() {
                control = Some(card_control.action(view.kind));
            }
        }

        // Connect-drag handles (link-gestures spec A), on a hovered card, and on every card while a drag runs.
        // Registered after the card and its controls: egui gives a press on a drag-only widget on top to that widget
        // alone (no click below), so the card neither moves nor opens.
        let mut handle_hovered = false;
        if (is_hovered || self.link.connect_drag.is_some()) && self.shows_link_handles() {
            let handles = crate::link_gestures::link_handle_rects(rect, pass.zoom_scale, self.graph_lod_simplified);
            for (side, handle) in handles {
                let handle_response = ui
                    .interact(handle, egui::Id::new(("link_handle", side, id)), egui::Sense::DRAG)
                    .on_hover_cursor(egui::CursorIcon::Crosshair);
                handle_hovered |= handle_response.hovered();
                let active = handle_response.hovered() || handle_response.dragged();
                crate::link_gestures::paint_link_handle(ui, handle, active, inv_zoom);
                if handle_response.drag_started() {
                    self.start_connect_drag(id);
                }
            }
        }

        // This frame's hover only: passing `is_hovered` (which includes
        // `self.interaction.hovered`) made a card stay hovered, ✕ and all, after the
        // pointer left it, until another card was hovered. Never during a
        // connect-drag, whose peek would cover the targets.
        let still_hovered = self.link.connect_drag.is_none() && (pointer_on_card || control_hovered || handle_hovered);
        if response.dragged() {
            self.interaction.dragging = Some(id.to_string());
            // Dragging one selected tab moves the whole selection, as one
            // unit (see `take_drag_unit`). Recomputed (and overwritten)
            // every dragged frame rather than only at drag start: cheap,
            // and it's always exactly this frame's moved set, so whatever
            // `drag_moved` holds when the drag ends is the full, correct
            // set to save -- see the release branch below.
            let delta = response.drag_delta();
            let moved: Vec<String> =
                if view.selected { self.interaction.selected.iter().cloned().collect() } else { vec![id.to_string()] };
            // The layout only: the document pos is written once, at release (`release_drag`).
            for moved_id in &moved {
                if self.nodes.contains_key(moved_id) {
                    self.layout.entry(moved_id.clone()).or_default().pos += delta;
                }
            }
            self.interaction.drag_moved = moved;
        } else if self.interaction.dragging.as_deref() == Some(id) {
            self.release_drag();
            // `drop_preview` was computed this frame for this card, and
            // nothing moved it since (no drag delta on a release frame).
            pass.actions.push(GraphAction::Drop { id: id.to_string(), target: pass.drop_preview.clone() });
        }
        if still_hovered {
            self.interaction.hovered = Some(id.to_string());
        } else if self.interaction.hovered.as_deref() == Some(id) {
            self.interaction.hovered = None;
        }

        if let Some(node) = self.nodes.get(id) {
            dim_if_filtered_out(
                deferred_overlays,
                &self.graph_filter,
                &card_filter_text(&node.runtime, self.own_text(id)),
                rect,
                style.dim_rounding,
            );
        }
        Some(CardOutput { response, control })
    }

    /// A graph card's own content, per kind -- everything under the
    /// shared frame's border (see `draw_graph_card`). Returns which part
    /// of the card is the click/drag handle and an optional tooltip.
    pub(crate) fn paint_card_body(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        id: &str,
        view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
        match view.kind.ty {
            TYPE_DOWNLOAD => self.paint_download_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            TYPE_NOTE => self.paint_note_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            TYPE_IMAGE => self.paint_image_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            TYPE_PDF => self.paint_pdf_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            TYPE_JSON => self.paint_json_body(ui, deferred_overlays, id, rect, zoom_scale),
            // Presentation matches the type, not `is_container` (see `card_style`).
            TYPE_GROUP => self.paint_group_body(ui, pass, id, view, rect),
            TYPE_TAB if view.kind.loaded() => {
                self.paint_tab_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale)
            }
            TYPE_TAB => self.paint_unloaded_tab_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            // A type this build doesn't know has no card of its own yet (2b-6 Ruling 8): the bare frame.
            _ => CardBody { interact_rect: rect, tooltip: None },
        }
    }

    /// Unloaded (see `UnloadedTabPayload`), by state: **Sleeping** draws the
    /// title (or url) and host dimmed, no icon; **Waking** adds the spinner
    /// over the same dimmed labels; **Failed** keeps the Task 1 look -- a
    /// warning icon, the url at the bottom, and which engine failed and why
    /// as the tooltip. LOD: skipped once simplified, as for
    /// `paint_download_body`.
    pub(crate) fn paint_unloaded_tab_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let Some(tab) = self.nodes.get(id).and_then(|n| n.as_unloaded()) else {
            return body;
        };
        let (url, title, state) = (tab.url.clone(), tab.title.clone(), tab.state.clone());
        let kind = self.mirror.engine(id);
        // A failed card whose engine isn't installed offers its Download, not a retry that can't work.
        let missing_engine = matches!(state, UnloadedState::Failed(_))
            && !matches!(pick_outcome(kind, &self.engine_state(kind)), PickOutcome::Switch(_));
        body.tooltip = Some(match &state {
            UnloadedState::Failed(_) if missing_engine => match self.engine_state(kind) {
                crate::engine_install::EngineState::Unavailable(reason) => {
                    PickOutcome::unavailable_toast(kind, &reason)
                }
                state => format!(
                    "{} {}\nClick to download it ({}).",
                    engine_label(kind),
                    missing_engine_phrases(&state).0,
                    crate::engine_info::megabytes(self.engine_download_size(kind))
                ),
            },
            UnloadedState::Failed(error) => {
                format!("Couldn't start {}: {error}\nClick to try again.", engine_label(kind))
            }
            UnloadedState::Sleeping | UnloadedState::Waking => sleeping_or_waking_tooltip(&state).to_string(),
        });
        let chip = self.jar_chip(id);
        body.tooltip = chain_tooltip(body.tooltip, chip.as_ref().and_then(|c| c.tooltip.clone()));
        if self.graph_lod_simplified {
            return body;
        }
        // Favicons aren't kept across restarts and no engine loads a sleeping tab's url, so the
        // card asks for its own. Only on-screen cards drawn in full detail get here (culled and
        // LOD-simplified ones don't), so a big restored session fetches for what's in view, once
        // per origin.
        let mut favicon = None;
        if let Some(origin) = self.cached_favicon_origin(&url) {
            if !self.favicon_resolved.contains(&origin) {
                self.favicon_fetcher.request(origin.clone());
            }
            favicon = self.gpu.as_ref().and_then(|g| g.favicon_id(&origin));
        }
        let failed = matches!(state, UnloadedState::Failed(_));
        let header = CardHeader {
            glyph: Some(if failed { egui_phosphor::light::WARNING } else { egui_phosphor::light::MOON }),
            glyph_color: failed.then_some(DANGER_COLOR),
            icon_image: if failed { None } else { favicon },
            name: if title.is_empty() { url.clone() } else { title },
            user_name: None,
            source: url,
            source_is_path: false,
            scrim: false,
            right_margin: 0.0,
        };
        let tall = self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
        // The middle of the body below the header: the failure icon, the waking spinner, or the
        // moon of a sleeping tab.
        let middle = card_body_rect(rect, tall, inv_zoom).center();
        match &state {
            UnloadedState::Failed(_) if missing_engine => {
                let up = egui::vec2(0.0, 8.0 * inv_zoom);
                push_centered_text(
                    deferred_overlays,
                    middle - up,
                    egui_phosphor::light::WARNING_CIRCLE,
                    24.0,
                    DANGER_COLOR,
                );
                let phrase = missing_engine_phrases(&self.engine_state(kind)).1;
                let text = format!("{} {phrase} (click to download)", engine_label(kind));
                push_centered_text(deferred_overlays, middle + up * 2.5, text, 11.0, DANGER_COLOR);
            }
            UnloadedState::Failed(_) => {
                push_centered_text(deferred_overlays, middle, egui_phosphor::light::WARNING_CIRCLE, 24.0, DANGER_COLOR);
            }
            UnloadedState::Waking => {
                ui.put(
                    egui::Rect::from_center_size(middle, egui::vec2(24.0, 24.0) * inv_zoom),
                    egui::Spinner::new().size(24.0 * inv_zoom),
                );
            }
            UnloadedState::Sleeping => {
                push_centered_text(
                    deferred_overlays,
                    middle,
                    egui_phosphor::light::MOON,
                    24.0,
                    ui.visuals().weak_text_color(),
                );
            }
        }
        // The jar chip just below the title: under the header, or at the top of a short card (whose title is on
        // the bottom strip).
        if let Some(chip) = chip.filter(|_| rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH) {
            let body_rect = card_body_rect(rect, tall, inv_zoom);
            let max_width = body_rect.width() * zoom_scale - 8.0;
            let warn = ui.visuals().warn_fg_color;
            push_jar_chip(deferred_overlays, body_rect.left_top(), egui::vec2(4.0, 4.0), max_width, chip, warn);
        }
        body
    }

    /// Download: no thumbnail/title-strip/audio-dot machinery (no
    /// live content behind it), just the filename, an icon, and a
    /// live progress bar while it's still downloading.
    pub(crate) fn paint_download_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let body = CardBody { interact_rect: rect, tooltip: None };
        let Some((download_path, completed, failed, total_bytes, bytes_received)) = self
            .nodes
            .get(id)
            .and_then(|n| n.as_download())
            .map(|d| (d.path.clone(), d.completed, d.failed, d.total_bytes, d.bytes_received))
        else {
            return body;
        };
        let download_url = self.copyable_url(id).map(str::to_owned);
        let filename = download_filename(&download_path);
        // LOD: the header text is skipped once simplified -- see `paint_tab_body`'s own LOD comment.
        if !self.graph_lod_simplified {
            let header = CardHeader {
                glyph: Some(egui_phosphor::light::DOWNLOAD_SIMPLE),
                glyph_color: None,
                icon_image: None,
                name: filename.to_owned(),
                user_name: None,
                source_is_path: download_url.is_none(),
                source: download_url.unwrap_or_else(|| download_path.clone()),
                scrim: false,
                right_margin: 0.0,
            };
            let tall = self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
            // The size (or how much has arrived) in the middle of the body below the header.
            if tall {
                let size = match (completed, total_bytes) {
                    _ if failed => "Download failed".to_owned(),
                    (true, total) => crate::human_bytes(total.unwrap_or(bytes_received)),
                    (false, Some(total)) => {
                        format!("{} of {}", crate::human_bytes(bytes_received), crate::human_bytes(total))
                    }
                    (false, None) => crate::human_bytes(bytes_received),
                };
                let middle = card_body_rect(rect, true, inv_zoom).center();
                let color = if failed { DANGER_COLOR } else { ui.visuals().weak_text_color() };
                push_centered_text(deferred_overlays, middle, size, 13.0, color);
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
        if !completed && !failed {
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
        body
    }

    /// Note: a plain user-authored text card, no engine process
    /// behind it -- see `NotePayload`'s own doc comment. Unlike
    /// every other kind, most of the card is a real editable
    /// `TextEdit`, not overlay-painted static text -- editing
    /// happens directly in place, on the graph. The header strip
    /// (not the text body) is the drag handle only while editing,
    /// via `CardBody::interact_rect`, so a click inside the text
    /// placing a cursor/selecting text is never mistaken for the
    /// start of a card drag; the frame's own hover-only interact
    /// covers the whole card regardless, so hovering the text body
    /// still counts even though it isn't the drag handle.
    pub(crate) fn paint_note_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let tall = tab_has_header(rect, inv_zoom);
        let paper = tab_thumb_rect(rect, inv_zoom);
        let text_rect = paper.shrink(6.0 * inv_zoom);
        // While editing, only the header is the drag handle (a short card has the strip instead).
        let handle = if tall {
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, paper.min.y))
        } else {
            egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - TAB_STRIP_POINTS * inv_zoom), rect.max)
        };
        let is_editing = self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);

        ui.painter().rect_filled(paper, 4.0 * inv_zoom, NOTE_CARD_COLOR);
        if !self.graph_lod_simplified {
            let words = self.note_text(id).split_whitespace().count();
            let header = CardHeader {
                glyph: Some(egui_phosphor::light::NOTE_PENCIL),
                glyph_color: None,
                icon_image: None,
                name: "Note".to_owned(),
                user_name: None,
                source: format!("{words} word{}", if words == 1 { "" } else { "s" }),
                source_is_path: false,
                scrim: false,
                right_margin: 0.0,
            };
            self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
        }

        if is_editing {
            body.interact_rect = handle;
            let mut lost_focus = false;
            if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                lost_focus = ui
                    .put(
                        text_rect,
                        egui::TextEdit::multiline(&mut note.text)
                            .id(egui::Id::new(("note_edit", id)))
                            .frame(egui::Frame::NONE)
                            .font(egui::FontId::proportional(13.0))
                            .text_color(NOTE_TEXT)
                            .hint_text("Type a note..."),
                    )
                    .lost_focus();
            }
            // The box edits the payload, the edit buffer; the mirror gets it on blur (and at each save), not on
            // every keystroke -- a note can reach 1 MB, and `save_session` is real file I/O (2b-4 Ruling 6).
            if lost_focus {
                // Escape ends the edit; focus goes back to the card so keyboard users keep their place
                // (a click elsewhere keeps the focus it took).
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    ui.memory_mut(|m| m.request_focus(egui::Id::new(("graph_card", id))));
                }
                self.blur_note(id);
            }
        } else if !self.graph_lod_simplified {
            // LOD: skip the markdown render (real parsing +
            // rich text layout, one of the pricier per-node
            // widgets in this whole view) once simplified --
            // see `paint_tab_body`'s own LOD comment. Never
            // applies to the `is_editing` branch above: a
            // note actively being typed into must never have
            // its live `TextEdit` yanked out from under the
            // user just because the rest of the graph got
            // busy.
            // Borrowed, not cloned (2b-4 Ruling 6): `text` borrows `self.mirror` and `cache` `self.commonmark_cache`,
            // two disjoint fields. Not being typed in, the note shows the mirror's text (`App::note_text`).
            let text = self.mirror.text(id);
            let cache = &mut self.commonmark_cache;
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::LEFT)),
                |ui| {
                    ui.style_mut().visuals.override_text_color = Some(NOTE_TEXT);
                    egui_commonmark::CommonMarkViewer::new().show(ui, cache, text);
                },
            );
        }
        body
    }

    /// A restored image/JSON card whose file couldn't be loaded: header F (a `FILE_X` glyph in the
    /// danger colour, the file name, its path) with the same glyph a failed PDF card shows in the
    /// middle of the body below it (see `ImagePayload::error`).
    pub(crate) fn paint_missing_file_card(
        &mut self,
        ui: &egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
        (name, source, source_is_path): (String, String, bool),
    ) {
        if self.graph_lod_simplified {
            return;
        }
        let header = CardHeader {
            glyph: Some(egui_phosphor::light::FILE_X),
            glyph_color: Some(DANGER_COLOR),
            icon_image: None,
            name,
            user_name: None,
            source,
            source_is_path,
            scrim: false,
            right_margin: 0.0,
        };
        let tall = self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
        let icon_pos = card_body_rect(rect, tall, inv_zoom).center();
        push_centered_text(deferred_overlays, icon_pos, egui_phosphor::light::FILE_X, 24.0, DANGER_COLOR);
    }

    /// Image: a locally-added picture, no engine/IPC behind it --
    /// see `ImagePayload`'s own doc comment. Unlike a note, nothing
    /// inside the card needs its own clicks (no embedded text
    /// editor), so the whole card is drag-sensitive via one plain
    /// `Sense::click_and_drag()`, same as a tab or download card.
    /// The card's own footprint comes from `image_node_card_size`
    /// (the decoded picture's real aspect ratio, capped at
    /// `IMAGE_NODE_MAX_SIZE`), not a fixed size the way every other
    /// kind's card is.
    pub(crate) fn paint_image_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let error = self.nodes.get(id).and_then(|n| n.as_image()).and_then(|i| i.error.clone());
        if let Some(error) = error {
            if let Some(path) = self.nodes.get(id).and_then(|n| n.as_image()).map(|i| i.path.clone()) {
                let text = file_header_text(&path, self.source_url(id));
                self.paint_missing_file_card(ui, deferred_overlays, rect, inv_zoom, zoom_scale, text);
            }
            body.tooltip = Some(error);
            return body;
        }
        let tex_id = self.gpu.as_ref().and_then(|g| g.image_id(id));
        // LOD: the real texture blit (and the broken-image
        // glyph text) skipped once simplified -- see
        // `paint_tab_body`'s own LOD comment. Unlike a tab card, an image
        // node has no separate background fill of its own (the
        // picture *is* the fill). The simplified state shows the
        // frame's plain fill, matching every other kind's
        // "background + border" simplified look.
        if !self.graph_lod_simplified {
            match tex_id {
                Some(tex_id) => {
                    ui.painter().image(tex_id, rect, FULL_UV, egui::Color32::WHITE);
                    if let Some(image) = self.nodes.get(id).and_then(|n| n.as_image()) {
                        let (name, source, source_is_path) = file_header_text(&image.path, self.source_url(id));
                        let header = CardHeader {
                            glyph: Some(egui_phosphor::light::IMAGE),
                            glyph_color: None,
                            icon_image: None,
                            name,
                            user_name: None,
                            source,
                            source_is_path,
                            scrim: true,
                            right_margin: 0.0,
                        };
                        self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
                    }
                }
                // Only reachable if the node somehow outlived its
                // own texture (freed but not removed) -- a broken-
                // image glyph is a more honest placeholder here
                // than the loading `Spinner` a tab/download
                // placeholder uses, since this picture isn't *about*
                // to arrive the way a first frame or fetch is.
                None => {
                    push_centered_text(
                        deferred_overlays,
                        rect.center(),
                        egui_phosphor::light::IMAGE_BROKEN,
                        24.0,
                        ui.visuals().weak_text_color(),
                    );
                }
            }
        }
        body
    }

    /// Pdf: same card shape as an image node (drag, hover-revealed
    /// close button, drop-onto-group), but with three visual states
    /// instead of one -- still rendering in the background
    /// (`outcome: None`, a plain spinner, same fixed
    /// `PDF_PENDING_NODE_SIZE` box a tab respawn placeholder uses
    /// `Spinner` for too), failed (`Some(Err(_))`, a real error
    /// glyph plus the message as a hover tooltip -- unlike an
    /// image's "somehow outlived its own texture" broken-image
    /// case, this is an *expected*, real outcome worth explaining,
    /// not a rare bug), or rendered (`Some(Ok(_))`, the page's own
    /// texture in `GpuState::images` -- the exact same map an
    /// `Image` node's texture lives in, since a rendered PDF page
    /// is just another RGBA bitmap by the time it's on the GPU).
    pub(crate) fn paint_pdf_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let Some(state) = self.nodes.get(id).and_then(|n| n.as_pdf()).map(|pdf| match &pdf.outcome {
            None => None,
            Some(Err(err)) => Some(Err(err.clone())),
            Some(Ok(outcome)) => Some(Ok((self.gpu.as_ref().and_then(|g| g.image_id(id)), outcome.page_count))),
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
        // content" simplification as `paint_image_body` -- see
        // `paint_tab_body`'s own LOD comment.
        if !self.graph_lod_simplified {
            match state {
                Some(Ok((Some(tex_id), page_count))) => {
                    ui.painter().image(tex_id, rect, FULL_UV, egui::Color32::WHITE);
                    if let Some(pdf) = self.nodes.get(id).and_then(|n| n.as_pdf()) {
                        let (name, source, source_is_path) = file_header_text(&pdf.path, self.source_url(id));
                        // The page count sits at the bottom right, so the fallback strip stops short of it.
                        let header = CardHeader {
                            glyph: Some(egui_phosphor::light::FILE_PDF),
                            glyph_color: None,
                            icon_image: None,
                            name,
                            user_name: None,
                            source,
                            source_is_path,
                            scrim: true,
                            right_margin: 60.0,
                        };
                        self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
                    }
                    // Shown even for a one-page PDF -- a glanceable
                    // "this is a PDF, not a plain image" cue on top
                    // of the file-type distinction the icon in the
                    // context menu/toolbar already gives.
                    if page_count > 0 && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                        let page_count_pos = rect.right_bottom();
                        let page_count_text = format!("{page_count} page{}", if page_count == 1 { "" } else { "s" });
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
                    push_centered_text(
                        deferred_overlays,
                        rect.center(),
                        egui_phosphor::light::IMAGE_BROKEN,
                        24.0,
                        ui.visuals().weak_text_color(),
                    );
                }
                Some(Err(_)) => {
                    push_centered_text(
                        deferred_overlays,
                        rect.center(),
                        egui_phosphor::light::FILE_X,
                        24.0,
                        DANGER_COLOR,
                    );
                }
                None => {
                    ui.put(rect, egui::Spinner::new().size(24.0 * inv_zoom));
                }
            }
        }
        body
    }

    /// Json: an error card keeps the missing-file box (fixed at
    /// `JSON_NODE_SIZE`). A node with a file instead previews its root --
    /// the file name, `{n}`/`[n]`, up to `PREVIEW_ROWS` root rows and
    /// `+ N more` -- sized by `json_view::preview_size` and painted by the
    /// JSON level's own `paint_json_content`, so the two never disagree
    /// about what a root looks like. Double-click enters the level
    /// (`current_group`/`sync_json_level`).
    pub(crate) fn paint_json_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        zoom_scale: f32,
    ) -> CardBody {
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let error = self.nodes.get(id).and_then(|n| n.as_json()).and_then(|j| j.error.clone());
        if let Some(error) = error {
            if let Some(path) = self.nodes.get(id).and_then(|n| n.as_json()).map(|j| j.path.clone()) {
                let text = file_header_text(&path, None);
                self.paint_missing_file_card(ui, deferred_overlays, rect, 1.0 / zoom_scale, zoom_scale, text);
            }
            body.tooltip = Some(error);
            return body;
        }
        let Some(json) = self.nodes.get(id).and_then(|n| n.as_json()) else { return body };
        let text = !self.graph_lod_simplified && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH;
        let len = crate::json_view::entry_count(&json.value);
        // The header is 40 screen points, so at other zooms the rows get more or less of the card:
        // show only the rows that fit, with a `+ N more` row when some are cut. A card too short for
        // the header (`tab_has_header`) keeps the level cards' compact one.
        let inv_zoom = 1.0 / zoom_scale;
        let preview = tab_has_header(rect, inv_zoom);
        let header_height = if preview { TAB_HEADER_POINTS * inv_zoom } else { crate::json_view::HEADER_HEIGHT };
        let fit = ((rect.height() - header_height) / crate::json_view::ROW_HEIGHT).floor().max(0.0) as usize;
        let mut rows = len.min(crate::json_view::PREVIEW_ROWS).min(fit);
        if rows < len {
            rows = rows.min(fit.saturating_sub(1));
        }
        let card = crate::json_view::Card {
            pointer: String::new(),
            parent: None,
            depth: 0,
            collapsed: false,
            skip: 0,
            rows,
            more: len - rows,
        };
        let mut header = crate::json_level_ui::header_parts(&json.value, &card, download_filename(&json.path));
        if preview && text {
            // The folder, cut at the start so its last part stays visible, as a download's path is.
            let folder = file_header_text(&json.path, None).1;
            let folder = folder.rsplit_once(['\\', '/']).map(|(dir, _)| dir.to_owned());
            // `paint_json_content`'s text width: the source starts at `HEADER_TEXT_X`.
            let width = (rect.width() - crate::json_level_ui::COUNT_WIDTH - crate::json_level_ui::PAD) * zoom_scale
                - HEADER_TEXT_X;
            header.source = folder.map(|dir| {
                let font = egui::FontId::proportional(11.0);
                truncate_start_to_width(&mut self.truncate_cache, ui.ctx(), &dir, &font, width.max(0.0))
            });
        }
        let pal = crate::json_level_ui::palette(ui.visuals().dark_mode);
        let look = crate::json_level_ui::CardLook {
            palette: &pal,
            text_color: ui.visuals().text_color(),
            inv_zoom,
            text,
            selected_row: None,
            hit_rows: &[],
            visible: egui::Rect::EVERYTHING,
            // No child cards or edges at the parent level: a port dot here
            // would point at nothing.
            ports: false,
            preview,
        };
        crate::json_level_ui::paint_json_content(
            ui.painter(),
            deferred_overlays,
            rect,
            &json.value,
            &header,
            0,
            rows,
            len - rows,
            &look,
        );
        body
    }

    /// Header F (folder glyph, name, item count) over a member thumbnail grid (header only when
    /// empty or with "Collapse Groups" on). LOD: skipped once simplified, leaving the frame's
    /// plain card.
    pub(crate) fn paint_group_body(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        id: &str,
        view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let member_ids = &view.members;
        let member_count = member_ids.len();
        if !self.graph_lod_simplified {
            // Owned, so the header can borrow it while `paint_card_header` borrows `self`.
            // name rule: drawn by display_name just below
            if let Some(stored) = self.is_container(id).then(|| self.mirror.name(id).to_owned()) {
                let segs = crate::names::display_name(&stored, crate::names::UNTITLED);
                body.tooltip = crate::names::hidden_hint(&segs);
                let header = CardHeader {
                    glyph: Some(egui_phosphor::light::FOLDER),
                    glyph_color: None,
                    icon_image: None,
                    name: String::new(),
                    user_name: Some(&segs),
                    source: format!("{member_count} item{}", if member_count == 1 { "" } else { "s" }),
                    source_is_path: false,
                    scrim: false,
                    right_margin: 0.0,
                };
                let tall = self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
                // Up to 4 member thumbnails, a 2x2 grid in the body below the header, like a folder
                // previewing its contents (`GpuState::thumbnail_id`, as a tab's own card does). A cell
                // with no thumbnail yet gets a plain tinted square. "Collapse Groups" leaves the
                // header alone.
                if member_count > 0 && !self.groups_collapsed {
                    let (top, bottom) =
                        if tall { (TAB_HEADER_POINTS * inv_zoom, 0.0) } else { (0.0, TAB_STRIP_POINTS * inv_zoom) };
                    let grid_rect =
                        egui::Rect::from_min_max(rect.min + egui::vec2(0.0, top), rect.max - egui::vec2(0.0, bottom))
                            .shrink(GRAPH_THUMB_MARGIN);
                    let gap = 3.0 * inv_zoom;
                    let cell_size = egui::vec2((grid_rect.width() - gap) / 2.0, (grid_rect.height() - gap) / 2.0);
                    for (i, member_id) in member_ids.iter().take(4).enumerate() {
                        let (col, row) = (i as f32 % 2.0, (i / 2) as f32);
                        let cell_min = grid_rect.min + egui::vec2(col * (cell_size.x + gap), row * (cell_size.y + gap));
                        let cell_rect = egui::Rect::from_min_size(cell_min, cell_size);
                        match self.gpu.as_ref().and_then(|g| g.thumbnail_id(member_id)) {
                            Some(tex_id) => {
                                ui.painter().image(tex_id, cell_rect, FULL_UV, egui::Color32::WHITE);
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
                    if member_count > 4 && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
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
                }
            }
        }
        body
    }

    /// A card's header, drawn over the top of its rect: glyph (or favicon), name, and the source
    /// line, 40 screen points tall. A card under `TAB_HEADER_MIN_CARD_POINTS` tall gets the name on
    /// a strip along the bottom instead. Returns whether the full header was drawn, so a caller
    /// can place its own content below it.
    pub(crate) fn paint_card_header(
        &mut self,
        ui: &egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
        header: CardHeader<'_>,
    ) -> bool {
        let tall = tab_has_header(rect, inv_zoom);
        if rect.width() * zoom_scale < MIN_LABEL_CARD_SCREEN_WIDTH {
            return tall;
        }
        let CardHeader { glyph, glyph_color, icon_image, name, user_name, source, source_is_path, scrim, right_margin } =
            header;
        let band_height = if tall { TAB_HEADER_POINTS } else { TAB_STRIP_POINTS } * inv_zoom;
        let band = if tall {
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), band_height))
        } else {
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - band_height), rect.right_bottom())
        };
        if scrim || !tall {
            let radius = (6.0 * inv_zoom).min(255.0) as u8;
            let corners = if tall {
                egui::CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 }
            } else {
                egui::CornerRadius { nw: 0, ne: 0, sw: radius, se: radius }
            };
            ui.painter().rect_filled(band, corners, if tall { IMAGE_TEXT_SCRIM } else { TITLE_STRIP_SCRIM });
        }
        let light = scrim || !tall;
        let title_color = if light { egui::Color32::from_gray(240) } else { ui.visuals().text_color() };
        let source_color = if light { egui::Color32::from_gray(190) } else { ui.visuals().weak_text_color() };
        let glyph_color = glyph_color.unwrap_or(source_color);
        // One size for measuring and drawing the title, so a strip's plain and rich (hidden-character) titles match.
        let title_size = if tall { HEADER_TITLE_SIZE } else { 12.0 };
        let title_font = egui::FontId::proportional(title_size);
        let source_font = egui::FontId::proportional(HEADER_SOURCE_SIZE);
        let text_left = if tall { HEADER_TEXT_X } else { 8.0 };
        let max_width = (rect.width() * zoom_scale - text_left - HEADER_RIGHT_RESERVE - right_margin).max(0.0);
        // A stored group name with a hidden character is a styled job (placeholders in the marker style, truncated
        // by the layout); any other title is cut text, as before (`truncate_to_width`'s cache).
        let rich = user_name.filter(|segs| crate::names::has_hidden(segs)).map(|segs| {
            let mut job = egui::text::LayoutJob::default();
            let format = egui::TextFormat::simple(title_font.clone(), title_color);
            crate::names::append_name(&mut job, segs, &format, source_color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
            job
        });
        let title = match (&rich, user_name) {
            (Some(_), _) => String::new(),
            // A clean name is one borrowed segment: cut it without copying (spaces collapsed need `to_plain`).
            (None, Some([crate::names::NameSeg::Plain(p)])) => {
                truncate_to_width(&mut self.truncate_cache, ui.ctx(), p, &title_font, max_width)
            }
            (None, Some(segs)) => {
                let plain = crate::names::to_plain(segs);
                truncate_to_width(&mut self.truncate_cache, ui.ctx(), &plain, &title_font, max_width)
            }
            (None, None) => truncate_to_width(&mut self.truncate_cache, ui.ctx(), &name, &title_font, max_width),
        };
        let source = match (tall, source_is_path) {
            (false, _) => String::new(),
            (true, true) => {
                truncate_start_to_width(&mut self.truncate_cache, ui.ctx(), &source, &source_font, max_width)
            }
            (true, false) => truncate_to_width(&mut self.truncate_cache, ui.ctx(), &source, &source_font, max_width),
        };
        if let (true, Some(tex_id)) = (tall, icon_image) {
            let center = band.left_top() + egui::vec2(18.0, HEADER_CENTER_Y) * inv_zoom;
            let uv = FULL_UV;
            ui.painter().image(
                tex_id,
                egui::Rect::from_center_size(center, FAVICON_SIZE * inv_zoom),
                uv,
                egui::Color32::WHITE.gamma_multiply(0.6),
            );
        }
        let origin = band.left_top();
        let band_screen_height = band_height * zoom_scale;
        let glyph = glyph.filter(|_| tall && icon_image.is_none());
        deferred_overlays.push(Box::new(move |overlay_painter, project| {
            let base = project(origin);
            if tall {
                if let Some(glyph) = glyph {
                    overlay_painter.text(
                        base + egui::vec2(HEADER_GLYPH_X, HEADER_CENTER_Y),
                        egui::Align2::LEFT_CENTER,
                        glyph,
                        egui::FontId::proportional(14.0),
                        glyph_color,
                    );
                }
                paint_card_title(
                    overlay_painter,
                    base + egui::vec2(text_left, HEADER_TITLE_Y),
                    rich,
                    title,
                    title_font,
                    title_color,
                );
                overlay_painter.text(
                    base + egui::vec2(text_left, HEADER_SOURCE_Y),
                    egui::Align2::LEFT_CENTER,
                    source,
                    source_font,
                    source_color,
                );
            } else {
                let pos = base + egui::vec2(text_left, band_screen_height / 2.0);
                paint_card_title(overlay_painter, pos, rich, title, title_font, title_color);
            }
        }));
        tall
    }

    /// A live thumbnail (or a spinner until the first frame) under a header of favicon, title and
    /// source URL; a card too short for the header (`tab_has_header`) gets the old title strip
    /// over the thumbnail, with the engine's color dot and the favicon.
    pub(crate) fn paint_tab_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        let Some((title, url, engine_kind, unresponsive)) =
            self.nodes.get(id).and_then(|n| n.as_tab()).map(|t| {
                (t.title.clone(), t.url.clone(), t.tab.kind, t.tab.is_unresponsive(std::time::Instant::now()))
            })
        else {
            return body;
        };
        let chip = self.jar_chip(id);
        body.tooltip = chip.as_ref().and_then(|c| c.tooltip.clone());
        // LOD: the thumbnail texture blit and the title/
        // favicon text below are this card's two genuinely
        // expensive parts (a real GPU texture sample, and
        // glyph rasterization queued on `overlay_painter`) --
        // skipped once `graph_lod_simplified` is set (see its
        // own doc comment), leaving the frame's plain card
        // and controls as the simplified card's whole
        // appearance.
        if !self.graph_lod_simplified {
            let header = tab_has_header(rect, inv_zoom);
            let thumb_rect = tab_thumb_rect(rect, inv_zoom);
            let thumb_id = self.gpu.as_ref().and_then(|g| g.thumbnail_id(id));
            if let Some(tex_id) = thumb_id {
                let uv = self
                    .gpu
                    .as_ref()
                    .and_then(|g| g.thumbnail_size(id))
                    .map_or(FULL_UV, |size| cover_uv(size, thumb_rect.size()));
                ui.painter().image(tex_id, thumb_rect, uv, egui::Color32::WHITE);
            } else {
                ui.put(
                    egui::Rect::from_center_size(thumb_rect.center(), egui::vec2(24.0, 24.0) * inv_zoom),
                    egui::Spinner::new().size(24.0 * inv_zoom),
                );
            }

            let label = if title.is_empty() { url.as_str() } else { title.as_str() };
            let favicon = self
                .cached_favicon_origin(&url)
                .and_then(|origin| self.gpu.as_ref().and_then(|g| g.favicon_id(&origin)));
            let show_text = rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH;
            if header {
                // Favicon (or the engine dot), title and source line over the card's own fill.
                let icon_center = egui::pos2(
                    rect.left() + (GRAPH_THUMB_MARGIN + 8.0) * inv_zoom,
                    rect.top() + HEADER_CENTER_Y * inv_zoom,
                );
                match favicon {
                    Some(tex_id) => {
                        ui.painter().image(
                            tex_id,
                            egui::Rect::from_center_size(icon_center, FAVICON_SIZE * inv_zoom),
                            FULL_UV,
                            egui::Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().circle_filled(icon_center, 4.0 * inv_zoom, engine_color(engine_kind));
                    }
                }
                // Title and source line: the shared header, which leaves the icon above to this card.
                let card_header = CardHeader {
                    glyph: None,
                    glyph_color: None,
                    icon_image: None,
                    name: label.to_owned(),
                    user_name: None,
                    source: url.clone(),
                    source_is_path: false,
                    scrim: false,
                    right_margin: 0.0,
                };
                self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, card_header);
            } else {
                // A flat scrim behind the title strip so it stays legible over a light thumbnail.
                let title_strip = egui::Rect::from_min_max(
                    egui::Pos2::new(thumb_rect.left(), thumb_rect.bottom() - TAB_STRIP_POINTS * inv_zoom),
                    thumb_rect.right_bottom(),
                );
                ui.painter().rect_filled(title_strip, 0.0, TITLE_STRIP_SCRIM);
                // Engine identity as a small colored dot; the strip is too small for more.
                ui.painter().circle_filled(
                    egui::Pos2::new(title_strip.left() + 8.0 * inv_zoom, title_strip.center().y),
                    4.0 * inv_zoom,
                    engine_color(engine_kind),
                );
                // Below `MIN_LABEL_CARD_SCREEN_WIDTH` the label would be a sliver; the dot alone remains.
                if show_text {
                    let label_font = egui::FontId::proportional(14.0);
                    // Screen width left for the label: the strip's, less the 16px left offset and some room on
                    // the right.
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
                if let Some(tex_id) = favicon {
                    let favicon_rect = egui::Rect::from_min_size(
                        rect.left_top() + egui::vec2(4.0, 4.0) * inv_zoom,
                        FAVICON_SIZE * inv_zoom,
                    );
                    ui.painter().image(tex_id, favicon_rect, FULL_UV, egui::Color32::WHITE);
                }
            }
            // The jar chip at the thumbnail's bottom-left, above a short card's title strip.
            if let Some(chip) = chip.filter(|_| show_text) {
                let strip = if header { 0.0 } else { TAB_STRIP_POINTS * inv_zoom };
                let corner = thumb_rect.left_bottom() + egui::vec2(0.0, -strip);
                let offset = egui::vec2(4.0, -4.0 - JAR_CHIP_HEIGHT);
                let max_width = thumb_rect.width() * zoom_scale - 8.0;
                push_jar_chip(deferred_overlays, corner, offset, max_width, chip, ui.visuals().warn_fg_color);
            }
        }
        // At every level of detail: a stuck engine is the one thing on this card that needs the user.
        if unresponsive {
            let middle = tab_thumb_rect(rect, inv_zoom).center();
            push_centered_text(deferred_overlays, middle, egui_phosphor::light::WARNING_CIRCLE, 24.0, DANGER_COLOR);
            body.tooltip = chain_tooltip(
                Some("Not responding. Right-click for Restart Engine or End Engine.".to_owned()),
                body.tooltip.take(),
            );
        }
        body
    }
}

/// A card title, left-centred at `pos`: the styled `rich` job when there is one, else the plain `title`.
fn paint_card_title(
    painter: &egui::Painter,
    pos: egui::Pos2,
    rich: Option<egui::text::LayoutJob>,
    title: String,
    font: egui::FontId,
    color: egui::Color32,
) {
    match rich {
        Some(job) => {
            let galley = painter.layout_job(job);
            let y = pos.y - galley.size().y / 2.0;
            painter.galley(egui::pos2(pos.x, y), galley, color);
        }
        None => {
            painter.text(pos, egui::Align2::LEFT_CENTER, title, font, color);
        }
    }
}

/// Graph toolbar filter box: `true` when the node should be painted
/// dimmed -- exactly the negation of `filter_matches`, kept as its own
/// named function since "dims" (not "doesn't match") is the meaning every
/// call site actually cares about.
fn graph_filter_dims(filter: &str, haystacks: &[&str]) -> bool {
    !filter_matches(filter, haystacks)
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
pub(crate) fn card_glow_shapes(
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

/// `truncate_core`'s cache key: a hash of the text (the text itself is in
/// the value, compared on a hit, so a lookup allocates nothing), the font
/// size as bits (`f32` isn't `Hash`/`Eq`), the width bucket, and which end
/// is cut. Every call site builds its `FontId` with `FontId::proportional`,
/// so the family is not in the key. See `truncate_to_width` for why the
/// width is bucketed rather than exact.
pub(crate) type TruncateCacheKey = (u64, u32, i32, bool);

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
pub(crate) fn truncate_to_width(
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

#[cfg(test)]
mod tests;
