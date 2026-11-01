//! Session-sync tests: the local-state file and per-device state.

use super::*;
use crate::local_state::LevelView;

fn world_rect(x0: f32, y0: f32, x1: f32, y1: f32) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
}

#[test]
fn hidden_personal_state_survives_a_launch_and_a_delete_drops_it() {
    let path = temp_dir("sync-hidden-local").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://t/", 1.5));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, tab, true);
    assert!(app.save_session());
    app.maybe_write_local(true);
    drop(app);
    let mut back = app_on(&path, 1);
    back.restore_session();
    assert_eq!(back.doc.hidden_local.get(&tab).and_then(|l| l.zoom), Some(1.5), "read back for the hidden node");
    other.begin().unwrap().commit(vec![OpBody::Delete { node: tab }]).unwrap();
    assert!(back.save_session());
    assert!(!back.doc.hidden_local.contains_key(&tab), "the Delete drops it (forget_deleted)");
}

/// Restore places a node at its document pos, whatever a settle showed before the restart (a settle derives it again
/// from the document), and the next save writes nothing for it.
#[test]
fn a_restart_places_a_node_at_its_document_position() {
    let path = temp_dir("pos-restore").join("session.mnf");
    let note = {
        let mut app = app_on(&path, 1);
        let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
        assert!(app.save_session());
        app.snap_layout(note, egui::vec2(70.0, 80.0)); // a settle
        app.close_store();
        note
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.layout_pos(note), egui::vec2(10.0, 20.0));
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(10.0, 20.0)));
    assert!(app.save_session());
    assert!(!app.doc.writes.has_fields());
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(Some((10.0, 20.0))));
}

/// Item view (`view_item`) restoring onto a tab that's still spawning
/// (a tab's restore is always async) must not bounce back to Graph the
/// first frame -- `build_tab_view_ui` (tab_view.rs) does exactly that
/// for any Item view whose focused id isn't a live tab yet, unless
/// `pending_respawn_focus` names it (the same guard a crash respawn
/// uses). `apply_local` sets it for this case; this only re-checks the
/// state it leaves behind, since the actual bounce-back is UI code this
/// module can't drive without a window.
#[test]
fn item_view_restoring_onto_a_still_spawning_tab_sets_the_respawn_focus_guard() {
    let path = crate::store::testutil::temp_dir("view-item-spawning").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://x/", 1.0));
    // A node whose id isn't live yet -- `spawning`, not `self.nodes` --
    // is exactly the shape a real tab restore has before its spawn
    // lands (`materialize`'s `Kind::Tab` arm never inserts directly).
    app.nodes.remove(&tab);
    app.spawns.spawning.insert(tab, shared(Kind::Tab, None, &[]));
    let state = LocalState { focused: Some(tab), view_item: true, ..LocalState::default() };
    app.apply_local(state);
    assert_eq!(app.view_mode, ViewMode::Item);
    assert_eq!(app.spawns.pending_respawn_focus, Some(tab));
}

/// An unloaded tab's title and mute survive a restart through personal
/// state, though it has no engine to report either.
#[test]
fn personal_state_keeps_an_unloaded_tabs_title_and_mute() {
    let mut app = app_on(&temp_dir("unloaded-local").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", "Saved title"));
    let set = |app: &mut App, title: &str, muted: bool| {
        if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
            (t.title, t.muted) = (title.to_string(), muted);
        }
    };
    set(&mut app, "Saved title", true);
    let entry = app.local_state().nodes[&tab].clone();
    assert_eq!((entry.title.as_str(), entry.muted), ("Saved title", true));
    set(&mut app, "", false);
    App::apply_local_node(app.nodes.get_mut(&tab).unwrap(), &entry);
    let t = app.nodes[&tab].as_unloaded().unwrap();
    assert_eq!((t.title.as_str(), t.muted), ("Saved title", true));
}

/// "Mute Others" mutes a sleeping or waking card too -- it has no
/// engine to send `SetMuted` to, but the landing already sends it
/// (`TabCarry::muted`) and personal state saves it either way, so a
/// woken tab comes back muted like every other tab "Mute Others" hit.
#[test]
fn mute_all_except_also_mutes_sleeping_tabs() {
    let mut app = app_on(&temp_dir("mute-others-sleeping").join("session.mnf"), 1);
    let keep = add(&mut app, Kind::Tab, sleeping_tab("https://keep/", "Keep"));
    let other = add(&mut app, Kind::Tab, sleeping_tab("https://other/", "Other"));
    app.mute_all_except(keep);
    assert!(!app.nodes[&keep].as_unloaded().unwrap().muted, "kept tab untouched");
    assert!(app.nodes[&other].as_unloaded().unwrap().muted);
    let entry = app.local_state().nodes[&other].clone();
    assert!(entry.muted, "personal state saves the sleeping card's mute");
}

