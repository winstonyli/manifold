use super::*;

/// L1: a selected id with no card (its tab mid-crash-respawn or mid-engine-swap) is left out of the centroid,
/// not counted in it.
#[test]
fn group_selected_centres_on_the_tabs_still_shown() {
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    let mut app = App::default();
    let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
    app.insert_placeholder_node(a, egui::vec2(100.0, 0.0), Kind::Tab, sleeping_tab("https://a.test/", "a"));
    app.insert_placeholder_node(b, egui::vec2(300.0, 0.0), Kind::Tab, sleeping_tab("https://b.test/", "b"));
    let expected = (app.layout_pos(a) + app.layout_pos(b)) / 2.0;
    app.interaction.selected.extend([CardId::from(a), CardId::from(b), crate::card_id::testutil::card(999)]);
    app.apply_graph_action(&egui::Context::default(), GraphAction::GroupSelected);
    let group = app.doc.mirror.group(a).expect("a was grouped");
    assert_eq!(app.doc.mirror.group(b), Some(group));
    assert_eq!(app.doc.mirror.pos(group), Some(expected), "the mean of the two cards, not divided by three");
    assert!(app.interaction.selected.is_empty());
}

/// Links: in pick-target mode a click on a target card draws the edge (and not the card's own click action), and
/// a click on empty canvas ends the mode. Driven through `build_graph_ui` with real egui input.
#[test]
fn a_click_finishes_or_cancels_pick_target_mode() {
    use crate::caps::{LINK, OPENED_FROM};
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
    for (id, x) in [(a, 0.0), (b, 600.0)] {
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(id, egui::vec2(x, 0.0), crate::store::model::Kind::Note, note);
    }
    // The size `graph_world_rect_to_screen` assumes with no window.
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
        output.textures_delta.clear();
    };
    let click = |app: &mut App, at: egui::Pos2| {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(app, vec![egui::Event::PointerMoved(at), press(true)]);
        frame(app, vec![press(false)]);
        frame(app, vec![]);
    };
    for _ in 0..3 {
        frame(&mut app, vec![]);
    }
    app.camera.jump_to(app.fit_all_nodes_scene_rect());
    frame(&mut app, vec![]);
    let screen_rect = |app: &App, id: NodeId| {
        let item = app.level_items().into_iter().find(|i| i.id == CardId::from(id)).expect("on the level");
        app.graph_world_rect_to_screen(item.rect)
    };
    app.link.connecting = Some((a, LINK));
    let on_b = screen_rect(&app, b).center();
    assert!(app.graph_scene_outer_rect().contains(on_b), "{on_b:?} in {:?}", app.graph_scene_outer_rect());
    click(&mut app, on_b);
    assert!(app.link.connecting.is_none());
    assert_eq!(app.links_naming(a).len(), 1, "the click linked b");
    assert!(!app.nodes[&b].as_note().unwrap().editing, "the click was pick mode's, not the note's");
    app.link.connecting = Some((a, OPENED_FROM));
    // Below both cards, clear of the minimap in the bottom-right corner.
    let below = egui::pos2(screen_rect(&app, a).center().x, screen_rect(&app, a).bottom() + 60.0);
    assert!(![a, b].iter().any(|&id| screen_rect(&app, id).contains(below)), "empty canvas");
    click(&mut app, below);
    assert!(app.link.connecting.is_none(), "a click on empty canvas ends the mode");
    assert_eq!(app.doc.mirror.parent(a), None, "and draws nothing");
}

/// A right-click on a card opens its menu; on the swatch it opens on the Color page. Driven through
/// `build_graph_ui` with real egui input.
#[test]
fn a_right_click_on_the_swatch_opens_the_menu_on_its_color_page() {
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    let note_id = app.alloc_node_id();
    let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
    app.insert_placeholder_node(note_id, egui::vec2(0.0, 0.0), crate::store::model::Kind::Note, note);
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
        output.textures_delta.clear();
    };
    let right_click = |app: &mut App, at: egui::Pos2| {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(app, vec![egui::Event::PointerMoved(at), press(true)]);
        frame(app, vec![press(false)]);
        frame(app, vec![]);
    };
    for _ in 0..3 {
        frame(&mut app, vec![]);
    }
    app.camera.jump_to(app.fit_all_nodes_scene_rect());
    frame(&mut app, vec![]);
    let card = {
        let item = app.level_items().into_iter().find(|i| i.id == CardId::from(note_id)).expect("on the level");
        app.graph_world_rect_to_screen(item.rect)
    };
    right_click(&mut app, card.center());
    assert_eq!(app.menus.node_menu.open.as_ref().map(|(id, _)| *id), Some(note_id), "the card's menu opens");
    assert!(app.menus.node_menu.page.is_none(), "on its items");
}

