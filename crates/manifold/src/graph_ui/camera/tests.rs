use super::*;

#[test]
fn the_target_is_where_an_ease_is_going() {
    let mut camera = GraphCamera::new();
    let to = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(50.0, 50.0));
    assert_eq!(camera.target(), camera.rect);
    camera.ease_to(to);
    assert_eq!(camera.target(), to);
}

/// A reopened card the layout put out of view is eased into it; one already in view leaves the camera alone.
#[test]
fn a_revealed_card_out_of_view_is_eased_to() {
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    let mut app = App::default();
    let far = app.alloc_node_id();
    app.insert_placeholder_node(far, egui::vec2(5000.0, 5000.0), Kind::Tab, sleeping_tab("https://a.test/", "t"));
    let near = app.alloc_node_id();
    app.insert_placeholder_node(near, egui::vec2(0.0, 0.0), Kind::Tab, sleeping_tab("https://b.test/", "t"));
    app.camera.jump_to(egui::Rect::from_center_size(egui::pos2(100.0, 100.0), egui::vec2(900.0, 700.0)));
    app.camera.reveal = Some(near);
    app.apply_camera_requests(false);
    assert!(!app.camera.is_moving() && app.camera.reveal.is_none(), "already in view");
    app.camera.reveal = Some(far);
    app.apply_camera_requests(false);
    assert!(app.camera.is_moving() && app.camera.reveal.is_none(), "eased to the far card");
}

fn app_with_far_card() -> (App, NodeId) {
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    let mut app = App::default();
    let far = app.alloc_node_id();
    app.insert_placeholder_node(far, egui::vec2(5000.0, 5000.0), Kind::Tab, sleeping_tab("https://a.test/", "t"));
    app.camera.jump_to(egui::Rect::from_center_size(egui::pos2(100.0, 100.0), egui::vec2(900.0, 700.0)));
    (app, far)
}

/// A card on another level has a rect but isn't drawn here: its reveal is dropped, not eased to and not kept.
#[test]
fn a_reveal_of_a_card_on_another_level_is_dropped() {
    let (mut app, far) = app_with_far_card();
    let group = app.alloc_node_id();
    app.insert_group_at(group, None);
    app.edit_group(far, Some(group));
    app.camera.reveal = Some(far);
    app.apply_camera_requests(false);
    assert!(!app.camera.is_moving() && app.camera.reveal.is_none());
}

/// A reopened tab may still be spawning: its reveal waits for the card.
#[test]
fn a_reveal_waits_for_a_card_with_no_rect_yet() {
    let (mut app, _) = app_with_far_card();
    let coming = app.alloc_node_id();
    app.camera.reveal = Some(coming);
    app.apply_camera_requests(false);
    assert_eq!(app.camera.reveal, Some(coming));
}

/// An ease overrides a held pan, so a tab landing under one is not eased to.
#[test]
fn a_reveal_is_dropped_while_the_background_is_held() {
    let (mut app, far) = app_with_far_card();
    app.camera.drag_history.push_back((std::time::Instant::now(), egui::pos2(0.0, 0.0)));
    app.camera.reveal = Some(far);
    app.apply_camera_requests(false);
    assert!(!app.camera.is_moving() && app.camera.reveal.is_none());
}

/// Going straight somewhere (Fit, the minimap) is the user choosing the view.
#[test]
fn a_jump_drops_a_waiting_reveal() {
    let (mut app, far) = app_with_far_card();
    app.camera.reveal = Some(far);
    app.camera.jump_to(egui::Rect::from_center_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 700.0)));
    assert_eq!(app.camera.reveal, None);
}

/// A reveal is for the view being looked at: leaving it drops the reveal rather than moving the camera on return.
#[test]
fn a_reveal_ends_when_the_graph_is_not_drawn() {
    let (mut app, far) = app_with_far_card();
    app.camera.reveal = Some(far);
    app.end_stale_reveal(true);
    assert_eq!(app.camera.reveal, Some(far));
    app.end_stale_reveal(false);
    assert_eq!(app.camera.reveal, None);
}

/// New cards reveal from where they are placed, on the level the user then sees.
#[test]
fn placing_a_card_reveals_it() {
    let mut app = App::default();
    let note = app.alloc_node_id();
    app.insert_note_at(note, None);
    assert_eq!(app.camera.reveal, Some(note));
    let group = app.alloc_node_id();
    app.insert_group_at(group, None);
    assert_eq!(app.camera.reveal, Some(group), "the last card placed");
}

