//! Session-sync tests: applying other actors' changes to the mirror and the live app.

use super::*;

/// Another replica's delete removes the node here and queues nothing: the document already lacks it.
#[test]
fn a_remote_removal_queues_no_delete() {
    let mut app = app_on(&temp_dir("remote-removal-queues-nothing").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    let group = add_group(&mut app, "G");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_group(tab, Some(group));
    let jar = add_jar(&mut app, "Work");
    assert!(app.save_session());
    for id in [note, tab, group, jar] {
        app.apply_remote(vec![(id, Remote::Deleted)]);
        assert!(!app.nodes.contains_key(&id) && !app.jar_defs().contains_key(&id), "{id} removed");
        assert!(!app.doc.writes.has_deletes(), "{id}: nothing queued");
    }
}

#[test]
fn a_tabs_and_a_groups_jar_edge_project_to_fields_and_back() {
    let jar = NodeId { actor: ActorId(7), counter: 1 };
    let nodes = BTreeMap::from([
        (
            node(1),
            shared(
                Kind::Tab,
                None,
                &[
                    (field::URL, FieldValue::Str("https://a.com/".into())),
                    (field::VIEW_KIND, FieldValue::ViewCode(view_code(ViewKind::Servo))),
                    (field::JAR, FieldValue::Id(Some(jar))),
                ],
            ),
        ),
        (node(2), shared(Kind::Group, None, &[(field::JAR, FieldValue::Id(Some(jar)))])),
    ]);
    let app = app_with(&nodes);
    assert!(app.nodes[&node(1)].runtime.as_unloaded().is_some());
    assert_eq!(app.doc.mirror.jar(node(1)), Some(jar));
    assert_eq!(app.doc.mirror.jar(node(2)), Some(jar));
    assert_eq!(snapshots(&app), nodes, "the live payloads project back to the same fields");
}

#[test]
fn a_remote_jar_edit_updates_a_sleeping_tab_and_a_group() {
    let jar = NodeId { actor: ActorId(2), counter: 50 };
    let tab = shared(Kind::Tab, None, &[(field::URL, FieldValue::Str("https://a.com/".into()))]);
    let group = shared(Kind::Group, None, &[]);
    let mut app = app_with(&BTreeMap::from([(node(1), tab.clone()), (node(2), group.clone())]));
    let mut tab_fields = tab.fields.clone();
    tab_fields.insert(field::JAR, FieldValue::Id(Some(jar)));
    let mut group_fields = group.fields.clone();
    group_fields.insert(field::JAR, FieldValue::Id(Some(jar)));
    app.apply_remote(vec![
        (node(1), Remote::Shown(snap(Kind::Tab, tab_fields, None))),
        (node(2), Remote::Shown(snap(Kind::Group, group_fields, None))),
    ]);
    assert_eq!(app.doc.mirror.jar(node(1)), Some(jar));
    assert_eq!(app.doc.mirror.jar(node(2)), Some(jar));
}

#[test]
fn a_restored_note_group_and_leaf_project_back_to_their_fields() {
    let red = color_value(Some(egui::Color32::from_rgb(200, 10, 10)));
    let nodes = BTreeMap::from([
        (
            node(0),
            shared(
                Kind::Note,
                Some(node(1)),
                &[
                    (field::TEXT, FieldValue::Str("hello".into())),
                    (field::POS, FieldValue::Pos(3.0, -4.0)),
                    (field::COLOR, red),
                ],
            ),
        ),
        (node(1), shared(Kind::Group, None, &[(field::NAME, FieldValue::Str("G".into()))])),
        (node(2), shared(Kind::Json, Some(node(1)), &[(field::PATH, FieldValue::Str("C:/data.json".into()))])),
    ]);
    assert_eq!(snapshots(&app_with(&nodes)), nodes);
}

#[test]
fn a_remote_view_change_updates_the_node_in_place() {
    let (mut app, node) = app_with_json_file("json_view_remote");
    let (value, path) = app.nodes[&node].as_json().map(|j| (j.value.clone(), j.path.clone())).unwrap();
    let mut fields = snapshots(&app)[&node].fields.clone();
    fields.insert(field::VIEW, FieldValue::Str(r#"{"":{"collapsed":true}}"#.into()));
    let snap = Snapshot { ty: crate::store::model::TYPE_JSON.into(), group: None, fields, trees: Default::default() };
    app.materialize(node, &snap, None);
    assert_eq!(app.doc.mirror.view(node).unwrap().get("").unwrap().collapsed, Some(true));
    let json = app.nodes[&node].as_json().unwrap();
    assert_eq!((&json.value, &json.path), (&value, &path), "only the view changed");
}

/// `materialize` seeds the mirror's `type` with the stored string as written, and a snapshot carries it back to the
/// store unchanged; the row it reads as is the canonical one.
#[test]
fn materialize_keeps_the_stored_type_string() {
    let mut app = App::default();
    let node = node(1);
    let fields = shared(Kind::Note, None, &[(field::TEXT, FieldValue::Str("n".into()))]).fields;
    let snap = Snapshot { ty: "Text/Markdown".into(), group: None, fields, trees: Default::default() };
    app.materialize(node, &snap, None);
    assert_eq!(app.doc.mirror.ty(node), "Text/Markdown");
    assert_eq!(app.card_kind(node).unwrap().ty, crate::store::model::TYPE_NOTE);
    assert_eq!(app.snapshot(node).unwrap().ty, "Text/Markdown");
}

#[test]
fn a_missing_content_file_with_a_known_url_becomes_a_placeholder_that_refetches() {
    let nodes = BTreeMap::from([(
        node(0),
        shared(
            Kind::Json,
            None,
            &[
                (field::PATH, FieldValue::Str(r"Z:\gone.json".into())),
                (field::URL, FieldValue::Str("https://x.test/a.json".into())),
            ],
        ),
    )]);
    let mut app = app_with(&nodes);
    let node_id = node(0);
    match &app.nodes[&node_id].runtime {
        Runtime::Json(p) => {
            assert!(p.error.is_none(), "a known-URL miss never reaches insert_json_node_with_id's own toast");
        }
        _ => panic!("expected a Json placeholder"),
    }
    assert_eq!(app.source_url(node_id), Some("https://x.test/a.json"));
    assert!(app.toasts.save_toast_queue.is_empty() && app.toasts.save_toast.is_none(), "no 'Couldn't open' toast");
    let results = drain_fetches(&mut app);
    assert_eq!(results.len(), 1, "materialize's own refresh_node queued exactly one fetch");
    assert!(matches!(&results[0].job.target, FetchTarget::Node(n) if *n == node_id));
    assert!(results[0].body.is_none(), "no content dir under test, so the fetch itself fails");
}

#[test]
fn a_remotely_removed_group_leaves_no_member_naming_it() {
    let nodes = BTreeMap::from([
        (node(0), group("Outer")),
        (node(1), shared(Kind::Group, Some(node(0)), &[(field::NAME, FieldValue::Str("Inner".into()))])),
        (node(2), note("stored member", Some(node(1)))),
    ]);
    let mut app = app_with(&nodes);
    // A note only this replica has, not yet saved, inside the group.
    let local = node(3);
    let payload = Runtime::Note(NotePayload { text: "local".into(), editing: false });
    app.insert_placeholder_node(local, egui::Vec2::ZERO, Kind::Note, payload);
    app.edit_group(local, Some(node(1)));
    app.current_group = Some(node(1));
    // What `remote_changes` reports when another replica deletes Inner:
    // the group gone, its stored member now shown in Outer.
    let member = nodes[&node(2)].fields.clone();
    app.apply_remote(vec![
        (node(1), Remote::Deleted),
        (node(2), Remote::Shown(snap(Kind::Note, member, Some(node(0))))),
    ]);
    let inner = node(1);
    assert!(!app.nodes.contains_key(&inner) && !app.node_order.contains(&inner));
    let outer = Some(node(0));
    assert_eq!(app.doc.mirror.group(node(2)), outer);
    assert_eq!(app.doc.mirror.group(local), outer, "the local-only member moves to the effective group");
    assert_eq!(app.current_group, outer, "the view leaves the removed level");
    assert!(app.nodes.keys().all(|n| app.doc.mirror.group(*n) != Some(inner)));
}

#[test]
fn a_remote_close_hides_without_deleting_reparenting_or_releasing() {
    let path = temp_dir("sync-remote-close").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let inner = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "in".into(), editing: false }));
    app.edit_group(inner, Some(g));
    let outside = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "out".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, failed_tab("https://t/", 1.25));
    app.edit_parent(outside, Some(tab));
    let _link = app.create_link(inner, outside);
    assert!(app.save_session());
    let links_before: Vec<NodeId> = app.doc.mirror.link_ids().collect();
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, g, true);
    remote_close(&mut other, tab, true);
    assert!(app.save_session());
    for id in [&g, &inner, &tab] {
        assert!(!app.nodes.contains_key(id) && !app.doc.mirror.has(*id), "{id} is hidden");
        assert!(live_in_doc(&app, *id), "{id} is still live in the document");
    }
    assert_eq!(app.doc.mirror.link_ids().collect::<Vec<_>>(), links_before, "the link to the outside node stays");
    assert_eq!(app.doc.mirror.parent(outside), Some(tab), "the child keeps its opened-from edge");
    assert_eq!(app.doc.hidden_local[&tab].zoom, Some(1.25), "the hidden tab's personal state is kept");
    assert!(!app.doc.writes.has_deletes(), "a hide queues no delete");
    // The clear brings them back, the group before its member, the tab with its zoom.
    remote_close(&mut other, g, false);
    remote_close(&mut other, tab, false);
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.group(inner), Some(g));
    assert!(matches!(&app.nodes[&tab].runtime, Runtime::Unloaded(t) if t.zoom_factor == 1.25));
    assert!(!app.doc.hidden_local.contains_key(&tab));
}

