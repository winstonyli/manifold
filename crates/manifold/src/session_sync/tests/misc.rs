//! Session-sync tests: tests of other modules that use these helpers.

use super::*;

#[test]
fn jar_fields_belong_to_tabs_groups_and_jars() {
    use field::*;
    assert_eq!(Kind::Tab.fields(), &[POS, COLOR, PARENT, URL, VIEW_KIND, PINNED, JAR, NAME, CLOSED]);
    assert_eq!(Kind::Group.fields(), &[POS, COLOR, NAME, JAR, CLOSED, HELD]);
    assert_eq!(Kind::Jar.fields(), &[POS, COLOR, NAME, EPHEMERAL, IS_DEFAULT, CLOSED]);
    assert_eq!(Kind::Link.fields(), &[NAME, FROM, TO, CLOSED]);
    assert_eq!(Kind::Unknown.fields(), &[POS, COLOR, PARENT, NAME, CLOSED]);
}

#[test]
fn alternating_store_notices_are_each_shown_once_until_a_commit() {
    let dir = crate::store::testutil::temp_dir("notices");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    for _ in 0..3 {
        app.notice("read-only".into());
        app.notice("damaged".into());
    }
    assert_eq!(app.toasts.save_toast_queue.len(), 2);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    app.notice("read-only".into());
    assert_eq!(app.toasts.save_toast_queue.len(), 3, "a commit lets a notice show again");
}

#[test]
fn recovery_notices_say_what_was_lost_and_where_the_original_is() {
    let r =
        Recovery { damaged: vec![(10, 20)], saved: Some("C:/x/session.mnf.damaged-1".into()), ..Recovery::default() };
    assert_eq!(
        recovery_notice(&r),
        "The session file was damaged; 1 damaged part was skipped. The original is saved at C:/x/session.mnf.damaged-1"
    );
    let r = Recovery { from_prev: true, saved: None, ..Recovery::default() };
    assert!(recovery_notice(&r).contains("rebuilt from its previous copy"));
}

/// Its refetch failing (no content dir under test) turns each into the
/// missing-file card, with no toast: the card and its Retry are the
/// feedback.
#[test]
fn a_failed_refetch_of_a_missing_file_shows_the_missing_file_card() {
    let url = |u: &str| (field::URL, FieldValue::Str(u.into()));
    let gone = |p: &str| (field::PATH, FieldValue::Str(p.into()));
    let nodes = BTreeMap::from([
        (id(0), shared(Kind::Json, None, &[gone(r"Z:\gone.json"), url("https://x.test/a.json")])),
        (id(1), shared(Kind::Image, None, &[gone(r"Z:\gone.png"), url("https://x.test/a.png")])),
        (id(2), shared(Kind::Pdf, None, &[gone(r"Z:\gone.pdf"), url("https://x.test/a.pdf")])),
    ]);
    let mut app = app_with(&nodes);
    let mut results = Vec::new();
    while results.len() < 3 {
        let more = drain_fetches(&mut app);
        assert!(!more.is_empty(), "every missing file was refetched");
        results.extend(more);
    }
    for FetchResult { job, body } in results {
        assert!(body.is_none());
        let FetchTarget::Node(node) = &job.target else { panic!("a node refetch") };
        app.refresh_fetch_failed(node);
    }
    let error = |n: u64| match &app.nodes[&id(n).to_string()].runtime {
        Runtime::Json(p) => p.error.clone(),
        Runtime::Image(p) => p.error.clone(),
        Runtime::Pdf(p) => p.outcome.as_ref().and_then(|o| o.as_ref().err()).cloned(),
        _ => None,
    };
    assert_eq!(error(0).as_deref(), Some("Couldn't fetch https://x.test/a.json"));
    assert_eq!(error(1).as_deref(), Some("Couldn't fetch https://x.test/a.png"));
    assert_eq!(error(2).as_deref(), Some("Couldn't fetch https://x.test/a.pdf"));
    assert!(app.toasts.save_toast_queue.is_empty() && app.toasts.save_toast.is_none(), "no toast");
}

