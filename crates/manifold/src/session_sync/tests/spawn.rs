//! Session-sync tests: tab spawn, respawn, restore and view swaps.

use super::*;

/// A Root/New spawn that fails keeps its place as a failed card, in the document whether or not a save had created
/// it while in flight; Split, whose pane is not a card, is deleted and only toasts.
#[test]
fn a_failed_new_spawn_leaves_a_failed_card() {
    for saved_first in [true, false] {
        let mut app = app_on(&temp_dir("failed-new-spawn").join("session.mnf"), 1);
        let id = app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
        if saved_first {
            assert!(app.save_session());
            assert!(live_in_doc(&app, id), "created while in flight");
        }
        fail_spawns(&mut app);
        let tab = app.nodes[&id].as_unloaded().expect("a failed card");
        assert!(matches!(tab.state, UnloadedState::Failed(_)), "saved_first={saved_first}");
        assert_eq!(tab.url, "https://a/");
        assert_eq!(app.card_kind(id).map(|kind| kind.ty), Some(crate::store::model::TYPE_TAB));
        assert!(live_in_doc(&app, id), "persisted, saved_first={saved_first}");
        assert!(!app.doc.writes.has_creates() && !app.doc.writes.has_deletes());
    }
}

/// A failed Root also leaves the app in Graph view (success does), so the card is seen.
#[test]
fn a_failed_root_spawn_shows_its_card_in_graph_view() {
    let mut app = app_on(&temp_dir("failed-root-spawn").join("session.mnf"), 1);
    assert_eq!(app.view_mode, ViewMode::Item, "the app starts in Item view");
    app.spawn_root_tab();
    fail_spawns(&mut app);
    assert_eq!(app.nodes.len(), 1);
    assert_eq!(app.view_mode, ViewMode::Graph);
}

/// A failed Split makes no card: its node is deleted, and the failure only toasts.
#[test]
fn a_failed_split_spawn_toasts_and_makes_no_card() {
    let mut app = app_on(&temp_dir("failed-split-spawn").join("session.mnf"), 1);
    let id =
        app.spawn_new_tab_with_kind(None, None, "https://s/", crate::ViewKind::Servo, SpawnPurpose::Split, None, None);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(!app.nodes.contains_key(&id), "no card for a split pane");
    assert!(!live_in_doc(&app, id), "deleted by the failure's save");
    assert!(app.toasts.save_toast_queue.iter().any(|toast| toast.contains("Couldn't start split view")));
}

/// A tab closed while its wake is in flight lands hidden; reopened before it lands, it shows when it does
/// (the landing must find its node, or the document would show a tab the graph lacks).
#[test]
fn closing_and_reopening_a_tab_while_it_wakes_shows_it_when_the_wake_lands() {
    let path = temp_dir("sync-close-reopen-waking").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    let other = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
    assert!(app.save_session());
    app.focus_tab(tab);
    assert!(app.spawns.spawning.contains_key(&tab), "waking");
    app.close_node(tab);
    assert!(!app.nodes.contains_key(&tab));
    let id: NodeId = tab;
    app.reopen_node(id);
    assert!(app.doc.journal.as_ref().unwrap().doc().is_shown(id), "open in the document");
    fail_spawns(&mut app);
    assert!(app.nodes.contains_key(&tab), "shown once the wake landed");
    assert!(app.nodes.contains_key(&other));
    assert!(app.spawns.remote_in_flight.is_empty() && app.spawns.wakes_in_flight.is_empty());

    // Closed and left closed: the landing hides it again, and the history holds it.
    app.focus_tab(other);
    app.close_node(other);
    fail_spawns(&mut app);
    assert!(!app.nodes.contains_key(&other));
    assert!(app.doc.journal.as_ref().unwrap().doc().nodes[&other].closed());
}

