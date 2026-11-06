//! Session-sync tests: timing of save and teardown.

use super::*;

/// A note can reach 1 MB, so the per-frame reader borrows: the mirror's own string when idle, the box's
/// buffer while typed in; neither is a copy.
#[test]
fn a_large_note_edit_does_not_clone_per_frame() {
    let mut app = App::default();
    let text = "x".repeat(1 << 20);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text, editing: false }));
    let idle: &str = app.note_text(note);
    assert_eq!(idle.as_ptr(), app.doc.mirror.text(note).as_ptr(), "idle: the mirror's string");
    assert_eq!(app.doc.mirror.text(note).as_ptr(), app.doc.mirror.text(note).as_ptr(), "`Mirror::text` borrows");
    app.nodes.get_mut(&note).unwrap().as_note_mut().unwrap().editing = true;
    let editing: &str = app.note_text(note);
    assert_eq!(editing.as_ptr(), app.nodes[&note].as_note().unwrap().text.as_ptr(), "editing: the buffer");
}

/// What the app's side of a save costs at realistic session sizes: the
/// snapshot of every node (`snapshots`, more than a save takes: it projects only the queued creates), a whole
/// `save_session` (projection, `begin`, the queued ops, commit) with nothing and with
/// one url to save, and the personal-state build and write. The store
/// side alone is `journal::tests::save_path_costs`. Timing, not
/// correctness, so ignored:
/// `cargo test --release -j 12 -p manifold -- --ignored --nocapture app_save_costs`
#[test]
#[ignore]
fn app_save_costs() {
    use crate::store::testutil::{report, timed};
    const N: usize = 40;
    for tabs in [300usize, 2000, 3000] {
        let dir = temp_dir(&format!("app-cost-{tabs}"));
        let mut app = app_on(&dir.join("session.mnf"), 1);
        let groups: Vec<NodeId> = (0..tabs / 10).map(|i| add_group(&mut app, &format!("Group {i}"))).collect();
        let mut ids = Vec::new();
        for i in 0..tabs {
            let url = format!("https://www.example{}.com/articles/2026/09/some-long-article-slug-{i}?ref=home", i % 50);
            let id = add(&mut app, Kind::Tab, failed_tab(&url, 1.0));
            app.edit_group(id, Some(groups[i % groups.len()]));
            let pos = egui::vec2(i as f32 * 3.5, i as f32 * -2.25);
            app.snap_layout(id, pos);
            app.edit_pos(id, Some(pos));
            ids.push(id);
        }
        assert!(app.save_session());
        println!("{tabs} tabs, {} groups:", groups.len());

        report("snapshot of every node", timed(N, || drop(snapshots(&app))));
        report("save_session, nothing changed", timed(N, || assert!(app.save_session())));
        let mut k = 0;
        report(
            "save_session, one url changed",
            timed(N, || {
                let id = ids[k % tabs];
                let url = format!("https://moved.example/{k}");
                if let Runtime::Unloaded(f) = &mut app.nodes.get_mut(&id).unwrap().runtime {
                    f.url = url.clone();
                }
                app.edit_url(id, &url);
                k += 1;
                assert!(app.save_session());
            }),
        );
        report("local_state (every 30 s, and at exit)", timed(N, || drop(app.local_state())));
        report(
            "maybe_write_local, changed (build + write)",
            timed(N, || {
                app.layout.get_mut(&ids[k % tabs]).unwrap().x += 1.0;
                k += 1;
                app.maybe_write_local(true);
            }),
        );
        let bytes = app.local_state().to_bytes().len();
        println!("  session.mnf.local: {bytes} B");
    }
}