#[test]
fn retry_missing_file_refetches_a_node_with_a_url_instead_of_retrying_its_local_path() {
    let nodes = BTreeMap::from([(
        id(0),
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
    let node_id = id(0).to_string();
    // `materialize` already queued one refetch on its own; drain and
    // discard it so the assertion below reflects only the click.
    drain_fetches(&mut app);
    app.retry_missing_file(&node_id);
    let results = drain_fetches(&mut app);
    assert_eq!(results.len(), 1, "the URL branch queued exactly one fetch");
    assert!(matches!(&results[0].job.target, FetchTarget::Node(n) if *n == node_id));
}

#[test]
fn backing_keep_holds_hidden_nodes_files() {
    let path = temp_dir("sync-backing-hidden").join("session.mnf");
    let mut app = app_on(&path, 1);
    let jar = add(&mut app, Kind::Jar, Runtime::Jar);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &jar, true);
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&jar));
    assert!(app.backing_keep(None).contains(&jar), "a closed jar keeps its cookie file");
    assert!(!app.backing_keep(Some(&jar)).contains(&jar), "`except` leaves it out");
}

/// Right after a typed navigation, a report still at the url it left
/// is from before the engine started it.
#[test]
fn a_report_still_at_the_url_a_typed_navigation_left_is_stale() {
    let mut app = App::default();
    let report = |app: &mut App, url: &str| app.reported_url("7", url.into(), true, "https://typed/".into());
    assert_eq!(report(&mut app, "https://old/"), "https://old/", "no typed navigation: as reported");
    app.spawns.navigated_from.insert("7".into(), ("https://old/".into(), std::time::Instant::now(), false));
    assert_eq!(report(&mut app, "https://old/"), "https://typed/");
    assert_eq!(report(&mut app, "https://typed/"), "https://typed/");
    assert_eq!(report(&mut app, "https://old/"), "https://old/", "after the engine moved on: real (going back)");
    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(2);
    app.spawns.navigated_from.insert("7".into(), ("https://old/".into(), long_ago, false));
    assert_eq!(report(&mut app, "https://old/"), "https://old/", "the navigation never started");
}

/// A typed navigation that never leaves the page (an attachment, a 204): CEF
/// reports it started and finished, both still at the old url, and nothing
/// after. The finish is the page's real url, not a stale report.
#[test]
fn a_typed_navigation_that_ends_at_the_url_it_left_keeps_that_url() {
    let mut app = App::default();
    let report =
        |app: &mut App, url: &str, loading: bool| app.reported_url("7", url.into(), loading, "https://typed/".into());
    app.spawns.navigated_from.insert("7".into(), ("https://old/".into(), std::time::Instant::now(), false));
    assert_eq!(report(&mut app, "https://old/", false), "https://typed/", "a finish from before it started");
    assert_eq!(report(&mut app, "https://old/", true), "https://typed/", "started, address not changed yet");
    assert_eq!(report(&mut app, "https://old/", false), "https://old/", "finished without leaving");
    assert!(app.spawns.navigated_from.is_empty());
}

/// A drag whose grabbed card stops being drawn ends with no drop: a remote pos then snaps its cards, and a later
/// release (the card drawn again) saves nothing over it.
#[test]
fn a_stale_drag_does_not_overwrite_a_remote_pos() {
    let path = temp_dir("pos-stale-drag").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let (a, b) = (a_note_at(&mut app, egui::vec2(10.0, 20.0)), a_note_at(&mut app, egui::vec2(300.0, 20.0)));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();

    // A level switch with the button held.
    drag(&mut app, &[&a, &b], egui::vec2(0.0, 50.0));
    app.switch_group_level(Some(g.clone()));
    assert!(app.interaction.dragging.is_none() && app.interaction.drag_moved.is_empty());
    remote_set(&mut other, &a, field::POS, FieldValue::Pos(5.0, 6.0));
    assert!(app.save_session());
    assert_eq!(app.layout_pos(&a), egui::vec2(5.0, 6.0), "no longer dragged, so the remote pos snaps it");
    app.switch_group_level(None);
    app.release_drag();
    assert!(app.save_session());
    assert_eq!(stored(&app, &a, field::POS), FieldValue::Pos(5.0, 6.0));
    assert_eq!(stored(&app, &b, field::POS), FieldValue::Pos(300.0, 20.0), "the ended drag saved nothing");

    // Another replica moves the grabbed card into another group mid-drag, and gives it a new pos.
    drag(&mut app, &[&a], egui::vec2(0.0, 50.0));
    let node = a.parse().unwrap();
    let pos = FieldValue::Pos(7.0, 8.0);
    let set = OpBody::Set { node, field: field::POS, enc: encode_field(field::POS, &pos) };
    other.begin().unwrap().commit(vec![OpBody::Move { node, group: Some(g.parse().unwrap()) }, set]).unwrap();
    assert!(app.save_session());
    assert_eq!(app.layout_pos(&a), egui::vec2(7.0, 8.0), "off the level, the drag no longer holds the remote pos back");
    app.end_stale_drag(true);
    assert!(
        app.interaction.dragging.is_none() && app.interaction.drag_moved.is_empty(),
        "the frame's check ends the drag"
    );
    app.release_drag();
    assert!(app.save_session());
    assert_eq!(stored(&app, &a, field::POS), FieldValue::Pos(7.0, 8.0));

    // Leaving Graph view ends a drag too.
    drag(&mut app, &[&b], egui::vec2(0.0, 50.0));
    app.end_stale_drag(false);
    assert!(app.interaction.dragging.is_none() && app.interaction.drag_moved.is_empty());
}