/// The same for a tab mid-respawn (no `nodes` entry), where the close parks `Remote::Hidden`: a reopen
/// supersedes it instead of the landing hiding the reopened tab again.
#[test]
fn closing_and_reopening_a_tab_mid_respawn_is_not_hidden_again_by_the_landing() {
    let path = temp_dir("sync-close-reopen-respawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    assert!(app.save_session());
    app.nodes.remove(&tab).unwrap();
    let spawn = crate::tabs::TabSpawn::new(
        tab,
        "https://a/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Respawn,
        1.0,
    );
    app.respawn_tab(spawn);
    assert!(app.spawns.spawning.contains_key(&tab) && !app.nodes.contains_key(&tab));
    app.close_node(tab);
    assert!(matches!(app.spawns.remote_in_flight.get(&tab), Some(Remote::Hidden)), "parked hidden");
    let id: NodeId = tab;
    app.reopen_node(id);
    assert!(matches!(app.spawns.remote_in_flight.get(&tab), Some(Remote::Shown(_))), "the reopen supersedes it");
    fail_spawns(&mut app);
    assert!(app.nodes.contains_key(&tab), "shown, not hidden again by the landing");
    assert!(!app.doc.journal.as_ref().unwrap().doc().nodes[&id].closed());
}

/// The layout table is keyed by id and outlives a respawn's remove-and-reinsert; `TabSpawn` carries no
/// position for it (this respawn starts at the origin), and a save mid-respawn keeps the entry.
#[test]
fn a_respawn_keeps_the_layout_position() {
    let path = temp_dir("pos-respawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.snap_layout(tab, egui::vec2(30.0, 40.0));
    assert!(app.save_session());
    start_respawn(&mut app, tab);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(app.nodes.contains_key(&tab), "landed as a failed card");
    assert_eq!(app.layout_pos(tab), egui::vec2(30.0, 40.0));
}

/// A new tab has no layout entry or mirror entry until it lands: it lands at its spawn position, which is its
/// document pos too.
#[test]
fn a_new_tab_lands_at_its_spawn_position_in_the_layout_and_the_document() {
    let path = temp_dir("pos-new-spawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = app.alloc_node_id();
    let at = egui::vec2(30.0, 40.0);
    app.spawn_tab_async(crate::tabs::TabSpawn::new(
        t,
        "https://t/".into(),
        ViewKind::Servo,
        at,
        SpawnPurpose::Restore,
        1.0,
    ));
    assert!(app.save_session(), "creates the spawning tab");
    assert_eq!(stored(&app, t, field::POS), FieldValue::Pos(Some((30.0, 40.0))));
    fail_spawns(&mut app);
    assert_eq!(app.layout_pos(t), at);
    assert_eq!(app.doc.mirror.pos(t), Some(at));
}

/// A restore starts only the focused tab; every other tab is a
/// sleeping card with its saved title.
#[test]
fn a_restore_starts_only_the_focused_tab() {
    let path = temp_dir("lazy-restore").join("session.mnf");
    let (a, b, c) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        let c = add(&mut app, Kind::Tab, sleeping_tab("https://c/", "C"));
        app.focused = Some(b);
        assert!(app.save_session());
        app.maybe_write_local(true);
        app.close_store();
        (a, b, c)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    let spawned: Vec<NodeId> = app.test_spawns.iter().map(|(s, _)| s.id).collect();
    assert_eq!(spawned, [b]);
    assert!(matches!(app.test_spawns[0].0.purpose, SpawnPurpose::Wake));
    for (id, title) in [(&a, "A"), (&c, "C")] {
        let t = app.nodes[id].as_unloaded().unwrap();
        assert_eq!((t.state.clone(), t.title.as_str()), (UnloadedState::Sleeping, title));
    }
    assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Waking);
}

/// A restored split session wakes both halves -- the focused tab and
/// the saved split tab -- leaving every other tab asleep (`apply_local`
/// restores `split_tab`, so waking only the focused tab would leave the
/// split pane empty).
#[test]
fn restoring_a_split_session_wakes_both_the_focused_and_split_tab() {
    let path = temp_dir("lazy-restore-split").join("session.mnf");
    let (a, b, c) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        let c = add(&mut app, Kind::Tab, sleeping_tab("https://c/", "C"));
        app.focused = Some(a);
        app.split_tab = Some(b);
        assert!(app.save_session());
        app.maybe_write_local(true);
        app.close_store();
        (a, b, c)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    let mut spawned: Vec<NodeId> = app.test_spawns.iter().map(|(s, _)| s.id).collect();
    spawned.sort();
    let mut expected = [a, b];
    expected.sort();
    assert_eq!(spawned, expected);
    assert!(app.test_spawns.iter().all(|(s, _)| matches!(s.purpose, SpawnPurpose::Wake)));
    assert_eq!(app.nodes[&c].as_unloaded().unwrap().state, UnloadedState::Sleeping);
}

/// Opening a sleeping tab, by any path, wakes it once: focused, in Tab
/// view (showing the respawning placeholder), one spawn. A second open
/// while it wakes issues none.
#[test]
fn opening_a_sleeping_tab_wakes_it_once() {
    let mut app = app_on(&temp_dir("lazy-open").join("session.mnf"), 1);
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "Alpha"));
    let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "Beta"));
    app.focus_item(a);
    app.focus_item(a);
    assert_eq!(app.test_spawns.len(), 1);
    assert_eq!(app.focused, Some(a));
    assert_eq!(app.view_mode, ViewMode::Item);
    assert_eq!(app.spawns.pending_respawn_focus, Some(a));
    app.focus_tab(b);
    assert_eq!(app.test_spawns.len(), 2);
    assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Waking);
}

