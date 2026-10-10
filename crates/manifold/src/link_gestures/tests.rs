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
    assert_eq!(app.card_within(rb.center(), tol, &a), Some(b.clone()), "an exact hit on a refused card stays (toast)");
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
    app.link.connecting = Some((c.clone(), crate::caps::LINK));
    h.click(&mut app, on);
    assert!(app.link.connecting.is_none(), "the click ended pick mode, as any click off a target does");
    assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "and deleted nothing");
    app.menus.node_menu = Some((c.clone(), egui::pos2(5.0, 60.0)));
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
    assert!(app.menus.node_menu.is_none(), "no menu on c");
    assert_eq!(app.links_naming(&a).len(), 1, "linked to b");
    h.frame(&mut app, vec![right(on_b, false)]);
    drag_from_handle(&h, &mut app, &a, HandleSide::Bottom, on_c, right_press_at(canvas));
    assert!(app.interaction.rect_select_start.is_none(), "no marquee");
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
    assert!(app.menus.node_menu.is_none(), "no menu on c");
    h.frame(&mut app, vec![right(to, false)]);
    let from = screen_rect(&app, &a).center();
    h.drag_with(&mut app, from, from + egui::vec2(40.0, 0.0), right_press_at(canvas));
    assert!(app.interaction.rect_select_start.is_none(), "no marquee");
}

/// A drag that starts on empty canvas near a link, but off its curve, still pans.
#[test]
fn a_drag_from_empty_canvas_beside_a_link_line_pans_and_deletes_nothing() {
    let (h, mut app, [a, b, _]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let before = app.camera.rect;
    let beside = line_middle(&app, &ab) + egui::vec2(0.0, 30.0);
    h.drag(&mut app, beside, beside + egui::vec2(120.0, 80.0));
    assert_ne!(app.camera.rect.center(), before.center(), "the drag panned");
    assert_eq!(app.links_naming(&a), [ab]);
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none());
}

/// The screen point at `t` along `edge`'s curve.
fn point_on(app: &App, edge: &LevelEdge, t: f32) -> egui::Pos2 {
    let world = bow_point(edge.from, edge.to, t);
    app.graph_world_rect_to_screen(egui::Rect::from_min_max(world, world)).min
}

fn link_edge(app: &App, link: &str) -> LevelEdge {
    app.level_edges().into_iter().find(|e| e.link.as_deref() == Some(link)).expect("drawn")
}

/// Presses on `from` (hovered two frames first) and moves to `to` in eight steps, leaving the button down.
fn hold(h: &Harness, app: &mut App, from: egui::Pos2, to: egui::Pos2) {
    h.frame(app, vec![egui::Event::PointerMoved(from)]);
    h.frame(app, vec![egui::Event::PointerMoved(from)]);
    h.frame(app, vec![press(from, true)]);
    for step in 1..=8 {
        h.frame(app, vec![egui::Event::PointerMoved(from + (to - from) * (step as f32 / 8.0))]);
    }
}

/// `hold` from the point at `t` along `edge`'s curve.
fn hold_on(h: &Harness, app: &mut App, edge: &LevelEdge, t: f32, to: egui::Pos2) {
    let from = point_on(app, edge, t);
    hold(h, app, from, to);
}

fn release(h: &Harness, app: &mut App, at: egui::Pos2) {
    h.frame(app, vec![press(at, false)]);
    h.frame(app, vec![]);
}

fn ends(app: &App, link: &str) -> (Option<String>, Option<String>) {
    let (from, to) = app.mirror.link_ends(link);
    (from.map(|n| n.to_string()), to.map(|n| n.to_string()))
}

fn toasts(app: &App) -> Vec<&String> {
    app.toasts.save_toast.iter().map(|(m, _)| m).chain(&app.toasts.save_toast_queue).collect()
}

/// Dragging a curve takes the end nearer the grab: that end follows the pointer, the card at the other end is the
/// connect-drag's source and its end of the link stays.
#[test]
fn a_drag_on_a_curve_detaches_the_end_nearer_the_grab_point() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &c).center(), 0.5);
    hold_on(&h, &mut app, &edge, 0.15, canvas);
    assert_eq!(app.link.connect_drag.as_deref(), Some(b.as_str()), "grabbed near a: a's end comes off, b stays");
    assert_eq!(app.link.retarget, Some(LinkRetarget { link: ab.clone(), fixed_is_from: false }));
    assert_eq!(app.link.connecting, Some((b.clone(), crate::caps::LINK)), "ringing the targets of b");
    let on_a = screen_rect(&app, &a).center();
    release(&h, &mut app, on_a);
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none() && app.link.connecting.is_none());
    hold_on(&h, &mut app, &edge, 0.85, canvas);
    assert_eq!(app.link.connect_drag.as_deref(), Some(a.as_str()), "grabbed near b: b's end comes off, a stays");
    assert_eq!(app.link.retarget, Some(LinkRetarget { link: ab.clone(), fixed_is_from: true }));
    let on_b = screen_rect(&app, &b).center();
    release(&h, &mut app, on_b);
    assert_eq!(ends(&app, &ab), (Some(a), Some(b)), "both released on their own cards: untouched");
}