/// The saves that reach the store (`Journal::begin`) per action through `apply_graph_action`, which
/// saves after the action's own save. With nothing left to write, the second one is redundant.
#[test]
fn save_count_per_action() {
    use crate::card::DropEffect;
    use crate::graph_ui::GraphAction;
    let mut app = app_on(&temp_dir("save-count").join("session.mnf"), 1);
    let ctx = egui::Context::default();
    let g = add_group(&mut app, "g");
    let (a, b, c, d) = (a_note(&mut app), a_note(&mut app), a_note(&mut app), a_note(&mut app));
    assert!(app.save_session());
    let begins = |app: &App| app.doc.journal.as_ref().unwrap().begins;
    let count = |app: &mut App, action: GraphAction| {
        let before = begins(app);
        app.apply_graph_action(&ctx, action);
        begins(app) - before
    };
    let delete = count(&mut app, GraphAction::Node(a, NodeAction::Delete));
    let join = count(
        &mut app,
        GraphAction::Drop {
            id: crate::card_id::CardId::from(b),
            target: Some((DropEffect::Join, Some(crate::card_id::CardId::from(g)))),
        },
    );
    app.link.connecting = Some((c, crate::caps::LINK));
    let finish = count(&mut app, GraphAction::FinishConnect(d));
    let enter = count(&mut app, GraphAction::EnterGroup(g));
    assert!(!app.nodes.contains_key(&a), "deleted");
    assert_eq!(app.doc.mirror.group(b), Some(g), "joined");
    assert_eq!(app.links_naming(c).len(), 1, "linked");
    assert_eq!(app.current_group, Some(g), "entered");
    assert_eq!(
        [("menu Delete", delete), ("enter a group", enter), ("Join drop", join), ("finish a link", finish)],
        [("menu Delete", 2), ("enter a group", 1), ("Join drop", 2), ("finish a link", 2)]
    );
}

/// Wave 2 (teardown order): replacing a session of 2000 notes and 300 links. Timing, not correctness, so ignored:
/// `cargo test --release -j 8 -p manifold --bins teardown_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn teardown_cost() {
    use crate::store::testutil::{best_median, report};
    let mut times = Vec::new();
    for _ in 0..10 {
        let mut app = app_on(&temp_dir("teardown-cost").join("session.mnf"), 1);
        let notes: Vec<NodeId> = (0..2000).map(|_| a_note(&mut app)).collect();
        for k in 0..300 {
            app.create_link(notes[k], notes[(k * 7 + 1) % 2000]);
        }
        assert!(app.save_session());
        let start = std::time::Instant::now();
        app.teardown_running_session();
        times.push(start.elapsed());
    }
    report("teardown, 2000 notes and 300 links", best_median(times));
}

/// What landing a session restore costs the UI thread now that it seeds each host: 50 Restore spawns on one 5,000-cookie
/// jar, from the first `handle_tab_spawn_result` to the last, each with its real `save_session`. Fake hosts only queue
/// the seed, so this is the landing and not its delivery. A later host waits for the landings ahead of it, out of a
/// margin of its start-up plus `COOKIE_SEED_WAIT`. Timing, not correctness, so ignored:
/// `cargo test --release -j 12 -p manifold -- --ignored --nocapture restore_landing_costs`
#[test]
#[ignore]
fn restore_landing_costs() {
    use crate::store::testutil::{best_median, report};
    const TABS: usize = 50;
    const COOKIES: usize = 5000;
    let cookie = |i: usize| crate::jar::Cookie {
        name: format!("cookie_{i}"),
        value: "a1b2c3d4".repeat(8),
        domain: format!(".site{}.example.com", i % 200),
        path: "/".into(),
        expires: Some(2_000_000_000.0),
        secure: true,
        http_only: true,
        same_site: crate::jar::SameSite::Lax,
    };
    let round = || {
        let mut app = app_on(&temp_dir("landing-cost").join("session.mnf"), 1);
        let jar = default_jar_id(&mut app);
        app.ensure_jar_loaded(jar);
        app.cookies.jars.get_mut(&jar).unwrap().cookies = (0..COOKIES).map(cookie).collect();
        for i in 0..TABS {
            let id = app.alloc_node_id();
            let url = format!("https://site{i}.example.com/");
            let mut spawn =
                crate::tabs::TabSpawn::new(id, url, ViewKind::Cef, egui::Vec2::ZERO, SpawnPurpose::Restore, 1.0);
            spawn.jar = Some(jar);
            app.spawn_tab_async(spawn);
        }
        let (mut outcomes, mut peers) = (Vec::new(), Vec::new());
        for (spawn, generation) in std::mem::take(&mut app.test_spawns) {
            let (tab, peer) = crate::process_manager::ViewTab::fake(spawn.kind);
            outcomes.push(crate::tabs::TabSpawnOutcome { spawn, generation, result: Ok(tab), jar: Some(jar) });
            peers.push(peer);
        }
        let start = std::time::Instant::now();
        for outcome in outcomes {
            app.handle_tab_spawn_result(outcome);
        }
        start.elapsed()
    };
    report("50 landings on a 5,000-cookie jar", best_median((0..5).map(|_| round()).collect()));
}