/// Tab lists (Quick Switcher, Ctrl+Tab, graph navigation, Close All,
/// pinning) include sleeping tabs, and opening one from them wakes it.
#[test]
fn tab_lists_include_sleeping_tabs() {
    let mut app = app_on(&temp_dir("lazy-lists").join("session.mnf"), 1);
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "Alpha"));
    let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "Beta"));
    app.open_omni_popup();
    app.omni_set_text(crate::omnibar::Host::Popup, "beta");
    app.omni_confirm(crate::omnibar::Host::Popup);
    assert_eq!(app.focused, Some(b));
    assert_eq!(app.test_spawns.len(), 1);
    app.cycle_focused_tab(true);
    assert_eq!(app.focused, Some(a));
    assert_eq!(app.test_spawns.len(), 2);
    assert_eq!(app.tabs_at_current_level(), [a, b]);
    app.set_all_pinned(true);
    assert!(app.doc.mirror.pinned(a) && app.doc.mirror.pinned(b));
    app.close_all_at_current_level();
    assert!(app.nodes.is_empty());
}

/// A failed wake leaves a failed card, still focused, and ends the
/// respawning placeholder (Tab view then falls back to Graph).
#[test]
fn a_failed_wake_leaves_a_failed_card() {
    let mut app = app_on(&temp_dir("lazy-fail").join("session.mnf"), 1);
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    app.focus_tab(a);
    fail_spawns(&mut app);
    assert!(matches!(app.nodes[&a].as_unloaded().unwrap().state, UnloadedState::Failed(_)));
    assert_eq!(app.nodes[&a].as_unloaded().unwrap().title, "A", "the wake carried its title");
    assert_eq!(app.spawns.pending_respawn_focus, None);
    assert!(app.spawns.wakes_in_flight.is_empty());
}

/// Closing a waking tab clears `pending_respawn_focus` when it names
/// the removed id -- otherwise it keeps pointing at a gone node, which
/// `build_tab_view_ui`'s guard reads to decide whether Item view stays
/// put during a respawn.
#[test]
fn removing_a_tab_clears_its_stale_pending_respawn_focus() {
    let mut app = app_on(&temp_dir("remove-pending-focus").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    app.focus_tab(tab);
    assert_eq!(app.spawns.pending_respawn_focus, Some(tab));
    app.destroy_tab(tab);
    assert_eq!(app.spawns.pending_respawn_focus, None);
}

/// The same intent failing again (same url, engine, group) refreshes the one card instead of adding another; a
/// different url is a different intent.
#[test]
fn a_repeat_failure_refreshes_one_card_and_a_new_url_adds_one() {
    let mut app = app_on(&temp_dir("failed-twin").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1, "the same intent is one card");
    assert!(app.save_session());
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.live().count(), 1, "and one document node");
    app.spawn_tab_at("https://b/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2, "another url is another intent");
}

/// A group is part of the intent: the same url failing in two groups is two cards.
#[test]
fn a_failed_twin_in_another_group_is_not_a_twin() {
    let mut app = app_on(&temp_dir("failed-twin-group").join("session.mnf"), 1);
    let group = add_group(&mut app, "G");
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, Some(group), None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2);
}

