//! Link gestures (spec `2026-10-07-link-gestures-design`): a click on a link line deletes that link, and a drag from a
//! card's edge handle links it to the card it is released on. Both act on `link` edges only; opened-from stays
//! menu-only.

use crate::card_id::CardId;
use crate::level::LevelEdge;
use crate::store::model::NodeId;
use crate::App;

/// A link line's stroke width in screen points (times `inv_zoom` in world units), and while the pointer is on it.
pub(crate) const LINK_STROKE_PX: f32 = 1.5;
pub(crate) const LINK_HOVER_STROKE_PX: f32 = 3.0;
/// How far from a link line, in screen points, a click still takes it.
pub(crate) const LINK_HIT_PX: f32 = 6.0;

/// How far from a card, in screen points, a connect-drag release still takes it (`App::card_within`).
pub(crate) const DROP_TOLERANCE_PX: f32 = 24.0;
/// How many straight pieces a bowed edge is flattened into for hit-testing and the dashed stroke.
pub(crate) const BOW_SEGMENTS: usize = 24;

/// The distance from `p` to the segment `a`-`b`; to `a` when the segment is a point.
pub(crate) fn distance_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The control point of the quadratic bezier every card-to-card edge is drawn as: the middle of `from`-`to` pushed
/// sideways (perpendicular, to the left of the direction of travel) by 15% of the distance, at most 30 world units, so
/// short edges between adjacent cards do not over-bow into a loop and an edge in any direction reads the same.
pub(crate) fn bow_control(from: egui::Pos2, to: egui::Pos2) -> egui::Pos2 {
    let delta = to - from;
    let dist = delta.length();
    let bow = (dist * 0.15).min(30.0);
    from + delta * 0.5 + egui::vec2(-delta.y, delta.x) / dist.max(0.0001) * bow
}

/// The point at `t` (0 to 1) along the bowed edge `from`-`to`.
pub(crate) fn bow_point(from: egui::Pos2, to: egui::Pos2, t: f32) -> egui::Pos2 {
    let control = bow_control(from, to);
    let u = 1.0 - t;
    (from.to_vec2() * (u * u) + control.to_vec2() * (2.0 * u * t) + to.to_vec2() * (t * t)).to_pos2()
}

/// The bowed edge as a polyline of `BOW_SEGMENTS + 1` points, ends included.
pub(crate) fn flatten_bow(from: egui::Pos2, to: egui::Pos2) -> Vec<egui::Pos2> {
    (0..=BOW_SEGMENTS).map(|i| bow_point(from, to, i as f32 / BOW_SEGMENTS as f32)).collect()
}

/// A box that contains the whole bowed edge (its end points and its control point: a bezier stays in their hull).
pub(crate) fn bow_bounds(from: egui::Pos2, to: egui::Pos2) -> egui::Rect {
    egui::Rect::from_points(&[from, bow_control(from, to), to])
}

/// The distance from `p` to the bowed edge `from`-`to`.
pub(crate) fn distance_to_bow(p: egui::Pos2, from: egui::Pos2, to: egui::Pos2) -> f32 {
    flatten_bow(from, to).windows(2).map(|w| distance_to_segment(p, w[0], w[1])).fold(f32::INFINITY, f32::min)
}

/// The one routine that draws a card-to-card edge: the bowed curve `from`-`to`, solid, or dashed (a link) with dashes
/// sized for `dash_inv_zoom` (6 on, 4 off, in screen points).
pub(crate) fn paint_bow(
    painter: &egui::Painter,
    from: egui::Pos2,
    to: egui::Pos2,
    stroke: egui::Stroke,
    dash_inv_zoom: Option<f32>,
) {
    if let Some(inv_zoom) = dash_inv_zoom {
        painter.add(egui::Shape::dashed_line(&flatten_bow(from, to), stroke, 6.0 * inv_zoom, 4.0 * inv_zoom));
    } else {
        painter.add(egui::epaint::QuadraticBezierShape::from_points_stroke(
            [from, bow_control(from, to), to],
            false,
            egui::Color32::TRANSPARENT,
            stroke,
        ));
    }
}