/// Dropped on a valid card, the grabbed end moves there in place: the Link node keeps its id and other end and
/// its direction, the old pair is no longer joined, and the new one is.
#[test]
fn dropping_a_dragged_curve_on_a_card_moves_that_end_there() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let on_c = screen_rect(&app, &c).center();
    hold_on(&h, &mut app, &edge, 0.85, on_c);
    assert!(app.link.retarget.is_some(), "mid-drag");
    release(&h, &mut app, on_c);
    assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "same Link node");
    assert_eq!(ends(&app, &ab), (Some(a.clone()), Some(c.clone())), "b's end moved to c");
    assert!(app.links_naming(&b).is_empty(), "b is no longer linked");
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none() && app.link.connecting.is_none());
    assert!(toasts(&app).is_empty());
    // The other end: grab near a, drop on b. a's end is the `from` end, so the link now runs b -> c.
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let on_b = screen_rect(&app, &b).center();
    hold_on(&h, &mut app, &edge, 0.15, on_b);
    release(&h, &mut app, on_b);
    assert_eq!(ends(&app, &ab), (Some(b), Some(c)), "the `from` end moved, `to` stayed");
    assert!(app.links_naming(&a).is_empty());
}

/// Released back on the card it came off, the link is as it was, silently.
#[test]
fn dropping_a_dragged_curve_on_its_old_card_leaves_the_link() {
    let (h, mut app, [a, b, _]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let on_b = screen_rect(&app, &b).center();
    hold_on(&h, &mut app, &edge, 0.85, on_b);
    release(&h, &mut app, on_b);
    assert_eq!(ends(&app, &ab), (Some(a.clone()), Some(b)), "back on its own card: nothing changes");
    assert_eq!(app.links_naming(&a), [ab]);
    assert!(toasts(&app).is_empty(), "silently");
}

/// Released on empty canvas, the dragged link is deleted, as a click on it would: gone from the mirror and from
/// the exported edges, and the drag did not pan.
#[test]
fn dropping_a_dragged_curve_on_empty_canvas_deletes_the_link() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &c).center(), 0.5);
    let before = app.camera.rect;
    hold_on(&h, &mut app, &edge, 0.85, canvas);
    release(&h, &mut app, canvas);
    assert!(app.links_naming(&a).is_empty() && app.links_naming(&b).is_empty(), "deleted");
    assert_eq!(app.camera.rect, before, "and the drag did not pan");
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none() && app.link.connecting.is_none());
    assert!(toasts(&app).is_empty(), "silently");
    h.frame(&mut app, vec![]);
    let state = app.export_state();
    assert!(state["edges"].as_array().unwrap().is_empty(), "{}", state["edges"]);
}

/// Losing window focus mid-drag cancels it: the button-up that follows moves nothing and deletes nothing.
#[test]
fn losing_focus_during_a_dragged_curve_keeps_the_link() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &c).center(), 0.5);
    hold_on(&h, &mut app, &edge, 0.85, canvas);
    assert!(app.link.retarget.is_some(), "mid-drag");
    app.on_window_focus(false);
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none() && app.link.connecting.is_none());
    release(&h, &mut app, canvas);
    assert_eq!(ends(&app, &ab), (Some(a), Some(b)), "untouched");
}

/// Releasing off the scene (the minimap, a popup, outside the graph) is no release over the canvas: the link stays.
#[test]
fn a_release_off_the_scene_keeps_the_link() {
    let (_h, mut app, [a, b, _]) = three_notes();
    let ab = app.create_link(&a, &b);
    app.start_connect_drag(&a);
    app.link.retarget = Some(LinkRetarget { link: ab.clone(), fixed_is_from: true });
    app.end_connect_drag(DragEnd::Abandoned);
    assert_eq!(ends(&app, &ab), (Some(a), Some(b)));
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none() && toasts(&app).is_empty());
}

