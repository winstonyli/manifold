use super::*;

#[test]
fn ctrl_reveals_hints_only_after_a_solo_hold() {
    use super::{ctrl_reveal_step as step, CtrlReveal};
    let (s, shown) = step(CtrlReveal::default(), true, false, 10.0);
    assert!(!shown, "just pressed");
    let (s, shown) = step(s, true, false, 10.4);
    assert!(shown, "held past the delay");
    let (s, shown) = step(s, true, true, 10.5);
    assert!(!shown, "a chord key cancels it");
    let (s, shown) = step(s, true, false, 12.0);
    assert!(!shown, "stays cancelled while Ctrl is held");
    let (s, _) = step(s, false, false, 12.1);
    assert_eq!(s, CtrlReveal::default(), "release resets");
}

/// Ctrl+Z reads as undo, Ctrl+Shift+Z and Ctrl+Y as redo, and one press is consumed once.
#[test]
fn undo_chords_read_as_undo_or_redo() {
    let press = |key, modifiers| {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.events.push(crate::debug::reader::debug_key_event(key, modifiers));
        let mut out = Vec::new();
        let mut full = ctx.run_ui(raw, |ui| {
            out.push(ui.input_mut(super::undo_chord));
            out.push(ui.input_mut(super::undo_chord));
        });
        full.textures_delta.clear();
        out
    };
    let cmd = egui::Modifiers::COMMAND;
    assert_eq!(press(egui::Key::Z, cmd), [Some(false), None]);
    assert_eq!(press(egui::Key::Z, cmd | egui::Modifiers::SHIFT), [Some(true), None]);
    assert_eq!(press(egui::Key::Y, cmd), [Some(true), None]);
    assert_eq!(press(egui::Key::Z, egui::Modifiers::NONE), [None, None]);
}

/// A JSON level keeps the digit keys out, but Ctrl+Z still undoes its inline edits.
#[test]
fn ctrl_z_on_a_json_level_undoes_an_edit() {
    let mut app = App::default();
    let id = app.alloc_node_id();
    app.insert_inline_json_at(id, None);
    let mut mint = |_: &str| app.alloc_node_id();
    let tree = crate::store::body::value_to_tree(&serde_json::json!({"a": 1}), &mut mint).unwrap();
    app.set_body(id, tree);
    app.refresh_inline(id);
    app.view_mode = ViewMode::Graph;
    app.current_group = Some(id);
    app.sync_json_level();
    let op = crate::json_edit::JsonOp::Set { pointer: "/a".into(), value: serde_json::json!(2) };
    app.edit_json(id, op).unwrap();
    assert_eq!(*app.json_level.as_ref().unwrap().value, serde_json::json!({"a": 2}));
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    raw.events.push(crate::debug::reader::debug_key_event(egui::Key::Z, egui::Modifiers::COMMAND));
    let mut full = ctx.run_ui(raw, |ui| app.handle_undo_keys(ui));
    full.textures_delta.clear();
    app.sync_json_level();
    assert_eq!(*app.json_level.as_ref().unwrap().value, serde_json::json!({"a": 1}));
}

#[test]
fn digits_wait_only_while_a_longer_number_could_still_name_a_card() {
    use super::digits_ready;
    assert_eq!(digits_ready("3", 5, 0.0), Some(3), "no card 30..39 exists");
    assert_eq!(digits_ready("1", 15, 0.0), None, "10..15 exist: wait for a second digit");
    assert_eq!(digits_ready("1", 15, 1.0), Some(1), "window elapsed");
    assert_eq!(digits_ready("12", 15, 0.0), Some(12));
    assert_eq!(digits_ready("0", 15, 1.0), Some(0), "0 names no card; the caller finds none");
}