/// The sides of two cards that face each other: the dominant axis of the centre-to-centre delta `a` to `b` picks the
/// pair, right of `a` and left of `b` when `b` lies more to the right than below or above, and so on. A tie goes to
/// the horizontal pair. Depends on the two centres only, so it is stable while neither card moves.
pub(crate) fn facing_sides(a: egui::Pos2, b: egui::Pos2) -> (HandleSide, HandleSide) {
    let d = b - a;
    if d.x.abs() >= d.y.abs() {
        if d.x >= 0.0 {
            (HandleSide::Right, HandleSide::Left)
        } else {
            (HandleSide::Left, HandleSide::Right)
        }
    } else if d.y >= 0.0 {
        (HandleSide::Bottom, HandleSide::Top)
    } else {
        (HandleSide::Top, HandleSide::Bottom)
    }
}

/// The midpoint of `side` of `card`.
pub(crate) fn side_midpoint(card: egui::Rect, side: HandleSide) -> egui::Pos2 {
    match side {
        HandleSide::Top => card.center_top(),
        HandleSide::Right => card.right_center(),
        HandleSide::Bottom => card.center_bottom(),
        HandleSide::Left => card.left_center(),
    }
}

/// Where an edge between cards `a` and `b` runs: the facing side midpoints, `a`'s then `b`'s.
pub(crate) fn facing_ends(a: egui::Rect, b: egui::Rect) -> (egui::Pos2, egui::Pos2) {
    let (side_a, side_b) = facing_sides(a.center(), b.center());
    (side_midpoint(a, side_a), side_midpoint(b, side_b))
}

/// The distance from `p` to `rect`; 0 inside it.
pub(crate) fn distance_to_rect(p: egui::Pos2, rect: egui::Rect) -> f32 {
    let dx = (rect.min.x - p.x).max(0.0).max(p.x - rect.max.x);
    let dy = (rect.min.y - p.y).max(0.0).max(p.y - rect.max.y);
    dx.hypot(dy)
}

/// The hit tolerance in world units at `inv_zoom`: 6 screen points, or half the hovered line if that were wider.
pub(crate) fn link_hit_tolerance(inv_zoom: f32) -> f32 {
    LINK_HIT_PX.max(LINK_HOVER_STROKE_PX / 2.0) * inv_zoom
}

/// The link line nearest `at` (world) within `tolerance`, or none. Opened-from lines are never taken. Of equals the
/// last wins: `level_edges` lists links in id order, so on one pair that is the newest within a launch.
pub(crate) fn nearest_link(edges: &[LevelEdge], at: egui::Pos2, tolerance: f32) -> Option<&LevelEdge> {
    let mut best: Option<(f32, &LevelEdge)> = None;
    for edge in edges.iter().filter(|e| e.link.is_some()) {
        let d = distance_to_bow(at, edge.from, edge.to);
        if d <= tolerance && best.is_none_or(|(b, _)| d <= b) {
            best = Some((d, edge));
        }
    }
    best.map(|(_, edge)| edge)
}

/// A connect-drag that began on an existing link's curve (`App::retarget`): `link` is the Link node whose end follows
/// the pointer. `fixed_is_from` says which end stays on its card (the connect-drag's source), so a link keeps its
/// direction when its free end lands on another card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkRetarget {
    pub link: NodeId,
    pub fixed_is_from: bool,
}

/// Where a connect-drag ended (`App::connect_drag_end`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DragEnd {
    /// No real release over the canvas: the button's up was lost or it was a click, or the pointer was off the scene,
    /// on the minimap or under a popup.
    Abandoned,
    /// Released over the canvas with no card under it or within the drop tolerance.
    Canvas,
    /// Released on the drag's source card.
    Source,
    /// Released on this card (not the source).
    Card(NodeId),
}

/// How a dragged link end is drawn for the outcome a release there would have (`App::retarget_preview`): the rubber
/// band's colour and width in screen points, the mark it ends in (a deletion or a refusal, so colour is not the
/// only signal), and the colour of the ring on the card in reach, which never promises a retarget the release
/// will not do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RetargetLook {
    pub band: egui::Color32,
    pub band_px: f32,
    pub glyph: Option<RetargetGlyph>,
    pub ring: egui::Color32,
}