/// A tab that lands clears the failed cards for its url, and only those.
#[test]
fn a_successful_spawn_clears_failed_cards_for_its_url() {
    let mut app = app_on(&temp_dir("failed-cleared").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    app.spawn_tab_at("https://b/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2);
    let stale = failed_cards(&app).into_iter().find(|id| app.nodes[id].as_unloaded().unwrap().url == "https://a/");
    let stale = stale.unwrap();
    let landed = succeed_spawn(&mut app, "https://a/");
    let left = failed_cards(&app);
    assert_eq!(left.len(), 1, "a/'s failed card is gone, b/'s stays");
    assert!(app.nodes[&left[0]].as_unloaded().unwrap().url == "https://b/");
    assert!(app.nodes.contains_key(&landed));
    assert!(!live_in_doc(&app, stale), "the stale card is deleted in the document");
    let closed = app.doc.journal.as_ref().unwrap().doc().closed_roots();
    assert!(closed.is_empty(), "and not in the history");
}

/// The group is part of the intent: a failed card for the same url in another group stays when a tab lands.
#[test]
fn a_successful_spawn_leaves_a_same_url_failed_card_in_another_group() {
    let mut app = app_on(&temp_dir("failed-kept-other-group").join("session.mnf"), 1);
    let group = add_group(&mut app, "G");
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, Some(group), None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    let landed = succeed_spawn(&mut app, "https://a/");
    assert_eq!(failed_cards(&app).len(), 1, "the card in group G is another intent");
    assert!(app.nodes.contains_key(&landed));
}

/// The clear queues its deletes without a save, so another replica's delete parked for the landing tab
/// (`remote_in_flight`) still lands before the first save of the landing.
#[test]
fn clearing_failed_cards_does_not_save_before_a_parked_remote_delete_lands() {
    let path = temp_dir("failed-cleared-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    let card = failed_cards(&app).remove(0);
    let id = app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    // The failed twin is not refreshed here: the spawn is still in flight, so it is its own node.
    assert!(app.save_session());
    assert!(app.doc.journal.as_ref().unwrap().doc().nodes.contains_key(&id), "the spawn is committed");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: id }]).unwrap();
    assert!(app.save_session());
    assert!(app.spawns.remote_in_flight.contains_key(&id), "the remote delete is parked");

    let at = app.test_spawns.iter().position(|(spawn, _)| spawn.id == id).expect("the spawn is parked");
    let (spawn, generation) = app.test_spawns.remove(at);
    let (tab, _peer) = crate::process_manager::ViewTab::fake(spawn.kind);
    app.handle_tab_spawn_result(crate::tabs::TabSpawnOutcome {
        spawn,
        generation,
        result: Ok(tab),
        seeded: Default::default(),
        jar: Default::default(),
        seed_clears: None,
    });
    assert!(app.save_session());
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert!(!app.nodes.contains_key(&id), "the remotely deleted landing is gone");
    assert!(!app.nodes.contains_key(&card), "the failed card was cleared");
    assert_eq!(doc.live().count(), 0, "the document agrees");
}

/// A respawn lands an existing tab: it is no new intent, so a same-url failed card stays.
#[test]
fn a_respawn_landing_leaves_a_same_url_failed_card() {
    let mut app = app_on(&temp_dir("failed-kept-on-respawn").join("session.mnf"), 1);
    let (tab, _peer) = live_tab(&mut app, ViewKind::Servo, "https://a/");
    assert!(app.save_session());
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    app.handle_view_message(tab, manifold_view_protocol::ViewToHost::Crashed { reason: "boom".into() });
    assert!(app.test_spawns.iter().any(|(s, _)| matches!(s.purpose, SpawnPurpose::Respawn)), "restarting");
    let _peers = land_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1, "the failed card stays");
    assert!(app.save_session());
}

/// A repeat Root failure refreshes the card and still shows Graph view.
#[test]
fn a_repeat_root_failure_still_shows_graph_view() {
    let mut app = app_on(&temp_dir("failed-root-twin").join("session.mnf"), 1);
    app.spawn_root_tab();
    fail_spawns(&mut app);
    app.view_mode = ViewMode::Item;
    app.spawn_root_tab();
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    assert!(matches!(app.view_mode, ViewMode::Graph));
}

/// Open Session (`replace_running_session`) restores every tab asleep,
/// same as a launch, and wakes only the focused (first) one -- the
/// existing `tabs_still_spawning_at_quit_are_saved` uses a single tab,
/// which can't tell "asleep" from "still spawning" apart.
#[test]
fn open_session_restores_asleep_and_wakes_only_the_focused_tab() {
    let mut app = app_on(&temp_dir("open-session-lazy").join("session.mnf"), 1);
    let json = JsonDoc {
        nodes: vec![
            json_node("t1", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
            json_node("t2", Kind::Tab, serde_json::json!({ "url": "https://b/" })),
            json_node("t3", Kind::Tab, serde_json::json!({ "url": "https://c/" })),
        ],
        local: None,
    };
    app.replace_running_session(&json, crate::store::json::Trust::Own);
    assert_eq!(app.test_spawns.len(), 1, "only the focused (first) tab wakes");
    assert!(matches!(app.test_spawns[0].0.purpose, SpawnPurpose::Wake));
    let focused = app.focused.unwrap();
    assert_eq!(app.test_spawns[0].0.id, focused);
    let asleep: Vec<&UnloadedTabPayload> =
        app.nodes.iter().filter(|(id, _)| **id != focused).filter_map(|(_, n)| n.as_unloaded()).collect();
    assert_eq!(asleep.len(), 2);
    assert!(asleep.iter().all(|t| t.state == UnloadedState::Sleeping));
}

/// Without personal state, restore focuses the first tab (as it always
/// has) and wakes only that one.
#[test]
fn restore_without_personal_state_focuses_and_wakes_the_first_tab() {
    let path = temp_dir("no-local-restore").join("session.mnf");
    let (a, b) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        assert!(app.save_session());
        // No `maybe_write_local`: no `session.mnf.local` at all, the
        // literal "without personal state" case, not just one whose
        // `focused` field happens to be unset.
        (a, b)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.focused, Some(a));
    let spawned: Vec<NodeId> = app.test_spawns.iter().map(|(s, _)| s.id).collect();
    assert_eq!(spawned, [a]);
    assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Sleeping);
}

