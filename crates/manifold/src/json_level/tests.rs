use super::*;
use crate::session_sync::Remote;
use crate::store::model::Kind;

// Real node ids: `snapshots()` skips any id that doesn't parse as
// a `NodeId`, which would make the store test pass vacuously.
const OUTER: &str = "0000000000000001-1";
const J: &str = "0000000000000001-2";

/// An App with a Json node `J` inside group `OUTER`, loaded from a temp
/// file holding `text`. `name` keeps each test's file its own, since
/// tests run in parallel in one process.
fn app_with_json(name: &str, text: &str) -> (App, String) {
    let dir = crate::store::testutil::temp_dir(&format!("json_level-{name}"));
    let path = dir.join("level.json");
    std::fs::write(&path, text).unwrap();
    let mut app = App::default();
    app.place_group(Some(OUTER.into()), "outer", Placement { group: None, pos: egui::Vec2::ZERO });
    app.insert_json_node_with_id(J.into(), path.to_string_lossy().into_owned(), egui::Vec2::ZERO, Some(OUTER.into()))
        .unwrap();
    (app, J.into())
}

/// Wave 2 (H1a): a search's hit rows are built with its result, per parent card, and go with it.
#[test]
fn a_search_builds_its_hit_rows_per_card() {
    let (mut app, j) = app_with_json("hit-rows", r#"{"a": {"b": 1}, "c": [1, 2]}"#);
    app.switch_group_level(Some(j));
    app.set_json_search("1".into());
    let rows = |app: &App| app.json_level.as_ref().unwrap().search.hit_rows.clone();
    assert_eq!(rows(&app), HashMap::from([("/a".to_string(), vec![0]), ("/c".to_string(), vec![0])]));
    app.set_json_search(String::new());
    assert!(rows(&app).is_empty());
}

/// Wave 2 (H1a): Tab and Shift+Tab walk the JSON cards through real frames, and a focused card panned out of view
/// keeps its focus. Pins the focus reads before they move out of the per-card loop.
#[test]
fn tab_walks_the_json_cards_and_focus_survives_a_pan() {
    let (mut app, j) = app_with_json("tab-walk", r#"{"a": {"x": 1}, "b": {"y": 2}, "c": {"z": 3}}"#);
    app.switch_group_level(Some(j));
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
        output.textures_delta.clear();
    };
    let tab = |shift: bool| egui::Event::Key {
        key: egui::Key::Tab,
        physical_key: Some(egui::Key::Tab),
        pressed: true,
        repeat: false,
        modifiers: if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE },
    };
    let card = |app: &App, i: usize| {
        let level = app.json_level.as_ref().unwrap();
        egui::Id::new(("json_card", &level.node_id, &level.cards[i].pointer))
    };
    for _ in 0..3 {
        frame(&mut app, vec![]);
    }
    assert_eq!(app.json_level.as_ref().unwrap().cards.len(), 4, "root, a, b, c");
    ctx.memory_mut(|m| m.request_focus(card(&app, 0)));
    frame(&mut app, vec![]);
    let focused = || ctx.memory(|m| m.focused());
    assert_eq!(focused(), Some(card(&app, 0)));
    // An idle frame follows each key: egui applies a card's Tab lock filter only once it has held focus for a
    // frame (`set_focus_lock_filter` checks `had_focus_last_frame`), so a key in the very next frame would also
    // run egui's own Tab move and skip a card. That consecutive-frame skip is today's behavior, not pinned here.
    frame(&mut app, vec![tab(false)]);
    assert_eq!(focused(), Some(card(&app, 1)), "Tab: the next card");
    frame(&mut app, vec![]);
    assert_eq!(focused(), Some(card(&app, 1)), "an idle frame keeps it");
    frame(&mut app, vec![tab(false)]);
    assert_eq!(focused(), Some(card(&app, 2)));
    frame(&mut app, vec![]);
    frame(&mut app, vec![tab(true)]);
    assert_eq!(focused(), Some(card(&app, 1)), "Shift+Tab: the previous card");
    frame(&mut app, vec![]);
    assert_eq!(focused(), Some(card(&app, 1)));
    app.camera.jump_to(egui::Rect::from_min_size(egui::pos2(50_000.0, 50_000.0), egui::vec2(800.0, 600.0)));
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    assert_eq!(focused(), Some(card(&app, 1)), "a focused card panned out of view keeps its focus");
}

