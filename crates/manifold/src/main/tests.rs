use super::*;

const SID: &str = r#"[{"name":"sid","value":"1","domain":"a.com","path":"/","expires":null,"secure":false,"http_only":true,"same_site":"Lax"}]"#;

/// C1: everything `forget_id` owns goes for the forgotten id, and only for it.
#[test]
fn forget_id_clears_every_id_keyed_table() {
    let mut app = App::default();
    let now = std::time::Instant::now();
    let t = || "t".to_string();
    app.skip_convert.insert(t(), true);
    app.pending_sleeps.insert(t(), sleep_state::PendingSleep { why: sleep_state::SleepWhy::Manual, sent: now });
    app.jar_tabs.insert(t(), (jar::JarKey::Default, jar::TabCookies::default()));
    app.jar_reseed.insert(t());
    app.cookie_holds.insert(t(), (now, tabs::Held::Snapshot(EngineKind::Webview2, String::new())));
    app.jar_pulls.insert(t(), now);
    app.jar_silent.insert(t(), 1);
    app.saved_scroll.insert(t(), sleep_state::SavedScroll::new(0.0, 1.0));
    app.saved_scroll.insert("u".into(), sleep_state::SavedScroll::new(0.0, 1.0));
    app.nav_history.insert(t(), nav_history::NavHistory::default());
    app.navigated_from.insert(t(), ("https://a.test/".into(), now, false));
    for _ in 0..popup_gate::POPUP_BURST {
        app.popup_gate.admit("t", true, now);
    }
    app.tab_crashes.insert(t(), vec![now]);
    app.selected_tabs.extend([t(), "u".to_string()]);
    app.naming_jar = Some((t(), String::new()));
    app.renaming_group = Some((t(), String::new()));
    app.node_menu = Some((t(), egui::Pos2::ZERO));
    app.pending_respawn_focus = Some(t());
    app.peek_suppressed = Some(t());
    app.dragging = Some(t());
    app.connecting = Some((t(), crate::caps::LINK));
    app.connect_drag = Some(t());
    app.hovered = Some(t());

    app.forget_id("t");

    assert!(app.skip_convert.is_empty() && app.pending_sleeps.is_empty());
    assert!(app.jar_tabs.is_empty() && app.jar_reseed.is_empty() && app.cookie_holds.is_empty());
    assert!(app.jar_pulls.is_empty() && app.jar_silent.is_empty());
    assert!(!app.saved_scroll.contains_key("t") && app.saved_scroll.contains_key("u"), "only t's entry goes");
    assert!(app.nav_history.is_empty() && app.navigated_from.is_empty() && app.tab_crashes.is_empty());
    assert_eq!(app.popup_gate.admit("t", true, now), popup_gate::Admit::Open, "t's popup burst count went");
    assert_eq!(app.selected_tabs, HashSet::from(["u".to_string()]));
    assert!(app.naming_jar.is_none() && app.renaming_group.is_none() && app.node_menu.is_none());
    assert!(app.pending_respawn_focus.is_none() && app.peek_suppressed.is_none());
    assert!(app.dragging.is_none() && app.connecting.is_none() && app.connect_drag.is_none());
    assert!(app.hovered.is_none());
}

/// C1: App fields that can hold a node id, but that `forget_id` deliberately leaves alone, each with why. A
/// field of an id-holding type that is in neither place fails `every_id_keyed_field_is_forgotten_or_excused`.
const NOT_FORGOTTEN: &[(&str, &str)] = &[
    ("node_order", "removed with `nodes` by each caller (kind-specific order)"),
    ("layout", "pruned to live nodes at every save (`layout.retain`)"),
    ("held_text", "pruned to live nodes at every save (`held_text.retain`)"),
    ("current_group", "kind-specific: `drop_node` and `dissolve_members` switch level"),
    ("focused", "`drop_tab`'s focus, split and view-mode logic"),
    ("split_tab", "`drop_tab`'s focus, split and view-mode logic"),
    ("viewed_image", "`remove_remotely_deleted`; `image_view` resets it when the image is gone"),
    ("jar_tabs", "via `clear_cookie_state`"),
    ("jar_reseed", "via `clear_cookie_state`"),
    ("cookie_holds", "via `clear_cookie_state`"),
    ("jar_pulls", "via `clear_cookie_state`"),
    ("jar_silent", "via `clear_cookie_state`"),
    ("connecting", "via `forget_connect_source`"),
    ("connect_drag", "via `forget_connect_source`"),
    ("wakes_in_flight", "spawn lifecycle: a tab closed while waking needs it so the landing commits its delete"),
    ("spawning", "spawn lifecycle: the landing or teardown removes it"),
    ("remote_in_flight", "spawn lifecycle: replayed after the landing"),
    ("host_loads", "spawn lifecycle: expired by `shares_url_change`, cleared at teardown"),
    ("drag_moved", "ends with the drag; a gone id is skipped at release"),
    ("line_press", "a link id, taken at release only if still hovered"),
    ("native_drag_tab", "taken at the button-up"),
    ("hidden_for_minimize", "drained at restore"),
    ("node_menu_cache", "the closing menu's fade copy; `node_menu` is the live one"),
    ("pending_session_delete", "a session name, not a node id"),
    ("favicon_resolved", "origins, not node ids"),
    ("favicon_origin_cache", "urls, not node ids"),
    ("graph_settled_key", "a level fingerprint, rebuilt when it differs"),
    ("debug_address_bar_typing", "typed text, not a node id"),
    ("pending_screenshot", "a file path, not a node id"),
    ("pending_print_to_pdf", "a file path, not a node id"),
    ("save_toast", "a message, not a node id"),
    ("cef_dir_draft", "a path draft, not a node id"),
    ("home_page_draft", "a url draft, not a node id"),
    ("renaming_bookmark_folder", "a bookmark folder, not a node id"),
    ("history_matches_cache", "a history query, not a node id"),
];

