//! Session-sync tests: import, export, replace and teardown of a session.

use super::*;

/// Open Session: the replaced session's nodes of every kind, jar cards included, are deleted from the document, and
/// the new nodes are created.
#[test]
fn teardown_deletes_every_old_document_node_jars_included() {
    let path = temp_dir("teardown-deletes").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "old".into(), editing: false }));
    let group = add_group(&mut app, "G");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://old/", ""));
    app.edit_group(&tab, Some(group.clone()));
    let jar = add_jar(&mut app, "Work");
    assert!(app.save_session());
    let json =
        JsonDoc { nodes: vec![json_node("t", Kind::Tab, serde_json::json!({ "url": "https://new/" }))], local: None };
    app.replace_running_session(&json, crate::store::json::Trust::Foreign);
    for old in [&note, &group, &tab] {
        assert!(!live_in_doc(&app, old), "{old} deleted");
    }
    assert!(!live_in_doc(&app, &jar.to_string()), "the jar card goes with the rest");
    let doc = app.doc.journal.as_ref().unwrap().doc();
    let urls: Vec<FieldValue> =
        doc.live().filter(|(_, n)| n.kind() == Kind::Tab).map(|(_, n)| n.get(field::URL)).collect();
    assert_eq!(urls, [FieldValue::Str("https://new/".into())]);
    assert!(!app.doc.writes.has_deletes());
}

/// A jar from an opened or imported file (a fresh id) is created like a local one.
#[test]
fn an_imported_jar_is_created() {
    let mut app = app_on(&temp_dir("imported-jar-created").join("session.mnf"), 1);
    app.replace_running_session(&file_with_a_work_jar(), crate::store::json::Trust::Foreign);
    let jar = *app.jar_defs().keys().next().expect("the jar imported");
    assert!(live_in_doc(&app, &jar.to_string()));
}

#[test]
fn teardown_forgets_hidden_personal_state() {
    let path = temp_dir("sync-teardown-hidden").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://t/", 1.5));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &tab, true);
    assert!(app.save_session());
    app.teardown_running_session();
    assert!(app.doc.hidden_local.is_empty());
    assert!(
        app.doc.writes.life(tab.parse().unwrap()) == Some(Life::Delete),
        "the closed node is deleted with the session (D12)"
    );
}

/// A group export round-trips as a JSON snapshot: `export_group_json`
/// gives the group and its one member, root first, and `import_group_
/// json` recreates both under fresh ids, leaving the originals in
/// place (so the level now holds four nodes, not two).
#[test]
fn a_group_export_imports_as_fresh_nodes() {
    let dir = crate::store::testutil::temp_dir("group-json");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    let g = app.alloc_node_id();
    app.insert_group_at(g.clone(), Some(egui::vec2(0.0, 0.0)));
    let n = app.alloc_node_id();
    app.insert_note_at(n.clone(), Some(egui::vec2(10.0, 0.0)));
    app.edit_group(&n, Some(g.clone()));
    let json = app.export_group_json(&g).unwrap();
    assert_eq!(json.nodes.len(), 2);
    assert_eq!(json.nodes[0].kind(), Some(Kind::Group));
    app.import_group_json(&json);
    assert_eq!(snapshots(&app).len(), 4, "the import added a second group and note");
}

/// A jar deleted on another device while this one was not running never reaches `remove_remotely_deleted` here, so
/// the startup prune deletes the files of jars nothing holds (old enough, past the grace), unless it may not know
/// them all.
#[test]
fn startup_deletes_the_files_of_jars_the_document_no_longer_has() {
    use crate::content::tests::{write_aged, OLD};
    let dir = crate::store::testutil::temp_dir("jar-prune-startup");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let work = add_jar(&mut app, "Work");
    app.save_session();
    drop(app);
    let gone = NodeId { actor: ActorId(0xdead), counter: 9 };
    let (w, g) = (work.to_string(), gone.to_string());
    let files = [format!("{w}.json"), format!("{g}.json"), format!("{g}.json.unreadable-5"), "default.json".into()];
    files.iter().for_each(|f| write_aged(&jars.join(f), OLD));
    let left = || files.iter().filter(|f| jars.join(f).exists()).cloned().collect::<Vec<_>>();
    let on = |actor| App { jars_dir_override: Some(jars.clone()), ..app_on(&path, actor) };

    // No journal: nothing is known, nothing goes.
    assert!(!App { jars_dir_override: Some(jars.clone()), ..App::default() }.may_prune());
    // A repaired store may have lost jar nodes.
    let mut repaired = on(2);
    repaired.restore_session();
    repaired.doc.store_recovered = true;
    assert!(!repaired.may_prune());
    drop(repaired);

    let mut back = on(4);
    assert!(back.restore_session());
    assert!(back.may_prune());
    back.prune_backing_dirs();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while left().len() > 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10)); // a poll interval, not a wait for a guessed duration
    }
    assert_eq!(left(), [format!("{w}.json")], "the deleted jar's files and an old default.json go");
}

