use super::*;
use crate::card_id::CardId;

/// The card the pane starts describing is revealed once; a second selected card is not. (A drag refuses the
/// ease itself: `a_drag_stops_and_refuses_camera_eases`.)
#[test]
fn the_pane_reveals_the_card_it_starts_describing() {
    let mut app = App::default();
    let a = crate::session_sync::tests::add_note(&mut app, "a");
    let b = crate::session_sync::tests::add_note(&mut app, "b");
    app.interaction.selected.insert(CardId::from(a));
    app.track_pane_card();
    assert_eq!(app.camera.reveal, Some(a));
    app.camera.reveal = None;
    app.track_pane_card();
    assert_eq!(app.camera.reveal, None, "only when it changes");
    app.interaction.selected.insert(CardId::from(b));
    app.track_pane_card();
    assert_eq!((app.pane_card(), app.camera.reveal), (None, None), "two cards: nothing to reveal");
    app.interaction.selected.clear();
    app.track_pane_card();
    app.interaction.selected.insert(CardId::from(b));
    app.track_pane_card();
    assert_eq!((app.pane_card(), app.camera.reveal), (Some(b), Some(b)), "back to one card: revealed");
}

/// The header names the card in text that reads on the panel, in each theme.
#[test]
fn the_pane_header_reads_on_the_panel() {
    for dark in [true, false] {
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "alpha");
        app.interaction.selected.insert(CardId::from(a));
        let name = app.accessible_name(a).unwrap();
        let ctx = crate::theme::testutil::ctx(dark);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.build_right_panel(ui, &mut Vec::new()));
        output.textures_delta.clear(); // nothing uploads the font atlas here
        let color = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == name => {
                    Some(t.override_text_color.unwrap_or(t.galley.job.sections[0].format.color))
                }
                _ => None,
            })
            .expect("the header draws the name");
        let panel = ctx.global_style().visuals.panel_fill;
        let ratio = crate::theme::testutil::contrast(color, panel);
        assert!(ratio >= 4.5, "dark={dark}: {color:?} on {panel:?} is {ratio:.2}");
    }
}

/// The Labels section lists every label with how much of the selection carries it.
#[test]
fn the_label_rows_show_how_much_of_the_selection_carries_each() {
    use crate::labels::LabelState;
    let mut app = App::default();
    let a = crate::session_sync::tests::add_note(&mut app, "a");
    let b = crate::session_sync::tests::add_note(&mut app, "b");
    let urgent = app.create_label("urgent", None).unwrap();
    let later = app.create_label("later", None).unwrap();
    app.interaction.selected.extend([CardId::from(a), CardId::from(b)]);
    app.label_selection(urgent, true);
    app.interaction.selected.remove(&CardId::from(b));
    let states = |app: &App| -> Vec<(NodeId, LabelState)> {
        app.pane_label_rows().into_iter().map(|(id, _, _, state)| (id, state)).collect()
    };
    assert!(states(&app).contains(&(urgent, LabelState::All)));
    app.interaction.selected.insert(CardId::from(b));
    app.set_label(&[b], urgent, false);
    assert!(states(&app).contains(&(urgent, LabelState::Some)));
    assert!(states(&app).contains(&(later, LabelState::None)));
}

/// The Text section: a note's text, a page's stored text, else nothing.
#[test]
fn the_pane_text_is_a_notes_text_or_a_pages_stored_text() {
    use crate::session_sync::tests::sleeping_tab;
    use crate::store::model::Kind;
    let mut app = App::default();
    let note = crate::session_sync::tests::add_note(&mut app, "milk and eggs");
    assert_eq!(app.pane_text(note), Some("milk and eggs"));
    let tab = app.alloc_node_id();
    app.insert_placeholder_node(tab, egui::Vec2::ZERO, Kind::Tab, sleeping_tab("https://a.test/", "a"));
    assert_eq!(app.pane_text(tab), None);
    app.search.texts.put(crate::search::text_store::TextRecord {
        id: tab,
        url: "https://a.test/".into(),
        title: "a".into(),
        text: "page words".into(),
        at: 0,
    });
    assert_eq!(app.pane_text(tab), Some("page words"));
}

/// One rename draft: the pane shows its field unless the card's menu is open on the card (the menu shows it).
#[test]
fn the_pane_shows_the_rename_field_only_when_the_menu_does_not() {
    let mut app = App::default();
    let note = crate::session_sync::tests::add_note(&mut app, "n");
    assert!(!app.pane_renaming(note));
    app.apply_node_action(&egui::Context::default(), note, crate::node_menu::NodeAction::Rename);
    assert!(app.pane_renaming(note));
    app.menus.node_menu.open(note, egui::Pos2::ZERO);
    assert!(!app.pane_renaming(note), "the menu's field has it");
}

