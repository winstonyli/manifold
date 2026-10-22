//! Session-sync tests: local edit operations and what they commit.

use super::*;

#[test]
fn a_json_nodes_view_round_trips_through_the_store() {
    let (mut app, node) = app_with_json_file("json_view_field");
    app.edit_view(node, r#"{"/users":{"collapsed":true}}"#);

    let shared = snapshots(&app);
    let fields = &shared[&node].fields;
    assert_eq!(fields.get(&field::VIEW), Some(&FieldValue::Str(r#"{"/users":{"collapsed":true}}"#.into())));

    // Restoring those fields gives the same view back.
    let mut restored = App::default();
    restored.materialize(node, crate::store::model::TYPE_JSON, fields, None, None);
    assert_eq!(restored.doc.mirror.view(node).unwrap().get("/users").unwrap().collapsed, Some(true));
}

/// Ruling 6: typing writes the mirror only when a save or the blur commits it, never per keystroke.
#[test]
fn typing_commits_on_blur_and_at_save() {
    let path = temp_dir("text-commit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.text(note), "v1");
    type_into(&mut app, note, "v2");
    assert_eq!(app.doc.mirror.text(note), "v1", "no mirror write per keystroke");
    assert_eq!(app.note_text(note), "v2", "the card shows the buffer while it is typed in");
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("v2".into()), "a save while editing commits it");
    assert_eq!(app.doc.mirror.text(note), "v2");
    assert!(app.nodes[&note].as_note().unwrap().editing, "a save doesn't end the edit");
    type_into(&mut app, note, "v3");
    app.blur_note(note);
    assert!(!app.nodes[&note].as_note().unwrap().editing);
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("v3".into()), "the blur commits it");
    assert_eq!((app.doc.mirror.text(note), app.note_text(note)), ("v3", "v3"));
}

/// Ruling 9: the rename commit is the group name's writer (`edit_name`), and another replica's rename reaches the
/// mirror.
#[test]
fn a_rename_saves_and_a_remote_rename_reaches_the_mirror() {
    let path = temp_dir("name-rename").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    assert!(app.save_session());
    assert_eq!(stored(&app, g, field::NAME), FieldValue::Str("G".into()), "a new group's Create carries its name");
    app.apply_node_action(&egui::Context::default(), g, NodeAction::CommitRename("Errands".into()));
    assert_eq!(app.doc.mirror.name(g), "Errands", "the mirror shows it at once");
    assert!(app.save_session());
    assert_eq!(stored(&app, g, field::NAME), FieldValue::Str("Errands".into()));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, g, field::NAME, FieldValue::Str("Theirs".into()));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.name(g), "Theirs");
    assert_eq!(stored(&app, g, field::NAME), FieldValue::Str("Theirs".into()), "not diffed back");
}

/// Ruling 8: a view edit (`edit_view`) saves, and the decoded cache follows every change of the mirror's view,
/// another replica's too.
#[test]
fn a_json_view_edit_saves_and_a_remote_one_refreshes_the_decoded_cache() {
    let path = temp_dir("view-edit").join("session.mnf");
    let (mut app, node) = json_on(&path, "view-edit-file");
    assert!(app.save_session());
    let users = r#"{"/users":{"collapsed":true}}"#;
    app.edit_view(node, users);
    assert_eq!(app.doc.mirror.view(node).and_then(|v| v.get("/users")).and_then(|o| o.collapsed), Some(true));
    assert!(app.save_session());
    assert_eq!(stored(&app, node, field::VIEW), FieldValue::Str(users.into()));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, node, field::VIEW, FieldValue::Str(r#"{"":{"collapsed":true}}"#.into()));
    assert!(app.save_session());
    let view = app.doc.mirror.view(node).expect("a view");
    assert_eq!(view.get("").and_then(|o| o.collapsed), Some(true), "the cache shows theirs");
    assert!(view.get("/users").is_none(), "and only theirs");
}

/// Ruling 2: a drag release writes the document pos; physics (a settle) moves only the layout.
#[test]
fn a_drag_release_saves_the_dropped_position() {
    let path = temp_dir("pos-drag").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 10.0));
    let other = a_note_at(&mut app, egui::vec2(52.0, 10.0));
    assert!(app.save_session());
    drag(&mut app, &[note], egui::vec2(40.0, 0.0));
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(10.0, 10.0), "a drag in progress is not saved");
    app.release_drag();
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(50.0, 10.0)), "shown at once");
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(50.0, 10.0));

    app.settle_current_level();
    assert_ne!(app.layout_pos(note), egui::vec2(50.0, 10.0), "the overlapping cards were pushed apart");
    assert_ne!(app.layout_pos(other), egui::vec2(52.0, 10.0));
    assert!(app.save_session());
    assert!(!app.doc.writes.has_fields());
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(50.0, 10.0), "physics never writes the document");
    assert_eq!(stored(&app, other, field::POS), FieldValue::Pos(52.0, 10.0));
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(50.0, 10.0)));
}