/// C1: a new App table that can hold a node id must be decided: forgotten by `forget_id` (permanent removal)
/// or excused in `NOT_FORGOTTEN` with why. The drift this replaces was five hand-written lists.
#[test]
fn every_id_keyed_field_is_forgotten_or_excused() {
    let source = include_str!("../main.rs");
    let fields: Vec<(&str, &str)> = source
        .lines()
        .skip_while(|l| *l != "struct App {")
        .skip(1)
        .take_while(|l| *l != "}")
        .map(str::trim)
        .filter(|l| !l.starts_with("//") && !l.starts_with('#'))
        .filter_map(|l| l.split_once(": "))
        .collect();
    assert!(fields.len() > 100, "App's fields were found ({})", fields.len());
    let body = source
        .lines()
        .skip_while(|l| !l.contains("fn forget_id("))
        .take_while(|l| *l != "    }")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains("self.skip_convert"), "forget_id's body was found");
    let mentions = |name: &str| {
        let needle = format!("self.{name}");
        body.match_indices(&needle)
            .any(|(at, _)| !body[at + needle.len()..].starts_with(|c: char| c == '_' || c.is_ascii_alphanumeric()))
    };
    const ID_SHAPES: [&str; 5] = ["HashMap<String", "HashSet<String", "Option<String", "Option<(String", "Vec<String"];
    let mut undecided = Vec::new();
    for &(name, ty) in &fields {
        if !ID_SHAPES.iter().any(|shape| ty.contains(shape)) {
            continue;
        }
        let excused = NOT_FORGOTTEN.iter().any(|&(n, _)| n == name);
        if mentions(name) == excused {
            let why = if excused { "both forgotten and excused" } else { "neither forgotten nor excused" };
            undecided.push(format!("{name}: {why}"));
        }
    }
    assert!(undecided.is_empty(), "decide each App field: {undecided:?}");
    for &(name, _) in NOT_FORGOTTEN {
        assert!(fields.iter().any(|&(n, _)| n == name), "{name} is excused but is no longer an App field");
    }
}