/// The small mark at the free end of a dragged link, so colour is not the only signal: `Cross` where the link would be
/// deleted, `Prohibited` (a circle with a diagonal slash) where the drop is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetargetGlyph {
    Cross,
    Prohibited,
}

/// `RetargetLook` for `outcome`: the usual accent for a retarget or a cancel, a thin red band ending in an "x" for a
/// deletion, a yellow for a refusal (`theme::refused_color`), and a weak-text ring where nothing would change.
pub(crate) fn retarget_look(outcome: crate::links::RetargetOutcome, visuals: &egui::Visuals) -> RetargetLook {
    use crate::links::RetargetOutcome::*;
    use crate::theme::{accent, danger, palette, refused_color};
    let accent = accent(visuals);
    let danger = danger(visuals);
    let usual = RetargetLook { band: accent, band_px: LINK_STROKE_PX, glyph: None, ring: accent };
    match outcome {
        Retargeted => usual,
        Cancelled => RetargetLook { ring: palette(visuals).text_weak, ..usual },
        Refused => RetargetLook {
            band: refused_color(visuals),
            glyph: Some(RetargetGlyph::Prohibited),
            ring: refused_color(visuals),
            ..usual
        },
        Deleted => RetargetLook {
            band: danger,
            band_px: LINK_STROKE_PX * 0.6,
            glyph: Some(RetargetGlyph::Cross),
            ring: danger,
        },
    }
}

/// Drops from `edges` (what is drawn) the link being retargeted: its rubber band stands for it.
pub(crate) fn without_retargeted(edges: &mut Vec<LevelEdge>, retarget: Option<&LinkRetarget>) {
    if let Some(retarget) = retarget {
        edges.retain(|edge| edge.link != Some(retarget.link));
    }
}

/// Whether the end of `edge` nearer the grab point `at` (world) is its `from` end; a tie goes to `from`.
pub(crate) fn grabs_from_end(edge: &LevelEdge, at: egui::Pos2) -> bool {
    at.distance(edge.from) <= at.distance(edge.to)
}

/// A side of a card, where a connect-drag handle sits (centred on that edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HandleSide {
    Top,
    Right,
    Bottom,
    Left,
}

impl HandleSide {
    pub(crate) const ALL: [HandleSide; 4] = [HandleSide::Top, HandleSide::Right, HandleSide::Bottom, HandleSide::Left];

    /// The key EXPORTSTATE's `link_handles` uses. Not `name`: `names::tests` reads every `.name` as a stored name.
    pub(crate) fn key(self) -> &'static str {
        match self {
            HandleSide::Top => "top",
            HandleSide::Right => "right",
            HandleSide::Bottom => "bottom",
            HandleSide::Left => "left",
        }
    }
}

/// A handle is a pill straddling its card edge: this long along the edge and `HANDLE_SHORT_PX` across it, in screen
/// points.
pub(crate) const HANDLE_LONG_PX: f32 = 28.0;
pub(crate) const HANDLE_SHORT_PX: f32 = 16.0;
/// The shortest on-screen card edge that shows its handle. A top or right handle clears the 16-point Audio control
/// in the top-right corner (`CardControl::rect`) only while half the edge is at least 16 points plus half the pill's
/// long side: the old 44 (a 12-point circle) is 2 * (16 + 6), this is 2 * (16 + 14).
pub(crate) const HANDLE_MIN_EDGE_PX: f32 = 2.0 * (16.0 + HANDLE_LONG_PX / 2.0);

