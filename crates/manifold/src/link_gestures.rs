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

impl App {
    /// Whether a click on a link line may delete it now. Not in a JSON level, and not while a card drag, pick-target
    /// mode, a card or new-node menu, the right-drag marquee, or a popup that a click outside dismisses (Recently
    /// Closed and the other anchored popups, the shortcuts help) owns the pointer: that click only dismisses.
    pub(crate) fn line_click_allowed(&self) -> bool {
        self.json_level.is_none()
            && self.dragging.is_none()
            && self.connecting.is_none()
            && self.node_menu.is_none()
            && self.pending_new_node.is_none()
            && self.rect_select_start.is_none()
            && self.open_popup.is_none()
            && !self.shortcuts_help_open
    }

    /// The link line under the pointer. Called inside `build_graph_ui`'s Scene, where `ui.response()` is the Scene's
    /// own background: hovered only when no card, control or popup is on top and nothing is being dragged. Off the
    /// minimap (`minimap`, screen points), which is painted, not a widget. Sets the pointing-hand cursor when there is
    /// a line, so a click is never a surprise.
    pub(crate) fn hovered_link_line(
        &self,
        ui: &egui::Ui,
        edges: &[LevelEdge],
        inv_zoom: f32,
        minimap: Option<egui::Rect>,
    ) -> Option<String> {
        if !self.line_click_allowed() {
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
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::apply_fonts(&ctx);
            Self { ctx }
        }

        /// One frame at the size `graph_world_rect_to_screen` assumes with no window. Returns the cursor it asked for.
        fn frame(&self, app: &mut App, events: Vec<egui::Event>) -> egui::CursorIcon {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| app.build_graph_ui(ui));
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
}