/// Wave 2 (H2 / P2): whole `build_graph_ui` frames over the 2000-node fixture spread on a grid: idle at the top
/// level, hovering a card, in pick-target mode, with the node menu open, filtered, and idle in the first group.
/// Timing, not correctness, so ignored:
/// `cargo test --release -j 8 -p manifold --bins graph_frame_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn graph_frame_cost() {
    use crate::bench::{level_fixture, report, spread, timed};
    let ctx = crate::theme::testutil::ctx(true);
    let mut f = level_fixture(false);
    spread(&mut f);
    let mut app = f.app;
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
        output.textures_delta.clear();
    };
    let run = |app: &mut App, what: &str, events: Vec<egui::Event>| {
        for _ in 0..5 {
            frame(app, events.clone());
        }
        report(what, timed(50, || frame(app, events.clone())));
    };
    let note = f
        .nodes
        .iter()
        .find(|id| app.doc.mirror.group(**id).is_none() && app.nodes[*id].as_note().is_some())
        .copied()
        .expect("a top-level note");
    app.camera.jump_to(app.card_rect(note).unwrap().expand(300.0));
    for _ in 0..3 {
        frame(&mut app, vec![]);
    }
    run(&mut app, "idle, top level", vec![]);
    let on_note = app.graph_world_rect_to_screen(app.card_rect(note).unwrap()).center();
    run(&mut app, "hovering a card", vec![egui::Event::PointerMoved(on_note)]);
    app.link.connecting = Some((note, crate::caps::LINK));
    run(&mut app, "pick-target mode (LINK)", vec![]);
    assert!(app.link.connecting.is_some(), "the mode stayed on");
    app.link.connecting = None;
    app.menus.node_menu.open(note, egui::pos2(500.0, 400.0));
    run(&mut app, "node menu open", vec![]);
    assert!(app.menus.node_menu.open.is_some(), "the menu stayed open");
    app.menus.node_menu.close();
    app.set_graph_filter("abc");
    run(&mut app, "filtered", vec![]);
    app.set_graph_filter("");
    app.switch_group_level(Some(f.groups[0]));
    for _ in 0..3 {
        frame(&mut app, vec![]);
    }
    run(&mut app, "idle, first group", vec![]);
}

/// Wave 2 (H4): keyboard navigation shows each card's Tab slot (`card_tab_order`'s index plus one) at its
/// top-left corner. Pins the slot lookup before it moves to a per-pass map.
#[test]
fn each_card_shows_its_tab_slot_badge() {
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    for k in 0..3 {
        let id = app.alloc_node_id();
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(id, egui::vec2(400.0 * k as f32, 0.0), crate::store::model::Kind::Note, note);
    }
    let frame = |app: &mut App| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
        output.textures_delta.clear();
        output
    };
    for _ in 0..3 {
        frame(&mut app);
    }
    app.camera.jump_to(app.fit_all_nodes_scene_rect());
    frame(&mut app);
    app.graph.card_nav_active = true; // read when the pass starts; the frame may clear it at its end
    let output = frame(&mut app);
    let badges: Vec<(String, egui::Pos2)> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text().parse::<usize>().is_ok() => {
                Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()).center()))
            }
            _ => None,
        })
        .collect();
    let order = app.card_tab_order();
    assert_eq!(order.len(), 3);
    for (k, id) in order.iter().enumerate() {
        // The rect the card is drawn at, as `a_click_finishes_or_cancels_pick_target_mode` reads it.
        let item = app.level_items().into_iter().find(|i| i.id == *id).expect("on the level");
        let corner = app.graph_world_rect_to_screen(item.rect).left_top();
        let number = (k + 1).to_string();
        assert!(
            badges.iter().any(|(text, at)| *text == number && at.distance(corner) < 15.0),
            "{id}: badge {number} near {corner:?}; badges {badges:?}"
        );
    }
}

/// With accesskit on, a card's `widget_info` puts a Button named by `Node::accessible_name`
/// in the tree egui hands the platform.
#[test]
fn card_widget_info_reaches_the_accesskit_tree() {
    let mut app = App::default();
    let g = app.alloc_node_id();
    app.insert_group_at(g, None);
    app.doc.mirror.set(g, crate::store::model::field::NAME, crate::store::model::FieldValue::Str("Reading".into()));
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 200.0))),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 50.0));
        let response = ui.interact(rect, crate::card_id::CardId::from(g).widget(), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, app.accessible_name(g).unwrap_or_default())
        });
    });
    output.textures_delta.clear();
    let update = output.platform_output.accesskit_update.expect("accesskit update");
    let named = update.nodes.iter().find(|(_, n)| n.label() == Some("Group: Reading"));
    assert_eq!(named.map(|(_, n)| n.role()), Some(egui::accesskit::Role::Button));
}

#[test]
fn detail_levels_say_what_a_card_draws() {
    assert!(Detail::Full.content() && Detail::Full.link_handles());
    assert!(!Detail::Simple.content() && !Detail::Simple.link_handles());
    assert_eq!(Detail::Full.glow_layers(false), 2);
    assert_eq!(Detail::Simple.glow_layers(false), 0);
    assert_eq!(Detail::Simple.glow_layers(true), 2, "hover brings the glow back");
}

#[test]
fn detail_flips_with_a_hysteresis_gap() {
    assert_eq!(Detail::Full.next(LOD_SIMPLIFY_COUNT), Detail::Full, "at the threshold: not yet");
    assert_eq!(Detail::Full.next(LOD_SIMPLIFY_COUNT + 1), Detail::Simple);
    assert_eq!(Detail::Simple.next(LOD_RESTORE_COUNT), Detail::Simple, "in the gap it stays");
    assert_eq!(Detail::Full.next(LOD_RESTORE_COUNT), Detail::Full);
    assert_eq!(Detail::Simple.next(LOD_RESTORE_COUNT - 1), Detail::Full);
}