#[test]
fn view_table_matches_the_store_and_round_trips() {
    for (i, kind) in ViewKind::ALL.into_iter().enumerate() {
        let desc = kind.desc();
        assert_eq!(desc.code, i as u64 + 1, "codes are 1-based positions in ALL");
        assert_eq!(crate::store::model::VIEW_KINDS[i], desc.name, "the store names the same engine");
        assert_eq!(serde_json::to_value(Wrap(kind)).unwrap(), serde_json::json!(desc.name));
        assert_eq!(view_kind_of(view_code(kind)), kind);
        assert_eq!(ViewKind::from_name(&desc.name.to_uppercase()), Some(kind));
    }
    assert_eq!(ViewKind::from_name("gecko"), None);
    assert_eq!(ViewKind::from_code(0), None);
}

#[test]
fn colors_round_trip() {
    let c = egui::Color32::from_rgb(1, 2, 3);
    assert_eq!(color_of(&color_value(Some(c))), Some(c));
    assert_eq!(color_value(Some(egui::Color32::BLACK)), FieldValue::Color(Some(0)), "black is encoded");
    assert_eq!(color_of(&color_value(None)), None);
}

/// A session saved by a build that wrote no `parent` for a download loads unchanged: no link, nothing lost.
#[test]
fn an_older_sessions_download_without_a_parent_loads_unchanged() {
    let path = temp_dir("parent-old").join("session.mnf");
    let node = NodeId { actor: ActorId(2), counter: 1 };
    {
        let mut old = Journal::open(&path, ActorId(2)).unwrap();
        let fields = [
            (field::GROUP, FieldValue::Id(None)),
            (field::POS, FieldValue::Pos(1.0, 2.0)),
            (field::COLOR, FieldValue::Color(None)),
            (field::URL, FieldValue::Str("https://x.test/f.bin".into())),
            (field::PATH, FieldValue::Str("f.bin".into())),
        ]
        .iter()
        .flat_map(|(no, value)| encode_field(*no, value))
        .collect();
        old.begin().unwrap().commit(vec![OpBody::create(node, crate::store::model::TYPE_DOWNLOAD, fields)]).unwrap();
    }
    let mut app = app_on(&path, 1);
    assert!(app.restore_session());
    let restored = &app.nodes[&node.to_string()];
    assert_eq!(app.doc.mirror.parent(&node.to_string()), None);
    assert!(restored.as_download().is_some());
    assert_eq!(app.copyable_url(&node.to_string()), Some("https://x.test/f.bin"));
    assert_eq!(app.layout_pos(&node.to_string()), egui::vec2(1.0, 2.0));
    assert_eq!(app.doc.mirror.pos(&node.to_string()), Some(egui::vec2(1.0, 2.0)));
    // No link encodes as the absent register, so a save has nothing to write back.
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "nothing written");
}

#[test]
fn a_session_with_only_jars_still_restores_them() {
    let dir = crate::store::testutil::temp_dir("jar-only-restore");
    let path = dir.join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = add_jar(&mut app, "Work");
    app.save_session();
    app.remove_tab(&tab);
    app.save_session();
    drop(app);
    let mut back = app_on(&path, 3);
    assert!(!back.restore_session(), "no graph node: a fresh start");
    assert_eq!(back.jar_defs()[&jar].name, "Work", "but the jar is kept, so the next save keeps it");
    back.save_session();
    assert!(back.doc.journal.as_ref().unwrap().doc().nodes[&jar].is_live());
}