/// Card `card`'s handles (world space) at `zoom_scale`, each a `HANDLE_LONG_PX` by `HANDLE_SHORT_PX` pill on screen
/// (long side along the edge), in `HandleSide::ALL` order. A side whose edge is under `HANDLE_MIN_EDGE_PX` on screen
/// has none, and a `Simple` detail level (`graph_detail`) has none at all.
pub(crate) fn link_handle_rects(
    card: egui::Rect,
    zoom_scale: f32,
    detail: crate::graph_ui::Detail,
) -> Vec<(HandleSide, egui::Rect)> {
    if !detail.link_handles() {
        return Vec::new();
    }
    let (long, short) = (HANDLE_LONG_PX / zoom_scale, HANDLE_SHORT_PX / zoom_scale);
    HandleSide::ALL
        .into_iter()
        .filter_map(|side| {
            let (edge, size) = match side {
                HandleSide::Top | HandleSide::Bottom => (card.width(), egui::vec2(long, short)),
                HandleSide::Right | HandleSide::Left => (card.height(), egui::vec2(short, long)),
            };
            (edge * zoom_scale >= HANDLE_MIN_EDGE_PX)
                .then(|| (side, egui::Rect::from_center_size(side_midpoint(card, side), size)))
        })
        .collect()
}

/// A handle: an accent-outlined pill, filled while hovered or dragged from.
pub(crate) fn paint_link_handle(ui: &egui::Ui, handle: egui::Rect, active: bool, inv_zoom: f32) {
    let accent = crate::theme::accent(ui.visuals());
    let fill = if active { accent } else { ui.visuals().panel_fill };
    let stroke = egui::Stroke::new(1.5 * inv_zoom, accent);
    let radius = handle.width().min(handle.height()) / 2.0;
    ui.painter().rect(handle, radius, fill, stroke, egui::StrokeKind::Inside);
}

impl App {
    /// Whether a click on a link line may delete it now. Not in a JSON level, and not while a card drag, pick-target
    /// mode, a connect-drag (which implies pick-target mode, listed as the spec lists it), a card or new-node menu, the
    /// right-drag marquee, or a popup that a click outside dismisses (Recently Closed and the other anchored popups,
    /// the shortcuts help) owns the pointer: that click only dismisses.
    pub(crate) fn line_click_allowed(&self) -> bool {
        self.json_level.is_none()
            && self.interaction.dragging.is_none()
            && self.link.connecting.is_none()
            && self.link.connect_drag.is_none()
            && self.menus.node_menu.open.is_none()
            && self.interaction.pending_new_node.is_none()
            && self.interaction.rect_select_start.is_none()
            && self.menus.open_popup.is_none()
            && !self.menus.shortcuts_help_open
    }

    /// The link line under the pointer. Called inside `build_graph_ui`'s Scene, where `ui.response()` is the Scene's
    /// own background: hovered only when no card, control or popup is on top and nothing is being dragged. Off the
    /// minimap (`minimap`, screen points), which is painted, not a widget. Sets the pointing-hand cursor when there is
    /// a line, so a click is never a surprise. None while a text field has the keys (`typing`, read at frame start:
    /// a note being edited, a group rename, the quick switcher, the filter box): egui hands focus back on a click
    /// elsewhere, so that click only ends the edit. Not in `line_click_allowed`, which also gates the connect handles:
    /// those stay usable while a note is being edited.
    pub(crate) fn hovered_link_line(
        &self,
        ui: &egui::Ui,
        edges: &[LevelEdge],
        inv_zoom: f32,
        minimap: Option<egui::Rect>,
        typing: bool,
    ) -> Option<NodeId> {
        if !self.line_click_allowed() || typing || self.renaming_group.is_some() || self.menus.omni_popup.open.is_some()
        {
            return None;
        }
        let background = ui.response();
        if !background.hovered() || background.dragged() {
            return None;
        }
        let pointer = ui.input(|i| i.pointer.hover_pos())?;
        if minimap.is_some_and(|rect| rect.contains(pointer)) {
            return None;
        }
        // World space: `Response::hover_pos` applies the Scene layer's transform.
        let at = background.hover_pos()?;
        let link = nearest_link(edges, at, link_hit_tolerance(inv_zoom))?.link?;
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        Some(link)
    }

