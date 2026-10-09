//! Link gestures (spec `2026-10-07-link-gestures-design`): a click on a link line deletes that link, and a drag from a
//! card's edge handle links it to the card it is released on. Both act on `link` edges only; opened-from stays
//! menu-only.

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
/// has none, and a `simplified` level (`graph_lod_simplified`) has none at all.
pub(crate) fn link_handle_rects(card: egui::Rect, zoom_scale: f32, simplified: bool) -> Vec<(HandleSide, egui::Rect)> {
    if simplified {
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
    let fill = if active { crate::theme::ACCENT } else { crate::theme::BG };
    let stroke = egui::Stroke::new(1.5 * inv_zoom, crate::theme::ACCENT);
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
            && self.dragging.is_none()
            && self.connecting.is_none()
            && self.connect_drag.is_none()
            && self.node_menu.is_none()
            && self.pending_new_node.is_none()
            && self.rect_select_start.is_none()
            && self.open_popup.is_none()
            && !self.shortcuts_help_open
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
    ) -> Option<String> {
        if !self.line_click_allowed() || typing || self.renaming_group.is_some() || self.quick_switcher.is_some() {
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
        let link = nearest_link(edges, at, link_hit_tolerance(inv_zoom))?.link.clone();
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        link
    }

    /// The link a click this frame takes, given `line_hover` (this frame's `hovered_link_line`) and whether the Scene's
    /// background reported a click. Call once per frame, before `track_pan_momentum` clears a glide on the press. A
    /// line is taken only if it was already under the pointer when the press began, with the camera still (no glide,
    /// no tween), and the camera has not moved since: a tap that stops a glide ("grab it to stop it") or lands mid
    /// animation aims at no line, though one may be under the pointer by its release. Only a single click: the later
    /// clicks of a double- or triple-click take nothing, or they would delete a pair's duplicate link (now under the
    /// pointer) along with the first.
    pub(crate) fn line_click(&mut self, ui: &egui::Ui, line_hover: Option<String>, clicked: bool) -> Option<String> {
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
            self.line_press = line_hover.clone().filter(|_| still).map(|link| (link, self.camera.rect));
        }
        let press = if released { self.line_press.take() } else { None };
        let (link, rect) = press.filter(|_| clicked && !repeat)?;
        (line_hover.as_ref() == Some(&link) && rect == self.camera.rect).then_some(link)
    }

    /// A click on link line `link`: deletes exactly that Link node, as the node menu's Disconnect row does, and saves.
    pub(crate) fn click_link_line(&mut self, link: &str) {
        if link.parse::<NodeId>().is_ok_and(|id| self.mirror.is_link(id)) {
            self.delete_link(link);
            self.save_session();
        }
    }

    /// Whether a card shows its connect-drag handles (while hovered, or all the time during a connect-drag): every
    /// card does during a connect-drag, so any can be seen as an end. Otherwise only when nothing else owns the
    /// pointer (as `line_click_allowed`), whether or not the card has a valid link target: a drag with none just
    /// cancels, as an empty release does.
    pub(crate) fn shows_link_handles(&self) -> bool {
        self.connect_drag.is_some() || self.line_click_allowed()
    }

    /// A press on one of card `id`'s handles: pick-target mode for `link` from it, which rings the targets, run as a
    /// drag. Like any press on the graph it stops a glide ("grab it to stop it"), which `track_pan_momentum` skips
    /// during a connect-drag, so the cards hold still under the rubber band.
    pub(crate) fn start_connect_drag(&mut self, id: &str) {
        self.connecting = Some((id.to_string(), crate::caps::LINK));
        self.connect_drag = Some(id.to_string());
        self.camera.momentum = None;
    }

    /// The card a release at world point `at` takes: the topmost card under it (`level_items` is bottom-first, as
    /// right-click's hit-test reads it), or when none is, the nearest card other than `source` within `tolerance`
    /// (world units) of its rect that is a valid link target for it (`valid_targets`, the cards the ring marks), the
    /// topmost of equals. A card under the point wins even if it is `source` or refused: `source` is then no target at
    /// all, and a refused card toasts on release.
    pub(crate) fn card_within(&self, at: egui::Pos2, tolerance: f32, source: &str) -> Option<String> {
        let items = self.level_items();
        let cards = || items.iter().rev().filter(|item| matches!(item.kind, crate::level::LevelItemKind::Node(_)));
        if let Some(under) = cards().find(|item| item.rect.contains(at)) {
            return (under.id != source).then(|| under.id.clone());
        }
        // Only cards the ring marks (`valid_targets`) count here, so a refused neighbour never shadows a valid one.
        let edge = self.connecting.as_ref().map_or(crate::caps::LINK, |(_, edge)| edge);
        let valid = self.valid_targets(edge, source);
        let mut best: Option<(f32, &crate::level::LevelItem)> = None;
        for item in cards().filter(|item| valid.contains(&item.id)) {
            let d = distance_to_rect(at, item.rect);
            if d <= tolerance && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, item));
            }
        }
        best.map(|(_, item)| item.id.clone())
    }

    /// The pointer in world space, inside `build_graph_ui`'s Scene (this frame's layer transform).
    fn scene_pointer(ui: &egui::Ui) -> Option<egui::Pos2> {
        let pointer = ui.input(|i| i.pointer.interact_pos())?;
        Some(ui.ctx().layer_transform_from_global(ui.layer_id()).map_or(pointer, |t| t * pointer))
    }

    /// During a connect-drag, the card a release at screen point `pointer` would link to: the topmost card under it,
    /// or else the nearest one within `DROP_TOLERANCE_PX` of its edge (`card_within`), never the source, inside
    /// `scene_outer_rect`, off the `minimap` (painted over the Scene, not a widget) and
    /// under no interactable egui Area above the graph (a popup or menu, by `Context::layer_id_at`, the layer test
    /// egui's own hit-test uses, so a click there would reach no card either; a non-interactable toast lets both
    /// through). `(zoom_scale, translation)` maps world to screen (`graph_world_to_screen_transform`). Both the bright
    /// target ring and the release read this, so the ring shows exactly where a release links. egui hovers only the
    /// dragged handle meanwhile, so the card's own hover cannot say so.
    pub(crate) fn connect_drag_target(
        &self,
        ctx: &egui::Context,
        pointer: Option<egui::Pos2>,
        scene_outer_rect: egui::Rect,
        minimap: Option<egui::Rect>,
        (zoom_scale, translation): (f32, egui::Vec2),
    ) -> Option<String> {
        let source = self.connect_drag.as_ref()?;
        let covered = |p: egui::Pos2| ctx.layer_id_at(p).is_some_and(|layer| layer.order != egui::Order::Background);
        let pointer = pointer
            .filter(|p| scene_outer_rect.contains(*p) && !minimap.is_some_and(|m| m.contains(*p)) && !covered(*p))?;
        let at = ((pointer.to_vec2() - translation) / zoom_scale).to_pos2();
        self.card_within(at, DROP_TOLERANCE_PX / zoom_scale, source)
    }

    /// The rubber band: the bowed dashed accent curve a link would be drawn as, from the source's side that faces the
    /// pointer to the pointer, over the cards. With a `target` (`connect_drag_target`, within drop tolerance or
    /// under the pointer) it ends on the target's facing side midpoint, where the handle is, as the finished link will.
    pub(crate) fn draw_connect_rubber_band(&self, ui: &egui::Ui, inv_zoom: f32, target: Option<&str>) {
        let (Some(source), Some(pointer)) = (&self.connect_drag, Self::scene_pointer(ui)) else { return };
        let Some((from, to)) = self.rubber_band_ends(source, pointer, target) else { return };
        let stroke = egui::Stroke::new(LINK_STROKE_PX * inv_zoom, crate::theme::ACCENT);
        paint_bow(ui.painter(), from, to, stroke, Some(inv_zoom));
    }

    /// The rubber band's two ends (world): the source's side midpoint, and the pointer or the target's facing side.
    fn rubber_band_ends(
        &self,
        source: &str,
        pointer: egui::Pos2,
        target: Option<&str>,
    ) -> Option<(egui::Pos2, egui::Pos2)> {
        let source_rect = self.card_rect(source)?;
        Some(match target.and_then(|t| self.card_rect(t)) {
            Some(target_rect) => facing_ends(source_rect, target_rect),
            None => (side_midpoint(source_rect, facing_sides(source_rect.center(), pointer).0), pointer),
        })
    }

    /// Ends a connect-drag at `target`, the card under the pointer at release (or none). Any card other than the
    /// source finishes the connect (`finish_connect`, whose toast covers a refused pair). Empty canvas or the source
    /// cancels silently.
    pub(crate) fn end_connect_drag(&mut self, target: Option<String>) {
        let Some(source) = self.connect_drag.take() else { return };
        let owns_mode =
            self.connecting.as_ref().is_some_and(|(from, edge)| *from == source && *edge == crate::caps::LINK);
        match target.filter(|t| *t != source) {
            Some(target) if owns_mode => self.finish_connect(&target),
            _ => self.cancel_connect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{EdgeCurve, EdgeKind, LevelEdge};
    use crate::node::{NotePayload, Runtime};
    use crate::store::model::{Kind, NodeId};
    use crate::App;

    #[test]
    fn distance_to_a_segment_clamps_to_its_ends() {
        let (a, b) = (egui::pos2(0.0, 0.0), egui::pos2(10.0, 0.0));
        assert_eq!(distance_to_segment(egui::pos2(5.0, 3.0), a, b), 3.0);
        assert_eq!(distance_to_segment(egui::pos2(-4.0, 3.0), a, b), 5.0, "before the start: to the start");
        assert_eq!(distance_to_segment(egui::pos2(13.0, 4.0), a, b), 5.0, "past the end: to the end");
        assert_eq!(distance_to_segment(egui::pos2(3.0, 4.0), a, a), 5.0, "a point segment");
    }

    #[test]
    fn the_hit_tolerance_is_six_screen_points_at_any_zoom() {
        assert_eq!(link_hit_tolerance(1.0), 6.0);
        assert_eq!(link_hit_tolerance(2.0), 12.0, "zoomed out (inv_zoom 2): 12 world units, 6 on screen");
    }

    /// A 100-long horizontal edge at height `y`: its bow puts the curve's middle at (50, y + 7.5).
    fn edge(link: Option<&str>, y: f32) -> LevelEdge {
        LevelEdge {
            from: egui::pos2(0.0, y),
            to: egui::pos2(100.0, y),
            from_id: "x".into(),
            to_id: "y".into(),
            curve: EdgeCurve::Bow,
            highlight: false,
            kind: if link.is_some() { EdgeKind::Link } else { EdgeKind::Opened },
            link: link.map(str::to_string),
        }
    }

    #[test]
    fn nearest_link_takes_the_closest_link_line_within_tolerance() {
        let edges = [edge(None, 7.5), edge(Some("far"), 12.5), edge(Some("near"), 9.5)];
        let at = egui::pos2(50.0, 15.0);
        assert_eq!(nearest_link(&edges, at, 6.0).and_then(|e| e.link.as_deref()), Some("near"), "opened-from never");
        assert!(nearest_link(&edges, egui::pos2(50.0, 40.0), 6.0).is_none(), "nothing within 6");
        let twins = [edge(Some("older"), 1.0), edge(Some("newer"), 1.0)];
        assert_eq!(nearest_link(&twins, egui::pos2(50.0, 8.5), 6.0).and_then(|e| e.link.as_deref()), Some("newer"));
    }

    /// The hit-test follows the curve, not the chord: of two points the same distance from the chord's middle, the one
    /// on the bow side is on the line and the one on the other side is not.
    #[test]
    fn a_link_is_hit_on_its_curve_and_missed_on_its_chord() {
        let chord = edge(Some("l"), 0.0);
        let edges = [chord];
        assert!(nearest_link(&edges, egui::pos2(50.0, 7.5), 1.0).is_some(), "the curve's middle");
        assert!(nearest_link(&edges, egui::pos2(50.0, 0.0), 6.0).is_none(), "the chord's middle is 7.5 off the curve");
        assert!(nearest_link(&edges, egui::pos2(50.0, 15.0), 6.0).is_none(), "and so is the control point");
        assert!(nearest_link(&edges, egui::pos2(0.0, 0.0), 1.0).is_some(), "the curve starts at the chord's end");
        let middle = bow_point(egui::pos2(0.0, 0.0), egui::pos2(100.0, 0.0), 0.5);
        assert!((middle - egui::pos2(50.0, 7.5)).length() < 1e-4, "{middle:?}");
        let long = bow_control(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 0.0));
        assert_eq!(long, egui::pos2(500.0, 30.0), "the bow is capped at 30");
        let bounds = bow_bounds(egui::pos2(0.0, 0.0), egui::pos2(100.0, 0.0));
        assert!(bounds.contains(egui::pos2(50.0, 15.0)), "the culling box covers the control point");
    }

    #[test]
    fn the_facing_side_rule_follows_the_dominant_axis_and_ties_go_horizontal() {
        use HandleSide::{Bottom, Left, Right, Top};
        let o = egui::pos2(0.0, 0.0);
        assert_eq!(facing_sides(o, egui::pos2(100.0, 30.0)), (Right, Left), "b to the right");
        assert_eq!(facing_sides(o, egui::pos2(-100.0, 30.0)), (Left, Right), "b to the left");
        assert_eq!(facing_sides(o, egui::pos2(30.0, 100.0)), (Bottom, Top), "b below");
        assert_eq!(facing_sides(o, egui::pos2(30.0, -100.0)), (Top, Bottom), "b above");
        assert_eq!(facing_sides(o, egui::pos2(50.0, 50.0)), (Right, Left), "a tie is horizontal");
        assert_eq!(facing_sides(o, egui::pos2(-50.0, -50.0)), (Left, Right), "and keeps its direction");
        assert_eq!(facing_sides(o, o), (Right, Left), "coincident centres");
        let (a, b) = (egui::pos2(10.0, 20.0), egui::pos2(-80.0, 200.0));
        let (sa, sb) = facing_sides(a, b);
        assert_eq!(facing_sides(b, a), (sb, sa), "swapping the cards swaps the sides");
        let ra = egui::Rect::from_center_size(a, egui::vec2(100.0, 60.0));
        let rb = egui::Rect::from_center_size(b, egui::vec2(100.0, 60.0));
        assert_eq!(facing_ends(ra, rb), (ra.center_bottom(), rb.center_top()));
    }

    #[test]
    fn the_nearest_card_within_tolerance_takes_a_release_but_never_the_source() {
        let (mut app, a, b) = two_notes();
        let c = app.alloc_node_id();
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(c.clone(), egui::vec2(0.0, 400.0), Kind::Note, note);
        let (ra, rb, rc) = (app.card_rect(&a).unwrap(), app.card_rect(&b).unwrap(), app.card_rect(&c).unwrap());
        let tol = 24.0;
        let inside_a = ra.center();
        assert_eq!(app.card_within(inside_a, tol, &a), None, "a point on the source is no target");
        assert_eq!(app.card_within(inside_a, tol, &b), Some(a.clone()), "an exact hit wins");
        let near_b = rb.left_center() - egui::vec2(tol - 1.0, 0.0);
        assert_eq!(app.card_within(near_b, tol, &a), Some(b.clone()), "just outside b");
        let far_b = rb.left_center() - egui::vec2(tol + 1.0, 0.0);
        assert_eq!(app.card_within(far_b, tol, &a), None, "beyond tolerance: nothing");
        let corner = rb.left_top() - egui::vec2(tol * 0.7, tol * 0.7);
        assert_eq!(app.card_within(corner, tol, &a), Some(b.clone()), "diagonal distance counts: 0.7 * 1.41 * 24 < 24");
        let corner_far = rb.left_top() - egui::vec2(tol * 0.8, tol * 0.8);
        assert_eq!(app.card_within(corner_far, tol, &a), None, "0.8 * 1.41 * 24 > 24");
        let near_a = ra.right_center() + egui::vec2(tol - 1.0, 0.0);
        assert_eq!(app.card_within(near_a, tol, &a), None, "near only the source: nothing");
        // Of b and c, the nearer wins (the tolerance is huge, so both qualify).
        let above_c = rc.center_top() - egui::vec2(0.0, 10.0);
        assert!(distance_to_rect(above_c, rc) < distance_to_rect(above_c, rb));
        assert_eq!(app.card_within(above_c, 1e6, &a), Some(c.clone()), "the nearer of two");
    }

    #[test]
    fn the_tolerance_pass_skips_cards_the_ring_does_not_mark() {
        let (mut app, a, b) = two_notes();
        app.connect(crate::caps::LINK, &a, &b).unwrap();
        assert!(!app.valid_targets(crate::caps::LINK, &a).contains(&b), "b is refused: already linked");
        let rb = app.card_rect(&b).unwrap();
        let c = app.alloc_node_id();
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(
            c.clone(),
            (rb.center() + egui::vec2(rb.width() + 30.0, 0.0)).to_vec2(),
            Kind::Note,
            note,
        );
        let tol = 24.0;
        let between = egui::pos2(rb.right() + 10.0, rb.center().y);
        let rc = app.card_rect(&c).unwrap();
        assert!(distance_to_rect(between, rb) < distance_to_rect(between, rc), "the refused b is nearer");
        assert_eq!(app.card_within(between, tol, &a), Some(c.clone()), "the farther valid card wins");
        let beside_b = rb.left_center() - egui::vec2(10.0, 0.0);
        assert_eq!(app.card_within(beside_b, tol, &a), None, "only a refused card in tolerance: no target");
        assert_eq!(
            app.card_within(rb.center(), tol, &a),
            Some(b.clone()),
            "an exact hit on a refused card stays (toast)"
        );
    }

    /// Drives `build_graph_ui` headless, as `a_click_finishes_or_cancels_pick_target_mode` (graph_ui.rs) does.
    struct Harness {
        ctx: egui::Context,
        /// An interactable Foreground Area drawn over the graph each frame (as a popup is), when set.
        cover: std::cell::Cell<Option<egui::Rect>>,
        /// egui's input time for the next frame, in seconds: 1/60 per frame, plus any `pause`.
        clock: std::cell::Cell<f64>,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::apply_fonts(&ctx);
            Self { ctx, cover: std::cell::Cell::new(None), clock: std::cell::Cell::new(0.0) }
        }

        /// Lets `seconds` pass before the next frame, e.g. past egui's double-click window.
        fn pause(&self, seconds: f64) {
            self.clock.set(self.clock.get() + seconds);
        }

        /// One frame at the size `graph_world_rect_to_screen` assumes with no window. Returns the cursor it asked for.
        fn frame(&self, app: &mut App, events: Vec<egui::Event>) -> egui::CursorIcon {
            let time = self.clock.get();
            self.clock.set(time + 1.0 / 60.0);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let cover = self.cover.get();
            let mut output = self.ctx.run_ui(input, |ui| {
                app.build_graph_ui(ui);
                if let Some(rect) = cover {
                    egui::Area::new(egui::Id::new("test_cover"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(rect.min)
                        .show(ui.ctx(), |ui| ui.allocate_exact_size(rect.size(), egui::Sense::click()));
                }
            });
            output.textures_delta.clear();
            output.platform_output.cursor_icon
        }

        fn click(&self, app: &mut App, at: egui::Pos2) {
            self.frame(app, vec![egui::Event::PointerMoved(at), press(at, true)]);
            self.frame(app, vec![press(at, false)]);
            self.frame(app, vec![]);
        }

        /// Hovers `from` for two frames (so a hover-only widget there is registered and hit-testable), presses,
        /// moves to `to` in eight steps (sending `midway` with the fourth), and releases there.
        fn drag_with(&self, app: &mut App, from: egui::Pos2, to: egui::Pos2, midway: Vec<egui::Event>) {
            self.frame(app, vec![egui::Event::PointerMoved(from)]);
            self.frame(app, vec![egui::Event::PointerMoved(from)]);
            self.frame(app, vec![press(from, true)]);
            for step in 1..=8 {
                let mut events = vec![egui::Event::PointerMoved(from + (to - from) * (step as f32 / 8.0))];
                if step == 4 {
                    events.extend(midway.iter().cloned());
                }
                self.frame(app, events);
            }
            self.frame(app, vec![press(to, false)]);
            self.frame(app, vec![]);
        }

        fn drag(&self, app: &mut App, from: egui::Pos2, to: egui::Pos2) {
            self.drag_with(app, from, to, Vec::new());
        }
    }

    fn press(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Three notes, a and b side by side and c below a, warmed up and fitted in view.
    fn three_notes() -> (Harness, App, [String; 3]) {
        let h = Harness::new();
        let mut app = App::default();
        let ids = [app.alloc_node_id(), app.alloc_node_id(), app.alloc_node_id()];
        for (id, at) in ids.iter().zip([egui::vec2(0.0, 0.0), egui::vec2(600.0, 0.0), egui::vec2(0.0, 400.0)]) {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id.clone(), at, Kind::Note, note);
        }
        for _ in 0..3 {
            h.frame(&mut app, vec![]);
        }
        app.camera.jump_to(app.fit_all_nodes_scene_rect());
        h.frame(&mut app, vec![]);
        (h, app, ids)
    }

    fn screen_rect(app: &App, id: &str) -> egui::Rect {
        app.graph_world_rect_to_screen(app.card_rect(id).expect("a card"))
    }

    /// The point halfway along `edge`'s curve, on screen.
    fn middle(app: &App, edge: &LevelEdge) -> egui::Pos2 {
        let world = bow_point(edge.from, edge.to, 0.5);
        app.graph_world_rect_to_screen(egui::Rect::from_min_max(world, world)).min
    }

    fn line_middle(app: &App, link: &str) -> egui::Pos2 {
        let edges = app.level_edges();
        middle(app, edges.iter().find(|e| e.link.as_deref() == Some(link)).expect("drawn"))
    }

    #[test]
    fn a_click_on_a_link_line_deletes_that_link() {
        let (h, mut app, [a, b, c]) = three_notes();
        let ab = app.create_link(&a, &b);
        let ac = app.create_link(&a, &c);
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &ab);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), [ac], "exactly the clicked link went");
    }

    #[test]
    fn hovering_a_link_line_shows_the_pointing_hand() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        let on = line_middle(&app, &ab);
        h.frame(&mut app, vec![egui::Event::PointerMoved(on)]);
        assert_eq!(h.frame(&mut app, vec![]), egui::CursorIcon::PointingHand);
        h.frame(&mut app, vec![egui::Event::PointerMoved(on + egui::vec2(0.0, 30.0))]);
        assert_ne!(h.frame(&mut app, vec![]), egui::CursorIcon::PointingHand, "30 points off the line");
    }

    #[test]
    fn a_click_beside_a_line_on_an_opened_from_line_or_on_a_card_deletes_nothing() {
        let (h, mut app, [a, b, c]) = three_notes();
        let ab = app.create_link(&a, &b);
        app.edit_parent(&c, Some(a.clone()));
        h.frame(&mut app, vec![]);
        let beside = line_middle(&app, &ab) + egui::vec2(0.0, 30.0);
        h.click(&mut app, beside);
        let opened = app.level_edges().into_iter().find(|e| e.kind == EdgeKind::Opened).expect("c opened from a");
        let on_opened = middle(&app, &opened);
        h.click(&mut app, on_opened);
        // b's centre is on the card, which takes the click.
        let on_b = screen_rect(&app, &b).center();
        h.click(&mut app, on_b);
        assert_eq!(app.links_naming(&a), [ab]);
        assert_eq!(app.mirror.parent(&c), Some(a.as_str()), "an opened-from line is not clickable");
    }

    #[test]
    fn a_line_click_is_ignored_in_pick_mode_and_with_a_menu_open() {
        let (h, mut app, [a, b, c]) = three_notes();
        let ab = app.create_link(&a, &b);
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &ab);
        app.connecting = Some((c.clone(), crate::caps::LINK));
        h.click(&mut app, on);
        assert!(app.connecting.is_none(), "the click ended pick mode, as any click off a target does");
        assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "and deleted nothing");
        app.node_menu = Some((c.clone(), egui::pos2(5.0, 60.0)));
        h.frame(&mut app, vec![]);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), [ab]);
    }

    #[test]
    fn two_links_on_one_pair_go_one_click_at_a_time_newest_first() {
        let (h, mut app, [a, b, _]) = three_notes();
        let older = app.create_link(&a, &b);
        let newer = app.create_link(&a, &b);
        assert!(newer.parse::<NodeId>().unwrap() > older.parse::<NodeId>().unwrap(), "one launch: ids grow");
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &older);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), [older]);
        h.pause(1.0);
        h.click(&mut app, on);
        assert!(app.links_naming(&a).is_empty());
    }

    /// Final review M1: the second click of a double-click (or a triple-click's third) on a pair's line takes nothing.
    #[test]
    fn a_double_or_triple_click_on_a_line_deletes_one_link() {
        let (h, mut app, [a, b, _]) = three_notes();
        for _ in 0..3 {
            app.create_link(&a, &b);
        }
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &app.links_naming(&a)[0]);
        h.click(&mut app, on);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a).len(), 2, "a double-click deletes one");
        h.pause(1.0);
        for _ in 0..3 {
            h.click(&mut app, on);
        }
        assert_eq!(app.links_naming(&a).len(), 1, "a triple-click deletes one");
    }

    fn right(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Moves the pointer to `at` and right-presses there: the `midway` of a drag.
    fn right_press_at(at: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerMoved(at), right(at, true)]
    }

    /// Task 2 review M2: during a connect-drag a right-press on a card opens no menu and one on the canvas starts no
    /// marquee; the drag still links where it is released.
    #[test]
    fn a_right_press_during_a_connect_drag_opens_no_menu_and_starts_no_marquee() {
        let (h, mut app, [a, b, c]) = three_notes();
        let (on_b, on_c) = (screen_rect(&app, &b).center(), screen_rect(&app, &c).center());
        let canvas = on_b.lerp(on_c, 0.5);
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, on_b, right_press_at(on_c));
        assert!(app.node_menu.is_none(), "no menu on c");
        assert_eq!(app.links_naming(&a).len(), 1, "linked to b");
        h.frame(&mut app, vec![right(on_b, false)]);
        drag_from_handle(&h, &mut app, &a, HandleSide::Bottom, on_c, right_press_at(canvas));
        assert!(app.rect_select_start.is_none(), "no marquee");
        assert_eq!(app.links_naming(&a).len(), 2, "linked to c");
    }

    /// Task 2 review M2, card-drag half: a right-press while a card is dragged opens no menu and starts no marquee.
    #[test]
    fn a_right_press_during_a_card_drag_opens_no_menu_and_starts_no_marquee() {
        let (h, mut app, [a, b, c]) = three_notes();
        let (on_b, on_c) = (screen_rect(&app, &b).center(), screen_rect(&app, &c).center());
        let canvas = on_b.lerp(on_c, 0.5);
        let from = screen_rect(&app, &a).center();
        let to = from + egui::vec2(0.0, -40.0);
        h.drag_with(&mut app, from, to, right_press_at(on_c));
        assert!(app.node_menu.is_none(), "no menu on c");
        h.frame(&mut app, vec![right(to, false)]);
        let from = screen_rect(&app, &a).center();
        h.drag_with(&mut app, from, from + egui::vec2(40.0, 0.0), right_press_at(canvas));
        assert!(app.rect_select_start.is_none(), "no marquee");
    }

    #[test]
    fn a_drag_from_a_link_line_pans_and_deletes_nothing() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        h.frame(&mut app, vec![]);
        let before = app.camera.rect;
        let on = line_middle(&app, &ab);
        h.drag(&mut app, on, on + egui::vec2(120.0, 80.0));
        assert_ne!(app.camera.rect.center(), before.center(), "the drag panned");
        assert_eq!(app.links_naming(&a), [ab]);
    }

    /// Review I1: Recently Closed and the shortcuts help close on a click outside them without consuming it, so the
    /// dismissing click must not also take the line under it.
    #[test]
    fn a_click_dismissing_recently_closed_or_the_shortcuts_help_deletes_nothing() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &ab);
        app.open_popup = Some(crate::popups::AnchoredPopup::RecentlyClosed);
        h.frame(&mut app, vec![]);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "Recently Closed was open");
        app.open_popup = None;
        app.shortcuts_help_open = true;
        h.frame(&mut app, vec![]);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "the shortcuts help was open");
        app.shortcuts_help_open = false;
        h.frame(&mut app, vec![]);
        h.pause(1.0); // past the double-click window: a second click there takes nothing
        h.click(&mut app, on);
        assert!(app.links_naming(&a).is_empty(), "with neither open the same click takes it");
    }

    /// Review I2: a tap that stops a glide or a camera animation ("grab it to stop it") takes no line, though a line
    /// may have moved under the pointer by its release.
    #[test]
    fn a_tap_during_a_glide_or_a_camera_tween_deletes_nothing() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &ab);
        app.camera.momentum = Some(crate::view_modes::PanMomentum {
            initial_velocity: egui::vec2(1.0, 0.0),
            start: std::time::Instant::now(),
            start_center: app.camera.rect.center(),
        });
        h.click(&mut app, on);
        assert!(app.camera.momentum.is_none(), "the tap stopped the glide");
        assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "and took no line");
        app.camera.ease_to(app.camera.rect);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "a tap during a tween takes no line");
        app.camera.jump_to(app.camera.rect);
        h.pause(1.0); // past the double-click window: a second click there takes nothing
        h.click(&mut app, on);
        assert!(app.links_naming(&a).is_empty(), "with the camera still the same tap takes it");
    }

    /// The hit tolerance follows the zoom the Scene draws at: at 50% a line 5 screen points away is hit (10 world
    /// units, within 12) and 8 away is not.
    #[test]
    fn at_half_zoom_the_tolerance_is_still_six_screen_points() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        let outer = app.graph_scene_outer_rect();
        app.camera.jump_to(egui::Rect::from_center_size(egui::pos2(300.0, 0.0), outer.size() * 2.0));
        h.frame(&mut app, vec![]);
        assert!((app.camera.zoom_pct - 50.0).abs() < 0.01, "at 50%: {}", app.camera.zoom_pct);
        let on = line_middle(&app, &ab);
        h.click(&mut app, on + egui::vec2(0.0, 8.0));
        assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "8 points off: a miss");
        h.pause(1.0); // past the double-click window: a second click there takes nothing
        h.click(&mut app, on + egui::vec2(0.0, 5.0));
        assert!(app.links_naming(&a).is_empty(), "5 points off: a hit");
    }

    /// The minimap is painted over the Scene, not a widget: a click on it over a line recentres and deletes nothing.
    #[test]
    fn a_click_on_the_minimap_over_a_line_deletes_nothing() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        // `draw_graph_minimap`'s rect: 160 by 110, 12 in from the scene's bottom-right corner.
        let outer = app.graph_scene_outer_rect();
        let minimap = egui::Rect::from_min_size(
            outer.right_bottom() - egui::vec2(160.0, 110.0) - egui::Vec2::splat(12.0),
            egui::vec2(160.0, 110.0),
        );
        // At 100%, put the line's middle (300, 0) under the minimap's centre; b is then out of view, so it shows.
        let center = egui::pos2(300.0, 0.0) - (minimap.center() - outer.center());
        app.camera.jump_to(egui::Rect::from_center_size(center, outer.size()));
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &ab);
        assert!(minimap.shrink(10.0).contains(on), "the line runs under the minimap: {on:?} in {minimap:?}");
        let before = app.camera.rect;
        h.click(&mut app, on);
        assert_ne!(app.camera.rect, before, "the minimap took the click and recentred");
        assert_eq!(app.links_naming(&a), [ab]);
    }

    #[test]
    fn handles_are_pills_on_each_edge_midpoint_at_a_constant_screen_size() {
        let card = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(180.0, 140.0));
        let rects = link_handle_rects(card, 2.0, false);
        assert_eq!(rects.iter().map(|(side, _)| *side).collect::<Vec<_>>(), HandleSide::ALL);
        let (long, short) = (HANDLE_LONG_PX / 2.0, HANDLE_SHORT_PX / 2.0);
        for (side, r) in &rects {
            let along_x = matches!(side, HandleSide::Top | HandleSide::Bottom);
            let expected = if along_x { egui::vec2(long, short) } else { egui::vec2(short, long) };
            assert_eq!(r.size(), expected, "{side:?}: 28 by 16 screen points at zoom 2, long side along the edge");
        }
        assert_eq!(rects[0].1.center(), card.center_top());
        assert_eq!(rects[1].1.center(), card.right_center());
        assert_eq!(rects[2].1.center(), card.center_bottom());
        assert_eq!(rects[3].1.center(), card.left_center());
    }

    #[test]
    fn handles_hide_on_short_edges_and_when_simplified() {
        let thin = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(180.0, 30.0));
        let sides: Vec<HandleSide> = link_handle_rects(thin, 1.0, false).into_iter().map(|(s, _)| s).collect();
        assert_eq!(sides, [HandleSide::Top, HandleSide::Bottom], "30 points tall: no side handles");
        assert!(link_handle_rects(thin, 0.2, false).is_empty(), "180 * 0.2 = 36 points wide: none");
        assert!(link_handle_rects(thin, 3.0, true).is_empty(), "a simplified level shows none");
        assert_eq!(HANDLE_MIN_EDGE_PX, 60.0);
        let edge = |on_screen: f32| egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(on_screen, 400.0));
        assert_eq!(link_handle_rects(edge(59.0), 1.0, false).len(), 2, "59 wide: only the side handles");
        assert_eq!(link_handle_rects(edge(60.0), 1.0, false).len(), 4, "60 wide: all four");
    }

    /// Two notes at the top level, not yet drawn.
    fn two_notes() -> (App, String, String) {
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        for (id, x) in [(&a, 0.0), (&b, 600.0)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id.clone(), egui::vec2(x, 0.0), Kind::Note, note);
        }
        (app, a, b)
    }

    #[test]
    fn handles_show_while_nothing_else_owns_the_pointer_and_on_every_card_during_a_drag() {
        let (mut app, a, b) = two_notes();
        assert!(app.shows_link_handles());
        app.dragging = Some(b.clone());
        assert!(!app.shows_link_handles(), "a card drag");
        app.dragging = None;
        app.node_menu = Some((b.clone(), egui::Pos2::ZERO));
        assert!(!app.shows_link_handles(), "a menu");
        app.node_menu = None;
        app.start_connect_drag(&a);
        assert!(app.shows_link_handles(), "during a connect-drag every card shows them, not only the source");
        app.cancel_connect();
        app.create_link(&a, &b);
        assert!(!app.has_valid_target(crate::caps::LINK, &a), "linked to the only other card: no target left");
        assert!(app.shows_link_handles(), "yet the handles still show: a drag from them just cancels");
    }

    /// With no valid target the hovered card still has handles, and a drag from one cancels as an empty release does.
    #[test]
    fn a_card_with_no_valid_target_still_shows_handles_and_its_drag_cancels() {
        let (h, mut app, [a, b, c]) = three_notes();
        app.create_link(&a, &b);
        app.create_link(&a, &c);
        assert!(!app.has_valid_target(crate::caps::LINK, &a), "a is linked to both others");
        let state = app.export_state();
        assert!(state["link_handles"].get(a.as_str()).is_some(), "so its handles are exported");
        let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &b).center(), 0.5);
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, canvas, vec![]);
        assert!(app.connect_drag.is_none() && app.connecting.is_none(), "cancelled");
        assert_eq!(app.links_naming(&a).len(), 2, "and nothing was linked");
    }

    /// Mid-drag every card has handles: the export lists them and the cards register them as widgets.
    #[test]
    fn during_a_handle_drag_every_card_has_handles() {
        let (h, mut app, [a, b, c]) = three_notes();
        assert!(app.export_state()["link_handles"].get(b.as_str()).is_some(), "before any drag all do");
        let centre = screen_rect(&app, &a).center();
        h.frame(&mut app, vec![egui::Event::PointerMoved(centre)]);
        let handle = handle_on_screen(&app, &a, HandleSide::Right);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![press(handle, true)]);
        let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &b).center(), 0.5);
        h.frame(&mut app, vec![egui::Event::PointerMoved(canvas)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(canvas + egui::vec2(0.0, 5.0))]);
        assert_eq!(app.connect_drag.as_deref(), Some(a.as_str()));
        let state = app.export_state();
        for id in [&a, &b, &c] {
            let sides = state["link_handles"][id.as_str()].as_object().unwrap_or_else(|| panic!("{id} has handles"));
            assert_eq!(sides.len(), 4, "{id}");
        }
        for (id, side) in [(&b, HandleSide::Left), (&c, HandleSide::Top)] {
            let widget = egui::Id::new(("link_handle", side, id.as_str()));
            assert!(h.ctx.read_response(widget).is_some(), "{id}'s {side:?} handle is a widget mid-drag");
        }
        h.frame(&mut app, vec![press(canvas, false)]);
    }

    #[test]
    fn a_connect_drag_ends_with_its_mode_and_with_its_source() {
        let (mut app, a, b) = two_notes();
        app.start_connect_drag(&a);
        assert_eq!(app.connecting, Some((a.clone(), crate::caps::LINK)), "it runs as pick-target mode for link");
        app.cancel_connect();
        assert!(app.connect_drag.is_none());
        app.start_connect_drag(&a);
        app.forget_connect_source(&a);
        assert!(app.connect_drag.is_none() && app.connecting.is_none(), "its source removed");
        app.start_connect_drag(&a);
        app.end_stale_connect(false);
        assert!(app.connect_drag.is_none(), "Graph view left");
        app.start_connect_drag(&a);
        app.connecting = None;
        app.end_stale_connect(true);
        assert!(app.connect_drag.is_none(), "a connect-drag without its mode is stale");
        app.start_connect_drag(&a);
        app.end_connect_drag(None);
        assert!(app.connect_drag.is_none() && app.connecting.is_none() && app.links_naming(&a).is_empty());
        app.start_connect_drag(&a);
        app.end_connect_drag(Some(b.clone()));
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
        assert_eq!(app.links_naming(&a).len(), 1, "released on b");
    }

    fn handle_on_screen(app: &App, id: &str, side: HandleSide) -> egui::Pos2 {
        let (zoom, _) = app.graph_world_to_screen_transform(app.graph_scene_outer_rect());
        let rects = link_handle_rects(app.card_rect(id).expect("a card"), zoom, false);
        let (_, rect) = rects.into_iter().find(|(s, _)| *s == side).expect("shown at this zoom");
        app.graph_world_rect_to_screen(rect).center()
    }

    /// Hovers card `id` (so its handles show), then drags from its `side` handle to `to`.
    fn drag_from_handle(
        h: &Harness,
        app: &mut App,
        id: &str,
        side: HandleSide,
        to: egui::Pos2,
        midway: Vec<egui::Event>,
    ) {
        h.frame(app, vec![egui::Event::PointerMoved(screen_rect(app, id).center())]);
        let from = handle_on_screen(app, id, side);
        h.drag_with(app, from, to, midway);
    }

    /// A release just outside a card (within `DROP_TOLERANCE_PX`) links it; one further out cancels.
    #[test]
    fn a_release_within_the_drop_tolerance_of_a_card_links_it_and_beyond_it_does_not() {
        let (h, mut app, [a, b, _]) = three_notes();
        let rb = screen_rect(&app, &b);
        let just_outside = rb.left_center() - egui::vec2(DROP_TOLERANCE_PX - 4.0, 0.0);
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, just_outside, vec![]);
        let links = app.links_naming(&a);
        assert_eq!(links.len(), 1, "linked from {just_outside:?}");
        assert_eq!(app.mirror.link_ends(&links[0]), (a.parse().ok(), b.parse().ok()), "a -> b");
        let too_far = rb.left_center() - egui::vec2(DROP_TOLERANCE_PX + 8.0, 0.0);
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, too_far, vec![]);
        assert_eq!(app.links_naming(&a).len(), 1, "nothing more from {too_far:?}");
    }

    /// The rubber band ends on the target's facing handle when there is a target, on the pointer otherwise.
    #[test]
    fn the_rubber_band_snaps_to_the_targets_facing_side() {
        let (app, a, b) = two_notes();
        let (ra, rb) = (app.card_rect(&a).unwrap(), app.card_rect(&b).unwrap());
        let pointer = rb.left_center() - egui::vec2(10.0, 0.0);
        assert_eq!(app.rubber_band_ends(&a, pointer, Some(&b)), Some((ra.right_center(), rb.left_center())));
        assert_eq!(
            app.rubber_band_ends(&a, pointer, None),
            Some((ra.right_center(), pointer)),
            "no target: the pointer"
        );
        let above = ra.center_top() - egui::vec2(0.0, 80.0);
        assert_eq!(app.rubber_band_ends(&a, above, None).map(|e| e.0), Some(ra.center_top()), "the side facing it");
    }

    #[test]
    fn dragging_a_handle_onto_a_card_links_them_and_moves_nothing() {
        let (h, mut app, [a, b, _]) = three_notes();
        let before = app.layout_pos(&a);
        let onto = screen_rect(&app, &b).center();
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, onto, vec![]);
        let links = app.links_naming(&a);
        assert_eq!(links.len(), 1);
        assert_eq!(app.mirror.link_ends(&links[0]), (a.parse().ok(), b.parse().ok()), "a -> b");
        assert_eq!(app.layout_pos(&a), before, "the card did not move");
        assert!(app.dragging.is_none() && app.connect_drag.is_none() && app.connecting.is_none());
    }

    #[test]
    fn a_handle_drag_released_on_canvas_or_on_its_source_links_nothing_silently() {
        let (h, mut app, [a, b, _]) = three_notes();
        let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &b).center(), 0.5);
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, canvas, vec![]);
        let own = screen_rect(&app, &a).center();
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, own, vec![]);
        assert!(app.links_naming(&a).is_empty());
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
        assert!(app.save_toast.is_none() && app.save_toast_queue.is_empty(), "no toast");
    }

    #[test]
    fn a_handle_drag_onto_a_card_already_linked_says_so() {
        let (h, mut app, [a, b, _]) = three_notes();
        app.create_link(&a, &b);
        let onto = screen_rect(&app, &b).center();
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, onto, vec![]);
        assert_eq!(app.links_naming(&a).len(), 1);
        let toasts: Vec<&String> = app.save_toast.iter().map(|(m, _)| m).chain(&app.save_toast_queue).collect();
        assert!(toasts.iter().any(|m| m.as_str() == "Can't connect these cards"), "{toasts:?}");
    }

    #[test]
    fn escape_cancels_a_handle_drag() {
        let (h, mut app, [a, b, _]) = three_notes();
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let onto = screen_rect(&app, &b).center();
        let before = app.layout_pos(&a);
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, onto, vec![escape]);
        assert_eq!(app.layout_pos(&a), before, "the rest of the held drag moves no card");
        assert!(app.links_naming(&a).is_empty());
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
    }

    #[test]
    fn pressing_a_handle_without_moving_ends_the_gesture_and_clicks_nothing() {
        let (h, mut app, [a, _, _]) = three_notes();
        let handle = handle_on_screen(&app, &a, HandleSide::Bottom);
        let centre = screen_rect(&app, &a).center();
        h.frame(&mut app, vec![egui::Event::PointerMoved(centre)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![press(handle, true)]);
        assert_eq!(app.connect_drag.as_deref(), Some(a.as_str()), "the press started it");
        h.frame(&mut app, vec![press(handle, false)]);
        h.frame(&mut app, vec![]);
        assert!(app.connect_drag.is_none() && app.connecting.is_none() && app.links_naming(&a).is_empty());
        assert!(!app.nodes[&a].as_note().unwrap().editing, "the press was the handle's, not a click on the note");
    }

    /// a's right handle sits where the a-b line starts (a's right side midpoint). A press and release there is the handle's: it records
    /// no line press, so the release is no line click, and the line stays. A handle drag released on a line's middle
    /// deletes nothing either.
    #[test]
    fn a_handle_over_a_link_line_takes_the_press_from_the_line() {
        let (h, mut app, [a, b, c]) = three_notes();
        let ab = app.create_link(&a, &b);
        h.frame(&mut app, vec![]);
        // The handle's outer half, off the card: only the Scene's background (and the line) lie under it.
        let handle = handle_on_screen(&app, &a, HandleSide::Right) + egui::vec2(4.0, 0.0);
        assert!(!screen_rect(&app, &a).contains(handle));
        let line = app.level_edges().into_iter().find(|e| e.link.as_ref() == Some(&ab)).expect("drawn");
        let (zoom, _) = app.graph_world_to_screen_transform(app.graph_scene_outer_rect());
        let in_world = app.card_rect(&a).unwrap().right_center() + egui::vec2(4.0 / zoom, 0.0);
        assert!(
            distance_to_bow(in_world, line.from, line.to) <= link_hit_tolerance(1.0 / zoom),
            "the handle's outer half is within the line's hit area: {handle:?} {line:?}"
        );
        let centre = screen_rect(&app, &a).center();
        h.frame(&mut app, vec![egui::Event::PointerMoved(centre)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![press(handle, true)]);
        assert_eq!(app.connect_drag.as_deref(), Some(a.as_str()));
        assert!(app.line_press.is_none(), "the handle's press is not a line press");
        h.frame(&mut app, vec![press(handle, false)]);
        h.frame(&mut app, vec![]);
        assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "the line under the handle stays");
        let on = line_middle(&app, &ab);
        drag_from_handle(&h, &mut app, &c, HandleSide::Top, on, vec![]);
        assert_eq!(app.links_naming(&a), [ab]);
        assert!(app.links_naming(&c).is_empty() && app.connect_drag.is_none() && app.line_press.is_none());
    }

    #[test]
    fn a_drag_on_the_card_body_still_moves_the_card() {
        let (h, mut app, [a, _, _]) = three_notes();
        let before = app.layout_pos(&a);
        let from = screen_rect(&app, &a).center();
        h.drag(&mut app, from, from + egui::vec2(-60.0, 0.0));
        assert_ne!(app.layout_pos(&a), before, "the card moved");
        assert!(app.connect_drag.is_none() && app.links_naming(&a).is_empty());
    }

    /// Review I1: a focused text field (a note being edited) keeps its keys, but Esc still cancels a connect-drag.
    #[test]
    fn escape_cancels_a_handle_drag_while_a_note_is_being_edited() {
        let (h, mut app, [a, b, c]) = three_notes();
        let on_a = screen_rect(&app, &a).center();
        h.click(&mut app, on_a);
        assert!(app.nodes[&a].as_note().unwrap().editing);
        assert!(h.ctx.memory(|m| m.has_focus(egui::Id::new(("note_edit", a.as_str())))), "a's editor has focus");
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let onto = screen_rect(&app, &c).center();
        drag_from_handle(&h, &mut app, &b, HandleSide::Bottom, onto, vec![escape]);
        assert!(app.links_naming(&b).is_empty(), "Esc cancelled it");
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
        // Final review M5: egui drops keyboard focus on Esc, so the same Esc also ended a's edit.
        assert!(!app.nodes[&a].as_note().unwrap().editing, "the same Esc ended the note edit");
    }

    /// Where `draw_graph_minimap` puts the minimap: 160 by 110, 12 in from the scene's bottom-right corner.
    fn minimap_rect(app: &App) -> egui::Rect {
        let outer = app.graph_scene_outer_rect();
        egui::Rect::from_min_size(
            outer.right_bottom() - egui::vec2(160.0, 110.0) - egui::Vec2::splat(12.0),
            egui::vec2(160.0, 110.0),
        )
    }

    /// Review M1: the bright ring and the release read one function, so a card under the minimap (painted over the
    /// Scene) is neither ringed nor linked.
    #[test]
    fn a_card_under_the_minimap_is_neither_ringed_nor_linked() {
        let (h, mut app, [a, b, _]) = three_notes();
        let (outer, minimap) = (app.graph_scene_outer_rect(), minimap_rect(&app));
        // At 100%, b's centre under the minimap's centre; c, below, is then out of view, so the minimap shows.
        let center = app.layout_pos(&b).to_pos2() - (minimap.center() - outer.center());
        app.camera.jump_to(egui::Rect::from_center_size(center, outer.size()));
        h.frame(&mut app, vec![]);
        let over = minimap.center();
        assert!(screen_rect(&app, &b).contains(over), "b lies under the minimap");
        app.start_connect_drag(&a);
        let transform = app.graph_world_to_screen_transform(outer);
        assert_eq!(
            app.connect_drag_target(&h.ctx, Some(over), outer, None, transform),
            Some(b.clone()),
            "without it: b"
        );
        assert_eq!(app.connect_drag_target(&h.ctx, Some(over), outer, Some(minimap), transform), None, "not ringed");
        app.cancel_connect();
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, over, vec![]);
        assert!(app.links_naming(&a).is_empty(), "not linked");
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
    }

    /// Review M5: handles exist only on a hovered card. A drag from where a's right handle would be, off the card and
    /// with a not hovered, pans.
    #[test]
    fn a_drag_from_a_handle_spot_on_an_unhovered_card_pans() {
        let (h, mut app, [a, _, _]) = three_notes();
        let spot = handle_on_screen(&app, &a, HandleSide::Right) + egui::vec2(4.0, 0.0);
        assert!(!screen_rect(&app, &a).contains(spot), "the handle's outer half, off the card");
        let before = app.camera.rect;
        h.drag(&mut app, spot, spot + egui::vec2(100.0, 60.0));
        assert_ne!(app.camera.rect.center(), before.center(), "the drag panned");
        assert!(app.connect_drag.is_none() && app.links_naming(&a).is_empty());
    }

    /// Review M5: released where two cards overlap, the connect-drag links the one drawn on top.
    #[test]
    fn the_topmost_of_overlapping_cards_takes_the_release() {
        let (h, mut app, [a, b, _]) = three_notes();
        let d = app.alloc_node_id();
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(d.clone(), app.layout_pos(&b), Kind::Note, note);
        h.frame(&mut app, vec![]);
        // Placed after the level settled its new count, so it stays overlapping b.
        app.snap_layout(&d, app.layout_pos(&b) + egui::vec2(40.0, 20.0));
        h.frame(&mut app, vec![]);
        let both = screen_rect(&app, &b).intersect(screen_rect(&app, &d));
        assert!(both.is_positive(), "b and d overlap");
        let items: Vec<String> = app.level_items().into_iter().map(|i| i.id).collect();
        let pos = |id: &String| items.iter().position(|i| i == id);
        assert!(pos(&d) > pos(&b), "d is drawn after b, on top");
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, both.center(), vec![]);
        let links = app.links_naming(&a);
        assert_eq!(links.len(), 1);
        assert_eq!(app.mirror.link_ends(&links[0]), (a.parse().ok(), d.parse().ok()), "a -> d, the top card");
    }

    /// Review M4: pressing a handle stops a glide ("grab it to stop it"), as any press on the graph does, so the
    /// cards hold still under the rubber band; the drag itself starts none.
    #[test]
    fn a_handle_press_stops_a_glide_and_the_drag_starts_none() {
        let (h, mut app, [a, b, _]) = three_notes();
        let centre = screen_rect(&app, &a).center();
        h.frame(&mut app, vec![egui::Event::PointerMoved(centre)]);
        let handle = handle_on_screen(&app, &a, HandleSide::Right);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        app.camera.momentum = Some(crate::view_modes::PanMomentum {
            initial_velocity: egui::vec2(1.0, 0.0),
            start: std::time::Instant::now(),
            start_center: app.camera.rect.center(),
        });
        h.frame(&mut app, vec![press(handle, true)]);
        assert_eq!(app.connect_drag.as_deref(), Some(a.as_str()));
        assert!(app.camera.momentum.is_none(), "the press stopped the glide");
        let held = app.camera.rect;
        let onto = screen_rect(&app, &b).center();
        for step in 1..=4 {
            h.frame(&mut app, vec![egui::Event::PointerMoved(handle + (onto - handle) * (step as f32 / 4.0))]);
        }
        h.frame(&mut app, vec![press(onto, false)]);
        h.frame(&mut app, vec![]);
        assert_eq!(app.camera.rect, held, "the camera held still");
        assert!(app.camera.momentum.is_none(), "and no glide after the release");
        assert_eq!(app.links_naming(&a).len(), 1);
    }

    /// Final review I1: while a text field has the keys, a click outside it only ends the edit (egui hands focus
    /// back on that click), so it takes no line.
    #[test]
    fn a_click_that_ends_a_note_edit_deletes_no_line() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        h.frame(&mut app, vec![]);
        let on_a = screen_rect(&app, &a).center();
        h.click(&mut app, on_a);
        assert!(h.ctx.memory(|m| m.has_focus(egui::Id::new(("note_edit", a.as_str())))), "a's editor has focus");
        let on = line_middle(&app, &ab);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "the line stays");
        assert!(!app.nodes[&a].as_note().unwrap().editing, "the click ended the edit");
        h.pause(1.0); // past the double-click window: a second click there takes nothing
        h.click(&mut app, on);
        assert!(app.links_naming(&a).is_empty(), "with no edit running the same click takes it");
    }

    /// Final review I2: losing window focus mid-drag cancels it (the button-up may never arrive), so a later
    /// ordinary click on a card links nothing.
    #[test]
    fn losing_window_focus_cancels_a_connect_drag() {
        let (h, mut app, [a, b, _]) = three_notes();
        let centre = screen_rect(&app, &a).center();
        h.frame(&mut app, vec![egui::Event::PointerMoved(centre)]);
        let handle = handle_on_screen(&app, &a, HandleSide::Right);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![press(handle, true)]);
        assert_eq!(app.connect_drag.as_deref(), Some(a.as_str()));
        app.on_window_focus(false);
        assert!(app.connect_drag.is_none() && app.connecting.is_none() && app.line_press.is_none());
        let on_b = screen_rect(&app, &b).center();
        h.frame(&mut app, vec![egui::Event::PointerMoved(on_b), press(on_b, false)]);
        h.frame(&mut app, vec![]);
        h.click(&mut app, on_b);
        assert!(app.links_naming(&a).is_empty());
    }

    /// A note and a group side by side, warmed up and fitted in view, driven through the app's own egui context
    /// (`App::egui_ctx`, the one the event loop runs and `on_window_focus` reaches) rather than a separate one.
    fn note_and_group_on_app_ctx() -> (Harness, App, String, String) {
        let mut app = App::default();
        let h =
            Harness { ctx: app.egui_ctx.clone(), cover: std::cell::Cell::new(None), clock: std::cell::Cell::new(0.0) };
        let (note, group) = (app.alloc_node_id(), app.alloc_node_id());
        let text = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(note.clone(), egui::vec2(0.0, 0.0), Kind::Note, text);
        app.insert_placeholder_node(group.clone(), egui::vec2(600.0, 0.0), Kind::Group, Runtime::None);
        for _ in 0..3 {
            h.frame(&mut app, vec![]);
        }
        app.camera.jump_to(app.fit_all_nodes_scene_rect());
        h.frame(&mut app, vec![]);
        (h, app, note, group)
    }

    /// Task 2 M3, card half: losing window focus mid card drag ends it as a release where the card is (its pos
    /// saved, as `release_drag` saves it), never as a drop onto what is under it. egui keeps the button down across
    /// the focus loss, so the card must also stop following the pointer until a real release, which drops nothing.
    #[test]
    fn losing_window_focus_ends_a_card_drag_in_place() {
        let (h, mut app, note, group) = note_and_group_on_app_ctx();
        let from = screen_rect(&app, &note).center();
        let over_group = screen_rect(&app, &group).center();
        h.frame(&mut app, vec![egui::Event::PointerMoved(from)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(from)]);
        h.frame(&mut app, vec![press(from, true)]);
        for step in 1..=8 {
            h.frame(&mut app, vec![egui::Event::PointerMoved(from + (over_group - from) * (step as f32 / 8.0))]);
        }
        assert_eq!(app.dragging.as_deref(), Some(note.as_str()), "mid-drag");
        assert!(
            matches!(app.drop_target(&note, Some(over_group), None), Some((crate::card::DropEffect::Join, _))),
            "a release here would join the group"
        );
        let held = app.layout_pos(&note);

        app.on_window_focus(false);
        assert!(app.dragging.is_none() && app.drag_moved.is_empty(), "the drag ended");
        assert_eq!(app.mirror.pos(&note), Some(held), "saved where it was");
        assert_eq!(app.group_of(&Some(note.clone())), None, "no drop: it joined nothing");

        // The button-up went to another window: egui still has the button down while the pointer moves on.
        for step in 1..=4 {
            h.frame(&mut app, vec![egui::Event::PointerMoved(over_group + egui::vec2(0.0, 30.0 * step as f32))]);
        }
        assert_eq!(app.layout_pos(&note), held, "the card no longer follows the pointer");
        assert!(app.dragging.is_none());
        let late = over_group + egui::vec2(0.0, 120.0);
        h.frame(&mut app, vec![press(late, false)]);
        h.frame(&mut app, vec![]);
        assert_eq!(app.layout_pos(&note), held);
        assert_eq!(app.mirror.pos(&note), Some(held));
        assert_eq!(app.group_of(&Some(note.clone())), None, "a late release drops nothing");
        assert!(app.nodes.contains_key(&note) && app.in_level(&note), "still on this level");
    }

    /// Final review nit: a level change drops a line press, as it drops a connect-drag.
    #[test]
    fn a_level_change_drops_a_line_press() {
        let (mut app, a, _) = two_notes();
        app.line_press = Some((a.clone(), app.camera.rect));
        app.start_connect_drag(&a);
        app.switch_group_level(None);
        assert!(app.line_press.is_none() && app.connect_drag.is_none());
    }

    /// Final review I2: a connect-drag whose button-up was lost (the button is up, yet no release came this frame)
    /// cancels, so the next ordinary click on a card links nothing.
    #[test]
    fn a_connect_drag_without_its_button_down_cancels() {
        let (h, mut app, [a, b, _]) = three_notes();
        let on_b = screen_rect(&app, &b).center();
        app.start_connect_drag(&a);
        h.frame(&mut app, vec![egui::Event::PointerMoved(on_b)]);
        assert!(app.connect_drag.is_none() && app.connecting.is_none(), "no button down: cancelled");
        h.click(&mut app, on_b);
        assert!(app.links_naming(&a).is_empty());
    }

    /// Final review M2: a plain click (no movement) on a handle's outer half, where it lies over a neighbouring card
    /// drawn earlier, ends the gesture: it links nothing.
    #[test]
    fn a_click_on_a_handle_over_a_neighbour_links_nothing() {
        let (h, mut app, [a, b, _]) = three_notes();
        // b's left edge 2 screen points right of a's right edge: b's left handle's outer half lies over a.
        let (zoom, _) = app.graph_world_to_screen_transform(app.graph_scene_outer_rect());
        let (ra, rb) = (app.card_rect(&a).unwrap(), app.card_rect(&b).unwrap());
        let b_centre = egui::pos2(ra.right() + 2.0 / zoom + rb.width() / 2.0, ra.center().y);
        app.snap_layout(&b, b_centre.to_vec2());
        h.frame(&mut app, vec![]);
        let spot = handle_on_screen(&app, &b, HandleSide::Left) - egui::vec2(4.0, 0.0);
        assert!(screen_rect(&app, &a).contains(spot) && !screen_rect(&app, &b).contains(spot), "over a, off b");
        let on_b = screen_rect(&app, &b).center();
        h.frame(&mut app, vec![egui::Event::PointerMoved(on_b)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(spot)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(spot)]);
        h.frame(&mut app, vec![press(spot, true)]);
        assert_eq!(app.connect_drag.as_deref(), Some(b.as_str()), "the press is b's handle's");
        h.frame(&mut app, vec![press(spot, false)]);
        h.frame(&mut app, vec![]);
        assert!(app.links_naming(&b).is_empty(), "a click on the handle links nothing");
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
    }

    /// Final review M3: a release over an interactable egui Area (a popup) links nothing under it, as a click there
    /// reaches no card.
    #[test]
    fn a_release_over_a_popup_links_nothing_under_it() {
        let (h, mut app, [a, b, _]) = three_notes();
        let on_b = screen_rect(&app, &b).center();
        h.cover.set(Some(egui::Rect::from_center_size(on_b, egui::vec2(60.0, 60.0))));
        h.frame(&mut app, vec![]);
        h.frame(&mut app, vec![]);
        assert_eq!(h.ctx.layer_id_at(on_b).map(|l| l.order), Some(egui::Order::Foreground), "the cover is on top");
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, on_b, vec![]);
        assert!(app.links_naming(&a).is_empty());
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
    }
}