/// Ruling 9 (2b-2's parked item): `recheck_edge_scope` reaches a tab mid-respawn, so a local move that takes its
/// edge out of reach clears it, and it lands without it.
#[test]
fn a_respawning_tab_follows_a_local_moves_edge_recheck() {
    let path = temp_dir("jar-respawn-move").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (_outer, g, t) = nested_tab(&mut app);
    let local = app.create_jar("Local", crate::fetched::Placement { group: Some(g.clone()), pos: egui::Vec2::ZERO });
    app.set_jar_edge(&t, Some(local));
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(Some(local)));
    start_respawn(&mut app, &t);
    app.edit_group(&t, None); // out of G: the jar inside G is no longer in reach
    app.recheck_edge_scope(&t);
    assert_eq!(app.doc.mirror.jar(&t), None);
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None));
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.jar(&t), None, "it lands without it");
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None));
}

#[test]
fn devtools_on_a_view_without_them_toasts_instead_of_sending() {
    let mut app = app_on(&temp_dir("caps-devtools").join("session.mnf"), 1);
    for (kind, has) in
        [(ViewKind::Cef, true), (ViewKind::Webview2, true), (ViewKind::Servo, false), (ViewKind::Webkit, false)]
    {
        let (id, mut peer) = live_tab(&mut app, kind, "https://a/");
        app.focused = Some(id);
        app.toasts.save_toast_queue.clear();
        app.open_devtools_shortcut();
        let sent_it = sent(&mut peer).iter().any(|m| matches!(m, HostToView::OpenDevTools));
        assert_eq!(sent_it, has, "{kind:?}");
        let toast = format!("{} has no DevTools", kind.desc().label);
        assert_eq!(app.toasts.save_toast_queue.contains(&toast), !has, "{kind:?}");
    }
}

#[test]
fn find_on_a_view_without_it_toasts_and_leaves_the_bar_shut() {
    let mut app = app_on(&temp_dir("caps-find").join("session.mnf"), 1);
    app.view_mode = ViewMode::Item;
    for (kind, has) in
        [(ViewKind::Cef, true), (ViewKind::Webview2, true), (ViewKind::Webkit, true), (ViewKind::Servo, false)]
    {
        let (id, _peer) = live_tab(&mut app, kind, "https://a/");
        app.focused = Some(id);
        app.find.open = false;
        app.toasts.save_toast_queue.clear();
        app.open_find_bar_shortcut();
        assert_eq!(app.find.open, has, "{kind:?}");
        let toast = format!("{} has no find in page", kind.desc().label);
        assert_eq!(app.toasts.save_toast_queue.contains(&toast), !has, "{kind:?}");
    }
}

/// A group's jar edge is the mirror's: `set_jar_edge` writes it through `edit_field` and the save stores it; a remote
/// change reaches the mirror and the jar its tabs use.
#[test]
fn a_groups_jar_edge_saves_and_a_remote_change_reaches_it() {
    let path = temp_dir("group-jar-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (work, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let g = add_group(&mut app, "G");
    app.set_jar_edge(&g, Some(work));
    assert_eq!(app.doc.mirror.jar(&g), Some(work));
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::JAR), FieldValue::Id(Some(work)));
    app.set_jar_edge(&g, None);
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::JAR), FieldValue::Id(None));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &g, field::JAR, FieldValue::Id(Some(home)));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.jar(&g), Some(home));
    assert_eq!(app.effective_jar(&g), Some((home, false)), "its tabs use Home");
}

#[test]
fn a_refused_drag_reparent_leaves_the_parent_alone() {
    let mut app = app_on(&temp_dir("reparent-refused").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(OPENED_FROM, &b, &a).unwrap();
    app.apply_drop(&b, Some((crate::card::DropEffect::Reparent, Some("nope".into()))));
    assert_eq!(app.doc.mirror.parent(&b), Some(a.as_str()), "a missing target");
    app.apply_drop(&a, Some((crate::card::DropEffect::Reparent, Some(b.clone()))));
    assert_eq!(app.doc.mirror.parent(&a), None, "a cycle");
}