#[test]
fn a_remote_move_into_a_closed_group_hides_the_node() {
    let path = temp_dir("sync-remote-move-closed").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, g, true);
    let (node, group) = (note, Some(g));
    other.begin().unwrap().commit(vec![OpBody::Move { node, group }]).unwrap();
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&note));
}

#[test]
fn a_remote_close_of_the_viewed_group_leaves_to_the_nearest_shown_level() {
    let path = temp_dir("sync-remote-close-level").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "Outer");
    let inner = add_group(&mut app, "Inner");
    app.edit_group(inner, Some(outer));
    assert!(app.save_session());
    app.switch_group_level(Some(inner));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, inner, true);
    assert!(app.save_session());
    assert_eq!(app.current_group, Some(outer));
}

#[test]
fn a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another() {
    let path = temp_dir("sync-fields").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();

    // Unsaved here: typing in the note, and a navigation (the live url).
    if let Runtime::Note(n) = &mut app.nodes.get_mut(&note).unwrap().runtime {
        n.text = "typing".into();
    }
    if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
        t.url = "https://u1/".into();
    }
    let red = color_value(Some(egui::Color32::RED));
    remote_set(&mut other, note, field::COLOR, red.clone());
    remote_set(&mut other, tab, field::PINNED, FieldValue::Bool(true));

    // The remote round applies, then the next round commits.
    assert!(app.save_session());
    let live = &app.nodes[&note];
    assert_eq!(app.doc.mirror.color(note), Some(egui::Color32::RED));
    assert!(matches!(&live.runtime, Runtime::Note(n) if n.text == "typing"));
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("typing".into()));
    assert!(app.doc.mirror.pinned(tab));
    assert!(matches!(&app.nodes[&tab].runtime, Runtime::Unloaded(t) if t.url == "https://u1/"));
    assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://u0/".into()));

    // A later remote edit to the same field does apply.
    remote_set(&mut other, note, field::TEXT, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert!(matches!(&app.nodes[&note].runtime, Runtime::Note(n) if n.text == "theirs"));
    assert_eq!(app.doc.mirror.color(note), Some(egui::Color32::RED));
}

/// Ruling 7: a remote text change that arrives while the note is typed in leaves the box alone and is held. At the
/// blur, what the user typed wins and the held value is dropped; with nothing typed, the held value applies.
#[test]
fn a_remote_text_change_is_held_while_the_note_is_typed_in() {
    for typed in [true, false] {
        let path = temp_dir("text-held").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        type_into(&mut app, note, "v1");
        remote_set(&mut other, note, field::TEXT, FieldValue::Str("theirs".into()));
        remote_set(&mut other, note, field::COLOR, red());
        assert!(app.save_session());
        assert_eq!(buffer(&app, note), "v1", "the box is not overwritten");
        assert_eq!(app.doc.mirror.text(note), "v1", "held");
        assert_eq!(app.doc.writes.held_text.get(&note).map(String::as_str), Some("theirs"));
        assert_eq!(app.doc.mirror.color(note), Some(egui::Color32::RED), "their other field arrives");
        if typed {
            type_into(&mut app, note, "mine");
        }
        app.blur_note(note);
        let want = if typed { "mine" } else { "theirs" };
        assert_eq!((app.doc.mirror.text(note), buffer(&app, note).as_str()), (want, want), "typed: {typed}");
        assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str(want.into()), "typed: {typed}");
        assert!(app.doc.writes.held_text.is_empty(), "typed: {typed}");
    }
}