/// Final-review fix: personal state startup couldn't read (another window
/// held the lock past `LOCK_WAIT`) is never written over with defaults,
/// even at exit; once a retry reads it, it is applied. Takes `LOCK_WAIT` (5 s) once.
#[test]
fn personal_state_startup_could_not_read_is_retried_not_overwritten() {
    let path = temp_dir("local-unread").join("session.mnf");
    let note = {
        let mut app = app_on(&path, 1);
        let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
        app.close_store();
        note
    };
    let entry = LocalNode { editing: true, ..LocalNode::default() };
    let side = crate::store::local::path_for(&path);
    {
        let journal = Journal::open(&path, ActorId(2)).unwrap();
        let state = LocalState { nodes: BTreeMap::from([(note, entry.clone())]), ..LocalState::default() };
        journal.write_local(&state.to_bytes()).unwrap();
    }
    let saved = std::fs::read(&side).unwrap();

    // A peer takes the session's lock once it's open, and holds it
    // through the personal-state read.
    let mut app = app_on(&path, 3);
    let lock_path = path.with_extension("mnf.lock");
    let peer = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).unwrap();
    peer.try_lock().unwrap();
    assert!(app.restore_session());
    assert!(app.doc.local_persist.local_unread.is_some(), "the read failed");
    // Due writes, a retry while the lock is still busy, and even exit's
    // forced write leave the file alone.
    app.maybe_write_local(true);
    app.doc.local_persist.local_unread.as_mut().unwrap().retry_at = std::time::Instant::now();
    app.poll_store();
    assert!(app.doc.local_persist.local_unread.is_some(), "still busy: retried later");
    assert_eq!(std::fs::read(&side).unwrap(), saved, "not overwritten while unread");

    // The lock is free: the next retry reads and applies it.
    drop(peer);
    app.doc.local_persist.local_unread.as_mut().unwrap().retry_at = std::time::Instant::now();
    app.poll_store();
    assert!(app.doc.local_persist.local_unread.is_none());
    assert_eq!(app.local_state().nodes.get(&note), Some(&entry), "applied");
    app.close_store();
    let bytes = Journal::open(&path, ActorId(4)).unwrap().read_local().unwrap().unwrap();
    let written = LocalState::from_bytes(&bytes).unwrap();
    assert_eq!(written.nodes.get(&note), Some(&entry));
}

/// A damaged `.local` is still a fresh start that the next write replaces
/// (`a_damaged_or_newer_sidecar_is_not_read_and_is_replaced`): nothing in
/// it is readable, and retrying can't change that.
#[test]
fn a_damaged_personal_state_file_is_not_retried() {
    let path = temp_dir("local-damaged-restore").join("session.mnf");
    {
        let mut app = app_on(&path, 1);
        add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
        app.close_store();
    }
    std::fs::write(crate::store::local::path_for(&path), b"MNFL damaged").unwrap();
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert!(app.doc.local_persist.local_unread.is_none());
    // An unchanged state isn't written (`local_written` is armed); a change is.
    app.groups_collapsed = true;
    app.maybe_write_local(true);
    let bytes = app.doc.journal.as_ref().unwrap().read_local().unwrap();
    assert!(bytes.is_some(), "replaced with this profile's state");
}

/// Open Session while startup's personal state is unread: that state is the
/// old session's, so a late read applies none of its view (here Item view, which would otherwise replace Graph).
#[test]
fn opening_a_session_while_unread_applies_nothing_on_retry() {
    let path = temp_dir("local-unread-open").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    mark_unread_with(&mut app, &path, &LocalState { view_item: true, ..LocalState::default() });
    let json =
        JsonDoc { nodes: vec![json_node("t", Kind::Tab, serde_json::json!({ "url": "https://new/" }))], local: None };
    app.replace_running_session(&json, crate::store::json::Trust::Own);
    assert!(app.doc.local_persist.local_unread.is_some_and(|u| u.fresh), "the open makes the retry a fresh start");
    app.poll_store();
    assert!(app.doc.local_persist.local_unread.is_none(), "read");
    assert_eq!(app.view_mode, ViewMode::Graph, "the old session's view isn't applied");
}