    /// The link a click this frame takes, given `line_hover` (this frame's `hovered_link_line`) and whether the Scene's
    /// background reported a click. Call once per frame, before `track_pan_momentum` clears a glide on the press. A
    /// line is taken only if it was already under the pointer when the press began, with the camera still (no glide,
    /// no tween), and the camera has not moved since: a tap that stops a glide ("grab it to stop it") or lands mid
    /// animation aims at no line, though one may be under the pointer by its release. Only a single click: the later
    /// clicks of a double- or triple-click take nothing, or they would delete a pair's duplicate link (now under the
    /// pointer) along with the first.
    pub(crate) fn line_click(&mut self, ui: &egui::Ui, line_hover: Option<NodeId>, clicked: bool) -> Option<NodeId> {
        let (pressed, released, repeat) = ui.input(|i| {
            let p = &i.pointer;
            let primary = egui::PointerButton::Primary;
            (
                p.primary_pressed(),
                p.primary_released(),
                p.button_double_clicked(primary) || p.button_triple_clicked(primary),
            )
        });
        if pressed {
            let still = self.camera.momentum.is_none() && self.camera.tween.is_none();
            self.link.line_press = line_hover.filter(|_| still).map(|link| (link, self.camera.rect));
        }
        let press = if released { self.link.line_press.take() } else { None };
        let (link, rect) = press.filter(|_| clicked && !repeat)?;
        (line_hover.as_ref() == Some(&link) && rect == self.camera.rect).then_some(link)
    }

    /// A click on link line `link`: deletes exactly that Link node, as the node menu's Disconnect row does, and saves.
    pub(crate) fn click_link_line(&mut self, link: NodeId) {
        if self.doc.mirror.is_link(link) {
            self.delete_link(link);
            self.save_session();
        }
    }

    /// Whether a card shows its connect-drag handles (while hovered, or all the time during a connect-drag): every
    /// card does during a connect-drag, so any can be seen as an end. Otherwise only when nothing else owns the
    /// pointer (as `line_click_allowed`), whether or not the card has a valid link target: a drag with none just
    /// cancels, as an empty release does.
    pub(crate) fn shows_link_handles(&self) -> bool {
        self.link.connect_drag.is_some() || self.line_click_allowed()
    }

    /// A press on one of card `id`'s handles: pick-target mode for `link` from it, which rings the targets, run as a
    /// drag. Like any press on the graph it stops a glide ("grab it to stop it"), which `track_pan_momentum` skips
    /// during a connect-drag, so the cards hold still under the rubber band.
    pub(crate) fn start_connect_drag(&mut self, id: NodeId) {
        self.link.connecting = Some((id, crate::caps::LINK));
        self.link.connect_drag = Some(id);
        self.camera.momentum = None;
    }

    /// A drag that began on a link's curve (`line_press`, set by the press on it) has passed egui's drag threshold:
    /// the end nearer where it was grabbed (`press_origin`) comes off its card and follows the pointer as a
    /// connect-drag from the other end, so the rest is `start_connect_drag`'s: rings on the valid targets, the
    /// release on `end_connect_drag`, Esc and `cancel_connect`. Undoes the pan the Scene applied on the frame the
    /// drag was recognised by restoring the camera of the press. False (nothing started) when no link was pressed,
    /// another gesture owns the pointer, or the link is gone.
    pub(crate) fn start_line_retarget(&mut self, ui: &egui::Ui, scene_outer_rect: egui::Rect) -> bool {
        let Some((link, press_rect)) = self.link.line_press.take() else { return false };
        let Some(origin) = ui.input(|i| i.pointer.press_origin()) else { return false };
        if !self.line_click_allowed() {
            return false;
        }
        let drawn = self.camera.rect;
        self.camera.rect = press_rect;
        let at = self.camera.transform(scene_outer_rect).inverse() * origin;
        let edges = self.level_edges();
        let Some(edge) = edges.iter().find(|e| e.link == Some(link)) else {
            self.camera.rect = drawn;
            return false;
        };
        let fixed_is_from = !grabs_from_end(edge, at);
        let fixed = if fixed_is_from { &edge.from_id } else { &edge.to_id };
        self.start_connect_drag(fixed.node().expect("a stored link end is a node"));
        self.link.retarget = Some(LinkRetarget { link, fixed_is_from });
        true
    }