/// Ruling 7, a commit between: text a save already committed is older than a remote change held after it, so with
/// nothing typed since, the held value applies at the blur. A remote change that arrives while a typed edit is
/// pending loses to it, as any pending edit wins.
#[test]
fn a_held_text_change_follows_the_order_of_the_commits() {
    let path = temp_dir("text-held-order").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    // Typed, then their change, then a save: the save queues ours first (`commit_note_buffers`), so theirs is held
    // behind that pending edit, never remembered in `held_text`, and ours commits over it.
    type_into(&mut app, note, "mine");
    remote_set(&mut other, note, field::TEXT, FieldValue::Str("before".into()));
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("mine".into()));
    assert!(app.doc.writes.held_text.is_empty(), "held behind the pending edit, not remembered");
    // Their change after that commit, nothing typed since: theirs applies at the blur.
    remote_set(&mut other, note, field::TEXT, FieldValue::Str("after".into()));
    assert!(app.save_session());
    assert_eq!(buffer(&app, note), "mine");
    app.blur_note(note);
    assert_eq!((app.doc.mirror.text(note), buffer(&app, note).as_str()), ("after", "after"));
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("after".into()));
}

/// Ruling 7: a remote text held while the note is typed in is older than a local commit that follows it, so that
/// commit drops it (`save_session`), and the blur keeps what was typed.
#[test]
fn a_held_remote_text_is_dropped_by_a_later_local_commit() {
    let path = temp_dir("text-held-dropped").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    type_into(&mut app, note, "v1");
    remote_set(&mut other, note, field::TEXT, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert_eq!(app.doc.writes.held_text.get(&note).map(String::as_str), Some("theirs"), "held, nothing typed yet");
    type_into(&mut app, note, "mine");
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("mine".into()));
    assert!(app.doc.writes.held_text.is_empty(), "the commit came after theirs");
    app.blur_note(note);
    assert_eq!((app.doc.mirror.text(note), buffer(&app, note).as_str()), ("mine", "mine"));
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("mine".into()));
}

/// As before 2b-4: a note nobody is typing in shows another replica's text at once.
#[test]
fn a_remote_text_change_updates_an_idle_note() {
    let path = temp_dir("text-idle").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, note, field::TEXT, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert_eq!((app.doc.mirror.text(note), app.note_text(note)), ("theirs", "theirs"));
    assert_eq!(buffer(&app, note), "theirs");
    assert!(app.doc.writes.held_text.is_empty());
}

/// Ruling 9 with 2b-3 Ruling 3: a rename waiting for a save wins over another replica's.
#[test]
fn a_pending_local_rename_wins_over_a_remote_one() {
    let path = temp_dir("name-pending").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_name(g, "mine");
    remote_set(&mut other, g, field::NAME, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.name(g), "mine");
    assert_eq!(stored(&app, g, field::NAME), FieldValue::Str("mine".into()));
}

#[test]
fn url_changes_after_a_host_started_load_stay_local_until_it_settles() {
    let mut app = App::default();
    assert!(app.shares_url_change(node(7), None), "an ordinary tab shares");
    app.note_host_load(node(7));
    assert!(!app.shares_url_change(node(7), None), "a redirect right after the host's navigation");
    assert!(!app.shares_url_change(node(7), None), "still within the grace");
    app.spawns.host_loads.insert(node(7), std::time::Instant::now() - HOST_LOAD_GRACE);
    assert!(app.shares_url_change(node(7), None), "settled: the next navigation is shared");
    assert!(app.spawns.host_loads.is_empty());
}

/// An engine that says how a navigation started overrides the grace.
#[test]
fn the_view_kinds_cause_decides_whether_a_url_change_is_shared() {
    let mut app = App::default();
    assert!(!app.shares_url_change(node(7), Some(false)), "a script's navigation stays local");
    app.note_host_load(node(7));
    assert!(app.shares_url_change(node(7), Some(true)), "a click right after the host's load is shared");
    assert!(app.spawns.host_loads.is_empty(), "and ends the host's load");
}

/// Ruling 4: an unsaved local pos beats another replica's; their other fields still arrive, and the layout is not
/// snapped to theirs.
#[test]
fn a_local_pos_edit_wins_over_a_remote_one() {
    let path = temp_dir("pos-local-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    drag(&mut app, &[note], egui::vec2(80.0, 0.0));
    app.release_drag();
    remote_set(&mut other, note, field::POS, FieldValue::Pos(5.0, 6.0));
    remote_set(&mut other, note, field::COLOR, red());
    // The remote round applies, then the next round commits.
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(90.0, 20.0)));
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(90.0, 20.0));
    assert_eq!(app.layout_pos(note), egui::vec2(90.0, 20.0), "not snapped to theirs");
    assert_eq!(app.doc.mirror.color(note), Some(egui::Color32::RED), "their other field arrives");
}

/// Ruling 4: another replica's pos reaches the mirror and snaps the layout, at rest; nothing is written back.
#[test]
fn a_remote_pos_snaps_the_layout_and_zeroes_its_velocity() {
    let path = temp_dir("pos-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
    assert!(app.save_session());
    app.layout.insert(note, crate::Layout { pos: egui::vec2(12.0, 22.0), vel: egui::vec2(3.0, 4.0) });
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, note, field::POS, FieldValue::Pos(5.0, 6.0));
    let pos_stamp = |doc: &Doc| doc.nodes[&note].fields[&crate::store::model::RegKey::of(field::POS)].stamp;
    let theirs = pos_stamp(other.doc());
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(5.0, 6.0)));
    assert_eq!(app.layout[&note], crate::Layout { pos: egui::vec2(5.0, 6.0), vel: egui::Vec2::ZERO });
    assert!(app.save_session());
    assert_eq!(pos_stamp(app.doc.journal.as_ref().unwrap().doc()), theirs, "nothing written back");
}

/// Ruling 4: a card the user is dragging is not snapped by a remote pos; its release then wins.
#[test]
fn a_remote_pos_does_not_snap_a_node_being_dragged() {
    let path = temp_dir("pos-remote-drag").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (a_note_at(&mut app, egui::vec2(10.0, 20.0)), a_note_at(&mut app, egui::vec2(300.0, 20.0)));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    // `a` is dragged, `b` moves with it as part of the selection.
    drag(&mut app, &[a, b], egui::vec2(0.0, 50.0));
    remote_set(&mut other, a, field::POS, FieldValue::Pos(5.0, 6.0));
    remote_set(&mut other, b, field::POS, FieldValue::Pos(7.0, 8.0));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.pos(a), Some(egui::vec2(5.0, 6.0)), "the document pos is theirs until the release");
    assert_eq!(app.layout_pos(a), egui::vec2(10.0, 70.0), "the dragged card stays under the pointer");
    assert_eq!(app.layout_pos(b), egui::vec2(300.0, 70.0), "and so does the rest of the selection");
    app.release_drag();
    assert!(app.save_session());
    assert_eq!(stored(&app, a, field::POS), FieldValue::Pos(10.0, 70.0), "the release wins");
    assert_eq!(stored(&app, b, field::POS), FieldValue::Pos(300.0, 70.0));
}