/// Opening the pane takes no focus, and Down with nothing focused stays the graph's.
#[test]
fn the_pane_takes_no_focus_when_it_opens() {
    let mut app = App::default();
    let group = app.alloc_node_id();
    app.insert_group_at(group, None);
    app.create_label("urgent", None).unwrap();
    app.interaction.selected.insert(CardId::from(group));
    let ctx = crate::theme::testutil::ctx(true);
    let mut down_left = false;
    for events in [
        vec![],
        vec![],
        vec![egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: Some(egui::Key::ArrowDown),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    ] {
        let mut actions = Vec::new();
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            app.build_right_panel(ui, &mut actions);
            down_left = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
        });
        output.textures_delta.clear();
    }
    assert_eq!(ctx.memory(|m| m.focused()), None);
    assert!(down_left, "the pane left Down to the graph");
}

/// A label rename takes the keys once; a click away cancels it and leaves the keys to the graph.
#[test]
fn a_click_away_cancels_a_label_rename() {
    let mut app = App::default();
    let a = crate::session_sync::tests::add_note(&mut app, "a");
    let urgent = app.create_label("urgent", None).unwrap();
    app.interaction.selected.insert(CardId::from(a));
    let ctx = crate::theme::testutil::ctx(true);
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let mut actions = Vec::new();
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| app.build_right_panel(ui, &mut actions));
        output.textures_delta.clear(); // nothing uploads the font atlas here
        actions
    };
    frame(&mut app, vec![]);
    app.interaction.label_section.start_editing(urgent, "urgent");
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    assert!(ctx.memory(|m| m.focused()).is_some(), "the field took the keys");
    let at = egui::pos2(10.0, 10.0); // the canvas side, left of the panel
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut app, vec![egui::Event::PointerMoved(at), button(true)]);
    let actions = frame(&mut app, vec![button(false)]);
    frame(&mut app, vec![]);
    assert!(app.interaction.label_section.editing.is_none(), "the click cancelled it");
    assert!(!actions.iter().any(|a| matches!(a, GraphAction::RenameLabel { .. })), "without renaming");
    assert_eq!(ctx.memory(|m| m.focused()), None, "and the field did not take the keys back");
}

/// A swatch click during a label rename recolours the label and keeps the rename and its draft. egui surrenders
/// the field's focus on the click's release, when the press is already over.
#[test]
fn a_swatch_click_keeps_a_label_rename() {
    let mut app = App::default();
    let a = crate::session_sync::tests::add_note(&mut app, "a");
    let urgent = app.create_label("urgent", None).unwrap();
    app.interaction.selected.insert(CardId::from(a));
    let ctx = crate::theme::testutil::ctx(true);
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let mut actions = Vec::new();
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| app.build_right_panel(ui, &mut actions));
        output.textures_delta.clear(); // nothing uploads the font atlas here
        actions
    };
    frame(&mut app, vec![]);
    app.interaction.label_section.start_editing(urgent, "urgent");
    frame(&mut app, vec![]);
    frame(&mut app, vec![egui::Event::Text("x".into())]);
    let field = ctx.memory(|m| m.focused()).expect("the field took the keys");
    let below = ctx.read_response(field).unwrap().rect.bottom();
    // The first swatch: the first small square widget under the field.
    let swatch = ctx.viewport(|v| {
        v.prev_pass
            .widgets
            .layers()
            .flat_map(|(_, rects)| rects)
            .map(|w| w.rect)
            .find(|r| r.top() > below && (r.width() - r.height()).abs() < 0.5 && r.width() < 30.0)
    });
    let at = swatch.expect("a swatch under the field").center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut app, vec![egui::Event::PointerMoved(at)]);
    frame(&mut app, vec![button(true)]);
    let actions = frame(&mut app, vec![button(false)]);
    frame(&mut app, vec![]);
    assert!(actions.iter().any(|a| matches!(a, GraphAction::RecolorLabel { .. })), "the swatch recoloured");
    assert_eq!(
        app.interaction.label_section.editing.as_ref().map(|(id, draft)| (*id, draft.as_str())),
        Some((urgent, "urgentx")),
        "the rename and its draft stay"
    );
    assert_eq!(ctx.memory(|m| m.focused()), Some(field), "and the field has the keys back");
}

/// A label rename in progress is dropped with the pane, not carried to the next selection.
#[test]
fn the_label_section_resets_when_the_pane_closes() {
    let mut app = App::default();
    let a = crate::session_sync::tests::add_note(&mut app, "a");
    let urgent = app.create_label("urgent", None).unwrap();
    app.interaction.selected.insert(CardId::from(a));
    app.interaction.label_section.editing = Some((urgent, "half-typed".into()));
    app.interaction.label_section.draft = "new".into();
    let ctx = crate::theme::testutil::ctx(true);
    let frame = |app: &mut App| {
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.build_right_panel(ui, &mut actions));
        output.textures_delta.clear(); // nothing uploads the font atlas here
    };
    frame(&mut app);
    assert!(app.interaction.label_section.editing.is_some(), "kept while the pane shows");
    app.interaction.selected.clear();
    frame(&mut app);
    assert!(app.interaction.label_section.editing.is_none() && app.interaction.label_section.draft.is_empty());
}