/// Jar name labels (retiring per-tab jars deviation 4): a jar is its id, never its name, so an imported jar is
/// always a new jar, even beside one with the same name, and the imported tab and group name the new one. The
/// old session's jar card goes with the rest of its cards.
#[test]
fn opening_a_session_gives_each_imported_jar_a_new_jar() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    app.replace_running_session(&file_with_a_work_jar(), crate::store::json::Trust::Foreign);
    assert_eq!(app.jar_defs().len(), 2, "the Default and the file's Work, which replaces the old one");
    assert!(!app.jar_defs().contains_key(&work), "the old jar card is torn down with its session");
    let new = *app.jar_defs().iter().find(|(_, d)| !d.is_default).unwrap().0;
    assert_eq!(app.jar_defs()[&new].name, "Work");
    let tab =
        app.nodes.iter().find(|(_, n)| n.runtime.as_unloaded().is_some()).map(|(id, _)| id).expect("tab imported");
    assert_eq!(app.doc.mirror.jar(tab), Some(new));
    let group = app.nodes.keys().find(|id| app.is_container(id)).expect("group imported");
    assert_eq!(app.doc.mirror.jar(group), Some(new));
}

/// Import Group the same, twice over: each import is another new jar.
#[test]
fn importing_a_group_gives_each_imported_jar_a_new_jar() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    for n in 2..=3 {
        app.import_group_json(&file_with_a_work_jar());
        assert_eq!(app.jar_defs().len(), n);
    }
    let new: BTreeSet<NodeId> = app.jar_defs().keys().copied().filter(|&j| j != work).collect();
    let tabs: BTreeSet<NodeId> = app
        .nodes
        .iter()
        .filter(|(_, n)| n.runtime.as_unloaded().is_some())
        .filter_map(|(id, _)| app.doc.mirror.jar(id))
        .collect();
    let groups: BTreeSet<NodeId> =
        app.nodes.keys().filter(|id| app.is_container(id)).filter_map(|id| app.doc.mirror.jar(id)).collect();
    assert_eq!((&tabs, &groups), (&new, &new), "each import's tab and group name its own new jar");
}

/// Same for a tab in an imported group: it sleeps in the new group.
#[test]
fn an_imported_groups_tab_arrives_asleep_in_the_new_group() {
    let mut app = app_on(&temp_dir("import-sleep").join("session.mnf"), 1);
    let g = add_group(&mut app, "G");
    let tab = add(&mut app, Kind::Tab, failed_tab("https://i/", 1.0));
    app.edit_group(&tab, Some(g.clone()));
    let json = app.export_group_json(&g).unwrap();
    app.import_group_json(&json);
    assert!(app.test_spawns.is_empty());
    let (copy_id, copy) = app.nodes.iter().find(|(id, n)| **id != tab && n.as_unloaded().is_some()).expect("a card");
    assert_eq!(copy.as_unloaded().unwrap().state, UnloadedState::Sleeping);
    let group = app.doc.mirror.group(copy_id).expect("in a group");
    assert!(group != g && app.is_container(group), "in the imported group");
}

/// A member whose group isn't part of the imported file lands where
/// the file's root does (the level being viewed), not at the top level.
#[test]
fn an_imported_member_whose_group_is_outside_the_file_joins_the_roots_level() {
    let mut app = app_on(&temp_dir("import-outside").join("session.mnf"), 1);
    let level = add_group(&mut app, "L");
    let elsewhere = add_group(&mut app, "E");
    app.current_group = Some(level.clone());
    let json = JsonDoc {
        nodes: vec![
            json_node("a", Kind::Group, serde_json::json!({ "name": "G" })),
            json_node("b", Kind::Note, serde_json::json!({ "text": "t", "group": elsewhere })),
        ],
        local: None,
    };
    app.import_group_json(&json);
    let (note, _) = app.nodes.iter().find(|(_, n)| matches!(&n.runtime, Runtime::Note(_))).expect("imported");
    assert_eq!(app.doc.mirror.group(note), Some(level.as_str()));
}