/// Another replica's url change moves a sleeping tab without waking it.
#[test]
fn a_remote_url_change_moves_a_sleeping_tab_without_waking_it() {
    let path = temp_dir("lazy-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", "S"));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::URL, FieldValue::Str("https://moved/".into()));
    assert!(app.save_session());
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://moved/");
    assert!(app.test_spawns.is_empty());
}

/// A tab another replica creates arrives asleep at its url, so another
/// instance opening a big session starts no engines here.
#[test]
fn a_remote_new_tab_arrives_asleep() {
    let url = FieldValue::Str("https://new/".into());
    let app = app_with(&BTreeMap::from([(node(1), shared(Kind::Tab, None, &[(field::URL, url)]))]));
    assert!(app.test_spawns.is_empty());
    let tab = app.nodes[&node(1)].as_unloaded().unwrap();
    assert_eq!((tab.url.as_str(), &tab.state), ("https://new/", &UnloadedState::Sleeping));
}

#[test]
fn remote_jar_create_rename_and_delete_reach_jar_defs() {
    let jar = node(40);
    let named = |name: &str| shared(Kind::Jar, None, &[(field::NAME, FieldValue::Str(name.into()))]);
    let mut app = app_with(&BTreeMap::from([(jar, named("Home"))]));
    assert_eq!(app.jar_defs()[&jar].name, "Home");
    assert!(app.nodes.contains_key(&jar), "a jar is a card");
    app.apply_remote(vec![(jar, Remote::Shown(snap(Kind::Jar, named("House").fields, None)))]);
    assert_eq!(app.jar_defs()[&jar].name, "House");
    assert_eq!(snapshots(&app)[&jar], named("House"), "projects back unchanged");
    app.apply_remote(vec![(jar, Remote::Deleted)]);
    assert!(!app.jar_defs().contains_key(&jar));
}

/// Final review M1: a jar deleted here whose delete is not saved yet must not come back (nameless, from an
/// ephemeral-only change, or whole) when the save's first round applies another device's edit to it.
#[test]
fn a_remote_edit_does_not_bring_back_a_jar_deleted_here() {
    for (n, (no, value)) in [(field::EPHEMERAL, FieldValue::Bool(true)), (field::NAME, FieldValue::Str("Job".into()))]
        .into_iter()
        .enumerate()
    {
        let path = crate::store::testutil::temp_dir(&format!("jar-deleted-here-{n}")).join("session.mnf");
        let mut app = app_on(&path, 1);
        add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
        let jar = add_jar(&mut app, "Work");
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        remote_set(&mut other, jar, no, value);
        app.remove_node(jar);
        assert!(app.save_session());
        assert!(!app.jar_defs().contains_key(&jar), "field {no}: deleted here, not recreated");
        assert_eq!(app.doc.mirror.get(jar, field::NAME), FieldValue::Str(String::new()), "field {no}");
        assert!(!app.doc.journal.as_ref().unwrap().doc().nodes[&jar].is_live(), "field {no}: the delete was saved");
    }
}

/// Ruling 9, extended to graph nodes (Task 4): a node closed here whose delete is not saved yet must not come back
/// when the save's first round applies another replica's edit to it; the delete then commits. Without the guard the
/// edit re-shows the node and its queued delete is lost.
#[test]
fn a_remote_edit_does_not_bring_back_a_node_closed_here() {
    let path = temp_dir("node-closed-here").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, note, field::TEXT, FieldValue::Str("theirs".into()));
    remote_set(&mut other, tab, field::URL, FieldValue::Str("https://b/".into()));
    app.remove_node(note);
    app.remove_tab(tab);
    assert!(app.save_session());
    for id in [&note, &tab] {
        assert!(!app.nodes.contains_key(id), "{id}: closed here, not shown again");
        assert!(!live_in_doc(&app, *id), "{id}: the delete was saved");
        assert!(!app.doc.mirror.has(*id), "{id}: nothing of it kept");
    }
    assert!(!app.doc.writes.has_deletes());
}

/// `mirror_remote` (Ruling 9) returns the fields another replica changed (old mirror value != new), so
/// `update_live_node` can act on them, leaving out those a pending local edit holds back, which keep the local value.
#[test]
fn mirror_remote_reports_the_changed_fields_but_not_the_held_ones() {
    let mut app = App::default();
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    app.edit_pos(tab, Some(egui::vec2(5.0, 5.0)));
    let mut fields = app.snapshot(tab).unwrap().fields;
    fields.insert(field::POS, FieldValue::Pos(9.0, 9.0));
    fields.insert(field::URL, FieldValue::Str("https://b/".into()));
    let (held, changed) = app.mirror_remote(tab, &fields, &Default::default(), None);
    assert_eq!(held, [field::POS], "a pending edit holds the field");
    assert_eq!(changed, [field::URL], "the url changed; pinned, engine and the rest did not");
    assert_eq!(app.doc.mirror.url(tab), "https://b/");
    assert_eq!(app.doc.mirror.pos(tab), Some(egui::vec2(5.0, 5.0)), "held: the local edit stands");
    let (held, changed) = app.mirror_remote(tab, &fields, &Default::default(), None);
    assert_eq!((held, changed), (vec![field::POS], vec![]), "the same values again change nothing");
}