/// Exit's forced retry applies what it reads so the write that follows
/// keeps it, but wakes nothing: the app is closing.
#[test]
fn the_forced_retry_at_exit_applies_without_waking() {
    let path = temp_dir("local-unread-exit").join("session.mnf");
    let (a, b) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        assert!(app.save_session());
        (a, b)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.focused, Some(a));
    let woken = app.test_spawns.len();
    mark_unread_with(&mut app, &path, &LocalState { focused: Some(b), ..LocalState::default() });
    app.close_store();
    assert_eq!(app.test_spawns.len(), woken, "nothing woken at exit");
    assert_eq!(app.focused, Some(b), "applied");
    let bytes = Journal::open(&path, ActorId(3)).unwrap().read_local().unwrap().unwrap();
    assert_eq!(LocalState::from_bytes(&bytes).unwrap().focused, Some(b), "and written");
}

/// A late read that finds no personal state (or a damaged one) applies
/// nothing: defaults would undo what the user changed since startup.
#[test]
fn a_late_read_of_no_personal_state_keeps_the_live_view() {
    let path = temp_dir("local-unread-none").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    app.doc.local_persist.local_unread = Some(LocalUnread { retry_at: std::time::Instant::now(), fresh: false });
    app.doc.local_persist.local_written = None;
    app.view_mode = ViewMode::Item;
    app.groups_collapsed = true;
    app.poll_store();
    assert!(app.doc.local_persist.local_unread.is_none(), "read: there is none");
    assert_eq!((app.view_mode, app.groups_collapsed), (ViewMode::Item, true), "not reset to defaults");
    app.maybe_write_local(true);
    let bytes = app.doc.journal.as_ref().unwrap().read_local().unwrap().unwrap();
    assert!(LocalState::from_bytes(&bytes).unwrap().levels[&None].collapsed, "the next write saves the live view");
}

/// Personal state's level views: the level in view takes its camera and Collapse Groups toggle at once, the others
/// wait in `level_views` for a switch, and all are written back, the level in view from the live camera (or where a
/// running ease is taking it).
#[test]
fn personal_state_applies_the_level_in_views_camera_and_keeps_the_others() {
    let mut app = app_on(&temp_dir("level-views-apply").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.groups_collapsed = true;
    let top = LevelView::new(world_rect(-500.0, -400.0, 500.0, 400.0), true);
    let inside = LevelView::new(world_rect(-300.0, -200.0, 300.0, 200.0), false);
    let levels = BTreeMap::from([(None, top), (Some(g), inside)]);
    app.apply_local(LocalState { current_group: Some(g), levels: levels.clone(), ..LocalState::default() });
    assert_eq!(app.current_group, Some(g));
    assert_eq!(Some(app.camera.rect), inside.camera_rect());
    assert!(!app.groups_collapsed, "g's toggle, not the top level's");
    assert_eq!(app.level_views[&None], top, "kept for the switch back");
    assert_eq!(app.local_state().levels, levels, "written back as read");
    let ahead = world_rect(0.0, 0.0, 50.0, 50.0);
    app.camera.ease_to(ahead);
    assert_eq!(app.local_state().levels[&Some(g)].camera_rect(), Some(ahead), "a running ease writes where it goes");
}

/// The written map keeps the level in view and every level the document still holds; an id it no longer holds is
/// pruned (as `hidden_local` is).
#[test]
fn personal_state_drops_the_views_of_levels_the_document_no_longer_holds() {
    let mut app = app_on(&temp_dir("level-views-prune").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    let gone = NodeId { actor: ActorId(7), counter: 1 };
    app.level_views.insert(Some(g), LevelView { camera: None, collapsed: true });
    app.level_views.insert(Some(gone), LevelView { camera: None, collapsed: true });
    let levels = app.local_state().levels;
    assert!(levels.contains_key(&Some(g)), "a shown group keeps its view");
    assert!(!levels.contains_key(&Some(gone)), "an id the document doesn't hold is dropped");
    assert!(levels.contains_key(&None), "the level in view is always written");
}

/// Personal state written before a restored tab's wake lands still
/// keeps theirs (title, zoom), and a fresh failed card's zoom
/// is kept too. A restored tab is an `Unloaded` card at once, so
/// `apply_local` applies its entry directly; the `pending_local` path
/// for a node still spawning (a group import, Recently Closed) isn't
/// covered here.
#[test]
fn personal_state_survives_onto_a_still_waking_restored_card_and_a_fresh_failed_cards_zoom() {
    let path = temp_dir("local-spawning").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, failed_tab("https://s/", 1.0));
        app.close_store();
        tab
    };
    let entry = LocalNode { title: "T".into(), zoom: Some(2.0), ..LocalNode::default() };
    {
        let journal = Journal::open(&path, ActorId(2)).unwrap();
        let state = LocalState { nodes: BTreeMap::from([(tab, entry.clone())]), ..LocalState::default() };
        journal.write_local(&state.to_bytes()).unwrap();
    }
    let mut app = app_on(&path, 3);
    assert!(app.restore_session());
    // Still waking, not landed -- `entry` is applied straight onto the
    // live `Unloaded` card (`apply_local_node`), not carried through
    // `pending_local`.
    assert_eq!(app.test_spawns.len(), 1, "the restored tab's wake is still in flight");
    let card = add(&mut app, Kind::Tab, failed_tab("https://f/", 1.5));
    let state = app.local_state();
    assert_eq!(state.nodes.get(&tab), Some(&entry));
    assert_eq!(state.nodes[&card].zoom, Some(1.5));
}

