//! Graph cards: the shared frame (`draw_graph_card`) with its sizes,
//! styles, corner controls and drop rules, and each kind's body painter.

use super::*;
use crate::caps::Click;
use crate::store::model::NodeId;
use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP, TYPE_IMAGE, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB};

mod download_body;
mod group_body;
mod image_body;
mod jar_chip;
mod json_body;
mod note_body;
mod pdf_body;
mod tab_body;
mod truncate;

pub(crate) use jar_chip::*;
pub(crate) use truncate::*;

/// Below this on-screen card width (in points), a node's label is hidden
/// entirely rather than truncated — a card this small can't fit more than
/// one or two letters plus "…" anyway (see `truncate_to_width`), and a
/// near-illegible sliver of text reads as noise, not information, at the
/// zoom levels this app's `GRAPH_ZOOM_MIN` allows.
pub(crate) const MIN_LABEL_CARD_SCREEN_WIDTH: f32 = 40.0;

/// Whether `rect`, drawn at `zoom_scale`, is wide enough on screen to carry a label (`MIN_LABEL_CARD_SCREEN_WIDTH`).
pub(crate) fn label_fits(rect: egui::Rect, zoom_scale: f32) -> bool {
    rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH
}

pub(crate) const GRAPH_THUMB_MARGIN: f32 = 6.0;

/// A group card's border tints the danger color once its member count exceeds
/// this threshold -- a cheap glanceable "this one's gotten big" cue. Only
/// applies while the group has no explicit tag color of its own (see its use
/// below) -- a deliberate user choice always wins over this automatic one,
/// same precedent tag colors already have over the plain default `ACCENT`
/// border.
pub(crate) const GROUP_OVERLOAD_THRESHOLD: usize = 8;

pub(crate) const GRAPH_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 110.0);

/// A jar card's fixed size: a header and nothing under it (spec "The jar card"). Exactly
/// `TAB_HEADER_MIN_CARD_POINTS` tall, so `tab_has_header` draws the header rather than the bottom strip from zoom 1 up.
pub(crate) const JAR_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, TAB_HEADER_MIN_CARD_POINTS);

/// A card of a type this build doesn't know: a header and nothing under it, like a jar's.
pub(crate) const UNKNOWN_NODE_SIZE: egui::Vec2 = JAR_NODE_SIZE;

/// What a card's header says (`App::paint_card_header`).
pub(crate) struct CardHeader<'a> {
    /// The icon at the header's left.
    pub(crate) identity: Identity,
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

/// Height of a card's header F (glyph or favicon, title, source line), in screen points.
pub(crate) const TAB_HEADER_POINTS: f32 = 40.0;
/// The strip a card too short for the header (`tab_has_header`) gets along its bottom instead, in screen points.
pub(crate) const TAB_STRIP_POINTS: f32 = 18.0;
/// Header F's layout, in screen points from the card's top-left. `paint_card_header` and the JSON
/// preview's own header (`json_level_ui::paint_json_content`) both place their text from these.
/// The icon's center (`Identity::paint`); the JSON preview's header uses it too.
pub(crate) const HEADER_ICON_X: f32 = 18.0;
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

/// Over a card the Graph toolbar filter doesn't match (`dim_if_filtered_out`): the canvas color at this alpha, so a
/// dimmed card fades into the canvas in either theme.
const FILTER_DIM_ALPHA: u8 = 190;

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
        paint_centered(overlay_painter, project(pos), &text, size, color);
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
/// dims the card at `rect` with one canvas-colored scrim (`scrim`), pushed as a deferred
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
    scrim: egui::Color32,
) {
    if graph_filter_dims(filter, haystacks) {
        push_dim_scrim(deferred_overlays, rect, rounding, scrim);
    }
}

/// The scrim `dim_if_filtered_out` pushes, for a caller that has already decided the card dims.
fn push_dim_scrim(deferred_overlays: &mut DeferredOverlays, rect: egui::Rect, rounding: f32, scrim: egui::Color32) {
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        overlay_painter.rect_filled(egui::Rect::from_min_max(project(rect.min), project(rect.max)), rounding, scrim);
    }));
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
    /// The border of the open tab (the accent) and of an overloaded group (the danger red).
    pub(crate) focus_border: egui::Color32,
    pub(crate) overload_border: egui::Color32,
    /// Whether the theme is dark: picks the tag-color table (`tag_shown`).
    pub(crate) dark: bool,
}