/// No ease moves the world under a dragged card, neither one already running (a click
/// on a half-hidden card, then a quick drag) nor a reveal asked for during the drag.
#[test]
fn a_drag_stops_and_refuses_camera_eases() {
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    let mut app = App::default();
    let far = app.alloc_node_id();
    app.insert_placeholder_node(far, egui::vec2(5000.0, 5000.0), Kind::Tab, sleeping_tab("https://a.test/", "t"));
    app.camera.jump_to(egui::Rect::from_center_size(egui::pos2(100.0, 100.0), egui::vec2(900.0, 700.0)));
    app.camera.reveal = Some(far);
    app.apply_camera_requests(false);
    assert!(app.camera.is_moving());
    app.interaction.dragging = Some(crate::card_id::CardId::from(far));
    app.apply_camera_requests(false);
    assert!(!app.camera.is_moving(), "the drag stopped the running ease");
    app.camera.reveal = Some(far);
    app.apply_camera_requests(false);
    assert!(!app.camera.is_moving() && app.camera.reveal.is_none(), "and a reveal during it is dropped");
}

#[test]
fn easing_the_camera_stops_a_glide() {
    let mut camera = GraphCamera::new();
    let start = std::time::Instant::now() - std::time::Duration::from_millis(50);
    camera.momentum =
        Some(PanMomentum { initial_velocity: egui::vec2(2000.0, 0.0), start, start_center: camera.rect.center() });
    let to = egui::Rect::from_center_size(egui::pos2(5000.0, 5000.0), egui::vec2(400.0, 300.0));
    camera.ease_to(to);
    // Once the ease ends, the glide it replaced must not take over again.
    camera.tween.as_mut().unwrap().start -= TRANSITION_DURATION;
    camera.advance();
    camera.advance();
    assert_eq!(camera.rect, to);
    assert!(camera.tween.is_none() && camera.momentum.is_none());
}

/// A wide fit in a squarer window: `fill` adds the world `Scene` shows
/// above and below it, and moves nothing on screen.
#[test]
fn filling_grows_the_rect_to_the_visible_world_without_moving_it() {
    let outer = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 600.0));
    let wide = egui::Rect::from_center_size(egui::pos2(250.0, 0.0), egui::vec2(1000.0, 300.0));
    let scale = |rect: egui::Rect| (outer.size() / rect.size()).min_elem();
    let mut camera = GraphCamera::new();
    camera.jump_to(wide);
    camera.fill(outer);
    assert_eq!((camera.rect.center(), scale(camera.rect)), (wide.center(), scale(wide)));
    assert_eq!(camera.rect.size(), egui::vec2(1000.0, 750.0));
    let filled = camera.rect;
    camera.fill(outer);
    assert_eq!(camera.rect, filled, "already filling: unchanged");
}

#[test]
fn the_camera_transform_maps_its_rect_onto_the_outer_rect() {
    let mut camera = GraphCamera::new();
    camera.rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 200.0));
    let outer = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(800.0, 400.0));
    let transform = camera.transform(outer);
    assert_eq!(transform.scaling, 2.0);
    assert_eq!(transform * camera.rect.center(), outer.center());
    camera.rect = egui::Rect::from_center_size(egui::Pos2::ZERO, egui::vec2(8.0, 4.0));
    assert_eq!(camera.transform(outer).scaling, GRAPH_ZOOM_MAX, "the zoom range clamps");
}

#[test]
fn begin_frame_fills_the_rect_to_the_outer_shape_and_sets_the_readout() {
    let ctx = egui::Context::default();
    let mut camera = GraphCamera::new();
    camera.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
    let outer = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
    assert_eq!(camera.begin_frame(&ctx, outer).scaling, 2.0);
    assert_eq!(camera.rect.size(), egui::vec2(200.0, 100.0));
    assert_eq!(camera.zoom_pct, 200.0);
}

#[test]
fn the_camera_is_moving_while_an_ease_or_a_glide_runs() {
    let mut camera = GraphCamera::new();
    assert!(!camera.is_moving());
    camera.ease_to(egui::Rect::from_center_size(egui::pos2(5000.0, 5000.0), egui::vec2(400.0, 300.0)));
    assert!(camera.is_moving());
    camera.tween.as_mut().unwrap().start -= TRANSITION_DURATION;
    camera.advance();
    assert!(!camera.is_moving());
    let start = std::time::Instant::now();
    camera.momentum =
        Some(PanMomentum { initial_velocity: egui::vec2(2000.0, 0.0), start, start_center: camera.rect.center() });
    assert!(camera.is_moving());
}

/// A card inside where the camera is headed needs no ease of its own: the running one stands.
#[test]
fn pan_to_judges_the_view_the_camera_is_headed_for() {
    let mut camera = GraphCamera::new();
    camera.jump_to(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0)));
    let to = egui::Rect::from_min_size(egui::pos2(5000.0, 5000.0), egui::vec2(400.0, 300.0));
    camera.ease_to(to);
    camera.pan_to(egui::Rect::from_min_size(egui::pos2(5100.0, 5100.0), egui::vec2(50.0, 50.0)));
    assert_eq!(camera.target(), to);
    camera.pan_to(egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(50.0, 50.0)));
    assert_eq!(camera.target().size(), to.size(), "the view's size, not a frame of the ease");
    assert_eq!(camera.target().center(), egui::pos2(125.0, 125.0));
}