/// Three notes and a running download d (a card no link may end on) to the right of b, with a -> b linked: the b
/// end is dragged out and released at `at(app, [a, b, c, d])`. Returns the app after the release.
fn drop_b_end_at(link_a_c: bool, at: impl Fn(&App, [&str; 4]) -> egui::Pos2) -> (App, [String; 4], String) {
    let (h, mut app, [a, b, c]) = three_notes();
    let d = app.alloc_node_id();
    let download = Runtime::Download(crate::node::DownloadPayload {
        path: "f.bin".into(),
        bytes_received: 0,
        total_bytes: None,
        completed: false,
        failed: false,
    });
    app.insert_placeholder_node(d.clone(), egui::vec2(1200.0, 0.0), Kind::Download, download);
    for _ in 0..3 {
        h.frame(&mut app, vec![]);
    }
    app.camera.jump_to(app.fit_all_nodes_scene_rect());
    h.frame(&mut app, vec![]);
    let ab = app.create_link(&a, &b);
    if link_a_c {
        app.create_link(&a, &c);
    }
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let to = at(&app, [&a, &b, &c, &d]);
    hold_on(&h, &mut app, &edge, 0.85, to);
    assert!(app.link.retarget.is_some(), "mid-drag");
    release(&h, &mut app, to);
    (app, [a, b, c, d], ab)
}

/// Only a release over truly empty canvas deletes: any card within the drop tolerance is where the end landed.
/// 10px outside its own old card: kept, silently.
#[test]
fn a_release_just_outside_the_ends_own_card_keeps_the_link() {
    let (app, [a, b, ..], ab) =
        drop_b_end_at(false, |app, [_, b, ..]| screen_rect(app, b).center_bottom() + egui::vec2(0.0, 10.0));
    assert_eq!(ends(&app, &ab), (Some(a), Some(b)));
    assert!(toasts(&app).is_empty(), "silently");
}

/// 10px outside a card the link may not end on (a running download): kept, with the usual toast.
#[test]
fn a_release_just_outside_a_refused_card_keeps_the_link_and_toasts() {
    let (app, [a, b, _, d], ab) =
        drop_b_end_at(false, |app, [.., d]| screen_rect(app, d).left_center() - egui::vec2(10.0, 0.0));
    assert!(app.is_running_download(&d));
    assert_eq!(ends(&app, &ab), (Some(a), Some(b)));
    assert!(toasts(&app).iter().any(|m| m.as_str() == "Can't connect these cards"), "{:?}", toasts(&app));
}

/// 10px outside the fixed card itself (a self link): refused, kept, with the toast.
#[test]
fn a_release_just_outside_the_fixed_card_keeps_the_link_and_toasts() {
    let (app, [a, b, ..], ab) =
        drop_b_end_at(false, |app, [a, ..]| screen_rect(app, a).left_center() - egui::vec2(10.0, 0.0));
    assert_eq!(ends(&app, &ab), (Some(a), Some(b)));
    assert!(toasts(&app).iter().any(|m| m.as_str() == "Can't connect these cards"), "{:?}", toasts(&app));
}

/// 10px outside a card already joined to the fixed card: the link is deleted, a -> c stays.
#[test]
fn a_release_just_outside_an_already_linked_card_deletes_the_link() {
    let (app, [a, _, c, _], ab) =
        drop_b_end_at(true, |app, [_, _, c, _]| screen_rect(app, c).center_bottom() + egui::vec2(0.0, 10.0));
    assert!(app.mirror.link_ends(&ab) == (None, None) || !app.links_naming(&a).contains(&ab), "deleted");
    assert_eq!(app.links_naming(&a).len(), 1);
    assert!(app.links_naming(&c).len() == 1 && toasts(&app).is_empty());
}

/// Further than the tolerance from every card: empty canvas, deleted.
#[test]
fn a_release_beyond_the_drop_tolerance_of_every_card_deletes_the_link() {
    let (app, [a, ..], ab) = drop_b_end_at(false, |app, [_, b, ..]| {
        screen_rect(app, b).center_bottom() + egui::vec2(0.0, DROP_TOLERANCE_PX + 16.0)
    });
    assert!(!app.links_naming(&a).contains(&ab), "deleted");
    assert!(toasts(&app).is_empty());
}