#[cfg(windows)]
#[test]
fn explorer_select_quotes_the_path() {
    assert_eq!(explorer_select_arg(r"C:\d\a,b.pdf"), r#"/select,"C:\d\a,b.pdf""#);
}

#[test]
fn window_geometry_is_saved_after_a_quiet_spell_not_per_event() {
    let mut app = App::default();
    let t0 = std::time::Instant::now();
    app.note_settings_changed();
    assert!(!app.flush_settings_if_due(t0), "not at once");
    app.note_settings_changed();
    assert!(app.flush_settings_if_due(t0 + SETTINGS_SAVE_DELAY * 2));
    assert!(!app.flush_settings_if_due(t0 + SETTINGS_SAVE_DELAY * 3), "flushed once");
    app.note_settings_changed();
    app.save_settings();
    assert!(!app.flush_settings_if_due(t0 + SETTINGS_SAVE_DELAY * 4), "an immediate save covers a pending one");
}

fn sid_key() -> jar::Key {
    ("sid".into(), "a.com".into(), "/".into())
}

fn on_default(app: &mut App, tab: &str) {
    app.jar_tabs.insert(tab.into(), (jar::JarKey::Default, jar::TabCookies::default()));
}

fn default_jar(app: &App) -> &jar::Jar {
    &app.jars[&jar::JarKey::Default]
}

#[test]
fn pulled_cookies_reach_the_jar_and_a_logout_removes_them() {
    let mut app = App::default();
    on_default(&mut app, "a");
    assert!(app.apply_pulled_cookies("a", SID));
    assert_eq!(default_jar(&app).cookies.len(), 1);
    assert!(app.apply_pulled_cookies("a", "[]"), "sid was known to a, so its absence is a logout");
    assert!(default_jar(&app).cookies.is_empty());
    assert!(!app.apply_pulled_cookies("a", "not json"), "a garbled pull changes nothing");
}

#[test]
fn a_tab_that_never_saw_a_cookie_cannot_delete_it() {
    let mut app = App::default();
    // Tab b was opened before a logged in, so it was seeded with nothing.
    on_default(&mut app, "a");
    on_default(&mut app, "b");
    assert!(app.apply_pulled_cookies("a", SID));
    assert!(!app.apply_pulled_cookies("b", "[]"), "b never saw sid: its pull must not delete it");
    assert_eq!(default_jar(&app).cookies.len(), 1);
    assert!(app.apply_pulled_cookies("a", "[]"), "a logout seen by a (sid in a's known) does delete it");
    assert!(default_jar(&app).cookies.is_empty());
}

#[test]
fn a_seeded_tab_deletes_a_cookie_only_once_it_has_reported_it() {
    let mut app = App::default();
    on_default(&mut app, "a");
    app.apply_pulled_cookies("a", SID);
    let seeded = jar::TabCookies::seeded(&default_jar(&app).cookies);
    app.jar_tabs.insert("b".into(), (jar::JarKey::Default, seeded));
    assert!(!app.apply_pulled_cookies("b", "[]"), "the seed may have failed: an empty first pull is no logout");
    assert_eq!(default_jar(&app).cookies.len(), 1);
    assert!(!app.apply_pulled_cookies("b", SID), "b now holds sid, unchanged from its seed");
    assert!(app.jar_tabs["b"].1.reported.contains(&sid_key()));
    assert!(app.apply_pulled_cookies("b", "[]"), "b held sid and lost it: a logout");
    assert!(default_jar(&app).cookies.is_empty());
}

#[test]
fn a_pull_merges_into_the_jar_the_tab_was_spawned_with() {
    let mut app = App::default();
    let work = app.create_jar("Work");
    app.ensure_jar_loaded(jar::JarKey::Named(work));
    app.jar_tabs.insert("a".into(), (jar::JarKey::Named(work), jar::TabCookies::default()));
    assert!(app.apply_pulled_cookies("a", SID));
    assert_eq!(app.jars[&jar::JarKey::Named(work)].cookies.len(), 1);
    assert!(app.jars.get(&jar::JarKey::Default).is_none_or(|j| j.cookies.is_empty()), "default untouched");
}

#[test]
fn a_pull_with_no_entry_or_a_deleted_jar_is_dropped() {
    let mut app = App::default();
    assert!(!app.apply_pulled_cookies("nobody", SID), "no jar_tabs entry: dropped (deviation 5)");
    let work = app.create_jar("Work");
    app.jar_tabs.insert("a".into(), (jar::JarKey::Named(work), jar::TabCookies::default()));
    app.forget_jar(work);
    assert!(!app.apply_pulled_cookies("a", SID), "its jar was deleted while it ran: dropped");
    assert!(app.jars.values().all(|j| j.cookies.is_empty()));
}

#[test]
fn resolved_jar_falls_back_to_default_and_flags_a_missing_jar() {
    let mut app = App::default();
    let work = app.create_jar("Work");
    let gone = store::model::NodeId { actor: store::model::ActorId(5), counter: 5 };
    let pick = |jar| jar::JarChoice { jar, set: true };
    assert_eq!(app.resolved_jar(&pick(None)), (jar::JarKey::Default, false));
    assert_eq!(app.resolved_jar(&pick(Some(work))), (jar::JarKey::Named(work), false));
    assert_eq!(app.resolved_jar(&pick(Some(gone))), (jar::JarKey::Default, true));
}

#[test]
fn an_ephemeral_jar_never_writes_a_file() {
    let dir = store::testutil::temp_dir("jar-ephemeral");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let tmp = app.create_jar("Throwaway");
    app.set_jar_ephemeral(tmp, true);
    app.ensure_jar_loaded(jar::JarKey::Named(tmp));
    app.jar_tabs.insert("a".into(), (jar::JarKey::Named(tmp), jar::TabCookies::default()));
    assert!(app.apply_pulled_cookies("a", SID));
    assert!(!jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists());
    app.set_jar_ephemeral(tmp, false);
    assert!(jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists(), "turned off: saved (deviation 10)");
    app.set_jar_ephemeral(tmp, true);
    assert!(!jar::Jar::path(&dir, jar::JarKey::Named(tmp)).exists(), "turned on: file deleted");
    assert_eq!(app.jars[&jar::JarKey::Named(tmp)].cookies.len(), 1, "cookies kept in memory until quit");
}

#[test]
fn deleting_a_jar_deletes_its_file_and_clearing_empties_it() {
    let dir = store::testutil::temp_dir("jar-delete-clear");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let work = app.create_jar("Work");
    let key = jar::JarKey::Named(work);
    app.ensure_jar_loaded(key);
    app.jar_tabs.insert("a".into(), (key, jar::TabCookies::default()));
    app.apply_pulled_cookies("a", SID);
    app.flush_dirty_jars(None); // the round ends
    assert!(jar::Jar::path(&dir, key).exists());
    app.clear_jar_cookies(key);
    assert!(app.jars[&key].cookies.is_empty());
    assert!(jar::Jar::load(&dir, key).cookies.is_empty(), "cleared on disk too");
    app.delete_jar(work);
    assert!(!jar::Jar::path(&dir, key).exists());
    assert!(!app.jars.contains_key(&key));
}

/// A jar `Work` whose file holds one cookie, saved by an earlier run: this run has not loaded it.
fn jar_saved_earlier(name: &str) -> (std::path::PathBuf, App, store::model::NodeId) {
    let dir = store::testutil::temp_dir(name);
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let work = app.create_jar("Work");
    let mut jar = jar::Jar::default();
    jar.cookies = jar::from_json(SID).unwrap();
    jar.save(&dir, jar::JarKey::Named(work)).unwrap();
    (dir, app, work)
}

/// Final review I1: ephemeral on, then off, for a jar not loaded this run must not lose its saved logins: on
/// loads them first (they stay in memory until quit), off saves them back.
#[test]
fn ephemeral_on_then_off_keeps_an_unloaded_jars_cookies() {
    let (dir, mut app, work) = jar_saved_earlier("jar-ephemeral-unloaded");
    let key = jar::JarKey::Named(work);
    app.set_jar_ephemeral(work, true);
    assert!(!jar::Jar::path(&dir, key).exists(), "on: the file goes");
    assert_eq!(app.jars[&key].cookies.len(), 1, "but its cookies were loaded first");
    assert_eq!(app.jar_snapshots.lock().unwrap().seed(key).0.len(), 1, "and seed tabs");
    app.set_jar_ephemeral(work, false);
    assert_eq!(jar::Jar::load(&dir, key).cookies.len(), 1, "off: saved back");
}

/// Final review I1: a locked jar's unreadable file (and its earlier copies) is never touched, ephemeral or not.
#[test]
fn ephemeral_on_leaves_a_locked_jars_files_alone() {
    let dir = store::testutil::temp_dir("jar-ephemeral-locked");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let work = app.create_jar("Work");
    let key = jar::JarKey::Named(work);
    // A directory where the file should be: reading it fails, so the jar loads locked.
    std::fs::create_dir(jar::Jar::path(&dir, key)).unwrap();
    let aside = dir.join(format!("{}.json.unreadable-100", key.file_stem()));
    std::fs::write(&aside, "x").unwrap();
    app.set_jar_ephemeral(work, true);
    assert!(app.mirror.jar_defs()[&work].ephemeral);
    assert!(jar::Jar::path(&dir, key).is_dir() && aside.exists());
    app.set_jar_ephemeral(work, false);
    assert!(jar::Jar::path(&dir, key).is_dir() && aside.exists(), "off saves nothing over it");
}

/// Final review I1: Settings asks before making a jar with saved logins ephemeral; one without, or turning it
/// off, acts at once.
#[test]
fn ephemeral_on_asks_first_only_when_the_jar_has_a_saved_file() {
    let (dir, mut app, work) = jar_saved_earlier("jar-ephemeral-confirm");
    app.request_jar_ephemeral(work, true);
    assert_eq!(app.pending_jar_confirm, Some(jar_defs::JarConfirm::Ephemeral(work)));
    assert!(!app.mirror.jar_defs()[&work].ephemeral, "nothing happens until confirmed");
    assert!(jar::Jar::path(&dir, jar::JarKey::Named(work)).exists());
    let (title, detail, button) = app.jar_confirm_text(jar_defs::JarConfirm::Ephemeral(work)).unwrap();
    assert_eq!((title.as_str(), button), ("Make Work ephemeral?", "Make Ephemeral"));
    assert!(detail.contains("deleted from disk now") && detail.contains("until Manifold quits"), "{detail}");
    app.pending_jar_confirm = None;
    let home = app.create_jar("Home");
    app.request_jar_ephemeral(home, true);
    assert!(app.pending_jar_confirm.is_none() && app.mirror.jar_defs()[&home].ephemeral, "no file: nothing to lose");
    app.request_jar_ephemeral(home, false);
    assert!(!app.mirror.jar_defs()[&home].ephemeral);
    app.set_jar_ephemeral(work, true);
    assert!(app.jar_confirm_text(jar_defs::JarConfirm::Ephemeral(work)).is_none(), "already ephemeral");
}

fn jar_saves(app: &App) -> usize {
    app.jar_saves.load(std::sync::atomic::Ordering::Relaxed)
}

/// Wave 2 (Jar H3): fifteen tabs pull a rotating cookie into one jar. They are saved once, when the round ends.
#[test]
fn a_burst_of_pulls_saves_the_jar() {
    let dir = store::testutil::temp_dir("jar-burst");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let work = app.create_jar("Work");
    let key = jar::JarKey::Named(work);
    app.ensure_jar_loaded(key);
    let before = jar_saves(&app);
    for t in 0..15 {
        let tab = format!("t{t}");
        app.jar_tabs.insert(tab.clone(), (key, jar::TabCookies::default()));
        let dump = SID.replace(r#""value":"1""#, &format!(r#""value":"v{t}""#));
        assert!(app.apply_pulled_cookies(&tab, &dump), "pull {t} changed the jar");
    }
    assert_eq!(jar_saves(&app) - before, 0, "nothing is written until the round ends");
    app.flush_dirty_jars(None);
    assert_eq!(jar_saves(&app) - before, 1, "then the jar is saved once");
    assert_eq!(jar::Jar::load(&dir, key).cookies[0].value, "v14", "the file holds the last value");
}

/// Wave 2 (Jar H3): a dirty jar is saved once no pull is pending, or once it has waited `JAR_FLUSH_AFTER`. A pull
/// that timed out unanswered keeps its entry but holds nothing back.
#[test]
fn a_dirty_jar_is_saved_when_its_pull_round_ends() {
    use std::time::{Duration, Instant};
    let dir = store::testutil::temp_dir("jar-flush-round");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    on_default(&mut app, "a");
    let t0 = Instant::now();
    app.jar_pulls.insert("b".into(), t0); // b's pull is still out
    assert!(app.apply_pulled_cookies("a", SID));
    // Deadlines from the jar's own dirty mark, not from `t0`: a loaded machine can take any time between the two.
    let dirty = *app.jar_dirty.values().next().expect("the pull marked the jar dirty");
    app.flush_dirty_jars(Some(dirty + Duration::from_millis(500)));
    assert_eq!(jar_saves(&app), 0, "b's pull is pending and the jar is not due yet");
    app.flush_dirty_jars(Some(dirty + tabs::JAR_FLUSH_AFTER));
    assert_eq!(jar_saves(&app), 1, "due: saved although b's pull is pending");
    app.flush_dirty_jars(Some(dirty + tabs::JAR_FLUSH_AFTER * 2));
    assert_eq!(jar_saves(&app), 1, "saved jars are clean");

    // The round ends before the jar is due (500 ms after its mark): first with b's pull timed out, then with none.
    assert!(app.apply_pulled_cookies("a", "[]"));
    let dirty = *app.jar_dirty.values().next().expect("the pull marked the jar dirty");
    let timed_out = dirty.checked_sub(tabs::JAR_PULL_TIMEOUT).expect("the clock reaches back 10 s");
    app.jar_pulls.insert("b".into(), timed_out);
    app.flush_dirty_jars(Some(dirty + Duration::from_millis(500)));
    assert!(app.jar_pulls.contains_key("b"), "a timed-out pull keeps its entry");
    assert_eq!(jar_saves(&app), 2, "but it is not pending: the round is over");

    assert!(app.apply_pulled_cookies("a", SID));
    let dirty = *app.jar_dirty.values().next().expect("the pull marked the jar dirty");
    app.jar_pulls.remove("b"); // b answered
    app.flush_dirty_jars(Some(dirty + Duration::from_millis(500)));
    assert_eq!(jar_saves(&app), 3, "no pull is out: the round is over");
}

/// Wave 2 (Jar H3): a jar whose first login is not on disk yet still asks before ephemeral deletes it.
#[test]
fn ephemeral_on_asks_first_for_a_jar_whose_cookies_are_not_saved_yet() {
    let dir = store::testutil::temp_dir("jar-ephemeral-dirty");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let work = app.create_jar("Work");
    let key = jar::JarKey::Named(work);
    app.ensure_jar_loaded(key);
    app.jar_tabs.insert("a".into(), (key, jar::TabCookies::default()));
    assert!(app.apply_pulled_cookies("a", SID));
    app.request_jar_ephemeral(work, true);
    assert_eq!(app.pending_jar_confirm, Some(jar_defs::JarConfirm::Ephemeral(work)));
}

/// Final review I2: an unreadable copy or a crashed save's temp file is a full copy of the jar's credentials, so
/// deleting the jar takes them too. A live process's temp file and other jars' files stay.
#[test]
fn deleting_a_jar_removes_its_unreadable_copies_and_dead_temp_files() {
    let dir = store::testutil::temp_dir("jar-delete-asides");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let work = app.create_jar("Work");
    let home = app.create_jar("Home");
    let (w, h) = (jar::JarKey::Named(work).file_stem(), jar::JarKey::Named(home).file_stem());
    let live = format!("{w}.json.{}.tmp", std::process::id());
    let gone = [format!("{w}.json"), format!("{w}.json.unreadable-100"), format!("{w}.json.4294967295.tmp")];
    let kept = [live, format!("{h}.json"), format!("{h}.json.unreadable-100"), "default.json".to_string()];
    for name in gone.iter().chain(&kept) {
        std::fs::write(dir.join(name), "x").unwrap();
    }
    app.delete_jar(work);
    for name in &gone {
        assert!(!dir.join(name).exists(), "{name} should be deleted");
    }
    for name in &kept {
        assert!(dir.join(name).exists(), "{name} should be kept");
    }
}

#[test]
fn a_sleeping_tab_never_needs_a_switch_and_a_deleted_jar_switches_to_default() {
    let mut app = App::default();
    let work = app.create_jar("Work");
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    // A stale entry from before it slept, on a key other than the one it now resolves to.
    app.jar_tabs.insert(t.clone(), (jar::JarKey::Default, jar::TabCookies::default()));
    app.set_tab_jar(&t, Some(work));
    assert!(!app.will_switch(&t), "asleep: it spawns into the new jar");
    app.jar_reseed.insert(t.clone());
    assert!(!app.will_switch(&t), "asleep: a reseed spawns into the cleared jar anyway");
    assert!(app.pending_sleeps.is_empty(), "the sweeps after each jar change asked nothing of it");
    // A host spawned with Work whose jar is then deleted must head for Default.
    app.jar_tabs.insert(t.clone(), (jar::JarKey::Named(work), jar::TabCookies::default()));
    app.forget_jar(work);
    assert_eq!(app.resolved_jar(&app.tab_jar(&t).unwrap()).0, jar::JarKey::Default);
}

#[test]
fn clearing_a_jar_marks_its_running_tabs_for_a_reseed() {
    let mut app = App::default();
    app.jar_tabs.insert("a".into(), (jar::JarKey::Default, jar::TabCookies::default()));
    let work = app.create_jar("Work");
    app.jar_tabs.insert("b".into(), (jar::JarKey::Named(work), jar::TabCookies::default()));
    app.clear_jar_cookies(jar::JarKey::Default);
    assert!(app.jar_reseed.contains("a"));
    assert!(!app.jar_reseed.contains("b"), "another jar's tab keeps its host");
    app.clear_cookie_state("a");
    assert!(!app.jar_reseed.contains("a"));
}

/// Final review I3: a Clear Cookies landing after a spawning host read its seed, but before the host arrived, is
/// honoured: the host holds the old cookies, so it is marked to switch like the hosts running at the clear, even
/// though the landing first wipes its id's old cookie state.
#[test]
fn a_clear_during_a_spawn_marks_the_host_when_it_lands() {
    let key = jar::JarKey::Default;
    let mut app = App::default();
    on_default(&mut app, "a");
    app.apply_pulled_cookies("a", SID);
    app.clear_cookie_state("a");
    let read_seed = |app: &App| {
        let (cookies, clears) = app.jar_snapshots.lock().unwrap().seed(key);
        (jar::TabCookies::seeded(&cookies), Some(clears))
    };
    let (old_seed, before) = read_seed(&app);
    assert_eq!(old_seed.baseline.len(), 1);
    app.clear_jar_cookies(key);
    app.note_spawned_jar("t", key, old_seed.clone(), before);
    assert!(app.jar_reseed.contains("t"), "seeded before the clear: it holds the old cookies");
    assert!(!app.apply_pulled_cookies("t", SID), "so its pulls cannot refill the cleared jar");
    assert!(default_jar(&app).cookies.is_empty());

    let (empty_seed, after) = read_seed(&app);
    app.jar_reseed.insert("u".into()); // an earlier host's mark: the landing wipes it
    app.note_spawned_jar("u", key, empty_seed.clone(), after);
    assert!(!app.jar_reseed.contains("u"), "seeded after the clear: the empty jar");
    app.clear_jar_cookies(key);
    app.note_spawned_jar("v", key, empty_seed, after);
    assert!(!app.jar_reseed.contains("v"), "an empty seed holds nothing the clear took");
    app.note_spawned_jar("w", key, old_seed.clone(), None);
    assert!(!app.jar_reseed.contains("w"), "never seeded (its engine ignores jars)");
    let work = app.create_jar("Work");
    app.clear_jar_cookies(jar::JarKey::Named(work));
    app.note_spawned_jar("x", key, old_seed, after.map(|n| n + 1));
    assert!(!app.jar_reseed.contains("x"), "only its own jar's clears count");
}

/// The pull `request_sleep` (or a timed pull, or one already in flight) sends a reseed tab still returns the
/// cookies its engine was seeded with; they must not refill the jar the user just cleared.
#[test]
fn a_cleared_jar_is_not_refilled_by_its_old_tabs_pulls() {
    let mut app = App::default();
    on_default(&mut app, "a");
    app.clear_jar_cookies(jar::JarKey::Default);
    assert!(!app.apply_pulled_cookies("a", SID), "a host still holding the old cookies: dropped");
    assert!(default_jar(&app).cookies.is_empty());
    app.clear_cookie_state("a"); // its replacement host landed
    on_default(&mut app, "a");
    assert!(app.apply_pulled_cookies("a", SID), "the new host's pulls count again");
}

#[test]
fn a_pending_pull_blocks_another_until_it_times_out() {
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let pulls: HashMap<String, Instant> = [("t".to_string(), now)].into();
    assert!(tabs::pull_pending(&pulls, "t", now + Duration::from_secs(1)));
    assert!(!tabs::pull_pending(&pulls, "t", now + Duration::from_secs(11)), "a silent host is asked again");
    assert!(!tabs::pull_pending(&pulls, "other", now));
}

fn sleep_answer() -> tabs::Held {
    tabs::Held::PageState(ipc_protocol::PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false })
}

fn swap_answer() -> tabs::Held {
    tabs::Held::Snapshot(process_manager::EngineKind::Cef, "http://x/".into())
}

#[test]
fn a_second_hold_cancels_the_first_so_the_latest_request_wins() {
    let mut app = App::default();
    app.jar_pulls.insert("t".into(), std::time::Instant::now());
    // Sleep, then swap: only the swap stays parked, and the sleep's pending entry is gone (no timeout sleep).
    app.pending_sleeps.insert(
        "t".into(),
        sleep_state::PendingSleep { why: sleep_state::SleepWhy::Manual, sent: std::time::Instant::now() },
    );
    assert!(app.hold_for_cookies("t", sleep_answer()).is_none());
    assert!(app.hold_for_cookies("t", swap_answer()).is_none());
    assert!(matches!(app.cookie_holds["t"].1, tabs::Held::Snapshot(..)));
    assert!(app.pending_sleeps.is_empty(), "the cancelled sleep left nothing behind");
    // Swap, then sleep: only the sleep stays parked.
    assert!(app.hold_for_cookies("t", sleep_answer()).is_none());
    assert!(matches!(app.cookie_holds["t"].1, tabs::Held::PageState(..)));
    assert_eq!(app.cookie_holds.len(), 1);
}

#[test]
fn a_respawn_drops_a_held_answer_meant_for_the_old_host() {
    let mut app = App::default();
    app.jar_pulls.insert("t".into(), std::time::Instant::now());
    app.pending_sleeps.insert(
        "t".into(),
        sleep_state::PendingSleep { why: sleep_state::SleepWhy::Manual, sent: std::time::Instant::now() },
    );
    assert!(app.hold_for_cookies("t", sleep_answer()).is_none());
    app.clear_cookie_state("t"); // what a spawn result for the same id does first
    assert!(app.cookie_holds.is_empty() && app.jar_pulls.is_empty() && app.pending_sleeps.is_empty());
    app.release_expired_cookie_holds();
    app.release_cookie_hold("t"); // nothing left to run against the new host
}

#[test]
fn closing_a_tab_clears_its_cookie_bookkeeping() {
    use crate::session_sync::tests::{add, sleeping_tab};
    use crate::store::model::Kind;
    let mut app = App::default();
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "A"));
    app.jar_tabs.insert(tab.clone(), (jar::JarKey::Default, jar::TabCookies::default()));
    app.jar_pulls.insert(tab.clone(), std::time::Instant::now());
    app.jar_silent.insert(tab.clone(), 2);
    app.cookie_holds.insert(
        tab.clone(),
        (std::time::Instant::now(), tabs::Held::Snapshot(process_manager::EngineKind::Cef, String::new())),
    );
    app.remove_tab(&tab);
    assert!(
        !app.jar_tabs.contains_key(&tab)
            && !app.jar_pulls.contains_key(&tab)
            && !app.jar_silent.contains_key(&tab)
            && !app.cookie_holds.contains_key(&tab)
    );
}