/// A jar rename not saved yet wins over another device's (last writer, as for every mirrored field).
#[test]
fn a_pending_local_jar_rename_wins_over_a_remote_one() {
    let path = temp_dir("jar-rename-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = add_jar(&mut app, "Work");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, jar, field::NAME, FieldValue::Str("House".into()));
    app.commit_rename(jar, "Job");
    assert!(app.save_session());
    assert_eq!(app.jar_defs()[&jar].name, "Job");
    assert_eq!(stored(&app, jar, field::NAME), FieldValue::Str("Job".into()));
}

/// Ruling 7: another device's ephemeral toggle has the local one's file side effects (`set_jar_ephemeral`): on deletes
/// the jar's file, off saves it back. It is applied, not queued as a local edit.
#[test]
fn a_remote_ephemeral_change_deletes_and_saves_the_jar_file() {
    let dir = temp_dir("jar-remote-ephemeral");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 1) };
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = add_jar(&mut app, "Work");
    assert!(app.save_session());
    let file = crate::jar::Jar::path(&jars, jar);
    std::fs::write(&file, r#"{"cookies":[]}"#).unwrap();
    let toggled = |app: &App, on: bool| {
        let mut fields = snapshots(app)[&jar].fields.clone();
        fields.insert(field::EPHEMERAL, FieldValue::Bool(on));
        vec![(jar, Remote::Shown(snap(Kind::Jar, fields, None)))]
    };
    app.apply_remote(toggled(&app, true));
    assert!(app.jar_defs()[&jar].ephemeral);
    assert!(!file.exists(), "on: the file goes");
    assert!(!app.doc.writes.has_fields(), "a remote value is not a local edit");
    app.apply_remote(toggled(&app, false));
    assert!(!app.jar_defs()[&jar].ephemeral);
    assert!(file.exists(), "off: saved back");
    assert!(!app.doc.writes.has_fields());
}

/// A local ephemeral toggle not saved yet wins over another device's, and the remote value runs no file side effect:
/// here the remote "on" must not delete the file the local "off" just saved.
#[test]
fn a_remote_ephemeral_toggle_under_a_pending_local_one_touches_no_file() {
    let dir = temp_dir("jar-ephemeral-pending");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 1) };
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = add_jar(&mut app, "Work");
    app.set_jar_ephemeral(jar, true);
    assert!(app.save_session());
    let file = crate::jar::Jar::path(&jars, jar);
    app.set_jar_ephemeral(jar, false);
    assert!(file.exists(), "the local off saved the file");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, jar, field::EPHEMERAL, FieldValue::Bool(true));
    assert!(app.save_session());
    assert!(file.exists(), "the held remote on deleted nothing");
    assert!(!app.jar_defs()[&jar].ephemeral);
    assert_eq!(stored(&app, jar, field::EPHEMERAL), FieldValue::Bool(false), "the local value is saved");
}

/// A jar first seen already ephemeral (created on another device, or restored) deletes a leftover file of it, as a
/// toggle on would, and queues nothing.
#[test]
fn a_jar_first_seen_ephemeral_deletes_its_leftover_file_and_queues_nothing() {
    let dir = temp_dir("jar-ephemeral-first");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let leftover = |jar| {
        let file = crate::jar::Jar::path(&jars, jar);
        std::fs::write(&file, r#"{"cookies":[]}"#).unwrap();
        file
    };
    // Created on another device.
    let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 1) };
    let remote = NodeId { actor: ActorId(2), counter: 7 };
    let file = leftover(remote);
    let fields = Fields::from([
        (field::COLOR, color_value(None)),
        (field::NAME, FieldValue::Str("Theirs".into())),
        (field::EPHEMERAL, FieldValue::Bool(true)),
    ]);
    app.apply_remote(vec![(remote, Remote::Shown(snap(Kind::Jar, fields, None)))]);
    assert!(app.jar_defs()[&remote].ephemeral);
    assert!(!file.exists(), "remote-created: the leftover file goes");
    assert!(!app.doc.writes.has_fields(), "remote-created: nothing queued");
    // Restored at startup.
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = add_jar(&mut app, "Work");
    app.set_jar_ephemeral(jar, true);
    assert!(app.save_session());
    drop(app);
    let file = leftover(jar);
    let mut back = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 3) };
    assert!(back.restore_session());
    assert!(back.jar_defs()[&jar].ephemeral);
    assert!(!file.exists(), "restored: the leftover file goes");
    assert!(!back.doc.writes.fields().any(|(node, ..)| node == jar), "restored: nothing queued for it");
}

/// A remote delete releases the removed jar's files at once, unless a closed node in the document holds them.
#[test]
fn a_remote_jar_delete_releases_its_file_unless_recently_closed_holds_it() {
    let dir = crate::store::testutil::temp_dir("release-remote");
    let jars = dir.join("jars");
    std::fs::create_dir_all(&jars).unwrap();
    let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&dir.join("session.mnf"), 1) };
    let (gone, held) = (add_jar(&mut app, "Gone"), add_jar(&mut app, "Held"));
    assert!(app.save_session());
    for jar in [gone, held] {
        std::fs::write(jars.join(format!("{jar}.json")), b"x").unwrap();
    }
    // `held` was closed here earlier: closed is live in the document, and the document holds its file.
    app.close_node(held);
    // The remote delete is real: the document no longer holds `gone` (a hold by the document is what keeps a file now).
    let mut other = Journal::open(&dir.join("session.mnf"), ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: gone }]).unwrap();
    assert!(app.save_session());
    for jar in [gone, held] {
        app.remove_remotely_deleted(jar);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while jars.join(format!("{gone}.json")).exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10)); // a poll interval, not a wait for a guessed duration
    }
    assert!(!jars.join(format!("{gone}.json")).exists(), "released");
    assert!(jars.join(format!("{held}.json")).exists(), "held by the document: closed is not deleted");
}

#[test]
fn a_remote_color_edit_reaches_the_mirror() {
    let path = temp_dir("edit-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, note, field::COLOR, red());
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.color(note), Some(egui::Color32::RED));
}

#[test]
fn a_pending_local_color_edit_wins_over_a_remote_edit_of_the_same_field() {
    let path = temp_dir("edit-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_field(note, field::COLOR, blue());
    remote_set(&mut other, note, field::COLOR, red());
    // The remote round applies first, the next round commits the local edit over it.
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.color(note), Some(egui::Color32::BLUE));
    assert_eq!(stored(&app, note, field::COLOR), blue());
}

/// A remote colour parked for a waking tab (`remote_in_flight`) must not land over a local edit committed after it.
#[test]
fn a_committed_local_edit_wins_over_a_remote_value_parked_for_a_waking_tab() {
    let path = temp_dir("edit-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    assert!(app.save_session());
    app.focus_tab(tab);
    assert!(app.spawns.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::COLOR, blue());
    app.edit_field(tab, field::COLOR, red());
    // The remote round parks blue, the next round commits red.
    assert!(app.save_session());
    assert!(app.spawns.remote_in_flight.contains_key(&tab), "the remote value is parked");
    assert_eq!(stored(&app, tab, field::COLOR), red());
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.color(tab), Some(egui::Color32::RED));
    assert_eq!(stored(&app, tab, field::COLOR), red());
}