/// A respawn removes the node from `nodes` until it lands; a save meanwhile keeps its mirror entry, so an edit
/// made then survives the landing (`Mirror::init` writes only an id with no entry).
#[test]
fn a_save_during_a_respawn_keeps_the_mirror_entry() {
    let path = temp_dir("edit-respawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    app.edit_field(tab, field::NAME, FieldValue::Str("one".into()));
    app.edit_field(tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    // What `finish_swap` and the crash path do: carry from the mirror, remove the node, respawn.
    app.nodes.remove(&tab).unwrap();
    let mut spawn = crate::tabs::TabSpawn::new(
        tab,
        "https://a/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Respawn,
        1.0,
    );
    spawn.carry.pinned = app.doc.mirror.pinned(tab);
    app.respawn_tab(spawn);
    assert!(app.save_session());
    assert!(app.doc.mirror.pinned(tab) && app.doc.mirror.name(tab) == "one", "kept while parked");
    app.edit_field(tab, field::NAME, FieldValue::Str("two".into()));
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.name(tab), "two");
    assert!(app.doc.mirror.pinned(tab));
    assert_eq!(stored(&app, tab, field::NAME), FieldValue::Str("two".into()));
    assert_eq!(stored(&app, tab, field::PINNED), FieldValue::Bool(true));
}

/// Remote: another replica deletes G while T respawns here. The cascade reaches T in the mirror
/// only, and its landing commits no move: T is in O on every replica, as the store shows it.
#[test]
fn a_remote_group_delete_during_a_respawn_lands_the_tab_in_its_group_without_a_move() {
    let path = temp_dir("respawn-remote-dissolve").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (outer, g, t) = nested_tab(&mut app);
    start_respawn(&mut app, t);
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: g }]).unwrap();
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.group(t), Some(outer), "the cascade reached the respawning tab");
    assert!(!app.doc.writes.has_moves(), "a remote cascade queues nothing");
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.group(t), Some(outer));
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(t), Some(outer));
    assert_eq!(doc.nodes[&t].group, Some(g), "no move was committed");
}

/// Local dissolve: a member mid-respawn moves up to the dissolved group's group.
#[test]
fn a_local_dissolve_during_a_respawn_moves_the_tab_up_a_level() {
    let path = temp_dir("respawn-local-dissolve").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (outer, g, t) = nested_tab(&mut app);
    start_respawn(&mut app, t);
    app.remove_node(g);
    assert_eq!(app.doc.mirror.group(t), Some(outer));
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.group(t), Some(outer));
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(t), Some(outer));
}

/// Local merge: a member of the merged group mid-respawn lands in the target.
#[test]
fn a_local_merge_during_a_respawn_moves_the_tab_into_the_target() {
    let path = temp_dir("respawn-local-merge").join("session.mnf");
    let mut app = app_on(&path, 1);
    let source = add_group(&mut app, "S");
    let target = add_group(&mut app, "T");
    let m = add(&mut app, Kind::Tab, sleeping_tab("https://m/", ""));
    app.edit_group(m, Some(source));
    assert!(app.save_session());
    start_respawn(&mut app, m);
    app.merge_groups(source, target);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.group(m), Some(target));
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(m), Some(target));
}

/// Parent: removing P locally while its child C respawns passes C up to P's parent, and commits it.
#[test]
fn a_local_removal_during_a_respawn_reparents_the_tab() {
    let path = temp_dir("respawn-local-parent").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, p, c) = parent_chain(&mut app);
    start_respawn(&mut app, c);
    app.remove_tab(p);
    assert_eq!(app.doc.mirror.parent(c), Some(a));
    assert!(app.save_session());
    assert_eq!(stored(&app, c, field::PARENT), FieldValue::Id(Some(a)));
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.parent(c), Some(a));
    assert_eq!(stored(&app, c, field::PARENT), FieldValue::Id(Some(a)));
}