#[test]
fn a_host_that_never_answers_is_given_up_on_after_three_pulls() {
    use std::time::{Duration, Instant};
    let (mut pulls, mut silent) = (HashMap::new(), HashMap::new());
    let t0 = Instant::now();
    assert!(tabs::start_pull(&mut pulls, &mut silent, "t", t0));
    assert!(!tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(1)), "pending");
    assert!(tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(30)), "second ask");
    assert!(tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(60)), "third ask");
    assert!(!tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(90)), "three silences: stop");
    assert!(!pulls.contains_key("t"), "so no hold waits on it");
    assert!(!tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(300)));
    silent.clear(); // a reply (or a respawn) resets it
    assert!(tabs::start_pull(&mut pulls, &mut silent, "t", t0 + Duration::from_secs(400)));
}

#[test]
fn a_sleep_answer_is_held_for_a_pending_pull_and_released_by_it() {
    let mut app = App::default();
    let state = || ipc_protocol::PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false };
    assert!(app.hold_for_cookies("t", tabs::Held::PageState(state())).is_some(), "no pull pending: act now");
    app.jar_pulls.insert("t".into(), std::time::Instant::now());
    assert!(app.hold_for_cookies("t", tabs::Held::PageState(state())).is_none());
    assert!(app.cookie_holds.contains_key("t"));
    app.release_cookie_hold("t");
    assert!(app.cookie_holds.is_empty());
}