/// An unsaved local parent beats another replica's edit to the same link; their other fields still arrive.
#[test]
fn a_local_parent_edit_wins_over_a_remote_one() {
    let path = temp_dir("parent-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b, c) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://c/", "")),
    );
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_parent(c, Some(a));
    remote_set(&mut other, c, field::PARENT, FieldValue::Id(Some(b)));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.parent(c), Some(a));
    assert_eq!(stored(&app, c, field::PARENT), FieldValue::Id(Some(a)));
}

/// A remote parent change reaches the mirror; a node removal re-parents children through the writer.
#[test]
fn remote_parent_change_and_removal_reparent_through_the_mirror() {
    let path = temp_dir("parent-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b, c) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://c/", "")),
    );
    app.edit_parent(b, Some(a));
    app.edit_parent(c, Some(b));
    assert!(app.save_session());
    app.remove_node(b);
    assert_eq!(app.doc.mirror.parent(c), Some(a), "children pass up to the removed node's parent");
    assert!(app.save_session());
    assert_eq!(stored(&app, c, field::PARENT), FieldValue::Id(Some(a)));
}

/// A remote parent parked for a waking tab (`remote_in_flight`) must not land over a local edit committed after it.
#[test]
fn a_committed_parent_edit_wins_over_a_remote_value_parked_for_a_waking_tab() {
    let path = temp_dir("parent-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b, tab) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://t/", "")),
    );
    assert!(app.save_session());
    app.focus_tab(tab);
    assert!(app.spawns.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::PARENT, FieldValue::Id(Some(b)));
    app.edit_parent(tab, Some(a));
    assert!(app.save_session());
    assert!(app.spawns.remote_in_flight.contains_key(&tab), "the remote value is parked");
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.parent(tab), Some(a));
    assert_eq!(stored(&app, tab, field::PARENT), FieldValue::Id(Some(a)));
}

/// A pending parent edit or move for a node the document no longer shows live (another replica deleted it) is
/// dropped: a `Set` or `Move` would make a placeholder.
#[test]
fn a_pending_link_edit_for_a_node_deleted_elsewhere_is_dropped() {
    let path = temp_dir("link-dropped").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let (a, b, c) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://c/", "")),
    );
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let deletes = vec![OpBody::Delete { node: b }, OpBody::Delete { node: c }];
    other.begin().unwrap().commit(deletes).unwrap();
    app.edit_parent(b, Some(a));
    app.edit_group(c, Some(g));
    let ops_before = app.doc.journal.as_ref().unwrap().doc().version[&ActorId(1)];
    assert!(app.save_session());
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.version[&ActorId(1)], ops_before, "nothing written");
    assert!(!doc.nodes[&b].is_live() && !doc.nodes[&c].is_live());
    assert!(!app.doc.writes.has_fields() && !app.doc.writes.has_moves());
}

/// A parked remote value for a waking tab is patched with what the commit wrote: a parent or group the commit
/// deleted lands as none, not as the dead id.
#[test]
fn a_parked_link_is_patched_with_the_resolved_value() {
    let path = temp_dir("link-parked-gone").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let (a, b, tab) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://t/", "")),
    );
    assert!(app.save_session());
    app.focus_tab(tab);
    assert!(app.spawns.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::PARENT, FieldValue::Id(Some(b)));
    app.edit_parent(tab, Some(a));
    app.edit_group(tab, Some(g));
    app.remove_tab(a);
    app.remove_node(g);
    assert!(app.save_session());
    assert!(app.spawns.remote_in_flight.contains_key(&tab), "the remote value is parked");
    assert_eq!(stored(&app, tab, field::PARENT), FieldValue::Id(None));
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.parent(tab), None);
    assert_eq!(app.doc.mirror.group(tab), None);
}

/// An unsaved local move beats another replica's move of the same node.
#[test]
fn a_local_group_move_wins_over_a_remote_one() {
    let path = temp_dir("group-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g1 = add_group(&mut app, "A");
    let g2 = add_group(&mut app, "B");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_group(t, Some(g1));
    other.begin().unwrap().commit(vec![OpBody::Move { node: t, group: Some(g2) }]).unwrap();
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.group(t), Some(g1));
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().effective_group(t), Some(g1));
}

/// Another replica's move reaches the mirror when nothing is pending locally.
#[test]
fn a_remote_group_move_reaches_the_mirror() {
    let path = temp_dir("group-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let mv = vec![OpBody::Move { node: t, group: Some(g) }];
    other.begin().unwrap().commit(mv).unwrap();
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.group(t), Some(g));
}

/// A remote move parked for a waking tab must not land over a local move committed after it.
#[test]
fn a_committed_move_wins_over_a_remote_move_parked_for_a_waking_tab() {
    let path = temp_dir("group-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g1 = add_group(&mut app, "A");
    let g2 = add_group(&mut app, "B");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.focus_tab(tab);
    assert!(app.spawns.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let mv = vec![OpBody::Move { node: tab, group: Some(g2) }];
    other.begin().unwrap().commit(mv).unwrap();
    app.edit_group(tab, Some(g1));
    assert!(app.save_session());
    assert!(app.spawns.remote_in_flight.contains_key(&tab), "the remote value is parked");
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.group(tab), Some(g1));
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().effective_group(tab), Some(g1));
}

/// Another replica merges G into T (`merge_groups`: `Delete G` and `Move(M, T)`). Applying G's delete first must not
/// queue a move of M that reverts the merge: the remote cascade only updates the mirror.
#[test]
fn a_remote_merge_is_not_reverted_by_the_dissolve_it_triggers() {
    let path = temp_dir("group-remote-merge").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = add_group(&mut app, "T");
    let g = add_group(&mut app, "G");
    let m = add(&mut app, Kind::Tab, sleeping_tab("https://m/", ""));
    assert!(g < m, "G's delete is applied before M's move");
    app.edit_group(m, Some(g));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let merge = vec![OpBody::Delete { node: g }, OpBody::Move { node: m, group: Some(t) }];
    other.begin().unwrap().commit(merge).unwrap();
    for _ in 0..2 {
        assert!(app.save_session());
        assert_eq!(app.doc.mirror.group(m), Some(t));
        let doc = app.doc.journal.as_ref().unwrap().doc();
        assert_eq!(doc.effective_group(m), Some(t));
    }
}

/// Another replica deletes P and re-parents P's child C to A. Applying P's delete first must not queue a parent edit
/// of C that reverts the re-parent.
#[test]
fn a_remote_reparent_is_not_reverted_by_the_removal_it_races() {
    let path = temp_dir("parent-remote-race").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    let p = add(&mut app, Kind::Tab, sleeping_tab("https://p/", ""));
    let c = add(&mut app, Kind::Tab, sleeping_tab("https://c/", ""));
    app.edit_parent(c, Some(p));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let ops = vec![
        OpBody::Delete { node: p },
        OpBody::set(c, field::PARENT, encode_field(field::PARENT, &FieldValue::Id(Some(a)))),
    ];
    other.begin().unwrap().commit(ops).unwrap();
    for _ in 0..2 {
        assert!(app.save_session());
        assert_eq!(app.doc.mirror.parent(c), Some(a));
        assert_eq!(stored(&app, c, field::PARENT), FieldValue::Id(Some(a)));
    }
}