pub(crate) fn card_style(kind: CardKind, visuals: &egui::Visuals) -> CardStyle {
    let themed = CardStyle {
        fill: visuals.faint_bg_color,
        border_hover: visuals.text_color(),
        border_idle: visuals.weak_text_color(),
        close_fill: visuals.extreme_bg_color,
        close_border: visuals.weak_text_color(),
        close_icon: visuals.text_color(),
        focus_border: accent(visuals),
        overload_border: danger(visuals),
        dark: visuals.dark_mode,
    };
    // Presentation matches the type; behaviour reads the container table (`is_container`). See the plan 2b-6
    // Outcome, "Presentation arms".
    match kind.ty {
        // The accent at rest, as group cards have always been.
        TYPE_GROUP => CardStyle { border_idle: accent(visuals), ..themed },
        // Every other type, an unknown one and a virtual kind (`TYPE_VIRTUAL_PREFIX`) included, takes the themed default.
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
    pub(crate) members: Vec<NodeId>,
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
    /// False for the Default jar, which has no Close.
    pub(crate) closable: bool,
}

/// A card's border width and color at hover progress `hover_t` (0 at
/// rest, 1 hovered): the tag color at rest, else the danger color for an
/// overloaded group, else the kind's idle color, easing toward its hover
/// color, from width 1 to 1.5. A focused tab's is ACCENT at 2.5.
pub(crate) fn card_border(view: &CardView, style: &CardStyle, hover_t: f32) -> (f32, egui::Color32) {
    // The strongest, least ambiguous signal: not eased, and over any tag.
    if view.focused {
        return (2.5, style.focus_border);
    }
    let resting = view.color.map(|c| tag_shown(c, style.dark)).unwrap_or(if view.overloaded {
        style.overload_border
    } else {
        style.border_idle
    });
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

    /// What clicking it does. Close is `Delete` (`App::close_node`) on every kind, a group included: the ✕, Del and
    /// middle-click close a group with its members, restorable from Recently Closed. Dissolving one is the explicit
    /// "Dissolve Group" menu row and the toolbar's Ungroup All.
    pub(crate) fn action(self) -> NodeAction {
        match self {
            CardControl::Swatch => NodeAction::CycleColor,
            CardControl::Pin => NodeAction::TogglePin,
            CardControl::Audio => NodeAction::ToggleMute,
            CardControl::Eject => NodeAction::RemoveFromGroup,
            CardControl::Close => NodeAction::Delete,
        }
    }

    /// Its hover text, so an icon-only control says what a click does.
    pub(crate) fn tip(self, view: &CardView) -> &'static str {
        match self {
            CardControl::Swatch => "Next color (right-click to pick)",
            CardControl::Pin if view.pinned => "Unpin: let the layout move this card",
            CardControl::Pin => "Pin: keep this card where it is",
            CardControl::Audio if view.muted => "Unmute",
            CardControl::Audio => "Mute",
            CardControl::Eject => "Remove from group",
            CardControl::Close => match del_verb(view.kind) {
                "close" => "Close",
                "remove" => "Remove",
                _ => "Delete",
            },
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
    kind.caps().delete_verb
}

/// What a double-click (or Enter) on a `kind` of card does, for the kinds whose click is a `NodeAction`
/// (`App::draw_graph_cards`; a card whose file is missing retries it instead):
/// - Tab, Unloaded: opens it into Tab view (an unloaded one wakes, `App::wake_tab`).
/// - Image: maximizes it into Item view, the gesture a tab uses.
/// - Download, Pdf: reveals the file. A PDF has no Item view, so it gets the other file card's click.
///
/// A Note's double-click edits it in place and a Group or Json opens on it too, in the caller. A single click only
/// selects (`App::select_only`).
pub(crate) fn click_action(kind: CardKind) -> Option<NodeAction> {
    kind.caps().click.map(|click| match click {
        Click::Open => NodeAction::Open,
        Click::Reveal => NodeAction::Reveal,
    })
}

/// The buttons `view`'s card shows, in registration order (see
/// `CardControl`): the swatch and the pin on every card, and a live tab's audio indicator while playing or muted, eject on
/// a grouped one, and a closable card's ✕ last, so it wins over anything under it.
pub(crate) fn card_controls(view: &CardView) -> Vec<CardControl> {
    // A virtual kind has no node to colour, pin, eject or close.
    if !view.kind.caps().stored_controls {
        return Vec::new();
    }
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
    if view.closable {
        controls.push(CardControl::Close);
    }
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

impl DropEffect {
    /// What a drop says it will do, naming the target after it: "Merge into Research". Eject has no target card.
    pub(crate) fn hint(self, target: &str) -> String {
        match self {
            DropEffect::Eject => "Move out of group".to_owned(),
            DropEffect::Merge => format!("Merge into {target}"),
            DropEffect::Join => format!("Move into {target}"),
            DropEffect::GroupWith => format!("Group with {target}"),
            DropEffect::Reparent => format!("Make child of {target}"),
        }
    }
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
        // A virtual card is carried, never moved or contained: dropped on any card that takes drops it is a Join, and
        // its source decides what that means (`VirtualSource::on_drop`).
        (DropTarget::Card(_), _) if dragged.ty.starts_with(crate::store::model::TYPE_VIRTUAL_PREFIX) => {
            Some(DropEffect::Join)
        }
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
    pub(crate) drop_preview: &'a Option<(DropEffect, Option<CardId>)>,
    /// Pick-target mode's valid targets (`App::valid_targets`), each ringed; empty outside the mode.
    pub(crate) pick_targets: &'a [CardId],
    /// During a connect-drag, the card under the pointer (`App::connect_drag_end`): its target ring is drawn bright,
    /// as a hovered one is in pick-target mode.
    pub(crate) connect_target: Option<CardId>,
    /// While a link's end is dragged (`App::retarget_preview`), the ring colour for the outcome of releasing on
    /// `connect_target`, which is then ringed even where it is no valid target (refused, its own card), so a ring
    /// never promises a retarget the release will not do. `None` otherwise: valid targets keep the accent ring.
    pub(crate) connect_ring: Option<egui::Color32>,
    /// Keyboard navigation is on (a card or control had focus last frame): each card shows its
    /// Tab-order number.
    pub(crate) show_order: bool,
    /// The level's card ids in Tab order (creation order, `App::card_tab_order`). Only the first is
    /// a Tab stop from outside the graph; `card_tab_step` moves between the rest.
    pub(crate) order: &'a [CardId],
    /// A card or one of its controls has keyboard focus this frame.
    pub(crate) any_focused: bool,
    /// Every group's members (`App::group_members`), built once before the pass.
    pub(crate) members: &'a HashMap<NodeId, Vec<NodeId>>,
    /// The world rect a card must meet to be drawn (the camera's, in Graph view).
    pub(crate) viewport: egui::Rect,
    /// Set to the rect of a card that gained keyboard focus this frame; the caller pans its camera to it.
    pub(crate) pan_target: Option<egui::Rect>,
    /// Whether a virtual card that may be a drag source is carried by a press-drag on it (the graph); off in the trail
    /// popup, where a drag pans.
    pub(crate) carry: bool,
}

/// An outline on a keyboard-focused card or control, white so it reads differently from the gold
/// border of the active tab. Outside the rect, in world units (`inv_zoom` keeps it a constant size).
pub(crate) fn card_focus_ring(ui: &egui::Ui, rect: egui::Rect, inv_zoom: f32) {
    ui.painter().rect_stroke(
        rect.expand(3.0 * inv_zoom),
        3.0 * inv_zoom,
        egui::Stroke::new(2.0 * inv_zoom, palette(ui.visuals()).text),
        egui::StrokeKind::Outside,
    );
}

/// One of a card's `card_controls`, hit-tested and drawn at its corner.
/// Returns its response: the frame turns a click into
/// `control.action()` and counts its hover as the card's. Icons are
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
    card: CardId,
    view: &CardView,
    style: &CardStyle,
    inv_zoom: f32,
    detail: crate::graph_ui::Detail,
) -> egui::Response {
    let rect = control.rect(card_rect, inv_zoom);
    let response = ui
        // Not focusable: Tab moves between cards only, and the keyboard reaches these actions
        // through Delete and the node menu.
        .interact(rect, egui::Id::new(("graph_card_control", control, card)), egui::Sense::CLICK)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    match control {
        // A simple card is a few pixels across: a plain square says the same as a tessellated circle.
        CardControl::Swatch if !detail.content() => {
            let color = view.color.map_or(style.border_idle, |c| tag_shown(c, style.dark));
            ui.painter().rect_filled(
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(12.0 * inv_zoom)),
                0.0,
                color,
            );
        }
        CardControl::Swatch => {
            match view.color {
                Some(color) => {
                    ui.painter().circle_filled(rect.center(), 6.0 * inv_zoom, tag_shown(color, style.dark));
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
            let pin_color = if view.pinned { accent(ui.visuals()) } else { ui.visuals().weak_text_color() };
            if detail.content() {
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
                if detail.content() {
                    push_centered_text(
                        deferred_overlays,
                        rect.center(),
                        egui_phosphor::light::SPEAKER_X,
                        12.0,
                        danger(ui.visuals()),
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
                    ui.painter().rect_filled(bar_rect, 1.0 * inv_zoom, palette(ui.visuals()).success);
                }
            }
        }
        CardControl::Eject => {
            paint_control_square(
                ui,
                deferred_overlays,
                rect,
                response.hovered(),
                accent(ui.visuals()),
                egui_phosphor::light::SIGN_OUT,
                style,
                inv_zoom,
                detail,
            );
        }
        CardControl::Close => {
            paint_control_square(
                ui,
                deferred_overlays,
                rect,
                response.hovered(),
                palette(ui.visuals()).danger_fill,
                egui_phosphor::light::X,
                style,
                inv_zoom,
                detail,
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
    detail: crate::graph_ui::Detail,
) {
    let rounding = if detail.content() { 2.0 * inv_zoom } else { 0.0 };
    ui.painter().rect_filled(rect, rounding, if hovered { hover_fill } else { style.close_fill });
    ui.painter().rect_stroke(
        rect,
        rounding,
        egui::Stroke::new(1.0 * inv_zoom, style.close_border),
        egui::StrokeKind::Inside,
    );
    if detail.content() {
        push_centered_text(deferred_overlays, rect.center(), icon, 14.0, style.close_icon);
    }
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
        Runtime::Jar => JAR_NODE_SIZE,
        // A placeholder has no real size: the same box a failed PDF gets.
        // No size yet (still decoding, `ImagePayload::decoding`) or a placeholder: the box a pending PDF gets.
        Runtime::Image(image) if image.error.is_some() || image.decoding() => PDF_PENDING_NODE_SIZE,
        Runtime::Image(image) => image_node_card_size(image.width, image.height),
        Runtime::Pdf(pdf) => pdf_card_size(pdf),
        // A file's own card previews its root (spec "The card at the
        // parent level"); an error card keeps the small box.
        Runtime::Json(json) if json.error.is_none() => crate::json_view::preview_size(&json.value),
        Runtime::Json(_) => JSON_NODE_SIZE,
        // A group has no runtime of its own.
        Runtime::None => GROUP_NODE_SIZE,
        Runtime::Unknown => UNKNOWN_NODE_SIZE,
    }
}

/// A card in world space: `node_card_size` centered on `pos`, its layout position (`App::card_rect`).
pub(crate) fn node_card_rect(pos: egui::Vec2, runtime: &Runtime) -> egui::Rect {
    egui::Rect::from_center_size(pos.to_pos2(), node_card_size(runtime))
}

/// What the Graph filter box matches a card against: a tab's title and
/// URL, a download's full path, a note's text, a file node's filename, a group's or jar's name (`text`,
/// `App::own_text`, for a note or a group). A card of a type this build doesn't know has nothing of its own: its
/// stored name is pushed beside it by the caller.
pub(crate) fn card_filter_text<'a>(runtime: &'a Runtime, text: &'a str) -> Vec<&'a str> {
    match runtime {
        Runtime::Tab(tab) => vec![&tab.title, &tab.url],
        Runtime::Unloaded(tab) => vec![&tab.title, &tab.url],
        Runtime::Download(download) => vec![&download.path],
        Runtime::Note(_) => vec![text],
        // The file kinds (`Caps::file`).
        Runtime::Image(_) | Runtime::Pdf(_) | Runtime::Json(_) => {
            runtime.file_path().map(download_filename).into_iter().collect()
        }
        // A group has no runtime of its own. it matches on its name, `text`, as a jar does.
        Runtime::None | Runtime::Jar => vec![text],
        Runtime::Unknown => Vec::new(),
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
fn missing_view_phrases(state: &crate::view_install::ViewStatus) -> (&'static str, &'static str) {
    match state {
        crate::view_install::ViewStatus::UpdateNeeded { .. } => ("needs an update", "needs an update"),
        _ => ("is not installed", "not installed"),
    }
}

impl App {
    /// Ends every note edit (`end_note_edit`), saving if one was on. A note's TextEdit that
    /// stops being laid out (its level left, the view changed) loses focus
    /// without `lost_focus()` ever firing, so `editing` would stay on.
    pub(crate) fn end_note_edits(&mut self) -> bool {
        let editing: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.as_note().is_some_and(|note| note.editing))
            .map(|(id, _)| *id)
            .collect();
        for id in &editing {
            self.end_note_edit(*id);
        }
        if !editing.is_empty() {
            self.save_session();
        }
        !editing.is_empty()
    }

    /// Every group's members, in `node_order`: built once per card pass.
    pub(crate) fn group_members(&self) -> HashMap<NodeId, Vec<NodeId>> {
        let mut members: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for id in &self.node_order {
            if let Some(group) = self.nodes.get(id).and(self.doc.mirror.group(*id)) {
                members.entry(group).or_default().push(*id);
            }
        }
        members
    }

    /// `entry`'s `CardView`, or `None` when its node is gone (a JSON level's card is drawn elsewhere). A virtual
    /// card has no node: no colour, group, pin, audio or ✕, and it can be selected but never focused. `members` is
    /// `group_members`, built once for the whole pass.
    pub(crate) fn card_view(&self, entry: &CardEntry, members: &HashMap<NodeId, Vec<NodeId>>) -> Option<CardView> {
        let card = entry.id;
        match entry.origin {
            Origin::Stored => {}
            Origin::Virtual(_) => {
                return Some(CardView {
                    kind: entry.kind,
                    color: None,
                    in_group: false,
                    overloaded: false,
                    members: Vec::new(),
                    focused: false,
                    pinned: false,
                    muted: false,
                    audio_playing: false,
                    selected: self.interaction.selected.contains(&card),
                    closable: false,
                });
            }
            Origin::Json => return None,
        }
        let id = card.node()?;
        let node = self.nodes.get(&id)?;
        let kind = self.card_kind(id)?;
        let members = if is_container(kind.ty) { members.get(&id).cloned().unwrap_or_default() } else { Vec::new() };
        let tab = node.as_tab();
        Some(CardView {
            kind,
            color: self.doc.mirror.color(id),
            in_group: self.doc.mirror.group(id).is_some(),
            overloaded: members.len() > GROUP_OVERLOAD_THRESHOLD,
            members,
            focused: self.focused == Some(id),
            pinned: self.doc.mirror.pinned(id),
            muted: tab.is_some_and(|t| t.muted),
            audio_playing: tab.is_some_and(|t| t.audio_playing),
            selected: self.interaction.selected.contains(&card),
            closable: !self.is_default_jar(id),
        })
    }

    /// One card of any kind on the graph: everything the kinds share
    /// (cull, click/drag and hover interacts, border, controls, selection
    /// wash, drag and drop through `drop_target`, filter dim), with the
    /// per-kind content painted by `paint_card_body`, plus the frame's own
    /// background, glow and eased hover. `None` when the node is gone or
    /// culled. The caller applies the kind's own click action from the
    /// returned response. A virtual `entry` has no node: what only a node
    /// has (drag, link handles, menu key, filter dim) is skipped for it.
    pub(crate) fn draw_graph_card(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        entry: &CardEntry,
    ) -> Option<CardOutput> {
        let out = self.draw_card_inside_overlay_scope(ui, pass, entry);
        pass.deferred_overlays.end_card();
        out
    }

    /// `draw_graph_card`'s body: it begins the card's overlay scope (`DeferredOverlays::begin_card`), and the caller
    /// ends it on every path.
    fn draw_card_inside_overlay_scope(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        entry: &CardEntry,
    ) -> Option<CardOutput> {
        let card = entry.id;
        let node = card.node();
        let stored = entry.origin == Origin::Stored;
        let inv_zoom = pass.inv_zoom;
        // A stored card's rect is read fresh: a drag earlier in this frame's loop may have moved it.
        let rect = match node.filter(|_| stored) {
            Some(id) => self.card_rect(id)?,
            None => entry.rect,
        };
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
        let keeps_keyboard = pass.order.first() == Some(&card)
            || ui.memory(|m| m.has_focus(card.widget()))
            || ui.memory(|m| m.has_focus(card.editor()));
        if self.interaction.dragging != Some(card) && !keeps_keyboard && !rect.intersects(pass.viewport) {
            return None;
        }
        let view = self.card_view(entry, pass.members)?;
        // Glow and fill go under the body, but hover is only known after
        // the interacts, which must follow the body (below). This slot is
        // filled in once it is.
        let background = ui.painter().add(egui::Shape::Noop);
        pass.deferred_overlays.begin_card(rect);
        let body = self.paint_card_body(ui, pass, entry, &view, rect);
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
        let card_id = card.widget();
        // The Tab entry is first in the order; the position (a scan) is only needed for the order badges.
        let is_tab_entry = pass.order.first() == Some(&card);
        // A card takes a drag (and so keeps the scene from panning under it) only if its kind may be picked up: a
        // stored card is moved by it, a virtual one carried (handled below).
        let draggable = entry.caps().drag_source && (stored || pass.carry);
        let focusable = is_tab_entry || ui.memory(|m| m.has_focus(card_id));
        let sense = match (draggable, focusable) {
            (false, true) => egui::Sense::click(),
            (false, false) => egui::Sense::CLICK,
            (true, true) => egui::Sense::click_and_drag(),
            (true, false) => egui::Sense::CLICK | egui::Sense::DRAG,
        };
        // Space on a focused card toggles it in the selection, as the marquee would select it. Taken
        // before the interact, which would otherwise read it as a click and open the tab. The key is consumed for
        // every card; only a `selectable` kind toggles.
        if ui.memory(|m| m.has_focus(card_id))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space))
            && entry.caps().selectable
        {
            self.toggle_selected(card);
        }
        let response = ui.interact(body.interact_rect, card_id, sense);
        // Not while a card's menu is open: the pointer is usually still on
        // the card it right-clicked, and the tooltip would cover the menu.
        let response = match body.tooltip {
            Some(tooltip) if self.menus.node_menu.open.is_none() && self.menus.trail_menu.is_none() => {
                response.on_hover_text(tooltip)
            }
            _ => response,
        };
        // Screen readers: the card is a button named by its kind and title (egui announces it on focus).
        if let Some(id) = node.filter(|id| self.nodes.contains_key(id)) {
            response.widget_info(|| {
                let name = self.accessible_name(id).unwrap_or_default();
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name)
            });
        }
        // Keyboard: a focus ring, the camera follows focus, and the Tab-order number while navigating.
        pass.any_focused |= response.has_focus();
        if response.has_focus() {
            card_focus_ring(ui, rect, inv_zoom);
        }
        // The focused card's hint wins over a hovered one's, whichever draws first.
        if response.has_focus() || (response.hovered() && self.card_hint.is_none()) {
            let to_screen = ui.ctx().layer_transform_to_global(ui.layer_id());
            self.card_hint = Some((view.kind, to_screen.map_or(rect, |t| t * rect)));
        }
        if response.gained_focus() {
            pass.pan_target = Some(rect);
        }
        if pass.show_order {
            let number = pass.order.iter().position(|o| *o == card).map_or(0, |s| s + 1);
            let corner = rect.left_top();
            let (text, panel) = (palette(ui.visuals()).text, ui.visuals().panel_fill);
            deferred_overlays.push_top(Box::new(move |painter, project| {
                // Kept on screen: a card at the edge would clip its badge.
                let center = project(corner);
                let center = center.clamp(painter.clip_rect().shrink(10.0).min, painter.clip_rect().shrink(10.0).max);
                // A keycap: the number is the key that jumps to this card.
                let galley = painter.layout_no_wrap(number.to_string(), egui::FontId::monospace(11.0), text);
                let cap = egui::Rect::from_center_size(center, galley.size() + egui::vec2(8.0, 4.0));
                painter.rect_filled(cap, 3.0, panel);
                painter.rect_stroke(cap, 3.0, egui::Stroke::new(1.0, text), egui::StrokeKind::Inside);
                painter.galley(cap.center() - galley.size() / 2.0, galley, text);
            }));
        }
        let hover = ui.interact(rect, egui::Id::new(("graph_card_hover", card)), egui::Sense::hover());
        let pointer_on_card = hover.hovered() || response.hovered();
        // Last frame's hover too, which keeps the ✕ drawn while the
        // pointer is on it: registered last, it takes the hover from both
        // interacts above.
        let is_hovered = self.interaction.hovered == Some(card) || pointer_on_card;

        // One eased hover drives the glow, the fill's brightening and the
        // border, so the three move together.
        let hover_t = ui.ctx().animate_bool_responsive(egui::Id::new(("graph_card_hover_t", card)), is_hovered);
        let style = card_style(view.kind, ui.visuals());
        let (border_width, border_color) = card_border(&view, &style, hover_t);
        // A soft black shadow at rest that takes on the border color as the card is hovered or open.
        let shadow_color = lerp_color32(egui::Color32::BLACK, border_color, if view.focused { 1.0 } else { hover_t });
        let mut background_shapes: Vec<egui::Shape> = card_glow_shapes(
            rect,
            CARD_RADIUS,
            shadow_color,
            inv_zoom,
            1.0 + hover_t * 0.8,
            palette(ui.visuals()).shadow,
            3.0,
        )
        .into_iter()
        .take(self.graph_detail.glow_layers(hover_t > 0.0))
        .collect();
        background_shapes.push(egui::Shape::rect_filled(
            rect,
            CARD_RADIUS * inv_zoom,
            style.fill.gamma_multiply(1.0 + hover_t * 0.35),
        ));
        // Rectangle-select's highlight: a translucent ACCENT wash, distinct
        // from the focus border (an outline only), so a selected tab still
        // reads differently from the focused one when both apply. In the
        // background slot, so it tints the card but not its thumbnail.
        if view.selected {
            background_shapes.push(egui::Shape::rect_filled(
                rect,
                CARD_RADIUS * inv_zoom,
                with_alpha(accent(ui.visuals()), 45),
            ));
        }
        ui.painter().set(background, egui::Shape::Vec(background_shapes));
        // An inside stroke painted over the body; every body keeps its
        // content inset from the edge.
        ui.painter().rect_stroke(
            rect,
            CARD_RADIUS * inv_zoom,
            egui::Stroke::new(border_width * inv_zoom, border_color),
            egui::StrokeKind::Inside,
        );

        // The card the dragged one would drop onto: a bold ring, expanded
        // well past this card so it still shows around a larger dragged
        // card on top of it. Tracks the pointer directly, not eased.
        if pass.drop_preview.as_ref().and_then(|(_, target)| *target) == Some(card) {
            ui.painter().rect_stroke(
                rect.expand(16.0 * inv_zoom),
                10.0 * inv_zoom,
                egui::Stroke::new(4.0 * inv_zoom, accent(ui.visuals())),
                egui::StrokeKind::Outside,
            );
        }
        // Pick-target mode: a valid target gets a thinner accent ring, closer in than the drop ring, and brighter
        // while hovered, where a click would connect.
        let reached = pass.connect_target == Some(card);
        if pass.pick_targets.contains(&card) || (reached && pass.connect_ring.is_some()) {
            let alpha = if pointer_on_card || reached { 255 } else { 160 };
            let ring = pass.connect_ring.filter(|_| reached).unwrap_or(accent(ui.visuals()));
            ui.painter().rect_stroke(
                rect.expand(6.0 * inv_zoom),
                6.0 * inv_zoom,
                egui::Stroke::new(2.5 * inv_zoom, with_alpha(ring, alpha)),
                egui::StrokeKind::Outside,
            );
        }
        // Registered after both interacts above, so they take the pointer.
        let caps = entry.caps();
        let delete = caps.stored_controls && response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Delete));
        // Shift+F10 on a focused card opens its menu, anchored at the card's bottom-left: a stored card's node menu,
        // or whatever menu a virtual card's source opens.
        if caps.menu && response.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F10))
        {
            let corner = rect.left_bottom();
            let at = ui.ctx().layer_transform_to_global(ui.layer_id()).map_or(corner, |t| t * corner);
            match entry.origin {
                Origin::Virtual(name) => {
                    if let Some(source) = self.virtual_source(name) {
                        source.on_menu(self, entry, at);
                    }
                }
                _ => {
                    if let Some(id) = node {
                        self.menus.node_menu.open(id, at);
                    }
                }
            }
        }
        let mut control = ((caps.stored_controls && response.clicked_by(egui::PointerButton::Middle)) || delete)
            .then(|| CardControl::Close.action());
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
                card,
                &view,
                &style,
                inv_zoom,
                self.graph_detail,
            );
            let control_response = control_response.on_hover_text(card_control.tip(&view));
            control_hovered |= control_response.hovered();
            // A right-click on the swatch opens the menu (the right-press already did) on its Color page.
            if card_control == CardControl::Swatch
                && control_response.secondary_clicked()
                && self.menus.node_menu.open.as_ref().is_some_and(|(menu_id, _)| Some(*menu_id) == node)
            {
                self.menus.node_menu.show_page(Some(crate::node_menu::MenuPage::Color));
                ui.ctx().request_repaint();
            }
            if control_response.clicked() && control.is_none() {
                control = Some(card_control.action());
            }
        }

        // Connect-drag handles (link-gestures spec A), on a hovered card, and on every card while a drag runs.
        // Registered after the card and its controls: egui gives a press on a drag-only widget on top to that widget
        // alone (no click below), so the card neither moves nor opens. Stored cards only, not an ability: a handle
        // starts a Link node between two nodes.
        let mut handle_hovered = false;
        if let Some(id) =
            node.filter(|_| stored && (is_hovered || self.link.connect_drag.is_some()) && self.shows_link_handles())
        {
            let handles = crate::link_gestures::link_handle_rects(rect, pass.zoom_scale, self.graph_detail);
            for (side, handle) in handles {
                let handle_response = ui
                    .interact(handle, egui::Id::new(("link_handle", side, card)), egui::Sense::DRAG)
                    .on_hover_cursor(egui::CursorIcon::Crosshair)
                    .on_hover_text("Drag to another card to link them");
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
        // A virtual card that is a `drag_source` is carried, never moved: its rect and the layout stay, a ghost
        // follows the pointer, and the release goes to its source (`VirtualSource::on_drop`).
        let carried = !stored && entry.caps().drag_source && pass.carry;
        if carried && response.dragged() {
            self.interaction.dragging = Some(card);
            self.interaction.drag_moved = Vec::new();
            let to_local = ui.ctx().layer_transform_from_global(ui.layer_id());
            let (press, now) = (ui.input(|i| i.pointer.press_origin()), response.interact_pointer_pos());
            let shift = match (press, now) {
                (Some(press), Some(now)) => now - to_local.map_or(press, |t| t * press),
                _ => egui::Vec2::ZERO,
            };
            let (ghost, fill, ring) = (rect.translate(shift), style.fill, accent(ui.visuals()));
            deferred_overlays.push_top(Box::new(move |painter, project| {
                let screen = egui::Rect::from_min_max(project(ghost.min), project(ghost.max));
                painter.rect_filled(screen, 2.0, fill.gamma_multiply(0.8));
                painter.rect_stroke(screen, 2.0, egui::Stroke::new(2.0, ring), egui::StrokeKind::Inside);
            }));
        } else if carried && self.interaction.dragging == Some(card) {
            self.release_drag();
            let target = *pass.drop_preview;
            // Where the pointer let go, in world space: the ghost's transform, applied to the latest pointer position.
            let to_local = ui.ctx().layer_transform_from_global(ui.layer_id());
            let at = ui.input(|i| i.pointer.interact_pos()).map_or(rect.center(), |p| to_local.map_or(p, |t| t * p));
            if let Origin::Virtual(name) = entry.origin {
                if let Some(source) = self.virtual_source(name) {
                    pass.actions.extend(source.on_drop(self, entry, target, at));
                }
            }
        }
        if (stored || carried) && response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if stored && response.dragged() {
            self.interaction.dragging = Some(card);
            // Dragging one selected tab moves the whole selection, as one
            // unit (see `take_drag_unit`). Recomputed (and overwritten)
            // every dragged frame rather than only at drag start: cheap,
            // and it's always exactly this frame's moved set, so whatever
            // `drag_moved` holds when the drag ends is the full, correct
            // set to save -- see the release branch below.
            let delta = response.drag_delta();
            let moved: Vec<CardId> =
                if view.selected { self.interaction.selected.iter().copied().collect() } else { vec![card] };
            // The layout only: the document pos is written once, at release (`release_drag`).
            for moved_id in moved.iter().filter_map(|moved| moved.node()) {
                if self.nodes.contains_key(&moved_id) {
                    self.layout.entry(moved_id).or_default().pos += delta;
                }
            }
            self.interaction.drag_moved = moved;
        } else if stored && self.interaction.dragging == Some(card) {
            self.release_drag();
            // `drop_preview` was computed this frame for this card, and
            // nothing moved it since (no drag delta on a release frame).
            pass.actions.push(GraphAction::Drop { id: card, target: *pass.drop_preview });
        }
        if still_hovered {
            self.interaction.hovered = Some(card);
        } else if self.interaction.hovered == Some(card) {
            self.interaction.hovered = None;
        }

        let scrim = with_alpha(palette(ui.visuals()).canvas, FILTER_DIM_ALPHA);
        if let Some((id, node)) = node.and_then(|id| self.nodes.get(&id).map(|n| (id, n))) {
            let mut needles = card_filter_text(&node.runtime, self.own_text(id));
            needles.push(self.doc.mirror.name(id)); // name rule: the filter matches the stored text (a rename; a group is matched twice)
            dim_if_filtered_out(deferred_overlays, &self.graph_filter, &needles, rect, CARD_RADIUS / inv_zoom, scrim);
        } else if self.virtual_card_dimmed(entry) {
            push_dim_scrim(deferred_overlays, rect, CARD_RADIUS / inv_zoom, scrim);
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
        entry: &CardEntry,
        view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        // A virtual card's source paints it (the entry's own rect: `rect` is the same for it).
        if let Origin::Virtual(name) = entry.origin {
            return match self.virtual_source(name) {
                Some(source) => source.paint_body(self, ui, pass, entry),
                None => CardBody { interact_rect: rect, tooltip: None },
            };
        }
        let Some(id) = entry.id.node() else {
            return CardBody { interact_rect: rect, tooltip: None };
        };
        match view.kind.ty {
            TYPE_DOWNLOAD => self.paint_download_body(ui, pass, id, view, rect),
            TYPE_NOTE => self.paint_note_body(ui, pass, id, view, rect),
            TYPE_JAR => self.paint_jar_body(ui, pass, id, view, rect),
            TYPE_IMAGE => self.paint_image_body(ui, pass, id, view, rect),
            TYPE_PDF => self.paint_pdf_body(ui, pass, id, view, rect),
            TYPE_JSON => self.paint_json_body(ui, pass, id, view, rect),
            // Presentation matches the type, not `is_container` (see `card_style`).
            TYPE_GROUP => self.paint_group_body(ui, pass, id, view, rect),
            TYPE_TAB if view.kind.loaded() => self.paint_tab_body(ui, pass, id, view, rect),
            TYPE_TAB => self.paint_unloaded_tab_body(ui, pass, id, view, rect),
            // `TYPE_UNKNOWN` (`card_kind::canonical_type`): every other row is matched above.
            _ => self.paint_unknown_body(ui, pass, id, view, rect),
        }
    }

    /// A jar card (spec "The jar card"): a cookie glyph in the jar's colour, its name through the display rule, and
    /// "Ephemeral" when it keeps no file. No body.
    pub(crate) fn paint_jar_body(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        id: NodeId,
        _view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
        let body = CardBody { interact_rect: rect, tooltip: None };
        if !self.graph_detail.content() {
            return body;
        }
        let def = self.doc.mirror.jar_def(id);
        let segs = crate::names::display_name(&def.name, crate::names::UNTITLED);
        let header = CardHeader {
            identity: Identity::Glyph {
                glyph: egui_phosphor::light::COOKIE,
                color: Some(def.color.unwrap_or(crate::theme::TAG_COLOR_PALETTE[0])),
            },
            name: "Jar".to_owned(),
            user_name: Some(&segs),
            source: if def.ephemeral { "Ephemeral".to_owned() } else { String::new() },
            source_is_path: false,
            scrim: false,
            right_margin: 0.0,
        };
        self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
        body
    }

    /// A card of a type this build doesn't know (spec 2026-10-14 decision 9): a question-mark glyph and its type
    /// string in the header. A stored name takes the title's place (`paint_card_header`) and the type moves under it.
    /// No body.
    pub(crate) fn paint_unknown_body(
        &mut self,
        ui: &mut egui::Ui,
        pass: &mut CardPass,
        id: NodeId,
        _view: &CardView,
        rect: egui::Rect,
    ) -> CardBody {
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
        let body = CardBody { interact_rect: rect, tooltip: None };
        if !self.graph_detail.content() {
            return body;
        }
        let ty = self.doc.mirror.ty(id).to_owned();
        let named = !self.doc.mirror.name(id).is_empty(); // name rule: only whether one is stored
        let header = CardHeader {
            identity: Identity::Glyph { glyph: egui_phosphor::light::QUESTION, color: None },
            source: if named { ty.clone() } else { String::new() },
            name: ty,
            user_name: None,
            source_is_path: false,
            scrim: false,
            right_margin: 0.0,
        };
        self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
        body
    }
}

/// Graph toolbar filter box: `true` when the node should be painted
/// dimmed -- exactly the negation of `filter_matches`, kept as its own
/// named function since "dims" (not "doesn't match") is the meaning every
/// call site actually cares about.
pub(crate) fn graph_filter_dims(filter: &str, haystacks: &[&str]) -> bool {
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
    shadow: u8,
    offset_y: f32,
) -> [egui::Shape; 2] {
    [(0.6_f32, 4.0_f32), (0.25, 12.0)].map(|(share, grow)| {
        let alpha = ((shadow as f32) * share * intensity).min(255.0) as u8;
        egui::Shape::rect_filled(
            rect.translate(egui::vec2(0.0, offset_y * inv_zoom)).expand(grow * inv_zoom),
            (base_corner_radius + grow) * inv_zoom,
            with_alpha(color, alpha),
        )
    })
}

#[cfg(test)]
mod tests;