/// Mid-drag, EXPORTSTATE's `retarget_preview` follows the pointer: a card, empty canvas, the fixed card, the old card.
#[test]
fn the_live_preview_follows_the_pointer_and_is_exported() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    assert!(app.export_state()["retarget_preview"].is_null(), "no drag");
    let edge = link_edge(&app, &ab);
    let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &c).center(), 0.5);
    hold_on(&h, &mut app, &edge, 0.85, canvas);
    let preview_at = |app: &mut App, at: egui::Pos2| {
        h.frame(app, vec![egui::Event::PointerMoved(at)]);
        h.frame(app, vec![egui::Event::PointerMoved(at)]);
        app.export_state()["retarget_preview"].as_str().map(str::to_string)
    };
    assert_eq!(preview_at(&mut app, canvas).as_deref(), Some("delete"));
    let (on_c, on_a, on_b) =
        (screen_rect(&app, &c).center(), screen_rect(&app, &a).center(), screen_rect(&app, &b).center());
    assert_eq!(preview_at(&mut app, on_c).as_deref(), Some("retarget"));
    assert_eq!(preview_at(&mut app, on_a).as_deref(), Some("refused"));
    assert_eq!(preview_at(&mut app, on_b).as_deref(), Some("cancel"));
    release(&h, &mut app, on_b);
    assert!(app.export_state()["retarget_preview"].is_null(), "drag over");
}

/// Each outcome has its own look: accent for a retarget or a cancel (a weak-text ring for the cancel), a thin red
/// band with an "x" for a deletion, blue for a refusal; the outcomes differ by hue, not brightness.
#[test]
fn each_retarget_outcome_has_its_own_look() {
    use crate::links::RetargetOutcome::*;
    use crate::theme::{refused_color, ACCENT, DANGER_COLOR, TEXT_WEAK};
    let look = retarget_look;
    assert_eq!(look(Retargeted), RetargetLook { band: ACCENT, band_px: LINK_STROKE_PX, glyph: None, ring: ACCENT });
    assert_eq!(look(Cancelled).band, ACCENT);
    assert_eq!(look(Cancelled).ring, TEXT_WEAK, "an own-card ring promises no retarget");
    assert_eq!(
        (look(Refused).band, look(Refused).ring, look(Refused).glyph),
        (refused_color(), refused_color(), Some(RetargetGlyph::Prohibited))
    );
    let deleted = look(Deleted);
    assert_eq!((deleted.band, deleted.ring, deleted.glyph), (DANGER_COLOR, DANGER_COLOR, Some(RetargetGlyph::Cross)));
    assert_ne!(RetargetGlyph::Cross, RetargetGlyph::Prohibited);
    let glyphs: Vec<_> = [Retargeted, Cancelled, Refused, Deleted].map(|o| look(o).glyph.is_some()).to_vec();
    assert_eq!(glyphs, [false, false, true, true], "only the deleted and the refused carry a mark");
    assert!(deleted.band_px < LINK_STROKE_PX, "thinner");
    let rings = [look(Retargeted).ring, look(Cancelled).ring, look(Refused).ring, look(Deleted).ring];
    for (i, a) in rings.iter().enumerate() {
        assert!(rings.iter().skip(i + 1).all(|b| a != b), "four distinct rings");
    }
    // Bands (Cancelled shares the accent band with Retargeted by design): pairwise distinct, and the refusal's
    // hue is far from both others. ACCENT (gold) and DANGER_COLOR (rust) are existing theme constants only about
    // 25 degrees apart, so that pair also has the thin band and the "x" to tell it apart.
    let hue = |c: egui::Color32| egui::ecolor::Hsva::from(c).h * 360.0;
    let apart = |a: egui::Color32, b: egui::Color32| {
        let d = (hue(a) - hue(b)).abs();
        d.min(360.0 - d)
    };
    let bands = [look(Retargeted).band, look(Refused).band, look(Deleted).band];
    assert!(bands[0] != bands[1] && bands[1] != bands[2] && bands[0] != bands[2], "{bands:?}");
    assert!(apart(bands[1], bands[0]) >= 40.0, "refused vs retarget: {}", apart(bands[1], bands[0]));
    assert!(apart(bands[1], bands[2]) >= 40.0, "refused vs delete: {}", apart(bands[1], bands[2]));
    assert!(apart(bands[0], bands[2]) >= 20.0, "retarget vs delete: {}", apart(bands[0], bands[2]));
}