/// A local move into a group another replica then deletes still commits, to the deleted group's own group.
#[test]
fn a_pending_move_into_a_remotely_deleted_group_lands_in_its_group() {
    let path = temp_dir("group-pending-remote-delete").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "O");
    let g = add_group(&mut app, "G");
    let m = add(&mut app, Kind::Tab, sleeping_tab("https://m/", ""));
    app.edit_group(g, Some(outer));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: g }]).unwrap();
    app.edit_group(m, Some(g));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.group(m), Some(outer));
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(m), Some(outer));
}

/// Ruling 3: an unsaved local url beats another replica's edit to it; their other fields still arrive. The tab is not
/// moved to their url (a live tab is sent no `Navigate`: `update_live_node` uses the one verdict for both).
#[test]
fn a_local_url_edit_wins_over_a_remote_one() {
    let path = temp_dir("url-local-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_url(tab, "https://mine/");
    set_live_url(&mut app, tab, "https://mine/");
    remote_set(&mut other, tab, field::URL, FieldValue::Str("https://theirs/".into()));
    remote_set(&mut other, tab, field::PINNED, FieldValue::Bool(true));
    // The remote round applies, then the next round commits.
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.url(tab), "https://mine/");
    assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://mine/".into()));
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://mine/", "not moved to theirs");
    assert!(app.doc.mirror.pinned(tab), "their other field arrives");
    assert!(app.test_spawns.is_empty());
}

/// Another replica's url reaches the mirror and moves a sleeping tab, and the next save writes nothing back.
#[test]
fn a_remote_url_change_reaches_the_mirror_and_moves_a_sleeping_tab() {
    let path = temp_dir("url-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::URL, FieldValue::Str("https://moved/".into()));
    let url_stamp = |doc: &Doc| doc.nodes[&tab].fields[&crate::store::model::RegKey::of(field::URL)].stamp;
    let theirs = url_stamp(other.doc());
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.url(tab), "https://moved/");
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://moved/");
    assert!(app.save_session());
    let ours = url_stamp(app.doc.journal.as_ref().unwrap().doc());
    assert_eq!(ours, theirs, "nothing written back: the register is still the remote write");
    assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://moved/".into()));
}

/// Ruling 3: an unsaved local jar pick beats another replica's; their other fields still arrive.
#[test]
fn a_local_jar_pick_wins_over_a_remote_one() {
    let path = temp_dir("jar-local-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (mine, theirs) = (add_jar(&mut app, "Mine"), add_jar(&mut app, "Theirs"));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.set_jar_edge(tab, Some(mine));
    remote_set(&mut other, tab, field::JAR, FieldValue::Id(Some(theirs)));
    remote_set(&mut other, tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.jar(tab), Some(mine));
    assert_eq!(stored(&app, tab, field::JAR), FieldValue::Id(Some(mine)));
    assert!(app.doc.mirror.pinned(tab), "their other field arrives");
}

/// Ruling 3: another replica's engine reaches the mirror and switches a live tab to it; nothing is written back.
#[test]
fn a_remote_view_change_on_a_live_tab_starts_a_swap() {
    let path = temp_dir("engine-remote-live").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::VIEW_KIND, view_kind(ViewKind::Cef));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef);
    assert_eq!(swap_target(&app, tab), Some(ViewKind::Cef), "the swap started");
    assert!(sent(&mut peer).iter().any(|m| matches!(m, HostToView::GetState)));
    assert!(!app.doc.writes.has_fields(), "nothing queued");
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
}

/// Ruling 3: a swap in flight here beats another replica's engine, while its edit is unsaved (the mirror holds the
/// remote value back) and after it is saved (written again), so the document ends on the engine this tab lands on.
#[test]
fn a_local_swap_in_flight_wins_over_a_remote_view_kind() {
    let path = temp_dir("engine-local-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.start_swap_to(tab, ViewKind::Cef);
    remote_set(&mut other, tab, field::VIEW_KIND, view_kind(ViewKind::Webview2));
    remote_set(&mut other, tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef);
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
    assert!(app.doc.mirror.pinned(tab), "their other field arrives");
    // Saved, and the old engine's state is still on its way: a later remote engine loses too.
    remote_set(&mut other, tab, field::VIEW_KIND, view_kind(ViewKind::Webview2));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef);
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Cef), "written again");
    assert_eq!(swap_target(&app, tab), Some(ViewKind::Cef), "still the one swap");
    let asked = sent(&mut peer).iter().filter(|m| matches!(m, HostToView::GetState)).count();
    assert_eq!(asked, 1, "no second swap started");
}

/// Review M8: another replica's engine reaches a sleeping card through the mirror alone (nothing spawns, nothing is
/// queued), and the card wakes into it.
#[test]
fn a_remote_view_change_on_a_sleeping_card_is_the_one_it_wakes_into() {
    let path = temp_dir("engine-remote-sleeping").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::VIEW_KIND, view_kind(ViewKind::Cef));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef);
    assert!(!app.doc.writes.has_fields() && app.test_spawns.is_empty(), "nothing queued or started");
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Sleeping);
    app.focus_tab(tab);
    assert_eq!(app.test_spawns[0].0.kind, ViewKind::Cef, "wakes into it");
}

/// Final review Minor 6: another replica's url reaches a live tab as a `Navigate`; a url held back by a pending
/// local edit sends none (the mirror refused it, 2b-3 Ruling 3).
#[test]
fn a_remote_url_navigates_a_live_tab_unless_a_local_url_is_pending() {
    let path = temp_dir("url-remote-live").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, ViewKind::Servo, "https://a/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::URL, FieldValue::Str("https://b/".into()));
    assert!(app.save_session());
    let navigated =
        |sent: &[HostToView], to: &str| sent.iter().any(|m| matches!(m, HostToView::Navigate { url, .. } if url == to));
    assert!(navigated(&sent(&mut peer), "https://b/"), "the live tab is sent there");
    assert_eq!(app.nodes[&tab].as_tab().unwrap().url, "https://b/");
    app.edit_url(tab, "https://local/");
    remote_set(&mut other, tab, field::URL, FieldValue::Str("https://c/".into()));
    assert!(app.save_session());
    let after = sent(&mut peer);
    assert!(!after.iter().any(|m| matches!(m, HostToView::Navigate { .. })), "held back: no Navigate, got {after:?}");
    assert_eq!(app.doc.mirror.url(tab), "https://local/");
    assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://local/".into()));
}

