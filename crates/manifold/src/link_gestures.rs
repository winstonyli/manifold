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

/// The hit tolerance in world units at `inv_zoom`: 6 screen points, or half the hovered line if that were wider.
pub(crate) fn link_hit_tolerance(inv_zoom: f32) -> f32 {
    LINK_HIT_PX.max(LINK_HOVER_STROKE_PX / 2.0) * inv_zoom
}

/// The link line nearest `at` (world) within `tolerance`, or none. Opened-from lines are never taken. Of equals the
/// last wins: `level_edges` lists links in id order, so on one pair that is the newest within a launch.
pub(crate) fn nearest_link(edges: &[LevelEdge], at: egui::Pos2, tolerance: f32) -> Option<&LevelEdge> {
    let mut best: Option<(f32, &LevelEdge)> = None;
    for edge in edges.iter().filter(|e| e.link.is_some()) {
        let d = distance_to_segment(at, edge.from, edge.to);
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

/// A handle's diameter in screen points.
pub(crate) const HANDLE_PX: f32 = 12.0;
/// The shortest on-screen card edge that shows its handle. A top or right handle clears the 16-point Audio control
/// in the top-right corner (`CardControl::rect`) only while half the edge is at least 16 + 6 points.
pub(crate) const HANDLE_MIN_EDGE_PX: f32 = 44.0;

/// Card `card`'s handles (world space) at `zoom_scale`, each `HANDLE_PX` on screen, in `HandleSide::ALL` order. A side
/// whose edge is under `HANDLE_MIN_EDGE_PX` on screen has none, and a `simplified` level (`graph_lod_simplified`) has
/// none at all.
pub(crate) fn link_handle_rects(card: egui::Rect, zoom_scale: f32, simplified: bool) -> Vec<(HandleSide, egui::Rect)> {
    if simplified {
        return Vec::new();
    }
    let size = egui::Vec2::splat(HANDLE_PX / zoom_scale);
    HandleSide::ALL
        .into_iter()
        .filter_map(|side| {
            let (centre, edge) = match side {
                HandleSide::Top => (card.center_top(), card.width()),
                HandleSide::Right => (card.right_center(), card.height()),
                HandleSide::Bottom => (card.center_bottom(), card.width()),
                HandleSide::Left => (card.left_center(), card.height()),
            };
            (edge * zoom_scale >= HANDLE_MIN_EDGE_PX).then(|| (side, egui::Rect::from_center_size(centre, size)))
        })
        .collect()
}

/// A handle: an accent ring, filled while hovered or dragged from.
pub(crate) fn paint_link_handle(ui: &egui::Ui, handle: egui::Rect, active: bool, inv_zoom: f32) {
    let fill = if active { crate::theme::ACCENT } else { crate::theme::BG };
    let stroke = egui::Stroke::new(1.5 * inv_zoom, crate::theme::ACCENT);
    ui.painter().circle(handle.center(), handle.width() / 2.0, fill, stroke);
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
    /// animation aims at no line, though one may be under the pointer by its release.
    pub(crate) fn line_click(&mut self, ui: &egui::Ui, line_hover: Option<String>, clicked: bool) -> Option<String> {
        let (pressed, released) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
        if pressed {
            let still = self.camera.momentum.is_none() && self.camera.tween.is_none();
            self.line_press = line_hover.clone().filter(|_| still).map(|link| (link, self.camera.rect));
        }
        let press = if released { self.line_press.take() } else { None };
        let (link, rect) = press.filter(|_| clicked)?;
        (line_hover.as_ref() == Some(&link) && rect == self.camera.rect).then_some(link)
    }

    /// A click on link line `link`: deletes exactly that Link node, as the node menu's Disconnect row does, and saves.
    pub(crate) fn click_link_line(&mut self, link: &str) {
        if link.parse::<NodeId>().is_ok_and(|id| self.mirror.is_link(id)) {
            self.delete_link(link);
            self.save_session();
        }
    }

    /// Whether card `id` shows its connect-drag handles while hovered. During a connect-drag only its source does.
    /// Otherwise only when nothing else owns the pointer (as `line_click_allowed`) and the card has a valid link
    /// target at this level, which also makes it a valid link end.
    pub(crate) fn shows_link_handles(&self, id: &str) -> bool {
        if let Some(source) = &self.connect_drag {
            return source == id;
        }
        self.line_click_allowed() && self.has_valid_target(crate::caps::LINK, id)
    }

    /// A press on one of card `id`'s handles: pick-target mode for `link` from it, which rings the targets, run as a
    /// drag. Like any press on the graph it stops a glide ("grab it to stop it"), which `track_pan_momentum` skips
    /// during a connect-drag, so the cards hold still under the rubber band.
    pub(crate) fn start_connect_drag(&mut self, id: &str) {
        self.connecting = Some((id.to_string(), crate::caps::LINK));
        self.connect_drag = Some(id.to_string());
        self.camera.momentum = None;
    }

    /// The topmost card under world point `at`. `level_items` is bottom-first, as right-click's hit-test reads it.
    pub(crate) fn card_at(&self, at: egui::Pos2) -> Option<String> {
        self.level_items()
            .into_iter()
            .rev()
            .find(|item| matches!(item.kind, crate::level::LevelItemKind::Node(_)) && item.rect.contains(at))
            .map(|item| item.id)
    }

    /// The pointer in world space, inside `build_graph_ui`'s Scene (this frame's layer transform).
    fn scene_pointer(ui: &egui::Ui) -> Option<egui::Pos2> {
        let pointer = ui.input(|i| i.pointer.interact_pos())?;
        Some(ui.ctx().layer_transform_from_global(ui.layer_id()).map_or(pointer, |t| t * pointer))
    }

    /// During a connect-drag, the card a release at screen point `pointer` would link to: the topmost card other than
    /// the source under it, inside `scene_outer_rect`, off the `minimap` (painted over the Scene, not a widget) and
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
        self.card_at(((pointer.to_vec2() - translation) / zoom_scale).to_pos2()).filter(|target| target != source)
    }

    /// The rubber band: a dashed accent line from the connect-drag's source centre to the pointer, over the cards.
    pub(crate) fn draw_connect_rubber_band(&self, ui: &egui::Ui, inv_zoom: f32) {
        let (Some(source), Some(pointer)) = (&self.connect_drag, Self::scene_pointer(ui)) else { return };
        let stroke = egui::Stroke::new(LINK_STROKE_PX * inv_zoom, crate::theme::ACCENT);
        let line = [self.layout_pos(source).to_pos2(), pointer];
        ui.painter().add(egui::Shape::dashed_line(&line, stroke, 6.0 * inv_zoom, 4.0 * inv_zoom));
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

    fn edge(link: Option<&str>, y: f32) -> LevelEdge {
        LevelEdge {
            from: egui::pos2(0.0, y),
            to: egui::pos2(100.0, y),
            from_id: "x".into(),
            to_id: "y".into(),
            curve: if link.is_some() { EdgeCurve::Straight } else { EdgeCurve::Bow },
            highlight: false,
            kind: if link.is_some() { EdgeKind::Link } else { EdgeKind::Opened },
            link: link.map(str::to_string),
        }
    }

    #[test]
    fn nearest_link_takes_the_closest_link_line_within_tolerance() {
        let edges = [edge(None, 0.0), edge(Some("far"), 5.0), edge(Some("near"), 2.0)];
        let at = egui::pos2(50.0, 0.0);
        assert_eq!(nearest_link(&edges, at, 6.0).and_then(|e| e.link.as_deref()), Some("near"), "opened-from never");
        assert!(nearest_link(&edges, egui::pos2(50.0, 20.0), 6.0).is_none(), "nothing within 6");
        let twins = [edge(Some("older"), 1.0), edge(Some("newer"), 1.0)];
        assert_eq!(nearest_link(&twins, at, 6.0).and_then(|e| e.link.as_deref()), Some("newer"), "last of equals");
    }

    /// Drives `build_graph_ui` headless, as `a_click_finishes_or_cancels_pick_target_mode` (graph_ui.rs) does.
    struct Harness {
        ctx: egui::Context,
        /// An interactable Foreground Area drawn over the graph each frame (as a popup is), when set.
        cover: std::cell::Cell<Option<egui::Rect>>,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::apply_fonts(&ctx);
            Self { ctx, cover: std::cell::Cell::new(None) }
        }

        /// One frame at the size `graph_world_rect_to_screen` assumes with no window. Returns the cursor it asked for.
        fn frame(&self, app: &mut App, events: Vec<egui::Event>) -> egui::CursorIcon {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
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

    /// The middle of `edge`'s straight segment, on screen.
    fn middle(app: &App, edge: &LevelEdge) -> egui::Pos2 {
        app.graph_world_rect_to_screen(egui::Rect::from_two_pos(edge.from, edge.to)).center()
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
        // b's centre is the link line's end: the card is on top and takes the click.
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
        h.click(&mut app, on);
        assert!(app.links_naming(&a).is_empty());
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
    fn handles_sit_on_each_edge_midpoint_at_a_constant_screen_size() {
        let card = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(180.0, 140.0));
        let rects = link_handle_rects(card, 2.0, false);
        assert_eq!(rects.iter().map(|(side, _)| *side).collect::<Vec<_>>(), HandleSide::ALL);
        assert!(rects.iter().all(|(_, r)| r.width() == HANDLE_PX / 2.0), "12 screen points at zoom 2");
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
    fn handles_show_only_while_nothing_else_owns_the_pointer() {
        let (mut app, a, b) = two_notes();
        assert!(app.shows_link_handles(&a));
        app.dragging = Some(b.clone());
        assert!(!app.shows_link_handles(&a), "a card drag");
        app.dragging = None;
        app.node_menu = Some((b.clone(), egui::Pos2::ZERO));
        assert!(!app.shows_link_handles(&a), "a menu");
        app.node_menu = None;
        app.start_connect_drag(&a);
        assert!(app.shows_link_handles(&a) && !app.shows_link_handles(&b), "during a connect-drag only its source");
        app.cancel_connect();
        app.create_link(&a, &b);
        assert!(!app.shows_link_handles(&a), "linked to the only other card: no target left");
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

    /// a's right handle sits on the a-b line (centre to centre). A press and release there is the handle's: it records
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
        let (from, to) = (app.graph_world_rect_to_screen(egui::Rect::from_min_max(line.from, line.from)), line.to);
        let to = app.graph_world_rect_to_screen(egui::Rect::from_min_max(to, to));
        assert!(distance_to_segment(handle, from.min, to.min) < 1.0, "the handle is on the line: {handle:?} {line:?}");
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
        let h = Harness { ctx: app.egui_ctx.clone(), cover: std::cell::Cell::new(None) };
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