/// Closing a tab while its wake is in flight deletes it from the store
/// once the (now-moot) spawn result lands -- the landing's "closed
/// while waking" branch used to return without saving, and a save
/// while the wake was in flight kept the id anyway (`spawning`), so it
/// lingered in the store forever (Fix 3, final-review).
#[test]
fn closing_a_tab_while_it_wakes_deletes_it_from_the_store() {
    let path = temp_dir("close-waking").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    assert!(app.save_session());
    app.focus_tab(tab);
    assert_eq!(app.test_spawns.len(), 1, "the wake is in flight, parked");
    assert!(app.spawns.spawning.contains_key(&tab));
    app.destroy_tab(tab);
    assert!(!app.nodes.contains_key(&tab), "closed");
    // Deliver the parked wake's result -- a failed one, the only kind a
    // unit test can produce (no fake `ViewTab` for a success).
    fail_spawns(&mut app);
    let still_live = app.doc.journal.as_ref().unwrap().doc().live().any(|(id, _)| id == tab);
    assert!(!still_live, "should have been deleted once the wake landed on a closed tab");
}

/// Pinning is not a tab's alone: a group toggles like a tab, and an unknown id is left alone.
#[test]
fn toggle_pin_works_on_any_node() {
    let mut app = app_on(&temp_dir("pin-any").join("session.mnf"), 1);
    let group = add_group(&mut app, "g");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    for id in [group, tab] {
        assert!(!app.doc.mirror.pinned(id));
        app.toggle_pin(id);
        assert!(app.doc.mirror.pinned(id), "{id} pinned");
        app.toggle_pin(id);
        assert!(!app.doc.mirror.pinned(id), "{id} unpinned");
    }
    app.toggle_pin(node(99));
    assert!(!app.doc.mirror.pinned(node(99)));
}

#[test]
fn closing_a_jar_leaves_the_tabs_that_named_it_on_the_default_jar() {
    let mut app = App::default();
    let default = default_jar_id(&mut app);
    let jar = add_jar(&mut app, "Work");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    app.set_jar_edge(tab, Some(jar));
    app.close_node(jar);
    assert_eq!(app.jar_defs().len(), 1, "only the Default is left");
    // The edge dangles: the field still names the closed jar, and the read rule resolves it to the Default jar.
    assert_eq!(app.doc.mirror.jar(tab), Some(jar));
    assert_eq!(app.effective_jar(tab), Some((default, true)));
}

#[test]
fn a_local_color_edit_is_committed_by_the_next_save() {
    let path = temp_dir("edit-field").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.edit_field(note, field::COLOR, red());
    assert_eq!(app.doc.mirror.color(note), Some(egui::Color32::RED), "the mirror shows it at once");
    assert_eq!(stored(&app, note, field::COLOR), FieldValue::Color(None), "nothing is written before a save");
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::COLOR), red());
    assert!(!app.doc.writes.has_fields(), "a commit clears what it wrote");
}

/// The node menu's colour actions (regression guards: they go through `edit_field`).
#[test]
fn the_set_color_action_edits_the_mirror_and_the_next_save_stores_it() {
    let path = temp_dir("menu-set-color").join("session.mnf");
    let mut app = app_on(&path, 1);
    let ctx = egui::Context::default();
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.apply_node_action(&ctx, note, NodeAction::SetColor(Some(egui::Color32::RED)));
    assert_eq!(app.doc.mirror.color(note), Some(egui::Color32::RED), "the mirror shows it at once");
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::COLOR), red());
    app.apply_node_action(&ctx, note, NodeAction::SetColor(None));
    assert_eq!(app.doc.mirror.color(note), None);
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::COLOR), FieldValue::Color(None));
}