/// The preview for a release position is the outcome the release then has, for all five ways it can end.
#[test]
fn the_preview_equals_the_release_outcome_for_every_kind_of_end() {
    use crate::links::RetargetOutcome::*;
    let cases = [
        ("a valid card", Retargeted, 3usize),
        ("empty canvas", Deleted, 5),
        ("an already-linked card", Deleted, 2),
        ("the fixed card", Refused, 0),
        ("its own old card", Cancelled, 1),
    ];
    for (what, expected, pick) in cases {
        let (_h, mut app, [a, b, c]) = three_notes();
        let d = app.alloc_node_id();
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(d.clone(), egui::vec2(600.0, 400.0), Kind::Note, note);
        let ab = app.create_link(&a, &b);
        app.create_link(&a, &c);
        let end = match pick {
            0 => DragEnd::Source,
            1 => DragEnd::Card(b.clone()),
            2 => DragEnd::Card(c.clone()),
            3 => DragEnd::Card(d.clone()),
            _ => DragEnd::Canvas,
        };
        app.start_connect_drag(&a);
        app.link.retarget = Some(LinkRetarget { link: ab.clone(), fixed_is_from: true });
        let preview = app.retarget_preview(&end);
        assert_eq!(preview, Some(expected), "preview over {what}");
        app.end_connect_drag(end);
        let kept = ends(&app, &ab);
        let toasted = !toasts(&app).is_empty();
        let done = match (kept.1.as_deref(), app.links_naming(&a).contains(&ab), toasted) {
            (_, false, _) => Deleted,
            (Some(to), true, _) if to == d => Retargeted,
            (_, true, true) => Refused,
            (_, true, false) => Cancelled,
        };
        assert_eq!(Some(done), preview, "release over {what} did what the preview said");
    }
}

/// With no retarget running there is no preview.
#[test]
fn a_plain_connect_drag_has_no_retarget_preview() {
    let (mut app, a, _) = two_notes();
    app.start_connect_drag(&a);
    assert_eq!(app.retarget_preview(&DragEnd::Canvas), None);
}

/// A pair the link cannot take (here another level) and the fixed end's own card refuse and toast, leaving the link.
#[test]
fn a_release_the_link_cannot_take_keeps_it_and_toasts() {
    let (_h, mut app, [a, b, c]) = three_notes();
    let g = crate::session_sync::tests::add_group(&mut app, "g");
    app.edit_group(&c, Some(g));
    let ab = app.create_link(&a, &b);
    for end in [DragEnd::Card(c.clone()), DragEnd::Source] {
        app.start_connect_drag(&a);
        app.link.retarget = Some(LinkRetarget { link: ab.clone(), fixed_is_from: true });
        app.end_connect_drag(end);
        assert_eq!(ends(&app, &ab), (Some(a.clone()), Some(b.clone())), "kept");
        assert!(toasts(&app).iter().any(|m| m.as_str() == "Can't connect these cards"), "{:?}", toasts(&app));
        app.toasts.save_toast_queue.clear();
        app.toasts.save_toast = None;
    }
}

#[test]
fn escape_cancels_a_dragged_curve() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let escape = egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let on_c = screen_rect(&app, &c).center();
    let before = app.camera.rect;
    hold_on(&h, &mut app, &edge, 0.85, on_c);
    assert!(app.link.retarget.is_some(), "mid-drag");
    h.frame(&mut app, vec![escape]);
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none() && app.link.connecting.is_none());
    // The button is still down: moving on must not pan the canvas the press began on.
    for step in 1..=4 {
        h.frame(&mut app, vec![egui::Event::PointerMoved(on_c + egui::vec2(25.0 * step as f32, 10.0))]);
    }
    assert_eq!(app.camera.rect, before, "no pan after Esc");
    release(&h, &mut app, on_c);
    assert_eq!(ends(&app, &ab), (Some(a), Some(b)), "released on c after Esc: untouched");
    assert!(toasts(&app).is_empty());
}

/// The link being retargeted is left out of what is drawn, every other edge is kept.
#[test]
fn the_retargeted_link_is_not_drawn() {
    let mut edges = vec![edge(Some("1"), 0.0), edge(Some("2"), 10.0), edge(None, 20.0)];
    without_retargeted(&mut edges, None);
    assert_eq!(edges.len(), 3, "no retarget: all drawn");
    let retarget = LinkRetarget { link: "2".into(), fixed_is_from: true };
    without_retargeted(&mut edges, Some(&retarget));
    let left: Vec<_> = edges.iter().map(|e| e.link.as_deref()).collect();
    assert_eq!(left, [Some("1"), None]);
}