/// Export Group of a group the store doesn't hold gives nothing (and
/// says so) rather than an empty file.
#[test]
fn exporting_a_group_the_store_lacks_gives_none() {
    let mut app = app_on(&temp_dir("export-missing").join("session.mnf"), 1);
    let g = app.alloc_node_id();
    assert!(app.export_group_json(&g).is_none());
    assert!(!app.toasts.save_toast_queue.is_empty(), "toasted");
}

/// Replacing the running session skips the renders of its PDFs that haven't started, as deleting one does.
#[test]
fn teardown_cancels_pending_pdf_renders() {
    let mut app = App::default();
    let pdf = add(&mut app, Kind::Pdf, Runtime::Pdf(crate::node::PdfPayload { path: "d.pdf".into(), outcome: None }));
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    app.teardown_running_session();
    assert!(app.pdf_renderer.is_cancelled(&pdf));
    assert!(!app.pdf_renderer.is_cancelled(&note), "only PDFs are marked");
}

/// Wave 2 (teardown order): replacing the session queues a delete for every live non-jar node of the document and
/// for every link (saved or not), leaves nothing to create, and leaves the mirror no links.
#[test]
fn teardown_queues_every_node_and_link_delete() {
    let mut app = app_on(&temp_dir("teardown-queues").join("session.mnf"), 1);
    let notes: Vec<String> = (0..6).map(|_| a_note(&mut app)).collect();
    app.connect(LINK, &notes[0], &notes[1]).unwrap();
    app.connect(LINK, &notes[2], &notes[3]).unwrap();
    assert!(app.save_session());
    app.connect(LINK, &notes[4], &notes[5]).unwrap(); // in the mirror, not saved yet
    a_note(&mut app); // queued for create, not saved yet
    let mut want: BTreeSet<NodeId> = app
        .doc
        .journal
        .as_ref()
        .unwrap()
        .doc()
        .live()
        .filter(|(_, n)| n.kind() != Kind::Jar)
        .map(|(id, _)| id)
        .collect();
    want.extend(app.doc.mirror.link_ids());
    assert!(want.len() >= 9, "six notes and three links: {want:?}");
    app.teardown_running_session();
    assert_eq!(app.doc.writes.deletes().collect::<BTreeSet<_>>(), want);
    assert!(!app.doc.writes.has_creates());
    assert_eq!(app.doc.mirror.link_ids().count(), 0);
}

/// Replacing the session drops every link from the mirror, a dangling one too, and the save still agrees with the
/// document (`check_graph_matches_document`).
#[test]
fn replacing_the_session_drops_its_links() {
    let mut app = app_on(&temp_dir("link-replace").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.create_link(&a, &b);
    app.create_link("not-a-card", "nor-this"); // dangling: no endpoint's cascade reaches it
    assert!(app.save_session());
    app.replace_running_session(&file_with_a_work_jar(), crate::store::json::Trust::Foreign);
    assert!(app.doc.mirror.link_ids().next().is_none(), "no stale link");
    assert!(app.save_session());
}

/// A foreign file's link naming a card outside the file is dropped; one inside it is kept.
#[test]
fn a_foreign_import_drops_a_link_naming_a_card_outside_the_file() {
    let mut app = app_on(&temp_dir("link-foreign").join("session.mnf"), 1);
    let json = serde_json::json!({ "nodes": [
        json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
        json_node("u", Kind::Tab, serde_json::json!({ "url": "https://b/" })),
        json_node("in", Kind::Link, serde_json::json!({ "from": "t", "to": "u" })),
        json_node("out", Kind::Link, serde_json::json!({ "from": "t", "to": "424242" })),
    ]});
    app.replace_running_session(&serde_json::from_value(json).unwrap(), crate::store::json::Trust::Foreign);
    let ends: Vec<_> = app.doc.mirror.link_ids().map(|l| app.doc.mirror.link_ends(&l.to_string())).collect();
    assert_eq!(ends.len(), 1, "only the link inside the file");
    assert!(ends[0].0.is_some() && ends[0].1.is_some());
    assert!(app.save_session());
}

/// An outside end that is a real, valid id of a live card must still be dropped from a foreign file: otherwise an
/// imported card would be linked to an unrelated card of this session. Fails if the `json.rs` FROM/TO drop is reverted.
#[test]
fn a_foreign_import_drops_a_link_naming_a_live_card_outside_the_file() {
    let mut app = app_on(&temp_dir("link-foreign-live").join("session.mnf"), 1);
    let live = succeed_spawn(&mut app, "https://live/");
    let doc = link_doc(vec![
        json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
        json_node("l", Kind::Link, serde_json::json!({ "from": "t", "to": live })),
    ]);
    app.import_group_json(&doc);
    assert_eq!(link_count(&app), 0);
    assert!(app.links_naming(&live).is_empty());
}

/// Export Group keeps a link with both ends in the group and leaves out one to a card outside it.
#[test]
fn export_group_includes_links_inside_the_group_only() {
    let mut app = app_on(&temp_dir("link-export-group").join("session.mnf"), 1);
    let outside = succeed_spawn(&mut app, "https://out/");
    let (x, y) = (succeed_spawn(&mut app, "https://x/"), succeed_spawn(&mut app, "https://y/"));
    let group = add_group(&mut app, "g");
    app.edit_group(&x, Some(group.clone()));
    app.edit_group(&y, Some(group.clone()));
    let inside = app.create_link(&x, &y);
    let crossing = app.create_link(&x, &outside);
    let json = app.export_group_json(&group).unwrap();
    let ids: Vec<&str> = json.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&inside.as_str()), "the inner link is exported");
    assert!(!ids.contains(&crossing.as_str()), "the crossing link is not");
    // And it round-trips through Import Group.
    app.import_group_json(&json);
    assert_eq!(link_count(&app), 3, "the two originals and one imported copy");
}

