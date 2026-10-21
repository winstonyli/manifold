//! Session-sync tests: timing of save and teardown.

use super::*;

/// Ruling 6: a note can reach 1 MB, so the per-frame reader borrows: the mirror's own string when idle, the box's
/// buffer while typed in; neither is a copy.
#[test]
fn a_large_note_edit_does_not_clone_per_frame() {
    let mut app = App::default();
    let text = "x".repeat(1 << 20);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text, editing: false }));
    let idle: &str = app.note_text(&note);
    assert_eq!(idle.as_ptr(), app.doc.mirror.text(&note).as_ptr(), "idle: the mirror's string");
    assert_eq!(app.doc.mirror.text(&note).as_ptr(), app.doc.mirror.text(&note).as_ptr(), "`Mirror::text` borrows");
    app.nodes.get_mut(&note).unwrap().as_note_mut().unwrap().editing = true;
    let editing: &str = app.note_text(&note);
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
    use std::time::{Duration, Instant};
    fn timed(n: usize, mut f: impl FnMut()) -> (Duration, Duration) {
        let mut times: Vec<Duration> = (0..n)
            .map(|_| {
                let start = Instant::now();
                f();
                start.elapsed()
            })
            .collect();
        times.sort();
        (times[0], times[n / 2])
    }
    fn report(what: &str, (best, median): (Duration, Duration)) {
        println!("  {what:<44} best {best:>10.1?}  median {median:>10.1?}");
    }
    const N: usize = 40;
    for tabs in [300usize, 2000, 3000] {
        let dir = temp_dir(&format!("app-cost-{tabs}"));
        let mut app = app_on(&dir.join("session.mnf"), 1);
        let groups: Vec<String> = (0..tabs / 10).map(|i| add_group(&mut app, &format!("Group {i}"))).collect();
        let mut ids = Vec::new();
        for i in 0..tabs {
            let url = format!("https://www.example{}.com/articles/2026/09/some-long-article-slug-{i}?ref=home", i % 50);
            let id = add(&mut app, Kind::Tab, failed_tab(&url, 1.0));
            app.edit_group(&id, Some(groups[i % groups.len()].clone()));
            let pos = egui::vec2(i as f32 * 3.5, i as f32 * -2.25);
            app.snap_layout(&id, pos);
            app.edit_pos(&id, Some(pos));
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
                let id = &ids[k % tabs];
                let url = format!("https://moved.example/{k}");
                if let Runtime::Unloaded(f) = &mut app.nodes.get_mut(id).unwrap().runtime {
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
                app.layout.get_mut(&ids[k % tabs]).unwrap().pos.x += 1.0;
                k += 1;
                app.maybe_write_local(true);
            }),
        );
        let bytes = app.local_state().to_bytes().len();
        println!("  session.mnf.local: {bytes} B");
    }
}

/// Wave 2 (H5): the saves that reach the store (`Journal::begin`) per action through `apply_graph_action`, which
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
    let delete = count(&mut app, GraphAction::Node(a.clone(), NodeAction::Delete));
    let join = count(&mut app, GraphAction::Drop { id: b.clone(), target: Some((DropEffect::Join, Some(g.clone()))) });
    app.link.connecting = Some((c.clone(), crate::caps::LINK));
    let finish = count(&mut app, GraphAction::FinishConnect(d.clone()));
    let enter = count(&mut app, GraphAction::EnterGroup(g.clone()));
    assert!(!app.nodes.contains_key(&a), "deleted");
    assert_eq!(app.doc.mirror.group(&b), Some(g.as_str()), "joined");
    assert_eq!(app.links_naming(&c).len(), 1, "linked");
    assert_eq!(app.current_group.as_deref(), Some(g.as_str()), "entered");
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
    use crate::bench::{best_median, report};
    let mut times = Vec::new();
    for _ in 0..10 {
        let mut app = app_on(&temp_dir("teardown-cost").join("session.mnf"), 1);
        let notes: Vec<String> = (0..2000).map(|_| a_note(&mut app)).collect();
        for k in 0..300 {
            app.create_link(&notes[k], &notes[(k * 7 + 1) % 2000]);
        }
        assert!(app.save_session());
        let start = std::time::Instant::now();
        app.teardown_running_session();
        times.push(start.elapsed());
    }
    report("teardown, 2000 notes and 300 links", best_median(times));
}