/// Parent: another replica deleting P while C respawns here passes C up in the mirror only.
#[test]
fn a_remote_removal_during_a_respawn_reparents_the_tab_in_the_mirror_only() {
    let path = temp_dir("respawn-remote-parent").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, p, c) = parent_chain(&mut app);
    start_respawn(&mut app, c);
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: p }]).unwrap();
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.parent(c), Some(a), "the cascade reached the respawning tab");
    assert!(!app.doc.writes.has_fields(), "a remote cascade queues nothing");
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.parent(c), Some(a));
}

/// The landing safety net (`seed_landing`): a new spawn has no mirror entry for the cascades to reach, so a group
/// dissolved while it was in flight is caught at landing. It lands where the document will show it once the
/// dissolve is saved (G's group), in the mirror only: no move is committed.
#[test]
fn a_new_spawn_into_a_group_dissolved_in_flight_lands_where_the_document_shows_it() {
    let path = temp_dir("spawn-dissolved").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "O");
    let g = add_group(&mut app, "G");
    app.edit_group(g, Some(outer));
    assert!(app.save_session());
    let t = app.alloc_node_id();
    let mut spawn = crate::tabs::TabSpawn::new(
        t,
        "https://t/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    spawn.group = Some(g);
    app.spawn_tab_async(spawn);
    assert!(app.save_session(), "creates the spawning tab in G");
    app.remove_node(g); // not saved before the landing
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.group(t), Some(outer));
    assert!(!app.doc.writes.has_moves());
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(t), Some(outer));
    assert_eq!(doc.nodes[&t].group, Some(g), "no move was committed");
}