#[test]
fn entering_builds_the_level_and_leaving_drops_it() {
    let (mut app, j) = app_with_json("enter", r#"{"a": {"b": 1}, "c": [1, 2]}"#);
    app.switch_group_level(Some(j.clone()));
    let level = app.json_level.as_ref().expect("entering a Json node builds its level");
    assert_eq!(level.node_id, j);
    let ids: Vec<String> = app.level_items().into_iter().map(|i| i.id).collect();
    assert_eq!(ids, [format!("{J}#"), format!("{J}#/a"), format!("{J}#/c")]);
    assert!(app.level_items().iter().all(|i| i.origin == Origin::Json));
    assert_eq!(app.level_edges().len(), 2);
    assert!(app.level_edges().iter().all(|e| e.link.is_none()), "no JSON-level line is a link");
    app.switch_group_level(Some(OUTER.into()));
    assert!(app.json_level.is_none());
}

/// Links: a JSON level's cards are not stored nodes, so no node menu there offers the Links section, and entering
/// one ends pick-target mode.
#[test]
fn a_json_level_hides_the_links_section_and_ends_pick_target_mode() {
    let (mut app, j) = app_with_json("links", r#"{"a": 1}"#);
    let note = app.alloc_node_id();
    app.insert_note_at(note.clone(), None);
    app.edit_group(&note, Some(OUTER.into()));
    app.switch_group_level(Some(OUTER.into()));
    assert!(!app.node_menu_view(&note).unwrap().edges.is_empty(), "the Json card is a target at OUTER's level");
    app.apply_node_action(&egui::Context::default(), &note, crate::NodeAction::Connect(crate::caps::LINK));
    app.switch_group_level(Some(j));
    assert!(app.link.connecting.is_none());
    assert!(app.node_menu_view(&note).unwrap().edges.is_empty());
}

/// Review C-B11: screen<->world agree with the outline open.
#[test]
fn screen_to_world_inverts_world_to_screen_with_the_outline_open() {
    let (mut app, j) = app_with_json("inverse", r#"{"a": 1}"#);
    app.switch_group_level(Some(j));
    app.json_ui.json_outline_visible = true;
    app.json_ui.json_outline_width = 300.0;
    let world = egui::Rect::from_min_size(egui::pos2(40.0, 60.0), egui::vec2(10.0, 10.0));
    let screen = app.graph_world_rect_to_screen(world);
    let back = app.graph_screen_point_to_world(screen.min);
    assert!((back - world.min.to_vec2()).length() < 0.01, "{back:?}");
}

#[test]
fn placement_leaves_a_json_level_and_uses_the_outer_group() {
    let (mut app, j) = app_with_json("placement", "[1]");
    app.switch_group_level(Some(j));
    let at = app.placement("n", None);
    assert_eq!(at.group.as_deref(), Some(OUTER));
    assert_eq!(app.current_group.as_deref(), Some(OUTER), "the level was left");
    assert!(app.json_level.is_none());
    assert_eq!(at.pos, graph_layout::GRAPH_WORLD_CENTER + graph_layout::spawn_jitter("n"), "no pos: jittered spawn");
    let explicit = egui::vec2(3.0, 4.0);
    assert_eq!(app.placement("n", Some(explicit)).pos, explicit, "an explicit pos wins");
}

#[test]
fn a_new_note_or_group_from_a_json_level_leaves_it_and_lands_in_the_outer_group() {
    let (mut app, j) = app_with_json("create_note_group", "[1]");
    for create in [App::new_note_shortcut, App::new_group_shortcut] {
        app.switch_group_level(Some(j.clone()));
        let before = app.node_order.len();
        create(&mut app);
        assert_eq!(app.node_order.len(), before + 1);
        assert!(app.json_level.is_none(), "the level was left");
        assert_eq!(app.doc.mirror.group(app.node_order.last().unwrap()), Some(OUTER));
    }
}

#[test]
fn a_new_tab_from_a_json_level_leaves_it_and_spawns_in_the_outer_group() {
    let (mut app, j) = app_with_json("new_tab", "[1]");
    app.switch_group_level(Some(j));
    app.new_tab_shortcut();
    assert!(app.json_level.is_none(), "the level was left");
    let (spawn, _) = app.test_spawns.last().expect("a tab spawn was issued");
    assert_eq!(spawn.group.as_deref(), Some(OUTER));
}

#[test]
fn place_node_inserts_at_the_placement_and_returns_the_id() {
    let (mut app, _) = app_with_json("place_node", "[1]");
    let note = || Runtime::Note(NotePayload { text: String::new(), editing: false });
    let at = Placement { group: Some(OUTER.into()), pos: egui::vec2(1.0, 2.0) };
    let id = app.place_node(None, Kind::Note, note(), at);
    assert_eq!(
        (app.doc.mirror.group(&id), app.layout_pos(&id), app.doc.mirror.parent(&id).is_none()),
        (Some(OUTER), egui::vec2(1.0, 2.0), true)
    );
    assert_eq!(app.doc.mirror.pos(&id), Some(egui::vec2(1.0, 2.0)), "the document pos is the placement too");
    assert_eq!(app.node_order.last(), Some(&id));
    let at = Placement { group: None, pos: egui::Vec2::ZERO };
    assert_eq!(app.place_node(Some("fixed".into()), Kind::Note, note(), at), "fixed", "a given id is kept");
}

#[test]
fn new_nodes_land_beside_the_json_node_not_inside_it() {
    let (mut app, j) = app_with_json("insertion", "[1]");
    app.current_group = Some(j.clone()); // before any frame syncs the level
    assert_eq!(app.insertion_group(), Some(OUTER.to_string()));
    app.current_group = Some(OUTER.into());
    assert_eq!(app.insertion_group(), Some(OUTER.to_string()));
}

#[test]
fn the_store_sees_nothing_of_the_level_but_view() {
    let (mut app, j) = app_with_json("store", r#"{"a": {"b": [1, 2, 3]}, "c": [1, 2]}"#);
    // (id, kind, group, fields without VIEW) for every stored node, as the mirror snapshot has it.
    let stored = |app: &App| {
        crate::session_sync::tests::snapshots(app)
            .into_iter()
            .map(|(id, mut s)| {
                s.fields.remove(&field::VIEW);
                (id, s.kind(), s.group, s.fields)
            })
            .collect::<Vec<_>>()
    };
    let before = stored(&app);
    assert_eq!(before.len(), 2, "both nodes are stored, so the comparison means something");
    app.switch_group_level(Some(j.clone()));
    // Selecting first: selecting reveals, which would undo the collapse.
    app.apply_json_edit(JsonEdit::Select("/a/b".into()));
    app.apply_json_edit(JsonEdit::Toggle("/a".into()));
    app.apply_json_edit(JsonEdit::More("/c".into()));
    app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(500.0, 40.0)));
    app.apply_json_edit(JsonEdit::Relayout);
    assert_eq!(stored(&app), before);
    // The toggle is still there after Relayout; the drag is not.
    let view = app.doc.mirror.view(&j).expect("a view");
    assert_eq!(view.get("/a").and_then(|o| o.collapsed), Some(true));
    assert_eq!(view.get("/a").and_then(|o| o.pos), None);
    app.switch_group_level(Some(OUTER.into()));
    assert_eq!(stored(&app), before);
}

#[test]
fn a_drag_saves_only_that_cards_position() {
    let (mut app, j) = app_with_json("drag", r#"{"a": {"x": 1}, "b": {"y": 2}}"#);
    app.switch_group_level(Some(j.clone()));
    let b_before = app.json_level.as_ref().unwrap().rects[2];
    app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(700.0, 10.0)));
    let level = app.json_level.as_ref().unwrap();
    assert_eq!(level.rects[1].min, egui::pos2(700.0, 10.0));
    assert_eq!(level.rects[2], b_before);
}

#[test]
fn rows_are_hit_by_their_world_position() {
    let (mut app, j) = app_with_json("rows", r#"{"a": 1, "b": {"c": 2}}"#);
    app.switch_group_level(Some(j));
    let level = app.json_level.as_ref().unwrap();
    let root = level.rects[0];
    let row =
        |i: f32| egui::pos2(root.center().x, root.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (i + 0.5));
    assert_eq!(level.row_at(0, row(0.0)), Some("/a".into()));
    assert_eq!(level.row_at(0, row(1.0)), Some("/b".into()));
    assert_eq!(
        level.row_at(0, egui::pos2(root.center().x, root.top() + 5.0)),
        None,
        "the header is the card, not a row"
    );
}

/// Wave 2 (H1b): a window past the first page puts `+ N earlier` under the header and every entry row one
/// lower; hit-testing and `entry_rect` follow it.
#[test]
fn a_moved_window_is_hit_below_its_earlier_row() {
    let mut view = JsonView::default();
    view.entry("").first = Some(2);
    let level = JsonLevel::new(J.into(), Arc::new(serde_json::json!((0..100).collect::<Vec<_>>())), &view);
    let (card, rect) = (&level.cards[0], level.rects[0]);
    assert_eq!((card.skip, card.rows, card.more, card.lead()), (40, 20, 40, 1));
    let row = |i: usize| {
        egui::pos2(rect.center().x, rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (i as f32 + 0.5))
    };
    assert!(level.earlier_row_at(0, row(0)) && !level.earlier_row_at(0, row(1)));
    assert_eq!(level.row_at(0, row(0)), None, "the earlier row is not an entry");
    assert_eq!(level.row_at(0, row(1)).as_deref(), Some("/40"));
    assert_eq!(level.row_at(0, row(20)).as_deref(), Some("/59"));
    assert_eq!(level.row_at(0, row(21)), None, "the more row is not an entry");
    assert!(level.more_row_at(0, row(21)) && !level.more_row_at(0, row(20)));
    let top = |p: &str| level.entry_rect(p).map(|r| r.top());
    assert_eq!(top("/40"), Some(rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT));
    assert_eq!(top("/59"), Some(rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * 20.0));
    assert_eq!((top("/39"), top("/60")), (None, None), "outside the window");
}

#[test]
fn copy_texts_follow_the_selection_and_the_path_form() {
    let (mut app, j) = app_with_json("copy", r#"{"users": [{"name": "Ada", "a/b": 1}]}"#);
    app.switch_group_level(Some(j));
    let copy = |app: &App, p: &str, what| app.json_copy_text(p, what);
    assert_eq!(copy(&app, "/users/0/name", CopyWhat::Value).as_deref(), Some("Ada"));
    assert_eq!(copy(&app, "/users/0", CopyWhat::Value).as_deref(), Some("{\n  \"name\": \"Ada\",\n  \"a/b\": 1\n}"));
    assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Key).as_deref(), Some("a/b"));
    assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Path).as_deref(), Some("/users/0/a~1b"));
    app.json_ui.json_accessor_paths = true;
    assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Path).as_deref(), Some(r#"users[0]["a/b"]"#));
    assert_eq!(copy(&app, "/nope", CopyWhat::Value), None);
}

