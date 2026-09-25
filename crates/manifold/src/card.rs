//! The shared graph-card frame: card sizes and styles, the corner controls,
//! drop rules and the per-card view/output types `draw_graph_card` works
//! with.

use super::*;

pub(crate) const GRAPH_NODE_SIZE: egui::Vec2 = egui::vec2(160.0, 110.0);

/// Smaller than `GRAPH_NODE_SIZE` — a download node has no live thumbnail,
/// just a filename, so it doesn't need the same footprint a tab's preview
/// does.
pub(crate) const DOWNLOAD_NODE_SIZE: egui::Vec2 = egui::vec2(130.0, 60.0);

/// A group card's footprint — same idea as `DOWNLOAD_NODE_SIZE` (no live
/// thumbnail, just a name/member-count), sized a bit larger since it's a
/// container rather than a single file. Also the drop-target rect a
/// dragged tab is tested against on release — see `GroupPayload`'s own doc
/// comment.
pub(crate) const GROUP_NODE_SIZE: egui::Vec2 = egui::vec2(150.0, 90.0);

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

/// A JSON node's card size -- always this fixed box, never derived from
/// content the way `Image`/`Pdf`'s own sizing is: there's no natural aspect
/// ratio for a JSON document, and the card only ever shows a compact shape
/// summary (double-click opens the real content in `build_json_viewer`).
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
/// card's shared frame (`App::draw_graph_card`) paints with. Note is the
/// one kind on a light (yellow) card, so its border and ✕ are dark
/// instead of the theme's text colors.
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
        ItemNodeKind::Note => CardStyle {
            fill: NOTE_CARD_COLOR,
            border_hover: egui::Color32::from_black_alpha(200),
            border_idle: egui::Color32::from_black_alpha(120),
            close_fill: NOTE_CARD_COLOR,
            close_border: egui::Color32::from_black_alpha(160),
            close_icon: egui::Color32::from_black_alpha(200),
            dim_rounding: 6.0,
        },
        ItemNodeKind::Download | ItemNodeKind::Tab | ItemNodeKind::FailedTab => CardStyle { dim_rounding: 6.0, ..themed },
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
/// `CardControl`): the swatch on every card, a tab's pin, and its audio
/// indicator while playing or muted, eject on a grouped one, and the ✕
/// last, so it wins over anything under it.
pub(crate) fn card_controls(view: &CardView) -> Vec<CardControl> {
    let mut controls = vec![CardControl::Swatch];
    if view.kind == ItemNodeKind::Tab {
        controls.push(CardControl::Pin);
        // Also while muted after audio stops, so a mute can always be
        // seen and undone from Graph view.
        if view.audio_playing || view.muted {
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
    use ItemNodeKind::{Group, Tab};
    match (dragged, target, inside_group) {
        (_, DropTarget::BackButton, true) => Some(DropEffect::Eject),
        (Group, DropTarget::Card(Group), _) => Some(DropEffect::Merge),
        (_, DropTarget::Card(Group), _) => Some(DropEffect::Join),
        (Tab, DropTarget::Card(Tab), false) => Some(DropEffect::GroupWith),
        (Tab, DropTarget::Card(Tab), true) => Some(DropEffect::Reparent),
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
        .interact(rect, egui::Id::new(("graph_card_control", control, id)), egui::Sense::click())
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
                    ui.painter().circle_stroke(rect.center(), 6.0 * inv_zoom, egui::Stroke::new(1.5 * inv_zoom, style.border_idle));
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
        }
        CardControl::Eject => {
            paint_control_square(ui, deferred_overlays, rect, response.hovered(), ACCENT, egui_phosphor::light::SIGN_OUT, style, inv_zoom);
        }
        CardControl::Close => {
            paint_control_square(ui, deferred_overlays, rect, response.hovered(), DANGER_COLOR, egui_phosphor::light::X, style, inv_zoom);
        }
    }
    response
}

/// The ✕ and eject buttons' look: a small square in the card's close
/// colors (`card_style`, so it matches a Note's dark-on-yellow palette),
/// filled `hover_fill` while hovered.
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
        overlay_painter.text(project(icon_pos), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(14.0), icon_color);
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
        NodePayload::Tab(_) | NodePayload::FailedTab(_) => GRAPH_NODE_SIZE,
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
pub(crate) fn node_card_rect(node: &GraphNode) -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(node.pos.x, node.pos.y), node_card_size(&node.payload))
}