#[test]
fn the_cycle_color_action_steps_through_the_tag_colors() {
    use crate::theme::next_tag_color;
    let path = temp_dir("menu-cycle-color").join("session.mnf");
    let mut app = app_on(&path, 1);
    let ctx = egui::Context::default();
    let note = a_note(&mut app);
    assert!(app.save_session());
    let first = next_tag_color(None);
    assert!(first.is_some());
    app.apply_node_action(&ctx, note, NodeAction::CycleColor);
    assert_eq!(app.doc.mirror.color(note), first);
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::COLOR), color_value(first));
    let second = next_tag_color(first);
    app.apply_node_action(&ctx, note, NodeAction::CycleColor);
    assert_eq!(app.doc.mirror.color(note), second);
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::COLOR), color_value(second));
}

#[test]
fn a_color_action_on_an_unknown_node_changes_nothing() {
    let mut app = App::default();
    let ctx = egui::Context::default();
    let ghost = node(99);
    app.apply_node_action(&ctx, ghost, NodeAction::SetColor(Some(egui::Color32::RED)));
    app.apply_node_action(&ctx, ghost, NodeAction::CycleColor);
    assert_eq!(app.doc.mirror.color(ghost), None);
    assert!(!app.doc.writes.has_fields());
}

#[test]
fn a_tab_pin_is_committed_and_a_remote_pin_reaches_the_mirror() {
    let path = temp_dir("pin-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0));
    assert!(app.save_session());
    app.edit_field(tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.doc.mirror.pinned(tab));
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::PINNED), FieldValue::Bool(true));

    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::PINNED, FieldValue::Bool(false));
    assert!(app.save_session());
    assert!(!app.doc.mirror.pinned(tab));
}

#[test]
fn the_pin_shortcut_pins_a_focused_note() {
    let mut app = App::default();
    let note = a_note(&mut app);
    app.focused = Some(note);
    app.pin_shortcut();
    assert!(app.doc.mirror.pinned(note));
}

#[test]
fn the_pin_shortcut_pins_a_whole_selection_then_unpins_it() {
    let mut app = App::default();
    let (a, b, c) = (a_note(&mut app), a_note(&mut app), a_note(&mut app));
    app.toggle_pin(a);
    app.focused = Some(c);
    app.interaction.selected.extend([crate::card_id::CardId::from(a), crate::card_id::CardId::from(b)]);
    app.pin_shortcut();
    assert!([a, b].iter().all(|&id| app.doc.mirror.pinned(id)), "one was unpinned, so all get pinned");
    assert!(!app.doc.mirror.pinned(c), "the focused card is not in the selection");
    app.pin_shortcut();
    assert!([a, b].iter().all(|&id| !app.doc.mirror.pinned(id)), "all were pinned, so all get unpinned");
}

#[test]
fn pin_all_pins_every_tab_and_only_tabs() {
    let mut app = App::default();
    let (a, b) = (
        add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0)),
        add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0)),
    );
    let note = a_note(&mut app);
    app.set_all_pinned(true);
    assert!(app.doc.mirror.pinned(a) && app.doc.mirror.pinned(b) && !app.doc.mirror.pinned(note));
    app.set_all_pinned(false);
    assert!(!app.doc.mirror.pinned(a) && !app.doc.mirror.pinned(b));
}

/// A parent edit shows at once and is committed by the next save.
#[test]
fn a_parent_edit_reads_back_and_commits() {
    let path = temp_dir("parent-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
    );
    assert!(app.save_session());
    app.edit_parent(b, Some(a));
    assert_eq!(app.doc.mirror.parent(b), Some(a));
    assert_eq!(stored(&app, b, field::PARENT), FieldValue::Id(None), "not written before the save");
    assert!(app.save_session());
    assert_eq!(stored(&app, b, field::PARENT), FieldValue::Id(Some(a)));
}

#[test]
fn a_group_edit_reads_back_and_commits_as_a_move() {
    let path = temp_dir("group-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.edit_group(t, Some(g));
    assert_eq!(app.doc.mirror.group(t), Some(g));
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().effective_group(t), None);
    assert!(app.save_session());
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().effective_group(t), Some(g));
}