#[test]
fn the_topmost_card_under_a_point_is_hit() {
    let (mut app, j) = app_with_json("card_at", r#"{"a": {"x": 1}}"#);
    app.switch_group_level(Some(j));
    app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(0.0, 0.0))); // onto the root
    let level = app.json_level.as_ref().unwrap();
    assert_eq!(level.card_at(egui::pos2(5.0, 5.0)), Some(1), "later cards draw on top");
    assert_eq!(level.card_at(egui::pos2(-50.0, -50.0)), None);
}

#[test]
fn a_remotely_deleted_json_node_leaves_its_level() {
    let (mut app, j) = app_with_json("remote_delete", r#"{"a": [1]}"#);
    app.switch_group_level(Some(j.clone()));
    // What `remote_changes` reports when another replica deletes J.
    app.apply_remote(vec![(j.parse().unwrap(), Remote::Deleted)]);
    assert!(!app.nodes.contains_key(&j));
    assert_eq!(app.current_group.as_deref(), Some(OUTER), "the view leaves the removed level");
    assert!(app.json_level.is_none());
    assert_eq!(app.insertion_group().as_deref(), Some(OUTER), "new nodes land in a group that exists");
}

#[test]
fn a_failed_refresh_leaves_the_level_for_the_error_card() {
    let dir = crate::store::testutil::temp_dir("json_level-failed_refresh");
    let mut app = App::default();
    app.place_group(Some(OUTER.into()), "outer", Placement { group: None, pos: egui::Vec2::ZERO });
    let body = crate::redirect_fetch::save(&dir, J, crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    let source = ContentSource::Web { url: "https://x.test/j".into(), body };
    app.insert_content(source, J.into(), egui::Vec2::ZERO, Some(OUTER.into())).unwrap();
    app.switch_group_level(Some(J.into()));
    assert!(app.json_level.is_some());
    let bad = WebBody::File {
        kind: crate::content::ContentKind::Json,
        path: dir.join("missing.json").to_string_lossy().into_owned(),
    };
    app.reload_fetched_node(J, bad);
    assert!(app.nodes[J].as_json().unwrap().error.is_some(), "the refresh failed");
    // Still the Json node's own group before any frame syncs the level.
    assert_eq!(app.insertion_group().as_deref(), Some(OUTER));
    app.sync_json_level(); // the start of the next frame
    assert_eq!(app.current_group.as_deref(), Some(OUTER), "no level is built from the placeholder");
    assert!(app.json_level.is_none());
}

#[test]
fn a_failed_reload_keeps_the_view() {
    let dir = crate::store::testutil::temp_dir("json_level-failed_reload_view");
    let mut app = App::default();
    let body = crate::redirect_fetch::save(&dir, J, crate::content::ContentKind::Json, br#"{"a": {"b": 1}}"#.to_vec())
        .unwrap();
    let source = ContentSource::Web { url: "https://x.test/j".into(), body };
    app.insert_content(source, J.into(), egui::Vec2::ZERO, None).unwrap();
    app.switch_group_level(Some(J.into()));
    app.apply_json_edit(JsonEdit::Toggle("/a".into()));
    app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(500.0, 40.0)));
    let before = app.doc.mirror.view(J).expect("a view").to_field();
    assert_ne!(before, "", "the view has overrides to lose");
    let bad = WebBody::File {
        kind: crate::content::ContentKind::Json,
        path: dir.join("missing.json").to_string_lossy().into_owned(),
    };
    app.reload_fetched_node(J, bad);
    let json = app.nodes[J].as_json().unwrap();
    assert!(json.error.is_some(), "the reload failed");
    assert_eq!(app.doc.mirror.view(J).expect("a view").to_field(), before, "the error card keeps the view for Retry");
}

/// Ruling 8: a reload removes the node and inserts it again, but the view lives in the mirror, which keeps it;
/// only overrides the new value can't resolve are dropped.
#[test]
fn a_reload_keeps_the_view() {
    let dir = crate::store::testutil::temp_dir("json_level-reload_view");
    let mut app = App::default();
    let first = br#"{"a": {"b": 1}, "c": {"d": 2}}"#.to_vec();
    let body = crate::redirect_fetch::save(&dir, J, crate::content::ContentKind::Json, first).unwrap();
    let source = ContentSource::Web { url: "https://x.test/j".into(), body };
    app.insert_content(source, J.into(), egui::Vec2::ZERO, None).unwrap();
    app.switch_group_level(Some(J.into()));
    app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(500.0, 40.0)));
    app.apply_json_edit(JsonEdit::Moved("/c".into(), egui::pos2(900.0, 40.0)));
    let second = br#"{"a": {"b": 3}}"#.to_vec();
    let body = crate::redirect_fetch::save(&dir, J, crate::content::ContentKind::Json, second).unwrap();
    app.reload_fetched_node(J, body);
    assert!(app.nodes[J].as_json().unwrap().error.is_none(), "the reload worked");
    let view = app.doc.mirror.view(J).expect("the view survived the reload");
    assert_eq!(view.get("/a").and_then(|o| o.pos), Some([500.0, 40.0]));
    assert!(view.get("/c").is_none(), "an override the new value can't resolve is dropped");
}