/// An Own-trust file with a dangling link (a real-format id of no node) creates no junk link.
#[test]
fn an_own_import_skips_a_link_whose_end_is_not_a_node() {
    let mut app = app_on(&temp_dir("link-own-dangling").join("session.mnf"), 1);
    let doc = link_doc(vec![
        json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
        json_node("l", Kind::Link, serde_json::json!({ "from": "t", "to": "ffffffffffffffff-999" })),
    ]);
    app.replace_running_session(&doc, crate::store::json::Trust::Own);
    assert_eq!(link_count(&app), 0);
    assert!(app.save_session());
}

/// Export Session leaves out a link with a dead end and keeps a good one.
#[test]
fn export_session_drops_links_with_a_dead_end() {
    let mut app = app_on(&temp_dir("link-export-session").join("session.mnf"), 1);
    let (a, b) = (succeed_spawn(&mut app, "https://a/"), succeed_spawn(&mut app, "https://b/"));
    let good = app.create_link(&a, &b);
    let dead = app.create_link(&a, "ffffffffffffffff-777");
    let json = app.export_session_json().unwrap();
    let ids: Vec<&str> = json.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&good.as_str()));
    assert!(!ids.contains(&dead.as_str()));
}

/// Export Session writes through `store::atomic::replace`: a blocked target fails the export and stays unwritten.
#[test]
fn export_session_is_written_through_the_atomic_replace() {
    let dir = temp_dir("export-session-atomic");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    succeed_spawn(&mut app, "https://a/");
    let path = dir.join("export.json");
    crate::store::atomic::block_for_test(&path, true);
    assert!(!app.export_session_to(&path));
    assert!(!path.exists(), "the write went around store::atomic::replace");
    crate::store::atomic::block_for_test(&path, false);
    assert!(app.export_session_to(&path));
    let written: JsonDoc = crate::json_file::load(&path, "session").unwrap();
    assert_eq!(written.nodes, app.export_session_json().unwrap().nodes);
}

/// A link end must be a card, a jar included: a file's link naming another link or itself loses that end, and is skipped.
#[test]
fn an_import_keeps_a_link_to_a_jar_and_drops_one_whose_end_is_a_link_or_itself() {
    let mut app = app_on(&temp_dir("link-bad-ends").join("session.mnf"), 1);
    let doc = link_doc(vec![
        json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
        json_node("j", Kind::Jar, serde_json::json!({ "name": "Work" })),
        json_node("l1", Kind::Link, serde_json::json!({ "from": "t", "to": "j" })),
        json_node("l2", Kind::Link, serde_json::json!({ "from": "t", "to": "l1" })),
        json_node("l3", Kind::Link, serde_json::json!({ "from": "l3", "to": "t" })),
    ]);
    for trust in [crate::store::json::Trust::Own, crate::store::json::Trust::Foreign] {
        app.replace_running_session(&doc, trust);
        assert_eq!(link_count(&app), 1, "{trust:?}: only the jar link");
    }
}