/// Dissolving a group moves its members up through the writer and the store agrees after a save.
#[test]
fn dissolving_a_group_moves_members_up_and_saves() {
    let path = temp_dir("group-dissolve").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "O");
    let inner = add_group(&mut app, "I");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_group(inner, Some(outer));
    app.edit_group(t, Some(inner));
    assert!(app.save_session());
    app.remove_node(inner);
    assert_eq!(app.doc.mirror.group(t), Some(outer));
    assert!(app.save_session());
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().effective_group(t), Some(outer));
}

/// A local move the store refuses as a cycle (another replica nested the target into the moved group first) leaves
/// the document acyclic, and the save still succeeds. The mirror shows what the document holds, not the refused
/// target: it would otherwise hold the cycle the store refused, indefinitely.
#[test]
fn a_move_the_store_refuses_as_a_cycle_leaves_the_document_acyclic() {
    let path = temp_dir("group-cycle").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = add_group(&mut app, "A");
    let b = add_group(&mut app, "B");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let nest = vec![OpBody::Move { node: b, group: Some(a) }];
    other.begin().unwrap().commit(nest).unwrap();
    app.edit_group(a, Some(b));
    assert!(app.save_session());
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(b), Some(a), "the remote move stands");
    assert_eq!(doc.effective_group(a), None, "the local move was refused");
    for _ in 0..2 {
        assert_eq!(app.doc.mirror.group(a), None, "the mirror shows the refused move as the document does");
        assert_eq!(app.doc.mirror.group(b), Some(a), "and the remote move");
        assert!(app.save_session());
    }
}

/// Ruling 4: a navigation is saved as it started (`navigate_tab` calls `edit_url` with the typed url), so a redirect
/// the page makes before the save stays the live url only.
#[test]
fn a_navigation_saves_the_url_it_started_with_not_the_redirect() {
    let path = temp_dir("url-redirect").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    app.edit_url(tab, "https://a/");
    set_live_url(&mut app, tab, "https://b/");
    assert_eq!(app.doc.mirror.url(tab), "https://a/", "shown at once");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://a/".into()));
    assert_eq!(app.doc.mirror.url(tab), "https://a/");
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://b/", "the live url is the page's");
}

/// Ruling 7: one writer for the field, queuing it only when it changed. Re-picking the same jar queues nothing.
#[test]
fn a_jar_pick_saves_the_field_and_only_when_it_changed() {
    let path = temp_dir("jar-pick").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = add_jar(&mut app, "Work");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let key = |no| (tab, no);
    app.set_jar_edge(tab, Some(work));
    assert_eq!(app.doc.mirror.jar(tab), Some(work), "shown at once");
    assert_eq!(app.doc.writes.fields().map(|(n, no, _)| (n, no)).collect::<Vec<_>>(), [key(field::JAR)]);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::JAR), FieldValue::Id(Some(work)));
    app.set_jar_edge(tab, Some(work));
    assert!(!app.doc.writes.has_fields(), "re-picking the same jar queues nothing");
    app.set_jar_edge(tab, None);
    assert_eq!(app.doc.writes.fields().map(|(n, no, _)| (n, no)).collect::<Vec<_>>(), [key(field::JAR)]);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::JAR), FieldValue::Id(None));
}

/// Closing a jar leaves a saved tab and a tab mid-respawn (its mirror entry outlives the respawn, Ruling 9) naming it,
/// and both resolve to the Default jar by the read rule (`resolve_jar`).
#[test]
fn closing_a_jar_leaves_saved_and_respawning_tabs_resolving_to_the_default_jar() {
    let path = temp_dir("jar-delete-doc").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = add_jar(&mut app, "Work");
    let asleep = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    let (live, _peer) = live_tab(&mut app, ViewKind::Servo, "https://l/");
    app.set_jar_edge(asleep, Some(work));
    app.set_jar_edge(live, Some(work));
    assert!(app.save_session());
    app.handle_view_message(live, manifold_view_protocol::ViewToHost::Crashed { reason: "boom".into() });
    assert!(app.spawns.spawning.contains_key(&live) && !app.nodes.contains_key(&live), "mid-respawn");
    app.close_node(work);
    assert!(app.save_session());
    let default = Some((default_jar_id(&mut app), true));
    for t in [asleep, live] {
        assert_eq!(stored(&app, t, field::JAR), FieldValue::Id(Some(work)), "nothing rewrites the edge");
        assert_eq!(app.effective_jar(t), default);
    }
    let _peers = land_spawns(&mut app);
    assert_eq!(app.effective_jar(live), default, "the respawn lands on the Default jar");
}

