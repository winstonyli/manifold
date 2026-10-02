//! Graph cards: the shared frame (`draw_graph_card`) with its sizes,
//! styles, corner controls and drop rules, and each kind's body painter.

use super::*;

pub(crate) const GRAPH_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 110.0);

/// What a card's header says (`App::paint_card_header`).
pub(crate) struct CardHeader {
    pub(crate) glyph: &'static str,
    /// The glyph's color; the dim text color if `None`.
    pub(crate) glyph_color: Option<egui::Color32>,
    /// Drawn in the glyph's place, if the card has one (a favicon).
    pub(crate) icon_image: Option<egui::TextureId>,
    pub(crate) name: String,
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
/// stays visible. Cut as `truncate_to_width` cuts the reversed text, which shares its cache.
fn truncate_start_to_width(
    cache: &mut HashMap<TruncateCacheKey, String>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
) -> String {
    let reversed: String = text.chars().rev().collect();
    let cut = truncate_to_width(cache, ctx, &reversed, font_id, max_width);
    match cut.strip_suffix('\u{2026}') {
        Some(kept) if kept.len() < reversed.len() => format!("\u{2026}{}", kept.chars().rev().collect::<String>()),
        _ => cut.chars().rev().collect(),
    }
}

/// Height of a Tab card's header (favicon, title, source line), in screen points.
const TAB_HEADER_POINTS: f32 = 40.0;
/// Below this card height (screen points) the header gives way to the old strip over the thumbnail.
const TAB_HEADER_MIN_CARD_POINTS: f32 = 80.0;

/// Whether a Tab card of `rect` (world units, `inv_zoom` world units per screen point) is tall
/// enough on screen to show its header.
pub(crate) fn tab_has_header(rect: egui::Rect, inv_zoom: f32) -> bool {
    rect.height() / inv_zoom >= TAB_HEADER_MIN_CARD_POINTS
}

/// The part of a `src`-sized texture (as a uv rect) that fills a `dst`-shaped rect without
/// stretching: the full width or height, cropped on the other axis around the top-center, so a
/// page's header stays in view.
pub(crate) fn cover_uv(src: (u32, u32), dst: egui::Vec2) -> egui::Rect {
    let full = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    if src.0 == 0 || src.1 == 0 || dst.x <= 0.0 || dst.y <= 0.0 {
        return full;
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
/// drop-target rect a dragged tab is tested against on release — see `GroupPayload`'s own doc
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

/// A JSON leaf's (from an old "Expand as Graph") or error card's fixed box
/// -- there's no root to preview in either case, just a one-line summary or
/// the missing-file card. A file-backed Json card is sized by
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
                egui::Color32::from_black_alpha(190),
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

pub(crate) fn card_style(kind: ItemNodeKind, visuals: &egui::Visuals) -> CardStyle {
    let themed = CardStyle {
        fill: visuals.extreme_bg_color,
        border_hover: visuals.text_color(),
        border_idle: visuals.weak_text_color(),
        close_fill: visuals.extreme_bg_color,
        close_border: visuals.weak_text_color(),
        close_icon: visuals.text_color(),
        dim_rounding: 4.0,
    };
    match kind {
        ItemNodeKind::Note | ItemNodeKind::Download | ItemNodeKind::Tab | ItemNodeKind::Unloaded => {
            CardStyle { dim_rounding: 6.0, ..themed }
        }
        // ACCENT at rest, as group cards have always been.
        ItemNodeKind::Group => CardStyle { border_idle: ACCENT, dim_rounding: 6.0, ..themed },
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
    pub(crate) kind: ItemNodeKind,
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

    pub(crate) fn action(self, kind: ItemNodeKind) -> NodeAction {
        match self {
            CardControl::Swatch => NodeAction::CycleColor,
            CardControl::Pin => NodeAction::TogglePin,
            CardControl::Audio => NodeAction::ToggleMute,
            CardControl::Eject => NodeAction::RemoveFromGroup,
            CardControl::Close if kind == ItemNodeKind::Group => NodeAction::Dissolve,
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

/// The buttons `view`'s card shows, in registration order (see
/// `CardControl`): the swatch on every card, a tab's (live or unloaded)
/// pin, and a live tab's audio indicator while playing or muted, eject on
/// a grouped one, and the ✕ last, so it wins over anything under it.
pub(crate) fn card_controls(view: &CardView) -> Vec<CardControl> {
    let mut controls = vec![CardControl::Swatch];
    if matches!(view.kind, ItemNodeKind::Tab | ItemNodeKind::Unloaded) {
        controls.push(CardControl::Pin);
        // Also while muted after audio stops, so a mute can always be
        // seen and undone from Graph view. An unloaded card has no engine
        // to be playing or muted, so it never gets this control.
        if view.kind == ItemNodeKind::Tab && (view.audio_playing || view.muted) {
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
    Card(ItemNodeKind),
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

/// Every drag-and-drop rule in Graph view, for every kind.
pub(crate) fn drop_action(dragged: ItemNodeKind, target: DropTarget, inside_group: bool) -> Option<DropEffect> {
    use ItemNodeKind::{Group, Tab, Unloaded};
    match (dragged, target, inside_group) {
        (_, DropTarget::BackButton, true) => Some(DropEffect::Eject),
        (Group, DropTarget::Card(Group), _) => Some(DropEffect::Merge),
        (_, DropTarget::Card(Group), _) => Some(DropEffect::Join),
        (Tab | Unloaded, DropTarget::Card(Tab | Unloaded), false) => Some(DropEffect::GroupWith),
        (Tab | Unloaded, DropTarget::Card(Tab | Unloaded), true) => Some(DropEffect::Reparent),
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
    /// Keyboard navigation is on (a card or control had focus last frame): each card shows its
    /// Tab-order number.
    pub(crate) show_order: bool,
    /// The level's card ids in Tab order (creation order, `App::card_tab_order`). Only the first is
    /// a Tab stop from outside the graph; `App::handle_card_tab` moves between the rest.
    pub(crate) order: &'a [String],
    /// A card or one of its controls has keyboard focus this frame.
    pub(crate) any_focused: bool,
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
#[allow(clippy::too_many_arguments)]
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
        // of the force-directed layout (see `TabPayload::pinned`).
        CardControl::Pin => {
            let pin_color = if view.pinned { ACCENT } else { ui.visuals().weak_text_color() };
            if !lod_simplified {
                let pin_icon_pos = rect.center();
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
                    let icon_pos = rect.center();
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
                    ui.painter().rect_filled(bar_rect, 1.0 * inv_zoom, waveform_color);
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
#[allow(clippy::too_many_arguments)]
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
    let icon_pos = rect.center();
    let icon_color = style.close_icon;
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        overlay_painter.text(
            project(icon_pos),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(14.0),
            icon_color,
        );
    }));
}

/// A restored image/JSON card whose file couldn't be loaded: the same
/// `FILE_X` glyph a failed PDF card shows (see `ImagePayload::error`).
pub(crate) fn paint_missing_file_card(
    deferred_overlays: &mut DeferredOverlays,
    rect: egui::Rect,
    lod_simplified: bool,
) {
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
pub(crate) fn node_card_size(payload: &NodePayload) -> egui::Vec2 {
    match payload {
        NodePayload::Tab(_) | NodePayload::Unloaded(_) => GRAPH_NODE_SIZE,
        NodePayload::Download(_) => DOWNLOAD_NODE_SIZE,
        NodePayload::Note(_) => NOTE_NODE_SIZE,
        // A placeholder has no real size: the same box a failed PDF gets.
        NodePayload::Image(image) if image.error.is_some() => PDF_PENDING_NODE_SIZE,
        NodePayload::Image(image) => image_node_card_size(image.width, image.height),
        NodePayload::Pdf(pdf) => pdf_card_size(pdf),
        // A file's own card previews its root (spec "The card at the
        // parent level"); a leaf or an error card keeps the small box.
        NodePayload::Json(json) if json.key.is_none() && json.error.is_none() => {
            crate::json_view::preview_size(&json.value)
        }
        NodePayload::Json(_) => JSON_NODE_SIZE,
        NodePayload::Group(_) => GROUP_NODE_SIZE,
    }
}

/// `node`'s card in world space: `node_card_size` centered on its position.
pub(crate) fn node_card_rect(node: &GraphNode) -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(node.pos.x, node.pos.y), node_card_size(&node.payload))
}

/// What the Graph filter box matches a card against: a tab's title and
/// URL, a download's full path, a note's text, a file node's filename, a
/// group's name.
pub(crate) fn card_filter_text(payload: &NodePayload) -> Vec<&str> {
    match payload {
        NodePayload::Tab(tab) => vec![&tab.title, &tab.url],
        NodePayload::Unloaded(tab) => vec![&tab.title, &tab.url],
        NodePayload::Download(download) => vec![&download.path],
        NodePayload::Note(note) => vec![&note.text],
        NodePayload::Image(image) => vec![download_filename(&image.path)],
        NodePayload::Pdf(pdf) => vec![download_filename(&pdf.path)],
        NodePayload::Json(json) => vec![download_filename(&json.path)],
        NodePayload::Group(group) => vec![&group.name],
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

impl App {
    /// `id`'s `CardView`, or `None` when the node is gone.
    pub(crate) fn card_view(&self, id: &str) -> Option<CardView> {
        let node = self.nodes.get(id)?;
        let kind = item_node_kind(&node.payload);
        let members: Vec<String> = if kind == ItemNodeKind::Group {
            self.nodes
                .iter()
                .filter(|(_, n)| n.group.as_deref() == Some(id))
                .map(|(member, _)| member.clone())
                .collect()
        } else {
            Vec::new()
        };
        let tab = node.as_tab();
        Some(CardView {
            kind,
            color: node.color,
            in_group: node.group.is_some(),
            overloaded: members.len() > GROUP_OVERLOAD_THRESHOLD,
            members,
            focused: self.focused.as_deref() == Some(id),
            pinned: node.payload.pinned(),
            muted: tab.is_some_and(|t| t.muted),
            audio_playing: tab.is_some_and(|t| t.audio_playing),
            selected: self.selected_tabs.contains(id),
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
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
        let rect = node_card_rect(self.nodes.get(id)?);
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
        // Tab can reach them and focus survives a pan that hasn't brought them into view yet.
        let keeps_keyboard = pass.order.first().is_some_and(|first| first == id)
            || ui.memory(|m| m.has_focus(egui::Id::new(("graph_card", id))));
        if self.dragging.as_deref() != Some(id) && !keeps_keyboard && !rect.intersects(self.camera.rect) {
            return None;
        }
        let view = self.card_view(id)?;
        // Glow and fill go under the body, but hover is only known after
        // the interacts, which must follow the body (below). This slot is
        // filled in once it is.
        let background = ui.painter().add(egui::Shape::Noop);
        let body = self.paint_card_body(ui, deferred_overlays, id, &view, rect, inv_zoom, zoom_scale);

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
        // Space on a focused tab card toggles it in the selection, as the marquee would select it. Taken
        // before the interact, which would otherwise read it as a click and open the tab.
        if matches!(view.kind, ItemNodeKind::Tab | ItemNodeKind::Unloaded)
            && ui.memory(|m| m.has_focus(card_id))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space))
            && !self.selected_tabs.remove(id)
        {
            self.selected_tabs.insert(id.to_string());
        }
        let response = ui.interact(body.interact_rect, card_id, sense);
        // Not while a card's menu is open: the pointer is usually still on
        // the card it right-clicked, and the tooltip would cover the menu.
        let response = match body.tooltip {
            Some(tooltip) if self.node_menu.is_none() => response.on_hover_text(tooltip),
            _ => response,
        };
        // Screen readers: the card is a button named by its kind and title (egui announces it on focus).
        if let Some(node) = self.nodes.get(id) {
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), node.accessible_name())
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
        let is_hovered = self.hovered.as_deref() == Some(id) || pointer_on_card;

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
        // Registered after both interacts above, so they take the pointer.
        let delete = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Delete));
        // Shift+F10 on a focused card opens its menu, anchored at the card's bottom-left.
        if response.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F10)) {
            let corner = rect.left_bottom();
            let at = ui.ctx().layer_transform_to_global(ui.layer_id()).map_or(corner, |t| t * corner);
            self.node_menu = Some((id.to_string(), at));
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
            if control_response.clicked() && control.is_none() {
                control = Some(card_control.action(view.kind));
            }
        }

        // This frame's hover only: passing `is_hovered` (which includes
        // `self.hovered`) made a card stay hovered, ✕ and all, after the
        // pointer left it, until another card was hovered.
        let still_hovered = pointer_on_card || control_hovered;
        if response.dragged() {
            self.dragging = Some(id.to_string());
            // Dragging one selected tab moves the whole selection, as one
            // unit (see `take_drag_unit`). Recomputed (and overwritten)
            // every dragged frame rather than only at drag start: cheap,
            // and it's always exactly this frame's moved set, so whatever
            // `drag_moved` holds when the drag ends is the full, correct
            // set to mark -- see the release branch below.
            let delta = response.drag_delta();
            let moved: Vec<String> =
                if view.selected { self.selected_tabs.iter().cloned().collect() } else { vec![id.to_string()] };
            for moved_id in &moved {
                if let Some(node) = self.nodes.get_mut(moved_id) {
                    node.pos += delta;
                }
            }
            self.drag_moved = moved;
        } else if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
            // Marked once, here at release, not every dragged frame above:
            // `explicit` is drained by any successful `save_session()`, and
            // saves fire independently of this gesture mid-drag (e.g.
            // `poll_store`, or a url change batched through
            // `drain_tab_events`), so marking every frame could commit an
            // intermediate position as its own op, repeatedly, before the
            // drag even ends.
            // A card removed mid-drag (closed, crashed, swapped) leaves its
            // id here; skip it rather than clearing at every removal site.
            for moved_id in std::mem::take(&mut self.drag_moved) {
                if self.nodes.contains_key(&moved_id) {
                    self.mark(&moved_id, field::POS);
                }
            }
            // `drop_preview` was computed this frame for this card, and
            // nothing moved it since (no drag delta on a release frame).
            pass.actions.push(GraphAction::Drop { id: id.to_string(), target: pass.drop_preview.clone() });
        }
        if still_hovered {
            self.hovered = Some(id.to_string());
        } else if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }

        if let Some(node) = self.nodes.get(id) {
            dim_if_filtered_out(
                deferred_overlays,
                &self.graph_filter,
                &card_filter_text(&node.payload),
                rect,
                style.dim_rounding,
            );
        }
        Some(CardOutput { response, control })
    }

    /// A graph card's own content, per kind -- everything under the
    /// shared frame's border (see `draw_graph_card`). Returns which part
    /// of the card is the click/drag handle and an optional tooltip.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_card_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        view: &CardView,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        match view.kind {
            ItemNodeKind::Download => self.paint_download_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            ItemNodeKind::Note => self.paint_note_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            ItemNodeKind::Image => self.paint_image_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            ItemNodeKind::Pdf => self.paint_pdf_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            ItemNodeKind::Json => self.paint_json_body(ui, deferred_overlays, id, rect, zoom_scale),
            ItemNodeKind::Group => self.paint_group_body(ui, deferred_overlays, id, view, rect, inv_zoom, zoom_scale),
            ItemNodeKind::Tab => self.paint_tab_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
            ItemNodeKind::Unloaded => {
                self.paint_unloaded_tab_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale)
            }
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
        body.tooltip = Some(match &state {
            UnloadedState::Failed(error) => {
                format!("Couldn't start {}: {error}\nClick to try again.", engine_label(tab.kind))
            }
            UnloadedState::Sleeping | UnloadedState::Waking => sleeping_or_waking_tooltip(&state).to_string(),
        });
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
            glyph: if failed { egui_phosphor::light::WARNING } else { egui_phosphor::light::MOON },
            glyph_color: failed.then_some(DANGER_COLOR),
            icon_image: if failed { None } else { favicon },
            name: if title.is_empty() { url.clone() } else { title },
            source: url,
            source_is_path: false,
            scrim: false,
            right_margin: 0.0,
        };
        let tall = self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
        // The middle of the body below the header: the failure icon, the waking spinner, or the
        // moon of a sleeping tab.
        let body_top = if tall { rect.top() + TAB_HEADER_POINTS * inv_zoom } else { rect.top() };
        let middle = egui::Rect::from_min_max(egui::pos2(rect.left(), body_top), rect.right_bottom()).center();
        match &state {
            UnloadedState::Failed(_) => {
                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                    overlay_painter.text(
                        project(middle),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::light::WARNING_CIRCLE,
                        egui::FontId::proportional(24.0),
                        DANGER_COLOR,
                    );
                }));
            }
            UnloadedState::Waking => {
                ui.put(
                    egui::Rect::from_center_size(middle, egui::vec2(24.0, 24.0) * inv_zoom),
                    egui::Spinner::new().size(24.0 * inv_zoom),
                );
            }
            UnloadedState::Sleeping => {
                let color = ui.visuals().weak_text_color();
                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                    overlay_painter.text(
                        project(middle),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::light::MOON,
                        egui::FontId::proportional(24.0),
                        color,
                    );
                }));
            }
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
        let Some((download_path, download_url, completed, total_bytes, bytes_received)) = self
            .nodes
            .get(id)
            .and_then(|n| n.as_download())
            .map(|d| (d.path.clone(), d.url.clone(), d.completed, d.total_bytes, d.bytes_received))
        else {
            return body;
        };
        let filename = download_filename(&download_path);
        // LOD: the header text is skipped once simplified -- see `paint_tab_body`'s own LOD comment.
        if !self.graph_lod_simplified {
            let header = CardHeader {
                glyph: egui_phosphor::light::DOWNLOAD_SIMPLE,
                glyph_color: None,
                icon_image: None,
                name: filename.to_owned(),
                source_is_path: download_url.is_none(),
                source: download_url.unwrap_or_else(|| download_path.clone()),
                scrim: false,
                right_margin: 0.0,
            };
            self.paint_card_header(ui, deferred_overlays, rect, inv_zoom, zoom_scale, header);
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
            egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 18.0 * inv_zoom), rect.max)
        };
        let is_editing = self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);

        ui.painter().rect_filled(paper, 4.0 * inv_zoom, NOTE_CARD_COLOR);
        if !self.graph_lod_simplified {
            let words = self.nodes.get(id).and_then(|n| n.as_note()).map_or(0, |n| n.text.split_whitespace().count());
            let header = CardHeader {
                glyph: egui_phosphor::light::NOTE_PENCIL,
                glyph_color: None,
                icon_image: None,
                name: "Note".to_owned(),
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
                            .text_color(egui::Color32::from_black_alpha(220))
                            .hint_text("Type a note..."),
                    )
                    .lost_focus();
            }
            // Committed on blur, not on every keystroke -- a note is
            // persisted (unlike every other per-node display state
            // in the session store, see `NotePayload`'s own doc
            // comment), and `save_session` is real file I/O, so
            // writing it once per edit *session* rather than once
            // per character avoids hammering disk while typing.
            if lost_focus {
                if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                    note.editing = false;
                }
                // Escape ends the edit; focus goes back to the card so keyboard users keep their place
                // (a click elsewhere keeps the focus it took).
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    ui.memory_mut(|m| m.request_focus(egui::Id::new(("graph_card", id))));
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
            // see `paint_tab_body`'s own LOD comment. Never
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
        body
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
            paint_missing_file_card(deferred_overlays, rect, self.graph_lod_simplified);
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
                    ui.painter().image(
                        tex_id,
                        rect,
                        egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                    if let Some(image) = self.nodes.get(id).and_then(|n| n.as_image()) {
                        let (name, source, source_is_path) = file_header_text(&image.path, image.url.as_deref());
                        let header = CardHeader {
                            glyph: egui_phosphor::light::IMAGE,
                            glyph_color: None,
                            icon_image: None,
                            name,
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
                    ui.painter().image(
                        tex_id,
                        rect,
                        egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                    if let Some(pdf) = self.nodes.get(id).and_then(|n| n.as_pdf()) {
                        let (name, source, source_is_path) = file_header_text(&pdf.path, pdf.url.as_deref());
                        // The page count sits at the bottom right, so the fallback strip stops short of it.
                        let header = CardHeader {
                            glyph: egui_phosphor::light::FILE_PDF,
                            glyph_color: None,
                            icon_image: None,
                            name,
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
                    ui.put(rect, egui::Spinner::new().size(24.0 * inv_zoom));
                }
            }
        }
        body
    }

    /// Json: an error card keeps the missing-file box, and a leaf from an
    /// old "Expand as Graph" keeps its one-line summary (both fixed at
    /// `JSON_NODE_SIZE`). A file-backed node instead previews its root --
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
            paint_missing_file_card(deferred_overlays, rect, self.graph_lod_simplified);
            body.tooltip = Some(error);
            return body;
        }
        let Some(json) = self.nodes.get(id).and_then(|n| n.as_json()) else { return body };
        let text = !self.graph_lod_simplified && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH;
        if json.key.is_some() {
            // A leaf from an old "Expand as Graph": its one-line summary.
            let key = json.key.clone().unwrap_or_default();
            let key_display =
                if key.chars().count() > 16 { format!("{}…", key.chars().take(15).collect::<String>()) } else { key };
            let summary = format!("{key_display}: {}", json_summary(&json.value));
            if text {
                let center = rect.center();
                let color = ui.visuals().text_color();
                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                    overlay_painter.text(
                        project(center),
                        egui::Align2::CENTER_CENTER,
                        summary,
                        egui::FontId::proportional(12.0),
                        color,
                    );
                }));
            }
            return body;
        }
        let len = crate::json_view::entry_count(&json.value);
        let rows = len.min(crate::json_view::PREVIEW_ROWS);
        let card = crate::json_view::Card {
            pointer: String::new(),
            parent: None,
            depth: 0,
            collapsed: false,
            rows,
            more: len - rows,
        };
        let mut header = crate::json_level_ui::header_parts(&json.value, &card, download_filename(&json.path));
        header.source = file_header_text(&json.path, None).1.rsplit_once(['\\', '/']).map(|(dir, _)| dir.to_owned());
        let pal = crate::json_level_ui::palette(ui.visuals().dark_mode);
        let look = crate::json_level_ui::CardLook {
            palette: &pal,
            text_color: ui.visuals().text_color(),
            inv_zoom: 1.0 / zoom_scale,
            text,
            selected_row: None,
            hit_rows: &[],
            // No child cards or edges at the parent level: a port dot here
            // would point at nothing.
            ports: false,
            preview: true,
        };
        crate::json_level_ui::paint_json_content(
            ui.painter(),
            deferred_overlays,
            rect,
            &json.value,
            &header,
            rows,
            len - rows,
            &look,
        );
        body
    }

    /// Header F (folder glyph, name, item count) over a member thumbnail grid (header only when
    /// empty or with "Collapse Groups" on). LOD: skipped once simplified, leaving the frame's
    /// plain card.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_group_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        view: &CardView,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let body = CardBody { interact_rect: rect, tooltip: None };
        let member_ids = &view.members;
        let member_count = member_ids.len();
        if !self.graph_lod_simplified {
            if let Some(node) = self.nodes.get(id).and_then(|n| n.as_group()) {
                let name = node.name.clone();
                let header = CardHeader {
                    glyph: egui_phosphor::light::FOLDER,
                    glyph_color: None,
                    icon_image: None,
                    name,
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
                    let (top, bottom) = if tall { (TAB_HEADER_POINTS * inv_zoom, 0.0) } else { (0.0, 18.0 * inv_zoom) };
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
                                ui.painter().image(
                                    tex_id,
                                    cell_rect,
                                    egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
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
        header: CardHeader,
    ) -> bool {
        let tall = tab_has_header(rect, inv_zoom);
        if rect.width() * zoom_scale < MIN_LABEL_CARD_SCREEN_WIDTH {
            return tall;
        }
        let CardHeader { glyph, glyph_color, icon_image, name, source, source_is_path, scrim, right_margin } = header;
        let band_height = if tall { TAB_HEADER_POINTS } else { 18.0 } * inv_zoom;
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
            ui.painter().rect_filled(band, corners, egui::Color32::from_black_alpha(if tall { 190 } else { 160 }));
        }
        let light = scrim || !tall;
        let title_color = if light { egui::Color32::from_gray(240) } else { ui.visuals().text_color() };
        let source_color = if light { egui::Color32::from_gray(190) } else { ui.visuals().weak_text_color() };
        let glyph_color = glyph_color.unwrap_or(source_color);
        let title_font = egui::FontId::proportional(13.0);
        let source_font = egui::FontId::proportional(11.0);
        let text_left = if tall { 28.0 } else { 8.0 };
        let max_width = (rect.width() * zoom_scale - text_left - 28.0 - right_margin).max(0.0);
        let title = truncate_to_width(&mut self.truncate_cache, ui.ctx(), &name, &title_font, max_width);
        let source = match (tall, source_is_path) {
            (false, _) => String::new(),
            (true, true) => {
                truncate_start_to_width(&mut self.truncate_cache, ui.ctx(), &source, &source_font, max_width)
            }
            (true, false) => truncate_to_width(&mut self.truncate_cache, ui.ctx(), &source, &source_font, max_width),
        };
        if let (true, Some(tex_id)) = (tall, icon_image) {
            let center = band.left_top() + egui::vec2(18.0, 20.0) * inv_zoom;
            let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
            ui.painter().image(
                tex_id,
                egui::Rect::from_center_size(center, FAVICON_SIZE * inv_zoom),
                uv,
                egui::Color32::WHITE.gamma_multiply(0.6),
            );
        }
        let origin = band.left_top();
        let band_screen_height = band_height * zoom_scale;
        let draw_glyph = tall && icon_image.is_none();
        deferred_overlays.push(Box::new(move |overlay_painter, project| {
            let base = project(origin);
            if tall {
                if draw_glyph {
                    overlay_painter.text(
                        base + egui::vec2(10.0, 20.0),
                        egui::Align2::LEFT_CENTER,
                        glyph,
                        egui::FontId::proportional(14.0),
                        glyph_color,
                    );
                }
                overlay_painter.text(
                    base + egui::vec2(text_left, 12.0),
                    egui::Align2::LEFT_CENTER,
                    title,
                    title_font,
                    title_color,
                );
                overlay_painter.text(
                    base + egui::vec2(text_left, 28.0),
                    egui::Align2::LEFT_CENTER,
                    source,
                    source_font,
                    source_color,
                );
            } else {
                overlay_painter.text(
                    base + egui::vec2(text_left, band_screen_height / 2.0),
                    egui::Align2::LEFT_CENTER,
                    title,
                    egui::FontId::proportional(12.0),
                    title_color,
                );
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
        let body = CardBody { interact_rect: rect, tooltip: None };
        let Some((title, url, engine_kind)) =
            self.nodes.get(id).and_then(|n| n.as_tab()).map(|t| (t.title.clone(), t.url.clone(), t.tab.kind))
        else {
            return body;
        };
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
                    .map_or(egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), |size| {
                        cover_uv(size, thumb_rect.size())
                    });
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
                let icon_center =
                    egui::pos2(rect.left() + (GRAPH_THUMB_MARGIN + 8.0) * inv_zoom, rect.top() + 20.0 * inv_zoom);
                match favicon {
                    Some(tex_id) => {
                        ui.painter().image(
                            tex_id,
                            egui::Rect::from_center_size(icon_center, FAVICON_SIZE * inv_zoom),
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().circle_filled(icon_center, 4.0 * inv_zoom, engine_color(engine_kind));
                    }
                }
                if show_text {
                    // Screen-constant text: the header is 40 screen points tall at any zoom.
                    let text_left = GRAPH_THUMB_MARGIN + 22.0;
                    // Leave room for the audio indicator at the top-right corner.
                    let max_width = (rect.width() * zoom_scale - text_left - 28.0).max(0.0);
                    let title_font = egui::FontId::proportional(13.0);
                    let source_font = egui::FontId::proportional(11.0);
                    let title_text =
                        truncate_to_width(&mut self.truncate_cache, ui.ctx(), label, &title_font, max_width);
                    let source_text =
                        truncate_to_width(&mut self.truncate_cache, ui.ctx(), &url, &source_font, max_width);
                    let (title_color, source_color) = (ui.visuals().text_color(), ui.visuals().weak_text_color());
                    let origin = rect.left_top();
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        let base = project(origin) + egui::vec2(text_left, 0.0);
                        overlay_painter.text(
                            base + egui::vec2(0.0, 12.0),
                            egui::Align2::LEFT_CENTER,
                            title_text,
                            title_font,
                            title_color,
                        );
                        overlay_painter.text(
                            base + egui::vec2(0.0, 28.0),
                            egui::Align2::LEFT_CENTER,
                            source_text,
                            source_font,
                            source_color,
                        );
                    }));
                }
            } else {
                // A flat scrim behind the title strip so it stays legible over a light thumbnail.
                let title_strip = egui::Rect::from_min_max(
                    egui::Pos2::new(thumb_rect.left(), thumb_rect.bottom() - 18.0 * inv_zoom),
                    thumb_rect.right_bottom(),
                );
                ui.painter().rect_filled(title_strip, 0.0, egui::Color32::from_black_alpha(160));
                // Engine identity as a small colored dot; the strip is too small for more.
                ui.painter().circle_filled(
                    egui::Pos2::new(title_strip.left() + 8.0 * inv_zoom, title_strip.center().y),
                    4.0 * inv_zoom,
                    engine_color(engine_kind),
                );
                // Below `MIN_LABEL_CARD_SCREEN_WIDTH` the label would be a sliver; the dot alone remains.
                if show_text {
                    let label_font = egui::FontId::proportional(14.0);
                    // Screen width left for the label: the strip's, less the 16px left offset and some room on the right.
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
                    ui.painter().image(
                        tex_id,
                        favicon_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json_view::JsonView;

    /// A `CardView` for `kind` with nothing set, for tests to override.
    fn test_view(kind: ItemNodeKind) -> CardView {
        CardView {
            kind,
            color: None,
            in_group: false,
            overloaded: false,
            members: Vec::new(),
            focused: false,
            pinned: false,
            muted: false,
            audio_playing: false,
            selected: false,
        }
    }

    #[test]
    fn card_border_resolves_focus_tag_overload_idle_and_hover() {
        // Opaque colors, so `lerp_color32` lands exactly on either end.
        let style = CardStyle {
            border_idle: egui::Color32::GRAY,
            border_hover: egui::Color32::WHITE,
            ..card_style(ItemNodeKind::Note, &egui::Visuals::dark())
        };
        let red = egui::Color32::RED;
        let view = |color| CardView { color, ..test_view(ItemNodeKind::Note) };
        // At rest: the tag color, else the kind's idle color.
        assert_eq!(card_border(&view(None), &style, 0.0), (1.0, egui::Color32::GRAY));
        assert_eq!(card_border(&view(Some(red)), &style, 0.0), (1.0, red));
        // An overloaded group rests on DANGER_COLOR, unless tagged.
        let overloaded = |color| CardView { color, overloaded: true, ..test_view(ItemNodeKind::Group) };
        assert_eq!(card_border(&overloaded(None), &style, 0.0), (1.0, DANGER_COLOR));
        assert_eq!(card_border(&overloaded(Some(red)), &style, 0.0), (1.0, red));
        // Fully hovered: the hover color at 1.5, whatever the resting color.
        assert_eq!(card_border(&view(Some(red)), &style, 1.0), (1.5, egui::Color32::WHITE));
        // The open tab: ACCENT at 2.5 over everything, not eased.
        let focused = CardView { color: Some(red), focused: true, ..test_view(ItemNodeKind::Tab) };
        assert_eq!(card_border(&focused, &style, 0.0), (2.5, ACCENT));
        assert_eq!(card_border(&focused, &style, 1.0), (2.5, ACCENT));
    }

    #[test]
    fn file_header_text_prefers_the_url_over_the_path() {
        let path = r"C:\pics\a.jpg";
        assert_eq!(file_header_text(path, None), ("a.jpg".to_owned(), path.to_owned(), true));
        let url = "https://x.test/a.jpg";
        assert_eq!(file_header_text(path, Some(url)), ("a.jpg".to_owned(), url.to_owned(), false));
    }

    #[test]
    fn cover_uv_crops_the_longer_axis_and_never_stretches() {
        let uv = |src, w, h| cover_uv(src, egui::vec2(w, h));
        // Same shape: the whole texture.
        assert_eq!(uv((256, 192), 128.0, 96.0), egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)));
        // Wider source: sides cropped evenly.
        let wide = uv((200, 100), 100.0, 100.0);
        assert_eq!((wide.min.x, wide.max.x, wide.min.y, wide.max.y), (0.25, 0.75, 0.0, 1.0));
        // Taller source: bottom cropped, top kept.
        let tall = uv((100, 200), 100.0, 100.0);
        assert_eq!((tall.min.y, tall.max.y, tall.min.x, tall.max.x), (0.0, 0.5, 0.0, 1.0));
        // A degenerate size falls back to the whole texture.
        assert_eq!(uv((0, 0), 10.0, 10.0).max, egui::pos2(1.0, 1.0));
    }

    #[test]
    fn tab_thumb_rect_leaves_room_for_the_header_only_on_tall_cards() {
        let card = egui::Rect::from_center_size(egui::Pos2::ZERO, GRAPH_NODE_SIZE);
        // Zoom 1: 110 points tall, so the header (40 points) takes the top.
        let thumb = tab_thumb_rect(card, 1.0);
        assert_eq!(thumb.top(), card.top() + TAB_HEADER_POINTS);
        assert_eq!(thumb.bottom(), card.bottom() - crate::GRAPH_THUMB_MARGIN);
        // Zoom 0.5: 55 points tall, so the plain inset thumbnail and the strip.
        assert!(!tab_has_header(card, 2.0));
        assert_eq!(tab_thumb_rect(card, 2.0), card.shrink(crate::GRAPH_THUMB_MARGIN));
    }

    #[test]
    fn card_controls_list_each_cards_buttons_in_pointer_order() {
        use CardControl::*;
        for kind in GRAPH_DRAW_ORDER.into_iter().filter(|&k| !matches!(k, ItemNodeKind::Tab | ItemNodeKind::Unloaded)) {
            assert_eq!(card_controls(&test_view(kind)), vec![Swatch, Close], "{kind:?}");
            let grouped = CardView { in_group: true, ..test_view(kind) };
            assert_eq!(card_controls(&grouped), vec![Swatch, Eject, Close], "{kind:?}");
            let close = if kind == ItemNodeKind::Group { NodeAction::Dissolve } else { NodeAction::Delete };
            assert_eq!(Close.action(kind), close, "{kind:?}");
        }
        // An unloaded card shows its pin too, but never the audio
        // indicator -- it has no engine to be playing or muted.
        assert_eq!(card_controls(&test_view(ItemNodeKind::Unloaded)), vec![Swatch, Pin, Close]);
        assert_eq!(
            card_controls(&CardView { audio_playing: true, muted: true, ..test_view(ItemNodeKind::Unloaded) }),
            vec![Swatch, Pin, Close]
        );
        // A tab always shows its pin; its audio indicator while playing or
        // muted. Close still registers last: it overlaps the indicator.
        let tab = |f: fn(&mut CardView)| {
            let mut view = test_view(ItemNodeKind::Tab);
            f(&mut view);
            card_controls(&view)
        };
        assert_eq!(tab(|_| {}), vec![Swatch, Pin, Close]);
        assert_eq!(tab(|v| v.pinned = true), vec![Swatch, Pin, Close]);
        assert_eq!(tab(|v| v.audio_playing = true), vec![Swatch, Pin, Audio, Close]);
        assert_eq!(tab(|v| v.muted = true), vec![Swatch, Pin, Audio, Close]);
        assert_eq!(
            tab(|v| {
                v.muted = true;
                v.in_group = true;
            }),
            vec![Swatch, Pin, Audio, Eject, Close]
        );
        assert_eq!(Close.action(ItemNodeKind::Tab), NodeAction::Delete);
        assert_eq!(Pin.action(ItemNodeKind::Tab), NodeAction::TogglePin);
        assert_eq!(Audio.action(ItemNodeKind::Tab), NodeAction::ToggleMute);
        assert!(!Pin.hover_only() && !Audio.hover_only());
        assert_eq!(Eject.action(ItemNodeKind::Note), NodeAction::RemoveFromGroup);
        assert_eq!(Swatch.action(ItemNodeKind::Note), NodeAction::CycleColor);
        assert!(Close.hover_only() && Eject.hover_only() && !Swatch.hover_only());
    }

    /// Pins each `CardControl`'s corner/offset anchor and size, at
    /// inv_zoom 1.0 and 2.0, as a regression check on `CardControl::rect`.
    #[test]
    fn card_control_rects_sit_on_their_corners() {
        use CardControl::*;
        let card = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 80.0));
        for inv_zoom in [1.0_f32, 2.0] {
            assert_eq!(
                Swatch.rect(card, inv_zoom),
                egui::Rect::from_center_size(card.right_bottom(), egui::Vec2::splat(16.0) * inv_zoom)
            );
            assert_eq!(
                Pin.rect(card, inv_zoom),
                egui::Rect::from_center_size(card.left_bottom(), egui::Vec2::splat(16.0) * inv_zoom)
            );
            assert_eq!(
                Audio.rect(card, inv_zoom),
                egui::Rect::from_center_size(
                    card.right_top() + egui::vec2(-8.0, 8.0) * inv_zoom,
                    egui::Vec2::splat(16.0) * inv_zoom
                )
            );
            assert_eq!(
                Eject.rect(card, inv_zoom),
                egui::Rect::from_center_size(card.left_top(), egui::Vec2::splat(18.0) * inv_zoom)
            );
            assert_eq!(
                Close.rect(card, inv_zoom),
                egui::Rect::from_center_size(card.right_top(), egui::Vec2::splat(18.0) * inv_zoom)
            );
        }
    }

    #[test]
    fn drop_action_matches_the_drop_table() {
        use ItemNodeKind as K;
        let card = DropTarget::Card;
        for kind in GRAPH_DRAW_ORDER {
            // The back button only exists inside a group.
            assert_eq!(drop_action(kind, DropTarget::BackButton, true), Some(DropEffect::Eject), "{kind:?}");
            assert_eq!(drop_action(kind, DropTarget::BackButton, false), None, "{kind:?}");
            let onto_group = if kind == K::Group { DropEffect::Merge } else { DropEffect::Join };
            for inside_group in [false, true] {
                assert_eq!(drop_action(kind, card(K::Group), inside_group), Some(onto_group), "{kind:?}");
            }
        }
        // An unloaded card is a tab on both sides of the drop.
        for (dragged, target) in
            [(K::Tab, K::Tab), (K::Tab, K::Unloaded), (K::Unloaded, K::Tab), (K::Unloaded, K::Unloaded)]
        {
            assert_eq!(
                drop_action(dragged, card(target), false),
                Some(DropEffect::GroupWith),
                "{dragged:?} -> {target:?}"
            );
            assert_eq!(
                drop_action(dragged, card(target), true),
                Some(DropEffect::Reparent),
                "{dragged:?} -> {target:?}"
            );
        }
        // Only a tab drops onto a tab, and nothing drops onto the file kinds.
        assert_eq!(drop_action(K::Note, card(K::Tab), false), None);
        assert_eq!(drop_action(K::Group, card(K::Tab), true), None);
        assert_eq!(drop_action(K::Tab, card(K::Note), false), None);
        assert_eq!(drop_action(K::Image, card(K::Json), true), None);
    }

    #[test]
    fn node_card_size_matches_each_kinds_card() {
        let download = NodePayload::Download(DownloadPayload {
            path: "f.bin".into(),
            url: None,
            bytes_received: 0,
            total_bytes: None,
            completed: false,
        });
        assert_eq!(node_card_size(&download), DOWNLOAD_NODE_SIZE);
        assert_eq!(
            node_card_size(&NodePayload::Note(NotePayload { text: String::new(), editing: false, url: None })),
            NOTE_NODE_SIZE
        );
        assert_eq!(
            node_card_size(&NodePayload::Group(GroupPayload { name: "g".into(), from_json: false })),
            GROUP_NODE_SIZE
        );
        let json = |value: serde_json::Value, key: Option<&str>, error: Option<&str>| {
            NodePayload::Json(JsonPayload {
                path: "a.json".into(),
                value,
                key: key.map(Into::into),
                error: error.map(Into::into),
                url: None,
                view: JsonView::default(),
            })
        };
        let five = serde_json::json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5});
        assert_eq!(node_card_size(&json(five.clone(), None, None)), crate::json_view::preview_size(&five));
        assert_eq!(node_card_size(&json(serde_json::json!({}), None, None)).x, crate::json_view::CARD_WIDTH);
        // A leaf from an old "Expand as Graph" and an error card keep the small size.
        assert_eq!(node_card_size(&json(serde_json::json!(1), Some("k"), None)), JSON_NODE_SIZE);
        assert_eq!(node_card_size(&json(five, None, Some("missing"))), JSON_NODE_SIZE);
        assert_eq!(
            node_card_size(&NodePayload::Unloaded(UnloadedTabPayload {
                url: "https://example.org/".into(),
                title: String::new(),
                kind: EngineKind::Cef,
                pinned: true,
                muted: false,
                zoom_factor: 1.5,
                state: UnloadedState::Failed("no binary".into()),
            })),
            GRAPH_NODE_SIZE
        );

        // Image: a small picture keeps its own size; an oversized one is
        // scaled down to fit `IMAGE_NODE_MAX_SIZE`, aspect ratio kept.
        let image = |width, height| {
            NodePayload::Image(ImagePayload { path: "p.png".into(), width, height, error: None, url: None })
        };
        assert_eq!(node_card_size(&image(40, 20)), egui::vec2(40.0, 20.0));
        let big = node_card_size(&image(100_000, 50_000));
        assert!(big.x <= IMAGE_NODE_MAX_SIZE.x && big.y <= IMAGE_NODE_MAX_SIZE.y);
        assert!((big.x / big.y - 2.0).abs() < 1e-3);

        // Pdf: pending and failed use the fixed pending box; rendered uses
        // the page's own size through the same image fitting.
        let pdf = |outcome| NodePayload::Pdf(PdfPayload { path: "d.pdf".into(), outcome, url: None });
        assert_eq!(node_card_size(&pdf(None)), PDF_PENDING_NODE_SIZE);
        assert_eq!(node_card_size(&pdf(Some(Err("bad".into())))), PDF_PENDING_NODE_SIZE);
        assert_eq!(
            node_card_size(&pdf(Some(Ok(PdfOutcome { width: 40, height: 20, page_count: 3 })))),
            egui::vec2(40.0, 20.0)
        );
        assert_eq!(node_card_size(&pdf(Some(Ok(PdfOutcome { width: 100_000, height: 50_000, page_count: 1 })))), big);
    }

    #[test]
    fn card_filter_text_matches_each_kinds_filter_input() {
        let download = NodePayload::Download(DownloadPayload {
            path: "C:/dl/f.bin".into(),
            url: None,
            bytes_received: 0,
            total_bytes: None,
            completed: true,
        });
        // Download filters on its full path; the file kinds on the filename only.
        assert_eq!(card_filter_text(&download), ["C:/dl/f.bin"]);
        assert_eq!(
            card_filter_text(&NodePayload::Note(NotePayload { text: "buy milk".into(), editing: false, url: None })),
            ["buy milk"]
        );
        assert_eq!(
            card_filter_text(&NodePayload::Image(ImagePayload {
                path: "C:/pics/cat.png".into(),
                width: 1,
                height: 1,
                error: None,
                url: None
            })),
            ["cat.png"]
        );
        assert_eq!(
            card_filter_text(&NodePayload::Pdf(PdfPayload {
                path: "C:/docs/paper.pdf".into(),
                outcome: None,
                url: None
            })),
            ["paper.pdf"]
        );
        assert_eq!(
            card_filter_text(&NodePayload::Json(JsonPayload {
                path: "C:/data/x.json".into(),
                value: serde_json::json!(1),
                key: None,
                error: None,
                url: None,
                view: JsonView::default()
            })),
            ["x.json"]
        );
        assert_eq!(
            card_filter_text(&NodePayload::Group(GroupPayload { name: "Reading".into(), from_json: false })),
            ["Reading"]
        );
        // Like a live tab: title and url, so the Graph filter can match a
        // sleeping card by its saved title, not just its url.
        assert_eq!(
            card_filter_text(&NodePayload::Unloaded(UnloadedTabPayload {
                url: "https://example.org/".into(),
                title: "Example".into(),
                kind: EngineKind::Cef,
                pinned: true,
                muted: false,
                zoom_factor: 1.5,
                state: UnloadedState::Failed("no binary".into()),
            })),
            ["Example", "https://example.org/"]
        );
        assert_eq!(
            card_filter_text(&NodePayload::Unloaded(UnloadedTabPayload {
                url: "https://example.org/".into(),
                title: String::new(),
                kind: EngineKind::Cef,
                pinned: true,
                muted: false,
                zoom_factor: 1.5,
                state: UnloadedState::Failed("no binary".into()),
            })),
            ["", "https://example.org/"]
        );
    }

    #[test]
    fn a_waking_cards_tooltip_differs_from_a_sleeping_ones() {
        assert_eq!(sleeping_or_waking_tooltip(&UnloadedState::Sleeping), "Sleeping — click to load");
        assert_eq!(sleeping_or_waking_tooltip(&UnloadedState::Waking), "Loading…");
    }
}