/// C1: replacing the running session forgets the old tabs' per-id state (and, with a GPU, frees their textures).
#[test]
fn teardown_forgets_the_old_tabs_per_id_state() {
    let mut app = App::default();
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://a.test/", "a"));
    let now = std::time::Instant::now();
    app.saved_scroll.insert(t.clone(), crate::sleep_state::SavedScroll::new(0.0, 1.0));
    app.spawns.tab_crashes.insert(t.clone(), vec![now]);
    app.spawns.skip_convert.insert(t.clone(), true);
    app.cookies.jar_pulls.insert(t.clone(), now);
    app.teardown_running_session();
    assert!(app.nodes.is_empty());
    assert!(app.saved_scroll.is_empty(), "saved_scroll");
    assert!(app.spawns.tab_crashes.is_empty(), "tab_crashes");
    assert!(app.spawns.skip_convert.is_empty(), "skip_convert");
    assert!(app.cookies.jar_pulls.is_empty(), "jar_pulls");
}

#[test]
fn a_session_of_only_jars_is_a_fresh_start_that_keeps_them() {
    let path = temp_dir("jar-only").join("session.mnf");
    let mut app = app_on(&path, 1);
    let jar = add_jar(&mut app, "Work").to_string();
    assert!(app.save_session());
    let mut back = app_on(&path, 2);
    assert!(!back.restore_session(), "only jars is a fresh start");
    assert!(back.nodes.contains_key(&jar));
}

#[test]
fn replacing_the_running_session_keeps_the_default_jar() {
    let dir = temp_dir("default-replace");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    app.ensure_default_jar();
    let default = app.default_jar().unwrap();
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    assert!(app.save_session());
    app.teardown_running_session();
    assert!(app.jar_exists(default));
    assert_eq!(app.nodes.len(), 1, "only the Default survives a teardown");
}

/// A file's own Default jar is dropped on import, and the tabs that named it name the running one.
#[test]
fn an_imported_default_jar_is_the_running_one() {
    let mut app = App::default();
    app.ensure_default_jar();
    let running = app.default_jar().unwrap();
    let doc = JsonDoc {
        nodes: vec![
            json_node("a", Kind::Jar, serde_json::json!({ "name": "Default", "is_default": true })),
            json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/", "jar": "a" })),
        ],
        local: None,
    };
    app.replace_running_session(&doc, crate::store::json::Trust::Foreign);
    assert_eq!(app.jar_defs().len(), 1, "no second Default");
    let tab = app
        .nodes
        .iter()
        .find(|(_, n)| n.runtime.as_unloaded().is_some())
        .map(|(id, _)| id.clone())
        .expect("tab imported");
    assert_eq!(app.doc.mirror.jar(&tab), Some(running), "an edge to the file's Default names the running one");
}

/// Replacing a session keeps the running Default; the file's other jars come with it, and tabs naming its Default
/// or a named jar get the right one.
#[test]
fn a_replaced_sessions_tabs_use_the_running_default() {
    let mut app = App::default();
    app.ensure_default_jar();
    let running = app.default_jar().unwrap();
    add(&mut app, Kind::Tab, sleeping_tab("https://old/", "Old"));
    let doc = JsonDoc {
        nodes: vec![
            json_node("d", Kind::Jar, serde_json::json!({ "name": "Default", "is_default": true })),
            json_node("w", Kind::Jar, serde_json::json!({ "name": "Work" })),
            json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/", "jar": "d" })),
            json_node("u", Kind::Tab, serde_json::json!({ "url": "https://b/", "jar": "w" })),
        ],
        local: None,
    };
    app.replace_running_session(&doc, crate::store::json::Trust::Foreign);
    assert_eq!(app.jar_defs().len(), 2, "the running Default and the file's Work");
    let work = *app.jar_defs().iter().find(|(_, d)| d.name == "Work").unwrap().0;
    let jar_of_url = |url: &str| {
        let id = app.nodes.iter().find(|(_, n)| n.runtime.as_unloaded().is_some_and(|u| u.url == url)).unwrap().0;
        app.doc.mirror.jar(id)
    };
    assert_eq!(jar_of_url("https://a/"), Some(running), "the file's Default is the running one");
    assert_eq!(jar_of_url("https://b/"), Some(work), "a named jar comes with the session");
}