/// A tab whose spawn hasn't landed is created with the url it starts at, and lands with it in the mirror.
#[test]
fn a_spawning_tab_is_created_and_lands_with_its_url() {
    let path = temp_dir("url-spawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = app.alloc_node_id();
    let spawn = crate::tabs::TabSpawn::new(
        t,
        "https://t/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    app.spawn_tab_async(spawn);
    assert!(app.save_session());
    assert_eq!(stored(&app, t, field::URL), FieldValue::Str("https://t/".into()));
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.url(t), "https://t/", "the failed card lands with it");
    assert!(app.save_session());
    assert_eq!(stored(&app, t, field::URL), FieldValue::Str("https://t/".into()));
}

/// A new tab is created with the jar it starts with and lands with it in the mirror. An edit to its group meanwhile
/// does not reach it (it has no mirror entry until it lands), so it never gives it a partial entry that would stop
/// the landing from seeding its colour; the tab keeps the jar it started with.
#[test]
fn a_spawning_tab_is_created_and_lands_with_its_jar() {
    let path = temp_dir("jar-spawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (work, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let g = add_group(&mut app, "G");
    app.set_jar_edge(g, Some(work));
    assert!(app.save_session());
    // A restored tab, started with its own edge to Work.
    let t = app.alloc_node_id();
    let mut spawn = crate::tabs::TabSpawn::new(
        t,
        "https://t/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    (spawn.group, spawn.jar) = (Some(g), Some(work));
    app.spawn_tab_async(spawn);
    assert!(app.save_session());
    assert_eq!(stored(&app, t, field::JAR), FieldValue::Id(Some(work)), "created with its jar");
    assert!(!app.linked_ids().any(|id| id == t) && app.doc.mirror.jar(t).is_none(), "out of the edits' reach");
    app.set_jar_edge(g, Some(home));
    assert!(!app.doc.mirror.has(t), "no partial entry");
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.jar(t), Some(work), "lands with it");
    assert!(app.save_session());
    assert_eq!(stored(&app, t, field::JAR), FieldValue::Id(Some(work)));
}

#[test]
fn only_jar_view_kinds_are_asked_for_cookies() {
    let mut app = app_on(&temp_dir("jar-engines-pull").join("session.mnf"), 1);
    for kind in ViewKind::ALL {
        let (id, mut peer) = live_tab(&mut app, kind, "https://a/");
        app.pull_cookies(id);
        let asked = sent(&mut peer).iter().any(|m| matches!(m, HostToView::GetCookies));
        assert_eq!(asked, JAR_VIEW_KINDS.contains(&kind), "{kind:?}");
    }
}

#[test]
fn only_jar_view_kinds_switch_when_their_jar_is_cleared() {
    let mut app = app_on(&temp_dir("jar-engines-switch").join("session.mnf"), 1);
    for kind in ViewKind::ALL {
        let (id, _peer) = live_tab(&mut app, kind, "https://a/");
        // Running on the default jar, which the user cleared: a jar engine must respawn into it.
        let default = default_jar_id(&mut app);
        app.cookies.jar_tabs.insert(id, (default, crate::jar::TabCookies::default()));
        app.cookies.jar_reseed.insert(id);
        assert_eq!(app.will_switch(id), JAR_VIEW_KINDS.contains(&kind), "{kind:?}");
    }
}

/// Quitting starts every engine's shutdown before the app is dropped: each fake host is sent `Shutdown` while the
/// app is still alive.
#[test]
fn quitting_sends_every_view_shutdown_before_the_drop() {
    let mut app = app_on(&temp_dir("quit-shutdown").join("session.mnf"), 1);
    let (_, mut a) = live_tab(&mut app, ViewKind::Servo, "https://a/");
    let (_, mut b) = live_tab(&mut app, ViewKind::Servo, "https://b/");
    app.begin_view_shutdown();
    for peer in [&mut a, &mut b] {
        assert!(matches!(sent(peer).last(), Some(HostToView::Shutdown)));
    }
    drop(app);
}

/// Closing several tabs at once starts all their shutdowns first (`begin_tabs_shutdown`), so their flush waits share
/// one deadline: the tabs under a group, nested groups included, and no others.
#[test]
fn the_tabs_under_a_group_all_get_shutdown_together() {
    let mut app = app_on(&temp_dir("group-shutdown").join("session.mnf"), 1);
    let outer = add_group(&mut app, "outer");
    let inner = add_group(&mut app, "inner");
    app.edit_group(inner, Some(outer));
    let (a, mut pa) = live_tab(&mut app, ViewKind::Servo, "https://a/");
    let (b, mut pb) = live_tab(&mut app, ViewKind::Servo, "https://b/");
    let (_, mut pc) = live_tab(&mut app, ViewKind::Servo, "https://c/");
    app.edit_group(a, Some(outer));
    app.edit_group(b, Some(inner));
    let mut under: Vec<NodeId> = app.members_under(outer).into_iter().filter(|m| !app.is_container(*m)).collect();
    under.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(under, want);
    app.begin_tabs_shutdown(&under);
    for peer in [&mut pa, &mut pb] {
        assert!(matches!(sent(peer).last(), Some(HostToView::Shutdown)));
    }
    assert!(!sent(&mut pc).iter().any(|m| matches!(m, HostToView::Shutdown)), "a tab outside is left running");
    drop(app);
}

/// Waking a tab into another engine saves that engine when the wake starts, not when it lands. A wake
/// ignored because one is in flight changes nothing.
#[test]
fn a_wake_into_another_view_is_saved_at_wake_start() {
    let path = temp_dir("engine-wake").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    app.wake_tab(tab, ViewKind::Cef);
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef, "shown at once");
    assert_eq!(app.test_spawns[0].0.kind, ViewKind::Cef);
    app.wake_tab(tab, ViewKind::Webview2);
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef, "an ignored wake changes nothing");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Cef), "saved while the wake is in flight");
    fail_spawns(&mut app);
    assert!(matches!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Failed(_)));
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef, "the failed card is on the engine that failed");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
}

/// Spawn failures that share a cause reach the toast queue as one message once the window has passed.
#[test]
fn spawn_failures_of_one_cause_toast_once_with_a_count() {
    let mut app = app_on(&temp_dir("failure-toasts").join("session.mnf"), 1);
    for i in 0..3 {
        app.spawn_tab_at(&format!("https://t{i}/"), egui::Vec2::ZERO, None, None);
    }
    fail_spawns(&mut app);
    assert!(app.toasts.save_toast_queue.is_empty(), "held inside the window");
    for message in app.toasts.failure_notices.flush(std::time::Instant::now() + crate::failure_toasts::WINDOW) {
        app.queue_toast(message);
    }
    assert_eq!(app.toasts.save_toast_queue.iter().collect::<Vec<_>>(), ["3 tabs couldn't start: no engine"]);
}