/// Escape (or Enter, or an arrow) typed into a text field is the field's, not the graph's.
#[test]
fn keys_typed_into_a_text_field_stay_out_of_graph_navigation() {
    let mut app = App::default();
    app.interaction.selected.insert(crate::card_id::testutil::card(1));
    let ctx = egui::Context::default();
    let field = egui::Id::new("test_field");
    let mut text = String::new();
    let mut frame = |app: &mut App, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            // Read first, as `build_graph_ui` does.
            let typing = crate::shortcuts::text_field_has_keys(ui.ctx());
            ui.add(egui::TextEdit::singleline(&mut text).id(field));
            app.handle_graph_keys(ui, typing);
        });
        output.textures_delta.clear();
    };
    ctx.memory_mut(|m| m.request_focus(field));
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    let key = |key| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut app, vec![key(egui::Key::Escape)]);
    assert!(app.interaction.selected.contains(&crate::card_id::testutil::card(1)), "Escape ended the edit only");
    assert!(!ctx.memory(|m| m.has_focus(field)), "the field let go");
    frame(&mut app, vec![key(egui::Key::Escape)]);
    assert!(app.interaction.selected.is_empty(), "with no field focused, Escape is the graph's");
}

/// Arrow presses that land in one frame (a slow frame, a key repeat) each move the graph focus one node.
#[test]
fn two_arrow_presses_in_one_frame_move_the_graph_focus_two_nodes() {
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    let mut app = App::default();
    let ids: Vec<crate::store::model::NodeId> = (0..3).map(|_| app.alloc_node_id()).collect();
    for (i, id) in ids.iter().enumerate() {
        let tab = sleeping_tab(&format!("https://{i}.test/"), &format!("t{i}"));
        app.insert_placeholder_node(*id, egui::vec2(100.0 + 200.0 * i as f32, 0.0), Kind::Tab, tab);
    }
    app.focused = Some(ids[0]);
    let ctx = egui::Context::default();
    let key = egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: Some(egui::Key::ArrowRight),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0))),
        events: vec![key.clone(), key],
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| app.handle_graph_keys(ui, false));
    output.textures_delta.clear();
    assert_eq!(app.focused, Some(ids[2]));
}

/// While a widget other than a card has focus (a card pane row, a toolbar button), the arrows and Enter
/// are its own: the graph focus stays and nothing opens.
#[test]
fn a_focused_widget_keeps_the_arrows_and_enter_from_the_graph() {
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    let mut app = App::default();
    let ids: Vec<crate::store::model::NodeId> = (0..2).map(|_| app.alloc_node_id()).collect();
    for (i, id) in ids.iter().enumerate() {
        let tab = sleeping_tab(&format!("https://{i}.test/"), &format!("t{i}"));
        app.insert_placeholder_node(*id, egui::vec2(100.0 + 200.0 * i as f32, 0.0), Kind::Tab, tab);
    }
    app.focused = Some(ids[0]);
    let ctx = egui::Context::default();
    let row = egui::Id::new("pane_row");
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            app.handle_graph_keys(ui, false);
            ui.interact(
                egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(40.0, 20.0)),
                row,
                egui::Sense::click(),
            );
        });
        output.textures_delta.clear();
    };
    ctx.memory_mut(|m| m.request_focus(row));
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    assert!(ctx.memory(|m| m.has_focus(row)));
    let key = |key| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut app, vec![key(egui::Key::ArrowRight), key(egui::Key::Enter)]);
    assert_eq!(app.focused, Some(ids[0]), "the arrow was the row's");
    assert!(app.transition.is_none(), "and so was Enter");
}