#[test]
fn remove_from_group_moves_up_one_level() {
    let mut app = App::default();
    use crate::store::model::Kind;
    let group = || (Kind::Group, Runtime::None);
    let note = || (Kind::Note, Runtime::Note(NotePayload { text: String::new(), editing: false }));
    for (id, group, (kind, payload)) in [
        ("outer", None, group()),
        ("inner", Some("outer"), group()),
        ("deep", Some("inner"), note()),
        ("shallow", Some("outer"), note()),
    ] {
        app.test_node(id, kind, payload);
        app.edit_group(id, group.map(String::from));
    }

    app.remove_from_group("deep");
    assert_eq!(app.mirror.group("deep"), Some("outer"));
    app.remove_from_group("shallow");
    assert_eq!(app.mirror.group("shallow"), None);
    app.remove_from_group("shallow");
    assert_eq!(app.mirror.group("shallow"), None);
}

/// Groups `a` and `b` inside each other in the mirror, plus group `s` outside them: the cycle two replicas'
/// racing moves can leave there until the next save aligns it with the document.
fn mirror_group_cycle() -> App {
    let mut app = App::default();
    for id in ["a", "b", "s"] {
        app.test_node(id, crate::store::model::Kind::Group, Runtime::None);
    }
    app.edit_group("a", Some("b".into()));
    app.edit_group("b", Some("a".into()));
    app
}