/// A link deleted mid-drag (say by sync) leaves nothing to move: the release cancels without a toast or a new link.
#[test]
fn releasing_a_dragged_curve_whose_link_was_deleted_cancels_silently() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let on_c = screen_rect(&app, &c).center();
    hold_on(&h, &mut app, &edge, 0.85, on_c);
    assert!(app.link.retarget.is_some(), "mid-drag");
    app.delete_link(&ab);
    release(&h, &mut app, on_c);
    assert!(toasts(&app).is_empty(), "{:?}", toasts(&app));
    assert!(app.links_naming(&a).is_empty() && app.links_naming(&c).is_empty());
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none() && app.link.connecting.is_none());
}

/// A press on the curve that wobbles under egui's drag threshold is still a click: it deletes the link.
#[test]
fn a_jittery_click_on_a_curve_still_deletes_the_link() {
    let (h, mut app, [a, b, _]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let on = line_middle(&app, &ab);
    h.drag(&mut app, on, on + egui::vec2(3.0, 0.0));
    assert!(app.links_naming(&a).is_empty(), "deleted");
    assert!(app.link.retarget.is_none());
}

/// A target already joined to the fixed card (here a already joins c) deletes the dragged link, and only it.
#[test]
fn dropping_a_dragged_curve_on_an_already_linked_card_deletes_the_link() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    let ac = app.create_link(&a, &c);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let on_c = screen_rect(&app, &c).center();
    hold_on(&h, &mut app, &edge, 0.85, on_c);
    release(&h, &mut app, on_c);
    assert_eq!(app.links_naming(&a), std::slice::from_ref(&ac), "the dragged link is gone, the other stays");
    assert!(app.links_naming(&b).is_empty());
    assert_eq!(ends(&app, &ac), (Some(a), Some(c)));
    assert!(toasts(&app).is_empty(), "silently");
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none());
}

/// The drop tolerance and the valid-only tolerance pass are the connect-drag's own: a release just outside c links.
#[test]
fn a_dragged_curve_snaps_to_a_card_within_the_drop_tolerance() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let edge = link_edge(&app, &ab);
    let near_c = screen_rect(&app, &c).center_bottom() + egui::vec2(0.0, DROP_TOLERANCE_PX - 4.0);
    hold_on(&h, &mut app, &edge, 0.85, near_c);
    release(&h, &mut app, near_c);
    assert_eq!(ends(&app, &ab), (Some(a), Some(c)));
}

/// A click on the curve without moving still deletes the link, and a curve drag does not.
#[test]
fn a_click_on_a_curve_still_deletes_and_a_drag_from_it_does_not() {
    let (h, mut app, [a, b, _]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let on = line_middle(&app, &ab);
    let back_on_a = screen_rect(&app, &a).center();
    h.drag(&mut app, on, back_on_a);
    assert_eq!(app.links_naming(&a).len(), 1, "a drag back onto its card deletes nothing");
    let on = line_middle(&app, &ab);
    h.click(&mut app, on);
    assert!(app.links_naming(&a).is_empty(), "a click does");
}

/// A drag from a handle whose outer half lies over a link's curve is the handle's: a connect-drag from that
/// card, no retarget, and the link it lay over is untouched.
#[test]
fn a_drag_from_a_handle_over_a_curve_is_the_handles_not_a_retarget() {
    let (h, mut app, [a, b, c]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let handle = handle_on_screen(&app, &a, HandleSide::Right) + egui::vec2(4.0, 0.0);
    let centre = screen_rect(&app, &a).center();
    h.frame(&mut app, vec![egui::Event::PointerMoved(centre)]);
    let on_c = screen_rect(&app, &c).center();
    h.drag(&mut app, handle, on_c);
    assert_eq!(ends(&app, &ab), (Some(a.clone()), Some(b)), "the link under the handle is untouched");
    assert_eq!(app.links_naming(&a).len(), 2, "the handle drag linked a to c");
    assert!(app.link.retarget.is_none());
}

/// Opened-from lines are not draggable: the drag pans as it always did.
#[test]
fn an_opened_from_line_is_not_draggable() {
    let (h, mut app, [a, _, c]) = three_notes();
    app.edit_parent(&c, Some(a.clone()));
    h.frame(&mut app, vec![]);
    let opened = app.level_edges().into_iter().find(|e| e.kind == EdgeKind::Opened).expect("c opened from a");
    let on = middle(&app, &opened);
    let before = app.camera.rect;
    h.drag(&mut app, on, on + egui::vec2(100.0, 0.0));
    assert!(app.link.connect_drag.is_none() && app.link.retarget.is_none());
    assert_ne!(app.camera.rect.center(), before.center(), "it panned");
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()));
}