/// P1: closing a group commits once, so another replica never reads it half deleted and a crash can't leave part of
/// it.
#[test]
fn closing_a_group_commits_one_frame() {
    let path = temp_dir("group-close-once").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    for url in ["https://a.test/", "https://b.test/", "https://c.test/"] {
        let t = add(&mut app, Kind::Tab, sleeping_tab(url, "t"));
        app.edit_group(t, Some(g));
    }
    assert!(app.save_session());
    let frames = || crate::store::journal::inspect(&std::fs::read(&path).unwrap()).unwrap().frames.len();
    let before = frames();
    app.close_node(g);
    assert_eq!(frames() - before, 1, "one commit for the group and its tabs");
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert!(live_in_doc(&app, g) && doc.nodes[&g].closed(), "closed, not deleted");
}

/// Duplicate, Close then Reopen, and a JSON round trip each make a shell of the type: the type string and the
/// generic fields, never a newer build's field or a known one its kind doesn't carry (spec decision 4).
#[test]
fn an_unknown_card_copies_as_a_shell_of_its_type() {
    use crate::store::model::GENERIC;
    let (mut app, key, _path) = restored_unknown();
    app.edit_name(key, "Mine");
    assert!(app.save_session());
    assert_eq!(app.accessible_name(key).as_deref(), Some("Card: Mine"));
    let shell = |app: &App, id: NodeId| {
        let doc = app.doc.journal.as_ref().unwrap().doc();
        assert_eq!(doc.nodes[&id].ty(), FUTURE);
        doc_fields(app, id).iter().all(|no| *no == field::TYPE || *no == field::GROUP || GENERIC.contains(no))
    };

    app.duplicate_nodes(&[key]);
    let copies: Vec<NodeId> = app
        .nodes
        .iter()
        .filter(|(k, n)| **k != key && matches!(n.runtime, Runtime::Unknown))
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(app.doc.mirror.name(copies[0]), "Mine");
    assert!(shell(&app, copies[0]), "{:?}", doc_fields(&app, copies[0]));

    let orig = doc_fields(&app, key);
    app.close_node(key);
    assert!(!app.nodes.contains_key(&key));
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().nodes[&key].ty(), FUTURE, "the real string stays");
    app.reopen_closed_shortcut();
    let reopened: Vec<NodeId> = app
        .nodes
        .iter()
        .filter(|(k, n)| **k != copies[0] && matches!(n.runtime, Runtime::Unknown))
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(reopened, [key], "the same node comes back whole");
    let kept = doc_fields(&app, reopened[0]);
    assert!(orig.iter().all(|no| kept.contains(no)), "raw fields intact: {orig:?} vs {kept:?}");

    let json = crate::store::json::doc_to_json(app.doc.journal.as_ref().unwrap().doc());
    let mut counter = 0;
    let next = || {
        counter += 1;
        NodeId { actor: ActorId(9), counter }
    };
    let imported = crate::store::json::import(&json, next, crate::store::json::Trust::Foreign).unwrap();
    let shells: Vec<_> = imported.iter().filter(|i| i.ty == FUTURE).collect();
    assert_eq!(shells.len(), 2);
    for s in shells {
        assert!(s.fields.keys().all(|no| *no == field::GROUP || GENERIC.contains(no)), "{:?}", s.fields);
    }
}

#[test]
fn a_link_and_a_group_take_an_unknown_card() {
    let (mut app, key, _path) = restored_unknown();
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    let g = add_group(&mut app, "G");
    assert!(app.save_session());
    assert!(app.connect(crate::caps::LINK, key, note).is_ok());
    assert!(app.connect(crate::caps::OPENED_FROM, key, note).is_err(), "it opens nothing");
    assert!(app.save_session());
    let links =
        |app: &App| app.doc.journal.as_ref().unwrap().doc().live().filter(|(_, n)| n.kind() == Kind::Link).count();
    assert_eq!(links(&app), 1);
    app.edit_group(key, Some(g));
    assert!(app.save_session());
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(key), Some(g));
    assert_eq!(app.doc.mirror.group(key), Some(g));
}