/// An open node menu keeps the arrows and Enter even with none of its rows focused (its first frame, or after a
/// page swap took the focused row away), which the focused-widget rule above can't see.
#[test]
fn an_open_node_menu_keeps_the_arrows_and_enter_without_a_focused_row() {
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    let mut app = App::default();
    let ids: Vec<crate::store::model::NodeId> = (0..2).map(|_| app.alloc_node_id()).collect();
    for (i, id) in ids.iter().enumerate() {
        let tab = sleeping_tab(&format!("https://{i}.test/"), &format!("t{i}"));
        app.insert_placeholder_node(*id, egui::vec2(100.0 + 200.0 * i as f32, 0.0), Kind::Tab, tab);
    }
    app.focused = Some(ids[0]);
    app.menus.node_menu.open = Some((ids[0], egui::pos2(0.0, 0.0)));
    let ctx = egui::Context::default();
    let key = |key| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let input =
        egui::RawInput { events: vec![key(egui::Key::ArrowRight), key(egui::Key::Enter)], ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| app.handle_graph_keys(ui, false));
    output.textures_delta.clear();
    assert!(ctx.memory(|m| m.focused()).is_none(), "no row has focus");
    assert_eq!(app.focused, Some(ids[0]), "the arrow was the menu's");
    assert!(app.transition.is_none(), "and so was Enter");
}

/// Links: Escape in pick-target mode ends the mode and does nothing else; the next one is the graph's as before.
#[test]
fn escape_ends_pick_target_mode_first() {
    let mut app = App::default();
    let note = app.alloc_node_id();
    app.insert_note_at(note, None);
    app.link.connecting = Some((note, crate::caps::LINK));
    app.interaction.selected.insert(crate::card_id::testutil::card(1));
    let ctx = egui::Context::default();
    let escape = egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let frame = |app: &mut App| {
        let input = egui::RawInput { events: vec![escape.clone()], ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| app.handle_graph_keys(ui, false));
        output.textures_delta.clear();
    };
    frame(&mut app);
    assert!(app.link.connecting.is_none());
    assert!(app.interaction.selected.contains(&crate::card_id::testutil::card(1)), "that Escape was pick mode's only");
    frame(&mut app);
    assert!(app.interaction.selected.is_empty());
}

/// Tab and Shift+Tab on a focused card step through the level in creation order and stop
/// handling at either end (egui then moves on to widgets outside the cards).
#[test]
fn tab_steps_through_cards_in_creation_order() {
    let mut app = App::default();
    for _ in 0..3 {
        app.new_note_shortcut();
    }
    let order = app.card_tab_order();
    assert_eq!(order.len(), 3);
    let card = |i: usize| order[i].widget();
    let ctx = egui::Context::default();
    let frame = |events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            // Cards the pass would register; egui drops focus from an id nothing registers.
            for (i, id) in order.iter().enumerate() {
                let rect = egui::Rect::from_min_size(egui::pos2(10.0 + 60.0 * i as f32, 10.0), egui::vec2(50.0, 50.0));
                ui.interact(rect, id.widget(), egui::Sense::click());
            }
            card_tab_step(ui, &order, |_| None);
        });
        output.textures_delta.clear();
    };
    let tab = |modifiers| egui::Event::Key {
        key: egui::Key::Tab,
        physical_key: Some(egui::Key::Tab),
        pressed: true,
        repeat: false,
        modifiers,
    };
    ctx.memory_mut(|m| m.request_focus(card(0)));
    frame(vec![]);
    frame(vec![tab(egui::Modifiers::NONE)]);
    assert!(ctx.memory(|m| m.has_focus(card(1))), "Tab: first -> second");
    frame(vec![]);
    frame(vec![tab(egui::Modifiers::NONE)]);
    assert!(ctx.memory(|m| m.has_focus(card(2))), "Tab: second -> third");
    frame(vec![]);
    frame(vec![tab(egui::Modifiers::SHIFT)]);
    assert!(ctx.memory(|m| m.has_focus(card(1))), "Shift+Tab: third -> second");
    frame(vec![]);
    frame(vec![tab(egui::Modifiers::NONE)]);
    frame(vec![]);
    // At the last card the handler does nothing (target is `None`), so focus is not moved back.
    frame(vec![tab(egui::Modifiers::NONE)]);
    assert!(!ctx.memory(|m| m.has_focus(card(0)) || m.has_focus(card(1))), "no wrap past the end");
}