#[test]
fn a_merge_into_a_group_cycle_is_refused() {
    let mut app = mirror_group_cycle();
    app.merge_groups("s", "a");
    assert!(app.nodes.contains_key("s"), "the merge is refused, so the source stays");
}

#[test]
fn destroying_a_group_in_a_cycle_ends() {
    let mut app = mirror_group_cycle();
    app.destroy_group("a");
    assert!(!app.nodes.contains_key("a") && !app.nodes.contains_key("b"), "both go");
    assert!(app.nodes.contains_key("s"));
}

/// Two concurrent re-parents can make a parent cycle: the store doesn't cycle-check `parent`.
#[test]
fn a_parent_cycle_counts_as_a_cycle() {
    let mut app = App::default();
    app.edit_parent("x", Some("y".into()));
    app.edit_parent("y", Some("x".into()));
    assert!(app.would_create_cycle("z", "x"));
}

#[test]
fn node_ids_without_a_store_are_unique_store_ids() {
    let mut app = App::default();
    let a = app.alloc_node_id();
    let b = app.alloc_node_id();
    assert_ne!(a, b);
    let a: crate::store::model::NodeId = a.parse().expect("a store node id");
    let b: crate::store::model::NodeId = b.parse().unwrap();
    assert_eq!((a.actor, a.counter + 1), (b.actor, b.counter));
}