/// Review I1: Recently Closed and the shortcuts help close on a click outside them without consuming it, so the
/// dismissing click must not also take the line under it.
#[test]
fn a_click_dismissing_recently_closed_or_the_shortcuts_help_deletes_nothing() {
    let (h, mut app, [a, b, _]) = three_notes();
    let ab = app.create_link(&a, &b);
    h.frame(&mut app, vec![]);
    let on = line_middle(&app, &ab);
    app.menus.open_popup = Some(crate::popups::AnchoredPopup::RecentlyClosed);
    h.frame(&mut app, vec![]);
    h.click(&mut app, on);
    assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "Recently Closed was open");
    app.menus.open_popup = None;
    app.menus.shortcuts_help_open = true;
    h.frame(&mut app, vec![]);
    h.click(&mut app, on);
    assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "the shortcuts help was open");
    app.menus.shortcuts_help_open = false;
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
    app.interaction.dragging = Some(b.clone());
    assert!(!app.shows_link_handles(), "a card drag");
    app.interaction.dragging = None;
    app.menus.node_menu = Some((b.clone(), egui::Pos2::ZERO));
    assert!(!app.shows_link_handles(), "a menu");
    app.menus.node_menu = None;
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
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none(), "cancelled");
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
    assert_eq!(app.link.connect_drag.as_deref(), Some(a.as_str()));
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
    assert_eq!(app.link.connecting, Some((a.clone(), crate::caps::LINK)), "it runs as pick-target mode for link");
    app.cancel_connect();
    assert!(app.link.connect_drag.is_none());
    app.start_connect_drag(&a);
    app.forget_connect_source(&a);
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none(), "its source removed");
    app.start_connect_drag(&a);
    app.end_stale_connect(false);
    assert!(app.link.connect_drag.is_none(), "Graph view left");
    app.start_connect_drag(&a);
    app.link.connecting = None;
    app.end_stale_connect(true);
    assert!(app.link.connect_drag.is_none(), "a connect-drag without its mode is stale");
    app.start_connect_drag(&a);
    app.end_connect_drag(DragEnd::Canvas);
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none() && app.links_naming(&a).is_empty());
    app.start_connect_drag(&a);
    app.end_connect_drag(DragEnd::Card(b.clone()));
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none());
    assert_eq!(app.links_naming(&a).len(), 1, "released on b");
}

fn handle_on_screen(app: &App, id: &str, side: HandleSide) -> egui::Pos2 {
    let (zoom, _) = app.graph_world_to_screen_transform(app.graph_scene_outer_rect());
    let rects = link_handle_rects(app.card_rect(id).expect("a card"), zoom, false);
    let (_, rect) = rects.into_iter().find(|(s, _)| *s == side).expect("shown at this zoom");
    app.graph_world_rect_to_screen(rect).center()
}

/// Hovers card `id` (so its handles show), then drags from its `side` handle to `to`.
fn drag_from_handle(h: &Harness, app: &mut App, id: &str, side: HandleSide, to: egui::Pos2, midway: Vec<egui::Event>) {
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
    assert_eq!(app.rubber_band_ends(&a, pointer, None), Some((ra.right_center(), pointer)), "no target: the pointer");
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
    assert!(app.interaction.dragging.is_none() && app.link.connect_drag.is_none() && app.link.connecting.is_none());
}

#[test]
fn a_handle_drag_released_on_canvas_or_on_its_source_links_nothing_silently() {
    let (h, mut app, [a, b, _]) = three_notes();
    let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &b).center(), 0.5);
    drag_from_handle(&h, &mut app, &a, HandleSide::Right, canvas, vec![]);
    let own = screen_rect(&app, &a).center();
    drag_from_handle(&h, &mut app, &a, HandleSide::Right, own, vec![]);
    assert!(app.links_naming(&a).is_empty());
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none());
    assert!(app.toasts.save_toast.is_none() && app.toasts.save_toast_queue.is_empty(), "no toast");
}