/// A host that cannot start is a spawn failure like any other: two such crashes with one reason read as one message.
#[test]
fn cannot_start_crashes_with_one_reason_toast_once_with_a_count() {
    let mut app = app_on(&temp_dir("crash-toasts").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.leave_failed_card(a, "no engine", LeftClosed::CannotStart);
    app.leave_failed_card(b, "no engine", LeftClosed::CannotStart);
    assert!(app.toasts.save_toast_queue.is_empty(), "held inside the window");
    assert_eq!(flushed_failure_toasts(&mut app), ["2 tabs couldn't start: no engine"]);
}

/// A failed card's retry is `Waking`, so a Root/New failure of the same intent meanwhile lands as a second card;
/// when the wake fails too, one card stays: the retried one, with the refreshed error.
#[test]
fn a_failed_retry_and_a_new_failure_of_one_intent_leave_one_card() {
    let mut app = app_on(&temp_dir("failed-retry-twin").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    let card = failed_cards(&app).remove(0);
    let kind = app.doc.mirror.view_kind(card);
    app.wake_tab(card, kind);
    assert_eq!(app.nodes[&card].as_unloaded().unwrap().state, UnloadedState::Waking);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    let mut parked = std::mem::take(&mut app.test_spawns);
    assert_eq!(parked.len(), 2);
    let wake = parked.remove(parked.iter().position(|(s, _)| s.id == card).unwrap());
    fail_spawn(&mut app, parked.remove(0));
    fail_spawn(&mut app, wake);
    assert_eq!(failed_cards(&app), std::slice::from_ref(&card), "one card for one intent, the retried one");
    assert_eq!(app.nodes.len(), 1);
}

/// A live tab that crash-loops into a failed card beside a failed card for its own intent leaves one card: the
/// crashed tab's, which keeps its id; the other is deleted.
#[test]
fn a_crashed_tab_beside_a_failed_card_for_its_intent_leaves_one_card() {
    let mut app = app_on(&temp_dir("crash-twin").join("session.mnf"), 1);
    let live = succeed_spawn(&mut app, "https://a/");
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1, "setup: a failed card beside the live tab");
    app.leave_failed_card(live, "keeps crashing", LeftClosed::KeepsCrashing);
    assert_eq!(failed_cards(&app), std::slice::from_ref(&live));
    assert!(
        app.doc.journal.as_ref().unwrap().doc().closed_roots().is_empty(),
        "the other card is deleted, not recorded"
    );
}

/// A tab whose engine stops reading is toasted once per stall, not once per drain.
#[test]
fn a_tab_that_stops_responding_is_toasted_once() {
    let mut app = app_on(&temp_dir("not-responding").join("session.mnf"), 1);
    let (id, _peer) = live_tab(&mut app, ViewKind::Servo, "https://a/");
    app.nodes.get(&id).and_then(|n| n.as_tab()).unwrap().tab.wedge();
    let toasts = |app: &App| app.toasts.save_toast_queue.iter().filter(|t| t.starts_with("Not responding: ")).count();
    app.drain_tab_events();
    assert_eq!(toasts(&app), 1);
    app.drain_tab_events();
    assert_eq!(toasts(&app), 1, "once per stall");
}

/// "Restart View" starts a new host for the tab under the same id, and says so.
#[test]
fn restart_view_respawns_the_tab_under_its_id() {
    let mut app = app_on(&temp_dir("restart-engine").join("session.mnf"), 1);
    let live = succeed_spawn(&mut app, "https://a/");
    assert!(app.restart_view(live));
    assert!(app.test_spawns.iter().any(|(s, _)| s.id == live), "a respawn under the same id");
    assert!(app.toasts.save_toast_queue.iter().any(|t| t == "Restarted: https://a/"));
    assert!(!app.restart_view(node(99)));
}

/// "End View" leaves a failed card for the tab, and says so.
#[test]
fn end_view_leaves_a_failed_card() {
    let mut app = app_on(&temp_dir("end-engine").join("session.mnf"), 1);
    let live = succeed_spawn(&mut app, "https://a/");
    assert!(app.end_view(live));
    assert_eq!(failed_cards(&app), std::slice::from_ref(&live));
    assert!(app.toasts.save_toast_queue.iter().any(|t| t == "Ended (not responding): https://a/"));
    assert!(!app.end_view(live), "a failed card is no live tab");
}

/// A startup Root spawn that failed leaves a saved failed card; the next launch restores it as the focused tab and
/// tries it again, so a failed startup is retried without a Root spawn (a non-empty session never makes one).
#[test]
fn a_failed_startup_card_is_retried_at_the_next_launch() {
    let path = temp_dir("failed-startup-retry").join("session.mnf");
    let mut app = app_on(&path, 1);
    app.spawn_root_tab();
    fail_spawns(&mut app);
    let card = failed_cards(&app).remove(0);
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.focused, Some(card));
    assert!(restored.test_spawns.iter().any(|(spawn, _)| spawn.id == card), "the card is woken");
}