#[test]
fn pan_to_eases_only_when_the_card_is_not_fully_in_view() {
    let mut camera = GraphCamera::new();
    camera.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
    camera.pan_to(egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 20.0)));
    assert!(camera.tween.is_none(), "already visible");
    let far = egui::Rect::from_min_size(egui::pos2(500.0, 0.0), egui::vec2(20.0, 20.0));
    camera.pan_to(far);
    let tween = camera.tween.expect("off screen: eases");
    assert_eq!(tween.to, egui::Rect::from_center_size(far.center(), camera.rect.size()));
}

/// An area that only narrows (the right panel opening) keeps the scale and the visible left edge, so the
/// panel covers a strip; widening back shows the same world again.
#[test]
fn a_change_of_width_alone_keeps_the_scale_and_the_left_edge() {
    let wide = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(1000.0, 600.0));
    let narrow = egui::Rect::from_min_size(wide.min, egui::vec2(700.0, 600.0));
    let mut camera = GraphCamera::new();
    camera.jump_to(egui::Rect::from_min_size(egui::pos2(-500.0, -300.0), egui::vec2(1000.0, 600.0)));
    camera.fill(wide);
    let before = camera.rect;
    let scale = camera.transform(wide).scaling;
    camera.fill(narrow);
    assert_eq!(camera.rect.min, before.min, "the left edge stays");
    assert_eq!(camera.transform(narrow).scaling, scale, "and the scale");
    assert_eq!(camera.rect.size(), egui::vec2(700.0, 600.0));
    camera.fill(wide);
    assert_eq!(camera.rect, before, "widening back shows the same world");
}

/// An ease still running when only the width changes keeps its scale; its endpoints are anchored like
/// `rect`, else the next frame's `advance` brings back their old shape and the centred fit zooms out.
#[test]
fn an_ease_running_through_a_width_change_keeps_its_scale() {
    let wide = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(1000.0, 600.0));
    let narrow = egui::Rect::from_min_size(wide.min, egui::vec2(700.0, 600.0));
    let mut camera = GraphCamera::new();
    camera.jump_to(egui::Rect::from_min_size(egui::pos2(-500.0, -300.0), egui::vec2(1000.0, 600.0)));
    camera.fill(wide);
    camera.ease_to(egui::Rect::from_min_size(egui::pos2(200.0, -300.0), egui::vec2(1000.0, 600.0)));
    camera.fill(narrow);
    for _ in 0..3 {
        camera.advance();
        camera.fill(narrow);
        assert!((camera.transform(narrow).scaling - 1.0).abs() < 1e-4, "scale {}", camera.transform(narrow).scaling);
    }
}

/// A rect of another shape (a fit, a tween target) is first fitted to the last area, so the left edge kept is
/// the one on screen, not the rect's own.
#[test]
fn a_narrowing_keeps_the_left_edge_shown_not_the_rects_own() {
    let wide = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(1000.0, 600.0));
    let narrow = egui::Rect::from_min_size(wide.min, egui::vec2(700.0, 600.0));
    let mut camera = GraphCamera::new();
    camera.fill(wide);
    // A square fit: `Scene` shows it with world beside it.
    camera.jump_to(egui::Rect::from_center_size(egui::pos2(0.0, 0.0), egui::vec2(600.0, 600.0)));
    camera.fill(narrow);
    assert_eq!(camera.rect.min, egui::pos2(-500.0, -300.0), "the left edge `wide` showed");
    assert_eq!(camera.rect.size(), egui::vec2(700.0, 600.0));
}

/// Between the panel opening and the next frame's `fill`, a conversion already maps points the way that frame
/// will draw them (a click computed from the selection's change lands where the card is drawn).
#[test]
fn the_transform_maps_a_narrower_area_as_the_next_fill_will() {
    let wide = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(1000.0, 600.0));
    let narrow = egui::Rect::from_min_size(wide.min, egui::vec2(700.0, 600.0));
    let mut camera = GraphCamera::new();
    camera.jump_to(egui::Rect::from_min_size(egui::pos2(-500.0, -300.0), egui::vec2(1000.0, 600.0)));
    camera.fill(wide);
    let early = camera.transform(narrow);
    camera.fill(narrow);
    assert_eq!(early, camera.transform(narrow));
}

/// Every other change gets the centred fit: a taller area keeps the centre.
#[test]
fn a_change_of_height_keeps_the_centred_fit() {
    let outer = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(1000.0, 600.0));
    let taller = egui::Rect::from_min_size(outer.min, egui::vec2(1000.0, 800.0));
    let mut camera = GraphCamera::new();
    camera.jump_to(egui::Rect::from_center_size(egui::pos2(250.0, 0.0), egui::vec2(1000.0, 600.0)));
    camera.fill(outer);
    camera.fill(taller);
    assert_eq!(camera.rect, egui::Rect::from_center_size(egui::pos2(250.0, 0.0), egui::vec2(1000.0, 800.0)));
}