#[test]
fn a_handle_drag_onto_a_card_already_linked_says_so() {
    let (h, mut app, [a, b, _]) = three_notes();
    app.create_link(&a, &b);
    let onto = screen_rect(&app, &b).center();
    drag_from_handle(&h, &mut app, &a, HandleSide::Right, onto, vec![]);
    assert_eq!(app.links_naming(&a).len(), 1);
    let toasts: Vec<&String> =
        app.toasts.save_toast.iter().map(|(m, _)| m).chain(&app.toasts.save_toast_queue).collect();
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
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none());
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
    assert_eq!(app.link.connect_drag.as_deref(), Some(a.as_str()), "the press started it");
    h.frame(&mut app, vec![press(handle, false)]);
    h.frame(&mut app, vec![]);
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none() && app.links_naming(&a).is_empty());
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
    assert_eq!(app.link.connect_drag.as_deref(), Some(a.as_str()));
    assert!(app.link.line_press.is_none(), "the handle's press is not a line press");
    h.frame(&mut app, vec![press(handle, false)]);
    h.frame(&mut app, vec![]);
    assert_eq!(app.links_naming(&a), std::slice::from_ref(&ab), "the line under the handle stays");
    let on = line_middle(&app, &ab);
    drag_from_handle(&h, &mut app, &c, HandleSide::Top, on, vec![]);
    assert_eq!(app.links_naming(&a), [ab]);
    assert!(app.links_naming(&c).is_empty() && app.link.connect_drag.is_none() && app.link.line_press.is_none());
}

#[test]
fn a_drag_on_the_card_body_still_moves_the_card() {
    let (h, mut app, [a, _, _]) = three_notes();
    let before = app.layout_pos(&a);
    let from = screen_rect(&app, &a).center();
    h.drag(&mut app, from, from + egui::vec2(-60.0, 0.0));
    assert_ne!(app.layout_pos(&a), before, "the card moved");
    assert!(app.link.connect_drag.is_none() && app.links_naming(&a).is_empty());
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
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none());
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
        app.connect_drag_end(&h.ctx, Some(over), outer, None, transform),
        DragEnd::Card(b.clone()),
        "without it: b"
    );
    assert_eq!(
        app.connect_drag_end(&h.ctx, Some(over), outer, Some(minimap), transform),
        DragEnd::Abandoned,
        "not ringed"
    );
    app.cancel_connect();
    drag_from_handle(&h, &mut app, &a, HandleSide::Right, over, vec![]);
    assert!(app.links_naming(&a).is_empty(), "not linked");
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none());
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
    assert!(app.link.connect_drag.is_none() && app.links_naming(&a).is_empty());
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
    assert_eq!(app.link.connect_drag.as_deref(), Some(a.as_str()));
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
    assert_eq!(app.link.connect_drag.as_deref(), Some(a.as_str()));
    app.on_window_focus(false);
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none() && app.link.line_press.is_none());
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
    let h = Harness { ctx: app.egui_ctx.clone(), cover: std::cell::Cell::new(None), clock: std::cell::Cell::new(0.0) };
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
    assert_eq!(app.interaction.dragging.as_deref(), Some(note.as_str()), "mid-drag");
    assert!(
        matches!(app.drop_target(&note, Some(over_group), None), Some((crate::card::DropEffect::Join, _))),
        "a release here would join the group"
    );
    let held = app.layout_pos(&note);

    app.on_window_focus(false);
    assert!(app.interaction.dragging.is_none() && app.interaction.drag_moved.is_empty(), "the drag ended");
    assert_eq!(app.mirror.pos(&note), Some(held), "saved where it was");
    assert_eq!(app.group_of(&Some(note.clone())), None, "no drop: it joined nothing");

    // The button-up went to another window: egui still has the button down while the pointer moves on.
    for step in 1..=4 {
        h.frame(&mut app, vec![egui::Event::PointerMoved(over_group + egui::vec2(0.0, 30.0 * step as f32))]);
    }
    assert_eq!(app.layout_pos(&note), held, "the card no longer follows the pointer");
    assert!(app.interaction.dragging.is_none());
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
    app.link.line_press = Some((a.clone(), app.camera.rect));
    app.start_connect_drag(&a);
    app.switch_group_level(None);
    assert!(app.link.line_press.is_none() && app.link.connect_drag.is_none());
}

/// Final review I2: a connect-drag whose button-up was lost (the button is up, yet no release came this frame)
/// cancels, so the next ordinary click on a card links nothing.
#[test]
fn a_connect_drag_without_its_button_down_cancels() {
    let (h, mut app, [a, b, _]) = three_notes();
    let on_b = screen_rect(&app, &b).center();
    app.start_connect_drag(&a);
    h.frame(&mut app, vec![egui::Event::PointerMoved(on_b)]);
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none(), "no button down: cancelled");
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
    assert_eq!(app.link.connect_drag.as_deref(), Some(b.as_str()), "the press is b's handle's");
    h.frame(&mut app, vec![press(spot, false)]);
    h.frame(&mut app, vec![]);
    assert!(app.links_naming(&b).is_empty(), "a click on the handle links nothing");
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none());
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
    assert!(app.link.connect_drag.is_none() && app.link.connecting.is_none());
}
