//! Session-sync tests: closing, reopening and restoring nodes and the trail.

use super::*;

/// Closing an unloaded card leaves its children naming it (decision 9): it is closed, not gone, and a reopen
/// restores the edge as it was.
#[test]
fn closing_an_unloaded_tab_keeps_its_childrens_edge() {
    let mut app = app_on(&temp_dir("unloaded-close").join("session.mnf"), 1);
    let root = add(&mut app, Kind::Tab, sleeping_tab("https://root/", ""));
    let mid = add(&mut app, Kind::Tab, failed_tab("https://mid/", 1.0));
    let leaf = add(&mut app, Kind::Tab, sleeping_tab("https://leaf/", ""));
    app.edit_parent(mid, Some(root));
    app.edit_parent(leaf, Some(mid));
    app.close_node(mid);
    assert!(!app.nodes.contains_key(&mid));
    assert_eq!(app.doc.mirror.parent(leaf), Some(mid), "the edge stays: mid is closed, not gone");
    assert_eq!(app.opened_from(leaf), None, "and reads as none while its parent is closed");
    app.reopen_node(mid);
    assert_eq!(app.opened_from(leaf), Some(mid), "and is back after a reopen");
}

/// A reopened tab comes back asleep, starting no engine, and keeps its
/// zoom and title (spec "Lazy tabs").
#[test]
fn a_reopened_tab_comes_back_asleep_with_its_zoom_and_title() {
    let mut app = app_on(&temp_dir("reopen-sleep").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://r/", "Page R"));
    if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
        t.zoom_factor = 1.5;
    }
    assert!(app.save_session());
    app.close_node(tab);
    assert!(!app.nodes.contains_key(&tab));
    app.reopen_node(tab);
    assert!(app.test_spawns.is_empty());
    assert_eq!(app.nodes.len(), 1, "the same node, no copy");
    let cards: Vec<(&NodeId, &UnloadedTabPayload)> =
        app.nodes.iter().filter_map(|(id, n)| Some((id, n.as_unloaded()?))).collect();
    assert!(
        matches!(&cards[..], [(id, t)] if **id == tab && t.url == "https://r/" && t.title == "Page R"
            && t.zoom_factor == 1.5 && t.state == UnloadedState::Sleeping),
        "no sleeping card"
    );
}

/// A session with nothing shown still keeps Recently Closed: the launch starts fresh, but the closed tab is still in
/// the document's history, and its personal state (the title) is kept for a reopen.
#[test]
fn an_empty_session_keeps_recently_closed_across_a_launch() {
    let path = temp_dir("empty-closed").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://r/", "Page R"));
        assert!(app.save_session());
        app.close_node(tab);
        app.close_store();
        tab
    };
    let mut app = app_on(&path, 2);
    assert!(!app.restore_session(), "nothing shown: the caller starts fresh");
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().closed_roots().len(), 1);
    assert_eq!(app.doc.hidden_local[&tab].title, "Page R");
}

/// Every link survives a close and comes back with the group under the same id, the outside one included.
#[test]
fn reopening_a_closed_group_restores_every_link_with_its_id() {
    let mut app = app_on(&temp_dir("link-closed").join("session.mnf"), 1);
    let outside = succeed_spawn(&mut app, "https://out/");
    let (x, y) = (succeed_spawn(&mut app, "https://x/"), succeed_spawn(&mut app, "https://y/"));
    let group = add_group(&mut app, "g");
    app.edit_group(x, Some(group));
    app.edit_group(y, Some(group));
    app.connect(crate::caps::LINK, x, y).unwrap();
    app.create_link(x, outside); // `connect` would refuse another level, so build this one directly
    assert!(app.save_session());
    let before: std::collections::BTreeSet<_> = app.doc.mirror.link_ids().collect();
    assert_eq!(before.len(), 2);
    app.close_node(group);
    app.reopen_node(group);
    let after: std::collections::BTreeSet<_> = app.doc.mirror.link_ids().collect();
    assert_eq!(after, before, "every link returns with its id");
    for l in &after {
        let (from, to) = app.doc.mirror.link_ends(*l);
        let (from, to) = (from.unwrap(), to.unwrap());
        assert!(app.nodes.contains_key(&from) && app.nodes.contains_key(&to), "between live cards");
    }
    assert!(app.save_session());
}