#[test]
fn a_refetch_placeholder_is_not_entered_and_keeps_its_view() {
    let dir = crate::store::testutil::temp_dir("json_level-placeholder");
    let mut app = App::default();
    app.place_group(Some(OUTER.into()), "outer", Placement { group: None, pos: egui::Vec2::ZERO });
    // What `materialize` inserts for a missing file with a known URL, its view already the mirror's.
    let view = JsonView::from_field(r#"{"/a":{"collapsed":true}}"#);
    app.edit_view(J, &view.to_field());
    let payload = JsonPayload {
        path: dir.join("not-fetched-yet.json").to_string_lossy().into_owned(),
        value: Default::default(),
        error: None,
    };
    app.insert_placeholder_node(J.into(), egui::Vec2::ZERO, Kind::Json, Runtime::Json(payload));
    let url = crate::store::model::FieldValue::Str("https://x.test/j".into());
    app.doc.mirror.set(J, crate::store::model::field::URL, url);
    app.edit_group(J, Some(OUTER.into()));
    assert!(!app.nodes[J].as_json().unwrap().enterable());
    app.switch_group_level(Some(J.into()));
    assert_eq!(app.current_group.as_deref(), Some(OUTER), "entering leaves for the placeholder's group");
    assert!(app.json_level.is_none());
    assert_eq!(app.doc.mirror.view(J), Some(&view));
}

#[test]
fn a_drop_from_a_json_level_leaves_it_and_lands_where_the_user_sees_it() {
    let (mut app, j) = app_with_json("create", r#"{"a": 1}"#);
    app.view_mode = ViewMode::Graph;
    app.switch_group_level(Some(j.clone()));
    // A drop leaves the level first, then lands where the user sees it.
    let file = crate::store::testutil::temp_dir("json_level-create-drop").join("dropped.txt");
    std::fs::write(&file, b"x").unwrap();
    app.drop_file(file, egui::pos2(300.0, 300.0));
    assert_eq!(app.current_group.as_deref(), Some(OUTER));
    assert!(app.json_level.is_none());
    let new: Vec<_> = app.nodes.iter().filter(|(_, n)| n.as_note().is_some()).map(|(id, _)| id).collect();
    assert_eq!(new.len(), 1, "the dropped text file is a note");
    assert_eq!(app.doc.mirror.group(new[0]), Some(OUTER));
}

#[test]
fn selecting_a_row_already_open_in_the_outline_does_not_rebuild_it() {
    let (mut app, j) = app_with_json("outline_rebuild", r#"{"a": {"b": 1}, "c": {"d": 2}}"#);
    app.switch_group_level(Some(j));
    app.poll_json_search();
    let rows =
        |app: &App| app.json_level.as_ref().unwrap().outline.rows.iter().map(|r| r.pointer.clone()).collect::<Vec<_>>();
    assert_eq!(rows(&app), ["", "/a", "/c"]);
    let built = app.json_level.as_ref().unwrap().outline.rows.as_ptr();
    app.apply_json_edit(JsonEdit::Select("/c".into()));
    let outline = &app.json_level.as_ref().unwrap().outline;
    assert!(!outline.dirty, "the root was already open");
    assert_eq!(outline.scroll_to.as_deref(), Some("/c"));
    app.poll_json_search();
    assert_eq!(app.json_level.as_ref().unwrap().outline.rows.as_ptr(), built, "the same rows, not rebuilt");
    // Selecting inside a closed container opens it: a rebuild.
    app.apply_json_edit(JsonEdit::Select("/a/b".into()));
    assert!(app.json_level.as_ref().unwrap().outline.dirty);
    app.poll_json_search();
    assert_eq!(rows(&app), ["", "/a", "/a/b", "/c"]);
}

#[test]
fn selecting_a_hidden_entry_reveals_it_and_saves_the_reveal() {
    let users: Vec<_> = (0..45).map(|i| serde_json::json!({"name": format!("u{i}")})).collect();
    let (mut app, j) = app_with_json("reveal", &serde_json::json!({ "users": users }).to_string());
    app.switch_group_level(Some(j.clone()));
    app.apply_json_edit(JsonEdit::Toggle("/users".into()));
    app.apply_json_edit(JsonEdit::Select("/users/33/name".into()));
    let level = app.json_level.as_ref().unwrap();
    assert!(level.index.contains_key("/users/33"), "the entry's card is on the canvas");
    assert_eq!(level.selected.as_deref(), Some("/users/33/name"));
    for open in ["", "/users", "/users/33"] {
        assert!(level.outline.open.contains(open), "the outline opens {open:?}");
    }
    let view = app.doc.mirror.view(&j).expect("a view");
    assert_eq!(view.pages("/users"), 2, "the reveal is saved like a manual one");
}

#[test]
fn selecting_a_pointer_that_does_not_resolve_is_ignored() {
    let (mut app, j) = app_with_json("select_missing", r#"{"a": {"b": 1}}"#);
    app.switch_group_level(Some(j));
    app.apply_json_edit(JsonEdit::Select("/a/b".into()));
    app.apply_json_edit(JsonEdit::Select("/a/nope".into()));
    assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/a/b"));
}

#[test]
fn selecting_pans_only_to_an_entry_out_of_view() {
    let users: Vec<_> = (0..45).map(|i| serde_json::json!({"name": format!("u{i}")})).collect();
    let (mut app, j) = app_with_json("pan", &serde_json::json!({ "a": 1, "users": users }).to_string());
    app.switch_group_level(Some(j));
    let level = app.json_level.as_ref().unwrap();
    let root = level.rects[level.index[""]];
    let row = level.entry_rect("/a").unwrap();
    assert!(root.contains_rect(row) && row.height() == json_view::ROW_HEIGHT, "a row on its parent's card");
    app.camera.jump_to(root.expand(10.0));
    app.apply_json_edit(JsonEdit::Select(String::new()));
    app.apply_json_edit(JsonEdit::Select("/a".into()));
    assert!(app.camera.tween.is_none(), "a visible entry leaves the camera where it was");
    app.apply_json_edit(JsonEdit::Select("/users/40/name".into()));
    let to = app.camera.tween.as_ref().expect("an off-screen entry pans").to;
    let target = app.json_level.as_ref().unwrap().entry_rect("/users/40/name").unwrap();
    assert!(to.contains_rect(target), "the camera heads to where the entry is in view");
}

#[test]
fn keys_walk_the_outline_and_expand_cards() {
    let (mut app, j) = app_with_json("keys", r#"{"a": {"x": 1}, "b": 2}"#);
    app.switch_group_level(Some(j));
    let selected = |app: &App| app.json_level.as_ref().unwrap().selected.clone();
    app.step_json_selection(1);
    assert_eq!(selected(&app).as_deref(), Some(""), "nothing selected: the first row");
    app.step_json_selection(1);
    assert_eq!(selected(&app).as_deref(), Some("/a"));
    // /a is closed in the outline, so Down skips its child; Right opens it.
    app.open_json_selection(true);
    app.step_json_selection(1);
    assert_eq!(selected(&app).as_deref(), Some("/a/x"));
    app.step_json_selection(1);
    app.step_json_selection(1);
    assert_eq!(selected(&app).as_deref(), Some("/b"), "clamped at the last row");
    app.step_json_selection(-1);
    app.step_json_selection(-1);
    assert_eq!(selected(&app).as_deref(), Some("/a"));
    // Left closes the open row, then selects the parent.
    app.open_json_selection(false);
    assert_eq!(selected(&app).as_deref(), Some("/a"));
    app.open_json_selection(false);
    assert_eq!(selected(&app).as_deref(), Some(""));
    // Enter toggles the selected container's card.
    let collapsed = |app: &App| {
        json_view::is_collapsed(
            app.doc.mirror.view(J).unwrap_or(&json_view::NO_VIEW),
            &app.json_level.as_ref().unwrap().expanded,
            "/a",
        )
    };
    let before = collapsed(&app);
    app.apply_json_edit(JsonEdit::Select("/a".into()));
    app.toggle_json_selection();
    assert_ne!(collapsed(&app), before);
}

#[test]
fn search_steps_through_matches_and_filters_the_outline() {
    let (mut app, j) = app_with_json("search", r#"{"a": {"name": "x1"}, "b": {"name": "x2"}, "c": 3}"#);
    app.switch_group_level(Some(j));
    app.set_json_search("X".into());
    app.poll_json_search();
    let rows =
        |app: &App| app.json_level.as_ref().unwrap().outline.rows.iter().map(|r| r.pointer.clone()).collect::<Vec<_>>();
    assert_eq!(rows(&app), ["", "/a", "/a/name", "/b", "/b/name"]);
    app.step_json_match();
    assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/a/name"));
    app.step_json_match();
    assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/b/name"));
    app.step_json_match();
    assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/a/name"), "wraps around");
    app.set_json_search(String::new());
    app.poll_json_search();
    // The two selections opened /a and /b in the outline.
    assert_eq!(rows(&app), ["", "/a", "/a/name", "/b", "/b/name", "/c"]);
}

#[test]
fn a_big_value_is_searched_on_a_worker() {
    let big: Vec<u32> = (0..json_view::WORKER_SEARCH_NODES as u32 + 1).collect();
    let (mut app, j) = app_with_json("worker", &serde_json::to_string(&big).unwrap());
    app.switch_group_level(Some(j));
    app.set_json_search("99999".into());
    assert!(app.json_level.as_ref().unwrap().search.pending.is_some());
    for _ in 0..1000 {
        app.poll_json_search();
        if app.json_level.as_ref().unwrap().search.result.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result = app.json_level.as_ref().unwrap().search.result.clone().expect("the worker answers");
    assert_eq!(result.matches, ["/99999"]);
}

/// Leaving the level marks its worker's search stale (`Drop for Search`), so `search_with` stops at its next
/// check rather than walking the rest of a big value.
#[test]
fn leaving_the_level_stops_its_worker() {
    let items: Vec<String> = (0..json_view::WORKER_SEARCH_NODES + 10).map(|i| format!("v{i}")).collect();
    let (mut app, j) = app_with_json("leave", &serde_json::to_string(&items).unwrap());
    app.switch_group_level(Some(j));
    app.set_json_search("v".into());
    let level = app.json_level.as_ref().unwrap();
    let (value, generation) = (level.value.clone(), level.search.generation.clone());
    let started = generation.load(Ordering::Relaxed);
    app.json_level = None;
    let checks = std::cell::Cell::new(0);
    let stale = || {
        checks.set(checks.get() + 1);
        generation.load(Ordering::Relaxed) != started
    };
    assert!(json_view::search_with(&value, "v", &stale).is_none(), "a worker of the left level stops");
    assert_eq!(checks.get(), 1, "at its first check");
}

/// Review D-B6: while a worker searches, the previous query's result is gone.
#[test]
fn a_pending_search_hides_the_previous_result() {
    let items: Vec<String> = (0..json_view::WORKER_SEARCH_NODES + 10).map(|i| format!("v{i}")).collect();
    let (mut app, j) = app_with_json("pending", &serde_json::to_string(&items).unwrap());
    app.switch_group_level(Some(j));
    app.set_json_search("v1".into());
    // Let v1's worker answer first, so there is an old result to go stale.
    for _ in 0..1000 {
        app.poll_json_search();
        if app.json_level.as_ref().unwrap().search.result.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.json_level.as_ref().unwrap().search.result.is_some(), "v1's worker answers");
    app.set_json_search("v2".into());
    let level = app.json_level.as_ref().unwrap();
    assert!(level.search.pending.is_none() || level.search.result.is_none(), "no stale result beside a pending search");
}

/// Escape in a text field (the search box) ends the edit only; egui drops the field's focus before any
/// widget runs, so only `shortcuts::text_field_has_keys` still knows the key was the field's.
#[test]
fn escape_in_a_text_field_keeps_the_json_selection() {
    let (mut app, j) = app_with_json("escape", r#"{"a": 1}"#);
    app.switch_group_level(Some(j));
    app.apply_json_edit(JsonEdit::Select("/a".into()));
    let ctx = egui::Context::default();
    let field = egui::Id::new("json_search_field");
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
    let escape = egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut app, vec![escape.clone()]);
    let selected = |app: &App| app.json_level.as_ref().unwrap().selected.clone();
    assert_eq!(selected(&app).as_deref(), Some("/a"), "Escape ended the edit only");
    frame(&mut app, vec![escape]);
    assert_eq!(selected(&app), None, "with no field focused, Escape clears the selection");
}

/// M5: a view in a format this build can't read (a newer build's) is never overwritten by an edit here, which
/// would replace it on every replica.
#[test]
fn an_unreadable_view_is_never_overwritten() {
    use crate::store::model::{field, FieldValue};
    let (mut app, j) = app_with_json("opaque-view", r#"{"a": {"b": 1}, "c": [1, 2]}"#);
    app.edit_view(&j, "[1]");
    app.switch_group_level(Some(j.clone()));
    app.apply_json_edit(JsonEdit::Toggle("/a".into()));
    // A live drag moves the card's rect frame by frame (`json_level_ui`); the refused drop must put it back.
    let a = app.json_level.as_ref().unwrap().index["/a"];
    let laid = app.json_level.as_ref().unwrap().rects[a];
    let dropped = laid.translate(egui::vec2(300.0, 0.0));
    app.json_level.as_mut().unwrap().rects[a] = dropped;
    app.apply_json_edit(JsonEdit::Moved("/a".into(), dropped.min));
    assert_eq!(app.doc.mirror.get(&j, field::VIEW), FieldValue::Str("[1]".into()));
    assert!(app.doc.mirror.view(&j).is_some_and(|view| view.is_opaque()));
    assert_eq!(app.json_level.as_ref().unwrap().rects[a], laid, "the refused drag snaps back to the layout");
}

/// A virtual source's cards are drawn only by `draw_graph_cards`, which a JSON level does not run, so they must not
/// be listed there: they would be hit-tested, selected and counted while invisible.
#[test]
fn a_json_level_lists_no_virtual_entries() {
    let (mut app, j) = app_with_json("no-virtual", r#"{"a": 1}"#);
    app.register_virtual_source(std::rc::Rc::new(crate::virtual_source::fake::FakeSource::default()));
    assert!(app.level_items().iter().any(|i| matches!(i.origin, Origin::Virtual(_))), "listed on a stored level");
    app.switch_group_level(Some(j));
    assert!(app.json_level.is_some());
    assert!(app.level_items().iter().all(|i| i.origin == Origin::Json), "{:?}", app.level_items());
}

/// A JSON level's cards carry their own kind row, which grants no ability: the shared chrome (selection, drop
/// targets, the marquee) then needs no `Origin::Json` special case.
#[test]
fn a_json_level_card_has_no_ability_through_its_own_row() {
    let (mut app, j) = app_with_json("no-abilities", r#"{"a": {"b": 1}}"#);
    app.switch_group_level(Some(j));
    let items = app.level_items();
    assert!(!items.is_empty());
    for item in &items {
        let caps = item.caps();
        assert!(
            !(caps.selectable || caps.drag_source || caps.drop_target || caps.menu || caps.stored_controls),
            "{item:?}: {caps:?}"
        );
    }
}