/// Final review I1, url: another replica's url, parked while the tab wakes (`remote_in_flight`), still navigates
/// it when it lands, though a commit in between (here an unrelated Delete) saw that url in the document: the mirror
/// keeps the url the tab had until the landing applies the parked change against it.
#[test]
fn a_remote_url_parked_during_a_wake_navigates_the_tab_when_it_lands() {
    let path = temp_dir("url-parked-wake").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    app.focus_tab(tab);
    assert_eq!(app.test_spawns.len(), 1, "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::URL, FieldValue::Str("https://r/".into()));
    assert!(app.save_session());
    assert!(app.spawns.remote_in_flight.contains_key(&tab), "parked for the landing");
    app.remove_node(note);
    assert!(app.save_session());
    assert!(!app.doc.journal.as_ref().unwrap().doc().nodes[&note].is_live(), "the Delete committed");
    let mut peers = land_spawns(&mut app);
    let to_r = sent(&mut peers[0]).iter().any(|m| matches!(m, HostToView::Navigate { url, .. } if url == "https://r/"));
    assert!(to_r, "the landed tab is sent to the remote url");
    assert_eq!(app.doc.mirror.url(tab), "https://r/");
}

/// Final review I1, engine: another replica's engine, parked while a swap's respawn is in flight, still reaches
/// the tab when it lands, though a jar edit (which reaches a respawning tab, Ruling 9) committed
/// a Set for the tab in between (the parked change takes the committed jar, so the landing does not replay the old
/// one over it). The document and the running engine end up agreeing: the landing starts a swap to it.
#[test]
fn a_remote_view_parked_during_a_swap_respawn_is_swapped_to_when_it_lands() {
    let path = temp_dir("engine-parked-swap").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = add_jar(&mut app, "Work");
    let g = add_group(&mut app, "G");
    let (tab, _peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    app.edit_group(tab, Some(g));
    assert!(app.save_session());
    app.start_swap_to(tab, ViewKind::Cef);
    assert!(app.save_session());
    let snapshot = manifold_view_protocol::TabState { url: "https://s/".into() };
    app.handle_view_message(tab, manifold_view_protocol::ViewToHost::StateSnapshot(snapshot));
    assert_eq!(app.test_spawns.len(), 1, "the swap's respawn is in flight");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::VIEW_KIND, view_kind(ViewKind::Webview2));
    assert!(app.save_session());
    assert!(app.spawns.remote_in_flight.contains_key(&tab), "parked for the landing");
    app.edit_field(tab, field::JAR, FieldValue::Id(Some(work)));
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::JAR), FieldValue::Id(Some(work)), "the edit committed for the tab");
    let _peers = land_spawns(&mut app);
    assert_eq!(app.nodes[&tab].as_tab().unwrap().tab.kind, ViewKind::Cef, "landed on the swap's target");
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Webview2);
    assert_eq!(app.swap_in_flight(tab), Some(ViewKind::Webview2), "and swaps on to the remote engine");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Webview2));
}

/// Ruling 6: a jar deleted on another device stays stored (dangling, it reads as the Default jar) instead of being
/// cleared by this device's next save.
#[test]
fn a_remote_deleted_group_jar_stays_stored() {
    let path = temp_dir("group-jar-dangling").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = add_jar(&mut app, "Work");
    let g = add_group(&mut app, "G");
    app.set_jar_edge(g, Some(work));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: work }]).unwrap();
    assert!(app.save_session());
    assert!(!app.jar_defs().contains_key(&work), "the jar is gone here");
    assert!(app.save_session());
    assert_eq!(stored(&app, g, field::JAR), FieldValue::Id(Some(work)), "not cleared");
    assert_eq!(app.doc.mirror.jar(g), Some(work));
}

/// A remote delete of an endpoint hides and removes the link as well.
#[test]
fn a_remote_delete_of_an_endpoint_removes_its_links() {
    let mut app = app_on(&temp_dir("link-remote-delete").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.create_link(a, b);
    // What `remote_changes` reports when another replica deletes `a`
    // (as `remote_jar_create_rename_and_delete_reach_jar_defs`):
    app.apply_remote(vec![(a, Remote::Deleted)]);
    assert!(app.links_naming(b).is_empty());
    assert!(app.doc.mirror.link_ids().next().is_none());
}

#[test]
fn an_unknown_type_is_a_card_from_a_restore_and_from_a_remote_create() {
    let (mut app, node, path) = restored_unknown();
    let key = node;
    assert!(matches!(app.nodes[&key].runtime, Runtime::Unknown));
    assert_eq!(app.doc.mirror.ty(key), FUTURE);
    assert_eq!(app.card_kind(key).unwrap().ty, crate::store::model::TYPE_UNKNOWN);
    assert_eq!(app.doc.mirror.name(key), "Old");
    assert_eq!(app.doc.mirror.pos(key), Some(egui::vec2(5.0, 5.0)));
    assert_eq!(app.accessible_name(key).as_deref(), Some("Card: Old"));
    let mut third = Journal::open(&path, ActorId(3)).unwrap();
    let mut txn = third.begin().unwrap();
    let second = txn.next_id();
    txn.commit(vec![OpBody::create(second, FUTURE, encode_field(field::NAME, &FieldValue::Str("New".into())))])
        .unwrap();
    assert!(app.save_session());
    assert!(matches!(app.nodes[&second].runtime, Runtime::Unknown), "another replica's shows too");
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(shown(doc), snapshots(&app));
}

#[test]
fn a_remote_delete_removes_an_unknown_card() {
    let (mut app, node, path) = restored_unknown();
    let mut other = Journal::open(&path, ActorId(3)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node }]).unwrap();
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&node));
}

/// A node an op named before its `Create` arrived stays hidden; once its type lands it is a card.
#[test]
fn a_typeless_placeholder_gets_its_card_when_its_type_arrives() {
    let node = node(5);
    let mut doc = Doc::new(crate::store::model::DocId([0; 16]));
    doc.nodes.insert(node, Node::placeholder());
    assert_eq!(App::remote_changes(&doc, &[node]), vec![(node, Remote::Deleted)]);
    doc.nodes.insert(node, Node::with_type(FUTURE));
    let changes = App::remote_changes(&doc, &[node]);
    assert!(matches!(&changes[0].1, Remote::Shown(s) if s.ty == FUTURE));
    let mut app = App::default();
    app.apply_remote(changes);
    assert!(matches!(app.nodes[&node].runtime, Runtime::Unknown));
}