/// Closing an outer group: a link between a member and the root, and one across nested groups, come back; the blank
/// note is deleted with its link.
#[test]
fn closing_keeps_links_to_the_root_and_across_nesting_and_deletes_a_blank_members_link() {
    let mut app = app_on(&temp_dir("link-closed-kept").join("session.mnf"), 1);
    let outer = add_group(&mut app, "outer");
    let inner = add_group(&mut app, "inner");
    app.edit_group(inner, Some(outer));
    let x = succeed_spawn(&mut app, "https://x/");
    let y = succeed_spawn(&mut app, "https://y/");
    app.edit_group(x, Some(outer));
    app.edit_group(y, Some(inner));
    let blank = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: " ".into(), editing: false }));
    app.edit_group(blank, Some(outer));
    app.create_link(x, outer); // member and root
    app.create_link(x, y); // across nested groups
    app.create_link(x, blank); // to a member that is deleted on close
    assert!(app.save_session());
    app.close_node(outer);
    assert!(
        !app.doc.journal.as_ref().unwrap().doc().nodes.contains_key(&blank)
            || !app.doc.journal.as_ref().unwrap().doc().nodes[&blank].is_live()
    );
    app.reopen_node(outer);
    assert_eq!(link_count(&app), 2);
    assert!(app.save_session());
}

#[test]
fn closing_a_jar_keeps_it_for_reopen() {
    let path = temp_dir("jar-close").join("session.mnf");
    let mut app = app_on(&path, 1);
    let jar = app.create_jar("Work", crate::fetched::Placement { group: None, pos: egui::vec2(5.0, 5.0) });
    assert!(app.save_session());
    app.close_node(jar);
    assert!(!app.nodes.contains_key(&jar) && app.jar_defs().is_empty());
    app.reopen_node(jar);
    let defs = app.jar_defs();
    assert_eq!(defs.len(), 1);
    assert!(defs.contains_key(&jar), "the same id");
    assert_eq!(defs.values().next().unwrap().name, "Work");
}

#[test]
fn a_closed_tab_keeps_its_trail_across_a_save_ctrl_shift_t_and_a_restart() {
    let dir = temp_dir("sync-trail-closed");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
    assert!(app.save_session());
    app.nav_history.insert(tab, crate::nav_history::NavHistory::visited(&["https://a/", "https://b/"]));
    app.close_node(tab);
    assert!(!app.nodes.contains_key(&tab));
    assert!(app.nav_history.contains_key(&tab), "the hide and the close's save kept it");
    let file = dir.join("tab-history.json");
    let (nodes, journal) = (&app.nodes, app.doc.journal.as_ref());
    crate::nav_history::save_to(&mut app.nav_history, |id| keep_trail(nodes, journal, id), Some(&file));
    assert!(crate::nav_history::load_from(&file).contains_key(&tab), "a restart keeps it");
    app.reopen_closed_shortcut();
    assert!(app.nodes.contains_key(&tab));
    assert!(app.nav_history.get(&tab).is_some_and(|h| h.can_go_back()), "Ctrl+Shift+T keeps it");
}

#[test]
fn deleting_a_shown_tab_drops_its_trail_at_once() {
    let mut app = app_on(&temp_dir("sync-trail-delete").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
    assert!(app.save_session());
    app.nav_history.insert(tab, crate::nav_history::NavHistory::visited(&["https://a/", "https://b/"]));
    app.destroy_node(tab);
    assert!(!app.nav_history.contains_key(&tab));
}

#[test]
fn keep_trail_adds_document_liveness_to_the_shown_test() {
    let mut app = app_on(&temp_dir("sync-trail-keep").join("session.mnf"), 1);
    let shown = add(&mut app, Kind::Tab, sleeping_tab("https://s/", "S"));
    let closed = add(&mut app, Kind::Tab, sleeping_tab("https://c/", "C"));
    let gone = add(&mut app, Kind::Tab, sleeping_tab("https://g/", "G"));
    assert!(app.save_session());
    app.close_node(closed);
    app.destroy_node(gone);
    assert!(app.save_session());
    let journal = app.doc.journal.as_ref();
    assert!(keep_trail(&app.nodes, journal, shown));
    assert!(keep_trail(&app.nodes, journal, closed), "closed: hidden, but live in the document");
    assert!(!keep_trail(&app.nodes, journal, gone));
    assert!(!keep_trail(&app.nodes, journal, node(99)));
    assert!(keep_trail(&app.nodes, None, node(99)), "no document: keep everything");
}

#[test]
fn a_session_replace_clears_stashed_trails() {
    let mut app = app_on(&temp_dir("sync-trail-teardown").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
    assert!(app.save_session());
    app.nav_history.insert(tab, crate::nav_history::NavHistory::visited(&["https://a/", "https://b/"]));
    app.close_node(tab);
    assert!(app.nav_history.contains_key(&tab));
    app.teardown_running_session();
    assert!(app.nav_history.is_empty());
}