/// A tab its page moved on without sharing (a redirect, a script's
/// navigation) reopens where it was, unless the shared url has changed
/// since.
#[test]
fn a_restart_reopens_a_tabs_live_url_while_the_shared_url_is_unchanged() {
    let path = temp_dir("live-url").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, failed_tab("https://s/", 1.0));
        assert!(app.save_session());
        if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
            t.url = "https://s/moved".into();
        }
        app.close_store();
        tab
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.test_spawns[0].0.url, "https://s/moved");
    assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://s/".into()), "still unshared");
    // The single tab is focused, so it is the one waking.
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://s/moved");
    app.close_store();

    // Moved on from a url the store no longer holds: the shared one wins.
    {
        let journal = Journal::open(&path, ActorId(3)).unwrap();
        let live_url = Some(LiveUrl { url: "https://s/moved".into(), from: "https://elsewhere/".into() });
        let entry = LocalNode { live_url, ..LocalNode::default() };
        let state = LocalState { nodes: BTreeMap::from([(tab, entry)]), ..LocalState::default() };
        journal.write_local(&state.to_bytes()).unwrap();
    }
    let mut app = app_on(&path, 4);
    assert!(app.restore_session());
    assert_eq!(app.test_spawns[0].0.url, "https://s/");
}

/// A url the page moved to without sharing it is never saved: not by a save, not after a restore reopens the tab at
/// it (`LiveUrl`) and its wake lands.
#[test]
fn an_unmarked_live_url_is_never_saved() {
    let path = temp_dir("url-unmarked").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
        assert!(app.save_session());
        set_live_url(&mut app, tab, "https://s/moved");
        assert!(app.save_session());
        assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://s/".into()));
        assert_eq!(app.doc.mirror.url(tab), "https://s/");
        app.maybe_write_local(true);
        app.close_store();
        tab
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://s/moved", "reopened at its live url");
    assert_eq!(app.doc.mirror.url(tab), "https://s/", "the mirror shows the shared one");
    fail_spawns(&mut app);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://s/".into()));
}

/// Where the camera is easing to after a switch.
fn eased_to(app: &App) -> Option<egui::Rect> {
    app.camera.tween.map(|t| t.to)
}