    /// The card a release at world point `at` takes: the topmost card under it (`level_items` is bottom-first, as
    /// right-click's hit-test reads it), or when none is, the nearest card other than `source` within `tolerance`
    /// (world units) of its rect that is a valid link target for it (`valid_targets`, the cards the ring marks), the
    /// topmost of equals. A card under the point wins even if it is `source` or refused: `source` is then no target at
    /// all, and a refused card toasts on release.
    pub(crate) fn card_within(&self, at: egui::Pos2, tolerance: f32, source: NodeId) -> Option<NodeId> {
        let items = self.level_items();
        let source_card = CardId::from(source);
        let cards = || items.iter().rev().filter(|item| item.origin == crate::level::Origin::Stored);
        if let Some(under) = cards().find(|item| item.rect.contains(at)) {
            return (under.id != source_card).then(|| under.id.node()).flatten();
        }
        // Only cards the ring marks (`valid_targets`) count here, so a refused neighbour never shadows a valid one.
        let edge = self.link.connecting.as_ref().map_or(crate::caps::LINK, |(_, edge)| edge);
        let valid = self.valid_targets(edge, source);
        let nearest = |take: &dyn Fn(&crate::level::CardEntry) -> bool| {
            let mut best: Option<(f32, &crate::level::CardEntry)> = None;
            for item in cards().filter(|item| take(item)) {
                let d = distance_to_rect(at, item.rect);
                if d <= tolerance && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, item));
                }
            }
            best.and_then(|(_, item)| item.id.node())
        };
        // A dragged link end deletes its link on empty canvas, so there any other card in reach (its own old card, a
        // refused one) is where it landed, tried after the valid ones; the release then resolves it, never as canvas.
        nearest(&|item| item.id.node().is_some_and(|id| valid.contains(&id)))
            .or_else(|| self.link.retarget.is_some().then(|| nearest(&|item| item.id != source_card)).flatten())
    }

    /// The pointer in world space, inside `build_graph_ui`'s Scene (this frame's layer transform).
    fn scene_pointer(ui: &egui::Ui) -> Option<egui::Pos2> {
        let pointer = ui.input(|i| i.pointer.interact_pos())?;
        Some(ui.ctx().layer_transform_from_global(ui.layer_id()).map_or(pointer, |t| t * pointer))
    }

    /// Where a connect-drag released at screen point `pointer` ends (`DragEnd`). A card (`DragEnd::Card`) is the topmost one under it,
    /// or else the nearest one within `DROP_TOLERANCE_PX` of its edge (`card_within`), never the source; all of it
    /// only inside `scene_outer_rect`, off the `minimap` (painted over the Scene, not a widget) and
    /// under no interactable egui Area above the graph (a popup or menu, by `Context::layer_id_at`, the layer test
    /// egui's own hit-test uses, so a click there would reach no card either; a non-interactable toast lets both
    /// through). `transform` maps world to screen (`GraphCamera::transform`). Both the bright
    /// target ring and the release read this, so the ring shows exactly where a release links. egui hovers only the
    /// dragged handle meanwhile, so the card's own hover cannot say so.
    pub(crate) fn connect_drag_end(
        &self,
        ctx: &egui::Context,
        pointer: Option<egui::Pos2>,
        scene_outer_rect: egui::Rect,
        minimap: Option<egui::Rect>,
        transform: egui::emath::TSTransform,
    ) -> DragEnd {
        let Some(source) = self.link.connect_drag.as_ref() else { return DragEnd::Abandoned };
        let covered = |p: egui::Pos2| ctx.layer_id_at(p).is_some_and(|layer| layer.order != egui::Order::Background);
        let Some(pointer) = pointer
            .filter(|p| scene_outer_rect.contains(*p) && !minimap.is_some_and(|m| m.contains(*p)) && !covered(*p))
        else {
            return DragEnd::Abandoned;
        };
        let at = transform.inverse() * pointer;
        let tolerance = DROP_TOLERANCE_PX / transform.scaling;
        if let Some(target) = self.card_within(at, tolerance, *source) {
            return DragEnd::Card(target);
        }
        // On the source, or (for a dragged link end, which deletes on canvas) within the tolerance of it.
        let reach = if self.link.retarget.is_some() { tolerance } else { 0.0 };
        if self.card_rect(*source).is_some_and(|rect| distance_to_rect(at, rect) <= reach || rect.contains(at)) {
            DragEnd::Source
        } else {
            DragEnd::Canvas
        }
    }

    /// The rubber band: the bowed dashed accent curve a link would be drawn as, from the source's side that faces the
    /// pointer to the pointer, over the cards. With a `target` (`connect_drag_end`, within drop tolerance or
    /// under the pointer) it ends on the target's facing side midpoint, where the handle is, as the finished link will.
    pub(crate) fn draw_connect_rubber_band(&self, ui: &egui::Ui, inv_zoom: f32, target: Option<NodeId>) {
        let (Some(source), Some(pointer)) = (&self.link.connect_drag, Self::scene_pointer(ui)) else { return };
        let Some((mut from, mut to)) = self.rubber_band_ends(*source, pointer, target) else { return };
        let free_end = to;
        // A retargeted link keeps its direction, so the band bows as the finished link will.
        if self.link.retarget.as_ref().is_some_and(|r| !r.fixed_is_from) {
            std::mem::swap(&mut from, &mut to);
        }
        // A dragged link end is drawn by what a release there would do (`retarget_look`).
        let look = self.link.retarget_preview.map(|outcome| retarget_look(outcome, ui.visuals()));
        let stroke = match look {
            Some(look) => egui::Stroke::new(look.band_px * inv_zoom, look.band),
            None => egui::Stroke::new(LINK_STROKE_PX * inv_zoom, crate::theme::accent(ui.visuals())),
        };
        paint_bow(ui.painter(), from, to, stroke, Some(inv_zoom));
        let r = 5.0 * inv_zoom;
        let mark = egui::Stroke::new(2.0 * inv_zoom, stroke.color);
        match look.and_then(|look| look.glyph) {
            Some(RetargetGlyph::Cross) => {
                ui.painter().line_segment([free_end + egui::vec2(-r, -r), free_end + egui::vec2(r, r)], mark);
                ui.painter().line_segment([free_end + egui::vec2(-r, r), free_end + egui::vec2(r, -r)], mark);
            }
            Some(RetargetGlyph::Prohibited) => {
                let s = r * std::f32::consts::FRAC_1_SQRT_2;
                ui.painter().circle_stroke(free_end, r, mark);
                ui.painter().line_segment([free_end + egui::vec2(-s, -s), free_end + egui::vec2(s, s)], mark);
            }
            None => {}
        }
    }

    /// The rubber band's two ends (world): the source's side midpoint, and the pointer or the target's facing side.
    fn rubber_band_ends(
        &self,
        source: NodeId,
        pointer: egui::Pos2,
        target: Option<NodeId>,
    ) -> Option<(egui::Pos2, egui::Pos2)> {
        let source_rect = self.card_rect(source)?;
        Some(match target.and_then(|t| self.card_rect(t)) {
            Some(target_rect) => facing_ends(source_rect, target_rect),
            None => (side_midpoint(source_rect, facing_sides(source_rect.center(), pointer).0), pointer),
        })
    }

    /// Ends a connect-drag at `end`. A new link's drag: any card other than the source finishes the connect
    /// (`finish_connect`, whose toast covers a refused pair); anything else cancels silently. A drag of an existing
    /// link's end (`retarget`) is decided by `finish_retarget`: empty canvas or an already-linked card deletes the
    /// link, a card it can move to takes it, a refused card toasts, and the rest cancels.
    pub(crate) fn end_connect_drag(&mut self, end: DragEnd) {
        let Some(source) = self.link.connect_drag.take() else { return };
        let owns_mode =
            self.link.connecting.as_ref().is_some_and(|(from, edge)| *from == source && *edge == crate::caps::LINK);
        if let (Some(retarget), true) = (self.link.retarget.take(), owns_mode) {
            self.link.connecting = None;
            self.finish_retarget(&retarget, source, &end);
            return;
        }
        match end {
            DragEnd::Card(target) if owns_mode && target != source => self.finish_connect(target),
            _ => self.cancel_connect(),
        }
    }
}

#[cfg(test)]
mod tests;