/// What the Graph filter box matches a card against: a tab's title and
/// URL, a download's full path, a note's text, a file node's filename, a
/// group's name.
pub(crate) fn card_filter_text(payload: &NodePayload) -> Vec<&str> {
    match payload {
        NodePayload::Tab(tab) => vec![&tab.title, &tab.url],
        NodePayload::FailedTab(tab) => vec![&tab.url],
        NodePayload::Download(download) => vec![&download.path],
        NodePayload::Note(note) => vec![&note.text],
        NodePayload::Image(image) => vec![download_filename(&image.path)],
        NodePayload::Pdf(pdf) => vec![download_filename(&pdf.path)],
        NodePayload::Json(json) => vec![download_filename(&json.path)],
        NodePayload::Group(group) => vec![&group.name],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn card_controls_list_each_cards_buttons_in_pointer_order() {
        use CardControl::*;
        for kind in GRAPH_DRAW_ORDER.into_iter().filter(|&k| k != ItemNodeKind::Tab) {
            assert_eq!(card_controls(&test_view(kind)), vec![Swatch, Close], "{kind:?}");
            let grouped = CardView { in_group: true, ..test_view(kind) };
            assert_eq!(card_controls(&grouped), vec![Swatch, Eject, Close], "{kind:?}");
            let close = if kind == ItemNodeKind::Group { NodeAction::Dissolve } else { NodeAction::Delete };
            assert_eq!(Close.action(kind), close, "{kind:?}");
        }
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
        assert_eq!(tab(|v| { v.muted = true; v.in_group = true; }), vec![Swatch, Pin, Audio, Eject, Close]);
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
        assert_eq!(drop_action(K::Tab, card(K::Tab), false), Some(DropEffect::GroupWith));
        assert_eq!(drop_action(K::Tab, card(K::Tab), true), Some(DropEffect::Reparent));
        // Only a tab drops onto a tab, and nothing drops onto the file kinds.
        assert_eq!(drop_action(K::Note, card(K::Tab), false), None);
        assert_eq!(drop_action(K::Group, card(K::Tab), true), None);
        assert_eq!(drop_action(K::Tab, card(K::Note), false), None);
        assert_eq!(drop_action(K::Image, card(K::Json), true), None);
    }

    #[test]
    fn node_card_size_matches_each_kinds_card() {
        let download = NodePayload::Download(DownloadPayload { path: "f.bin".into(), bytes_received: 0, total_bytes: None, completed: false });
        assert_eq!(node_card_size(&download), DOWNLOAD_NODE_SIZE);
        assert_eq!(node_card_size(&NodePayload::Note(NotePayload { text: String::new(), editing: false })), NOTE_NODE_SIZE);
        assert_eq!(node_card_size(&NodePayload::Group(GroupPayload { name: "g".into() })), GROUP_NODE_SIZE);
        let json = NodePayload::Json(JsonPayload { path: "a.json".into(), value: serde_json::json!({}), key: None, error: None });
        assert_eq!(node_card_size(&json), JSON_NODE_SIZE);
        assert_eq!(node_card_size(&NodePayload::FailedTab(FailedTabPayload {
            url: "https://example.org/".into(),
            kind: EngineKind::Cef,
            error: "no binary".into(),
            pinned: true,
            zoom_factor: 1.5,
            retrying: false,
        })), GRAPH_NODE_SIZE);

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
        assert_eq!(card_filter_text(&download), ["C:/dl/f.bin"]);
        assert_eq!(card_filter_text(&NodePayload::Note(NotePayload { text: "buy milk".into(), editing: false })), ["buy milk"]);
        assert_eq!(card_filter_text(&NodePayload::Image(ImagePayload { path: "C:/pics/cat.png".into(), width: 1, height: 1, error: None })), ["cat.png"]);
        assert_eq!(card_filter_text(&NodePayload::Pdf(PdfPayload { path: "C:/docs/paper.pdf".into(), outcome: None })), ["paper.pdf"]);
        assert_eq!(card_filter_text(&NodePayload::Json(JsonPayload { path: "C:/data/x.json".into(), value: serde_json::json!(1), key: None, error: None })), ["x.json"]);
        assert_eq!(card_filter_text(&NodePayload::Group(GroupPayload { name: "Reading".into() })), ["Reading"]);
        assert_eq!(card_filter_text(&NodePayload::FailedTab(FailedTabPayload {
            url: "https://example.org/".into(),
            kind: EngineKind::Cef,
            error: "no binary".into(),
            pinned: true,
            zoom_factor: 1.5,
            retrying: false,
        })), ["https://example.org/"]);
    }
}