/// A switch saves the view of the level it leaves and brings back the view of the one it enters: its camera (fit-all
/// the first time) and its own Collapse Groups toggle.
#[test]
fn switching_levels_returns_to_each_levels_view() {
    let mut app = app_on(&temp_dir("level-views-switch").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    let top = app.fit_all_nodes_scene_rect().translate(egui::vec2(25.0, 0.0));
    app.camera.jump_to(top);
    app.groups_collapsed = true;
    app.switch_group_level(Some(g));
    assert_eq!(eased_to(&app), Some(app.fit_all_nodes_scene_rect()), "a level never left before fits all");
    assert!(!app.groups_collapsed, "a level's toggle is its own");
    let inside = app.fit_all_nodes_scene_rect().translate(egui::vec2(0.0, 15.0));
    app.camera.jump_to(inside);
    app.switch_group_level(None);
    assert_eq!(eased_to(&app), Some(top));
    assert!(app.groups_collapsed);
    app.switch_group_level(Some(g));
    assert_eq!(eased_to(&app), Some(inside));
}

#[test]
fn leaving_mid_ease_saves_where_the_ease_was_going() {
    let mut app = app_on(&temp_dir("level-views-mid-ease").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.switch_group_level(Some(g));
    let fit = app.fit_all_nodes_scene_rect();
    assert!(app.camera.tween.is_some(), "still easing: no frame has run");
    app.switch_group_level(None);
    assert_eq!(app.level_views[&Some(g)].camera_rect(), Some(fit));
}

#[test]
fn a_saved_view_that_misses_the_level_falls_back_to_fit_all() {
    let mut app = app_on(&temp_dir("level-views-miss").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    let far = world_rect(1.0e6, 1.0e6, 1.0e6 + 100.0, 1.0e6 + 100.0);
    app.level_views.insert(Some(g), LevelView::new(far, true));
    app.switch_group_level(Some(g));
    assert_eq!(eased_to(&app), Some(app.fit_all_nodes_scene_rect()));
    assert!(app.groups_collapsed, "the toggle still comes back");
}

/// `take_out` removes the group, then switches out of it: the switch must not save a view for the deleted id.
#[test]
fn deleting_the_level_in_view_leaves_no_view_for_it() {
    let mut app = app_on(&temp_dir("level-views-delete").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.switch_group_level(Some(g));
    app.take_out(g, crate::node_store::Removal::Local);
    assert_eq!(app.current_group, None);
    assert!(!app.level_views.contains_key(&Some(g)));
    assert!(app.level_views.contains_key(&None), "the top level's view was saved on the way in");
}

/// Teardown saves nothing, and the replaced session's level views and toggle go with it.
#[test]
fn opening_a_session_drops_the_old_sessions_level_views() {
    let mut app = app_on(&temp_dir("level-views-teardown").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.camera.jump_to(world_rect(7.0, 7.0, 77.0, 77.0));
    app.switch_group_level(Some(g));
    app.groups_collapsed = true;
    assert!(app.level_views.contains_key(&None), "the top level's view was saved on the way in");
    let json =
        JsonDoc { nodes: vec![json_node("t", Kind::Tab, serde_json::json!({ "url": "https://new/" }))], local: None };
    app.replace_running_session(&json, crate::store::json::Trust::Own);
    assert!(app.level_views.is_empty(), "no level of the old session survives, the top level's included");
    assert!(!app.groups_collapsed);
    assert_eq!(
        app.local_state().levels.keys().copied().collect::<Vec<_>>(),
        [None],
        "only the level in view is written"
    );
}

/// `show_level_of` no longer fits the level, so a caller that selects a card must bring it into view itself.
#[test]
fn opening_a_ref_reveals_its_card() {
    let mut app = app_on(&temp_dir("level-views-open-ref").join("session.mnf"), 1);
    let (g, note) = group_with_note(&mut app);
    app.switch_group_level(Some(g));
    app.switch_group_level(None);
    app.open_ref(note);
    assert_eq!(app.current_group, Some(g));
    assert_eq!(app.camera.reveal, Some(note));
}

/// Each level's view goes through `session.mnf.local` and comes back: the level in view at once, the others on the
/// switch back.
#[test]
fn each_levels_view_survives_a_restart() {
    let path = temp_dir("level-views-restart").join("session.mnf");
    let (g, top, inside) = {
        let mut app = app_on(&path, 1);
        let (g, _) = group_with_note(&mut app);
        let top = app.fit_all_nodes_scene_rect().translate(egui::vec2(25.0, 0.0));
        app.camera.jump_to(top);
        app.groups_collapsed = true;
        app.switch_group_level(Some(g));
        let inside = app.fit_all_nodes_scene_rect().translate(egui::vec2(0.0, 15.0));
        app.camera.jump_to(inside);
        app.close_store();
        (g, top, inside)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.current_group, Some(g));
    assert_eq!(app.camera.rect, inside);
    assert!(!app.groups_collapsed, "g's own toggle");
    app.switch_group_level(None);
    assert_eq!(eased_to(&app), Some(top), "the top level's view came back with the file");
    assert!(app.groups_collapsed);
}

/// A camera still waiting for its session's framing (`GraphCamera::frame_restored`) is a placeholder, not a place the
/// user chose: leaving the level saves its toggle but no camera, so coming back fits it.
#[test]
fn leaving_a_level_before_its_framing_settled_saves_no_camera() {
    let mut app = app_on(&temp_dir("level-views-unsettled").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.camera.jump_to(world_rect(7.0, 7.0, 77.0, 77.0));
    app.camera.frame_restored = true;
    app.groups_collapsed = true;
    app.switch_group_level(Some(g));
    assert_eq!(app.level_views[&None], LevelView { camera: None, collapsed: true });
}

/// Closing the level in view (`leave_hidden`, then `hide_node`) keeps no view for it: a reopen fits it.
#[test]
fn closing_the_level_in_view_leaves_no_view_for_it() {
    let mut app = app_on(&temp_dir("level-views-close").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.switch_group_level(Some(g));
    app.leave_hidden(g);
    assert!(app.level_views.contains_key(&Some(g)), "saved on the way out");
    app.hide_node(g);
    assert!(!app.level_views.contains_key(&Some(g)));
}