#[test]
fn a_new_group_takes_the_smallest_free_number() {
    let mut app = App::default();
    let add = |app: &mut App, name: &str| {
        app.place_group(None, name, Placement { group: None, pos: egui::Vec2::ZERO });
    };
    assert_eq!(app.default_group_name(), "Group 1");
    add(&mut app, "Group 1");
    add(&mut app, "Group 3");
    assert_eq!(app.default_group_name(), "Group 2", "a dissolved group's number is reused, never duplicated");
    add(&mut app, "Group 2");
    assert_eq!(app.default_group_name(), "Group 4");
}

/// Save As (Settings or headless `SAVESESSIONAS`) with nothing left of the name does nothing; before, the headless
/// path skipped `sanitize_session_name`.
#[test]
fn save_as_with_nothing_left_of_the_name_does_nothing() {
    let mut app = App::default();
    app.save_named_session("\u{200B} ..");
    assert!(app.save_toast_queue.is_empty(), "{:?}", app.save_toast_queue);
    app.save_named_session("Trip");
    assert_eq!(app.save_toast_queue.len(), 1, "under test there is no session file");
}

/// A toast shows a long URL's start and an ellipsis, cut on a character boundary.
#[test]
fn a_toast_url_is_cut_to_eighty_characters() {
    assert_eq!(App::toast_url("https://a/"), "https://a/");
    let long = format!("data:text/plain,{}", "é".repeat(500));
    let shown = App::toast_url(&long);
    assert_eq!(shown.chars().count(), 81);
    assert!(shown.starts_with("data:text/plain,é") && shown.ends_with("é\u{2026}"), "{shown}");
    assert_eq!(App::toast_url(&"x".repeat(80)), "x".repeat(80));
}

/// Save As while startup's personal state is unread carries none into the copy; once read, it carries this
/// profile's.
#[test]
fn save_as_while_personal_state_is_unread_carries_none() {
    let mut app = App::default();
    assert!(app.local_state_for_copy().is_some());
    app.local_unread = Some(session_sync::LocalUnread { retry_at: std::time::Instant::now(), fresh: false });
    assert_eq!(app.local_state_for_copy(), None);
}

/// Review fix: a Windows device name survives `sanitize_session_name` but can't be a file; Save As says so
/// instead of "No session file".
#[test]
fn save_as_a_device_name_says_why() {
    let mut app = App::default();
    app.save_named_session("Con");
    assert_eq!(Vec::from(app.save_toast_queue.clone()), ["Not a session name: Con"]);
}
