use super::*;
use crate::card_id::testutil::node;
use crate::store::testutil::temp_dir;

fn visit(h: &mut NavHistory, urls: &[&str], now: Instant) {
    for url in urls {
        h.observe(url, "", true, false, now);
        h.observe(url, "", false, false, now);
    }
}

fn urls(h: &NavHistory) -> Vec<&str> {
    h.entries.iter().map(|e| e.url.as_str()).collect()
}

fn parents(h: &NavHistory) -> Vec<Option<usize>> {
    h.entries.iter().map(|e| e.parent).collect()
}

/// A native Back the engine completes: one settled report at the parent's URL, Forward now possible.
fn back(h: &mut NavHistory, now: Instant) {
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
    let parent = h.entries[h.at].parent.unwrap();
    let url = h.entries[parent].url.clone();
    h.observe(&url, "", false, true, now);
}

#[test]
fn tree_lists_every_page_with_the_cursor_and_the_path_from_the_root() {
    // /a -> /b -> /c and /a -> /d, cursor on /d; /b has a title.
    let mut h = NavHistory::with_tree(
        &[("/a", None, Some(3)), ("/b", Some(0), Some(2)), ("/c", Some(1), None), ("/d", Some(0), None)],
        3,
    );
    h.entries[1].title = "Bee".into();
    let pages = h.tree();
    assert_eq!(pages.iter().map(|p| p.serial).collect::<Vec<_>>(), [0, 1, 2, 3]);
    assert_eq!(pages.iter().map(|p| p.parent).collect::<Vec<_>>(), [None, Some(0), Some(1), Some(0)]);
    assert_eq!(pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["/a", "Bee", "/c", "/d"]);
    assert_eq!(pages.iter().map(|p| p.cursor).collect::<Vec<_>>(), [false, false, false, true]);
    assert_eq!(pages.iter().map(|p| p.on_path).collect::<Vec<_>>(), [true, false, false, true]);
    assert!(NavHistory::default().tree().is_empty());
}

fn labels(urls: &[&str], titles: &[&str]) -> Vec<String> {
    let mut h = NavHistory::visited(urls);
    for (e, t) in h.entries.iter_mut().zip(titles) {
        e.title = (*t).into();
    }
    h.tree().into_iter().map(|p| p.label).collect()
}

#[test]
fn pages_that_differ_only_in_their_query_are_labelled_by_what_differs() {
    let urls =
        ["https://s.test/find?q=cats&page=1", "https://s.test/find?q=cats&page=2", "https://s.test/find?q=dogs&page=2"];
    // Untitled: the path plus what differs, in front so a clipped label keeps it.
    assert_eq!(labels(&urls, &[]), ["q=cats page=1 · /find", "q=cats page=2 · /find", "q=dogs page=2 · /find"]);
    // Titled alike: the title stays after the differing part.
    assert_eq!(labels(&urls, &["Results"; 3])[1], "q=cats page=2 · Results");
    // What every page shares is not repeated.
    let same_q = ["https://s.test/find?q=cats&page=1", "https://s.test/find?q=cats&page=2"];
    assert_eq!(labels(&same_q, &[]), ["page=1 · /find", "page=2 · /find"]);
}

#[test]
fn a_page_with_no_look_alike_keeps_its_plain_label() {
    let urls = ["https://s.test/a?x=1", "https://s.test/b?x=2"];
    assert_eq!(labels(&urls, &["", "Bee"]), ["/a?x=1", "Bee"]);
    // The same URL on two entries (a loop in the tree) differs in nothing: no suffix to show.
    let twice =
        NavHistory::with_tree(&[("https://s.test/a?x=1", None, Some(1)), ("https://s.test/a?x=1", Some(0), None)], 1);
    assert_eq!(twice.tree().iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["/a?x=1", "/a?x=1"]);
}

#[test]
fn an_untitled_page_is_labelled_by_its_path_or_else_its_host_and_keeps_the_full_url() {
    let h = NavHistory::visited(&[
        "https://example.com/docs/intro?x=1#top",
        "https://example.com/",
        "https://example.com",
        "/plain",
    ]);
    let pages = h.tree();
    assert_eq!(
        pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(),
        ["/docs/intro?x=1", "example.com", "example.com", "/plain"]
    );
    assert_eq!(pages[0].url, "https://example.com/docs/intro?x=1#top");
}

#[test]
fn the_fork_rule_reuses_an_equal_child_and_otherwise_adds_a_sibling() {
    let now = Instant::now();
    let mut h = NavHistory::default();
    visit(&mut h, &["a", "b"], now);
    back(&mut h, now);
    visit(&mut h, &["c"], now);
    assert_eq!(urls(&h), ["a", "b", "c"]);
    assert_eq!(parents(&h), [None, Some(0), Some(0)]);
    assert_eq!((h.at, h.lin.clone(), h.lin_at), (2, vec![Some(0), Some(2)], 1));
    back(&mut h, now);
    visit(&mut h, &["b"], now);
    assert_eq!(h.entries.len(), 3, "an equal child is reused");
    assert_eq!((h.at, h.entries[0].fwd), (1, Some(1)));
}

#[test]
fn a_redirect_on_an_entry_with_children_adds_a_child_in_the_same_lin_slot() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    back(&mut h, now);
    assert!(h.fresh, "a commit marks the target fresh");
    h.observe("a", "", false, true, now);
    assert!(!h.fresh, "a later settled report clears it");
    h.observe("a", "", true, true, now);
    h.observe("a2", "", true, true, now);
    assert_eq!(urls(&h), ["a", "b", "a2"]);
    assert_eq!((h.at, h.entries[2].parent, h.entries[0].fwd), (2, Some(0), Some(2)));
    assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(2), Some(1)], 0), "the engine replaced its slot");
    assert_eq!(h.go(Dir::Back, true, true, now), NavAction::Navigate("a".into()), "no engine slot before a2");
}

#[test]
fn a_plain_back_is_a_new_engine_navigation_and_forward_then_navigates() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()), "the engine cannot go back");
    h.observe("a", "", true, false, now);
    h.observe("a", "", false, false, now);
    assert_eq!((h.at, h.entries[0].fwd), (0, Some(1)));
    assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(0), Some(1), Some(0)], 2));
    assert_eq!(h.go(Dir::Forward, true, true, now), NavAction::Navigate("b".into()), "no engine slot after a");
}

#[test]
fn view_reset_rebuilds_lin_and_makes_the_cursor_entry_fresh() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    back(&mut h, now);
    h.observe("a", "", false, true, now);
    h.view_reset("a");
    assert_eq!((h.lin.clone(), h.lin_at, h.fresh), (vec![Some(0)], 0, true));
    h.observe("a", "", true, false, now);
    h.observe("a2", "", true, false, now);
    assert_eq!(urls(&h), ["a2", "b"], "a fresh entry's redirect rewrites it, children or not");
    assert_eq!(h.go(Dir::Forward, false, true, now), NavAction::Navigate("b".into()));
}

#[test]
fn view_reset_with_another_url_empties_the_tree_and_the_next_report_makes_a_root() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    h.dirty = false;
    h.view_reset("x");
    assert!(h.entries.is_empty() && h.lin.is_empty() && h.dirty);
    h.observe("x", "", false, false, now);
    assert_eq!((urls(&h), h.at, h.lin.clone()), (vec!["x"], 0, vec![Some(0)]));
}

#[test]
fn view_reset_drops_a_pending_move() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
    h.view_reset("b");
    assert!(h.pending.is_none());
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::Navigate("a".into()), "a new engine has no slot before b");
}

#[test]
fn eviction_drops_the_oldest_unprotected_leaf_and_remaps() {
    // a -> {old, b}; b -> c. The cursor is b and Forward from it is c: the path and the fwd chain are a, b, c.
    let mut h = NavHistory::with_tree(
        &[("a", None, Some(2)), ("old", Some(0), None), ("b", Some(0), Some(3)), ("c", Some(2), None)],
        2,
    );
    h.lin = vec![Some(0), Some(2), Some(1)];
    h.evict_to(3);
    assert_eq!(urls(&h), ["a", "b", "c"]);
    assert_eq!(parents(&h), [None, Some(0), Some(1)]);
    assert_eq!((h.at, h.entries[0].fwd, h.entries[1].fwd), (1, Some(1), Some(2)));
    assert_eq!(h.lin, [Some(0), Some(1), None], "the dropped entry's slot is a tombstone");
}

#[test]
fn eviction_picks_the_lower_of_two_unprotected_leaves() {
    // a -> {x, y, b}: x and y are both unprotected leaves, x is older.
    let mut h = NavHistory::with_tree(
        &[("a", None, Some(3)), ("x", Some(0), None), ("y", Some(0), None), ("b", Some(0), None)],
        3,
    );
    h.evict_to(3);
    assert_eq!(urls(&h), ["a", "y", "b"]);
    assert_eq!(h.at, 2);
}

#[test]
fn eviction_of_a_chain_drops_the_root_and_moves_a_root_cursor() {
    let mut h = NavHistory::with_tree(&[("a", None, Some(1)), ("b", Some(0), Some(2)), ("c", Some(1), None)], 0);
    h.evict_to(2);
    assert_eq!(urls(&h), ["b", "c"]);
    assert_eq!(parents(&h), [None, Some(0)]);
    assert_eq!((h.at, h.entries[0].fwd), (0, Some(1)));
    assert!(!h.can_go_back(), "the new root's Back is the opened-from step's");
}

#[test]
fn an_evicted_lin_slot_is_a_tombstone_and_native_back_still_lines_up() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    h.view_reset("b");
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    h.observe("a", "", false, false, now);
    visit(&mut h, &["c"], now);
    assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(1), Some(0), Some(2)], 2));
    h.evict_to(2);
    assert_eq!(urls(&h), ["a", "c"]);
    assert_eq!((h.lin.clone(), h.lin_at, h.at), (vec![None, Some(0), Some(1)], 2, 1));
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack, "the engine's slot before c is still a");
}

#[test]
fn an_evicted_fwd_becomes_none() {
    // a -> {b, c}; b -> x, left for c. The cursor is c: x is the one unprotected leaf.
    let mut h = NavHistory::with_tree(
        &[("a", None, Some(3)), ("b", Some(0), Some(2)), ("x", Some(1), None), ("c", Some(0), None)],
        3,
    );
    h.evict_to(3);
    assert_eq!(urls(&h), ["a", "b", "c"]);
    assert_eq!((h.entries[1].fwd, h.entries[0].fwd, h.at), (None, Some(2), 2));
}

#[test]
fn a_pages_serial_survives_an_eviction_of_an_earlier_page() {
    let mut h = NavHistory::default();
    visit(&mut h, &["a", "b", "c"], Instant::now());
    let serial_c = h.serial_of(2).expect("c has a serial");
    h.remove(1);
    assert_eq!(h.index_of(serial_c), Some(1), "c moved down one slot and kept its serial");
    assert_eq!(h.serial_of(1), Some(serial_c));
}

#[test]
fn an_evicted_pages_serial_resolves_to_nothing() {
    let mut h = NavHistory::default();
    visit(&mut h, &["a", "b", "c"], Instant::now());
    let serial_b = h.serial_of(1).unwrap();
    h.remove(1);
    assert_eq!(h.index_of(serial_b), None);
    visit(&mut h, &["d"], Instant::now());
    let later = h.serial_of(h.entries.len() - 1);
    assert!(later.is_some() && later != Some(serial_b), "a serial is never reused");
}

#[test]
fn every_page_of_a_tree_has_its_own_serial_and_the_strip_gets_it() {
    let h = NavHistory::visited(&["a", "b", "c"]);
    let serials: Vec<u32> = h.tree().iter().map(|p| p.serial).collect();
    assert_eq!(serials.len(), 3);
    assert!(serials.iter().enumerate().all(|(i, s)| serials[..i].iter().all(|t| t != s)));
    assert_eq!(h.serial_of(3), None);
}

#[test]
fn two_histories_get_different_slots() {
    assert_ne!(NavHistory::default().slot(), NavHistory::default().slot());
}

#[test]
fn serials_are_assigned_in_order_on_load_and_the_next_push_continues() {
    let path = temp_dir("nav-history-serials").join("tab-history.json");
    let entries: Vec<Entry> = (0usize..3)
        .map(|n| Entry { url: format!("p{n}"), title: String::new(), parent: n.checked_sub(1), fwd: None, serial: 0 })
        .collect();
    let file = HashMap::from([(node(1).to_string(), Saved { entries, at: 2 })]);
    std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
    let mut loaded = load_from(&path);
    let h = loaded.get_mut(&node(1)).unwrap();
    assert_eq!((0..3).map(|i| h.serial_of(i)).collect::<Vec<_>>(), [Some(0), Some(1), Some(2)]);
    let pushed = h.child_or_push("p3");
    assert_eq!(h.serial_of(pushed), Some(3));
}

#[test]
fn the_tree_and_lin_are_capped() {
    let now = Instant::now();
    let pages: Vec<String> = (0..55).map(|n| format!("p{n}")).collect();
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    let mut h = NavHistory::default();
    visit(&mut h, &pages, now);
    assert_eq!((h.entries.len(), h.entries[0].url.as_str(), h.entries[0].parent), (MAX_ENTRIES, "p5", None));
    assert_eq!((h.lin.len(), h.lin_at, h.at), (MAX_ENTRIES, MAX_ENTRIES - 1, MAX_ENTRIES - 1));
    assert!(h.lin.iter().all(Option::is_some), "the front slots left lin with their entries");
}

#[test]
fn a_saved_tree_round_trips_and_a_restored_tab_goes_back_by_navigate() {
    let now = Instant::now();
    let path = temp_dir("nav-history-round-trip").join("tab-history.json");
    let mut h = NavHistory::visited(&["a", "b"]);
    back(&mut h, now);
    visit(&mut h, &["c"], now);
    h.observe("c", "C", false, false, now);
    let mut lists = HashMap::from([(node(1), h)]);
    save_to(&mut lists, |_| true, Some(&path));
    let mut loaded = load_from(&path);
    let r = loaded.get_mut(&node(1)).unwrap();
    assert_eq!((&r.entries, r.at), (&lists[&node(1)].entries, lists[&node(1)].at));
    assert!(r.lin.is_empty(), "lin is not saved");
    r.view_reset("c");
    assert_eq!(r.go(Dir::Back, true, false, now), NavAction::Navigate("a".into()));
}

#[test]
fn a_saved_tree_that_breaks_an_invariant_is_dropped() {
    let path = temp_dir("nav-history-invalid").join("tab-history.json");
    let e = |url: &str, parent: Option<usize>, fwd: Option<usize>| serde_json::json!({"url": url, "title": "", "parent": parent, "fwd": fwd});
    let k = |n| node(n).to_string();
    let file = serde_json::json!({
        k(1): {"entries": [e("a", None, Some(1)), e("b", Some(0), None)], "at": 1},
        k(2): {"entries": [e("a", None, None), e("b", Some(0), None)], "at": 2},
        k(3): {"entries": [e("a", None, None), e("b", None, None)], "at": 1},
        k(4): {"entries": [e("a", None, None), e("b", Some(2), None), e("c", Some(0), None)], "at": 2},
        k(5): {
            "entries": [e("a", None, None), e("b", Some(0), Some(2)), e("c", Some(0), None)],
            "at": 2
        },
        k(6): {"entries": [], "at": 0},
    });
    std::fs::write(&path, file.to_string()).unwrap();
    assert_eq!(load_from(&path).keys().copied().collect::<Vec<_>>(), [node(1)]);
}

#[test]
fn a_saved_tree_over_the_cap_is_evicted_on_load() {
    let path = temp_dir("nav-history-over-cap").join("tab-history.json");
    let entries: Vec<Entry> = (0usize..60)
        .map(|n| Entry { url: format!("p{n}"), title: String::new(), parent: n.checked_sub(1), fwd: None, serial: 0 })
        .collect();
    let file = HashMap::from([(node(1).to_string(), Saved { entries, at: 59 })]);
    std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
    let loaded = load_from(&path);
    let h = &loaded[&node(1)];
    assert_eq!((h.entries.len(), h.entries[0].url.as_str(), h.at), (MAX_ENTRIES, "p10", MAX_ENTRIES - 1));
}

#[test]
fn a_file_in_the_old_format_is_moved_aside() {
    let dir = temp_dir("nav-history-old-format");
    let path = dir.join("tab-history.json");
    std::fs::write(&path, r#"{"t": {"entries": ["a", "b"], "index": 1}}"#).unwrap();
    assert!(load_from(&path).is_empty());
    assert!(!path.exists());
    let aside = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().starts_with("tab-history.unreadable-"));
    assert!(aside, "kept for a look, not overwritten");
}

#[test]
fn a_tab_that_is_gone_leaves_the_file_even_when_nothing_changed() {
    let path = temp_dir("nav-history-prune").join("tab-history.json");
    let mut lists =
        HashMap::from([(node(1), NavHistory::visited(&["a", "b"])), (node(2), NavHistory::visited(&["c", "d"]))]);
    save_to(&mut lists, |_| true, Some(&path));
    assert!(lists.values().all(|h| !h.dirty));
    save_to(&mut lists, |id| id == node(1), Some(&path));
    assert_eq!(load_from(&path).keys().copied().collect::<Vec<_>>(), [node(1)]);
}

#[test]
fn only_a_tree_change_marks_it_dirty() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    assert!(h.dirty);
    h.dirty = false;
    h.observe("a", "", false, false, now);
    assert!(!h.dirty, "a repeated report");
    h.observe("a", "A", false, false, now);
    assert!(h.dirty, "a title");
    let left = NavHistory::with_tree(&[("a", None, Some(1)), ("b", Some(0), None), ("c", Some(0), None)], 0);
    let right = NavHistory::with_tree(&[("a", None, Some(2)), ("b", Some(0), None), ("c", Some(0), None)], 0);
    assert_ne!(left.saved_form(), right.saved_form(), "a fwd");
}

#[test]
fn left_to_is_cleared_by_a_move_or_a_fork_but_not_by_a_report() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    h.set_left_to(Some(node(5)));
    h.observe("b", "B", false, false, now);
    assert_eq!(h.left_to(), Some(node(5)));
    visit(&mut h, &["c"], now);
    assert_eq!(h.left_to(), None, "a fork");
    h.set_left_to(Some(node(5)));
    back(&mut h, now);
    assert_eq!(h.left_to(), None, "a commit");
}

#[test]
fn a_history_with_only_left_to_is_not_saved() {
    let path = temp_dir("nav-history-left-to-only").join("tab-history.json");
    let mut parent = NavHistory::default();
    parent.set_left_to(Some(node(5)));
    assert!(!parent.dirty);
    let mut lists = HashMap::from([(node(2), parent), (node(1), NavHistory::visited(&["a", "b"]))]);
    save_to(&mut lists, |_| true, Some(&path));
    assert_eq!(load_from(&path).keys().copied().collect::<Vec<_>>(), [node(1)]);
}

#[test]
fn a_repeated_old_url_keeps_the_pending_move() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
    h.observe("b", "", false, false, now);
    assert!(h.pending.is_some());
    h.observe("a", "", false, true, now);
    assert_eq!((h.at, h.lin_at), (0, 0));
}

#[test]
fn the_origin_settling_after_a_plain_move_starts_is_not_a_redirect() {
    // After a wake the engine knows only c, whose load is still finishing when Back (a plain move to b) arrives.
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b", "c"]);
    h.view_reset("c");
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("b".to_owned()));
    h.observe("b", "", true, false, now);
    assert_eq!((h.at, h.lin.clone()), (1, vec![Some(2), Some(1)]));
    h.observe("c", "", false, false, now);
    assert_eq!(urls(&h), ["a", "b", "c"], "c's late settle did not rewrite b");
    h.observe("b", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b", "c"], 1));
    assert!(h.observe("c", "", false, false, now), "reported stale: the tab keeps b");
    assert_eq!(urls(&h), ["a", "b", "c"], "also after the target settled (Servo orders it so)");
    assert!(!h.observe("b", "", false, false, now), "the target's own report is not stale");
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".to_owned()), "Back goes on to a");
}

#[test]
fn a_mismatching_arrival_forks_instead_of_moving() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
    h.observe("x", "", true, false, now);
    assert_eq!(urls(&h), ["a", "b", "x"]);
    assert_eq!((h.at, h.entries[2].parent, h.pending.is_none()), (2, Some(1), true));
}

#[test]
fn a_stale_move_changes_nothing_and_go_waits_for_it() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    let before = (h.entries.clone(), h.at, h.lin.clone(), h.lin_at);
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
    assert_eq!(h.go(Dir::Back, true, false, now + Duration::from_secs(1)), NavAction::None, "a second press");
    let later = now + Duration::from_secs(4);
    h.observe("b", "", false, false, later);
    assert_eq!((h.entries.clone(), h.at, h.lin.clone(), h.lin_at), before);
    assert!(h.pending.is_none());
    assert_eq!(h.go(Dir::Back, true, false, later), NavAction::GoBack);
}

#[test]
fn a_loading_origin_report_gives_the_move_a_long_window() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    h.observe("b", "", true, false, now);
    h.observe("a", "", true, false, now + Duration::from_secs(20));
    h.observe("a", "", false, false, now + Duration::from_secs(21));
    assert_eq!((urls(&h), h.at), (vec!["a", "b"], 0), "arrived late, no fork");
}

#[test]
fn a_slow_plain_move_keeps_its_deadline_alive_while_the_origin_reports() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    for s in [2, 4, 6] {
        h.observe("b", "", true, false, now + Duration::from_secs(s));
    }
    h.observe("a", "", false, false, now + Duration::from_secs(8));
    assert_eq!(urls(&h), ["a", "b"], "no fork");
    assert_eq!(h.at, 0);
}

/// A woken engine can be quiet for seconds before its first report (5.8 s, `2026-11-07-navlog-back`): the move sent
/// meanwhile is still in time, whichever report comes first.
#[test]
fn a_move_sent_to_a_new_engine_waits_for_its_first_report() {
    let now = Instant::now();
    let late = now + Duration::from_secs(6);
    for origin_first in [true, false] {
        let mut h = NavHistory::visited(&["a", "b"]);
        h.view_reset("b");
        assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
        if origin_first {
            h.observe("b", "", true, false, late);
        }
        h.observe("a", "", true, false, late);
        h.observe("a", "", false, false, late);
        assert_eq!((urls(&h), h.at), (vec!["a", "b"], 0), "arrived, not a redirect or a fork");
    }
}

#[test]
fn a_move_sent_during_a_load_gets_the_long_window() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    h.observe("b", "", true, false, now);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    h.observe("a", "", false, false, now + Duration::from_secs(6));
    assert_eq!((urls(&h), h.at), (vec!["a", "b"], 0));
}

#[test]
fn a_move_with_no_reports_expires_after_the_window() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    assert!(h.move_pending(now + Duration::from_secs(3)));
    assert!(!h.move_pending(now + Duration::from_secs(4)));
    h.observe("a", "", false, false, now + Duration::from_secs(4));
    assert_eq!(urls(&h), ["a", "b", "a"], "a late arrival with no reports in between forks");
}

#[test]
fn a_plain_move_whose_arrival_redirects_rewrites_the_target() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    h.observe("a", "", true, false, now);
    h.observe("a2", "", true, false, now);
    h.observe("a2", "", false, false, now);
    assert_eq!(urls(&h), ["a2", "b"], "the entry Back chose stays the cursor, children and all");
    assert_eq!((h.at, h.lin.clone(), h.lin_at), (0, vec![Some(0), Some(1), Some(0)], 2), "no extra slot");
}

#[test]
fn a_fresh_navigation_during_a_plain_move_leaves_the_target_alone() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    h.observe("x", "", true, false, now);
    assert_eq!(urls(&h), ["a", "b", "x"]);
    assert_eq!((h.at, h.entries[2].parent), (2, Some(1)));
}

/// `a`, then `b` loading, then a discarded loading `a` (4b) and a settled `a` that is no Back (no Forward flag): a redirect
/// of b's load, so b's entry is rewritten to `a` and the cursor shares its parent's URL.
fn equal_parent(now: Instant) -> NavHistory {
    let mut h = NavHistory::visited(&["a"]);
    h.observe("b", "", true, false, now);
    h.observe("a", "", true, false, now);
    assert_eq!(urls(&h), ["a", "b"], "discarded");
    h.observe("a", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "a"], 1));
    h
}

#[test]
fn a_move_between_equal_urls_commits_at_go() {
    let now = Instant::now();
    let mut h = equal_parent(now);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::None, "the tab already shows that page");
    assert_eq!(
        (h.at, h.entries[0].fwd, h.lin.clone(), h.pending.is_none()),
        (0, Some(1), vec![Some(0), Some(0)], true)
    );
    let mut h = equal_parent(now);
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
    assert_eq!((h.at, h.lin_at, h.pending.is_none()), (0, 0, true));
}

/// Step 4 (the cursor's own URL) runs before 4b: a repeated settled report with Forward set is no inferred Back.
#[test]
fn the_cursors_own_url_wins_over_an_inferred_back() {
    let now = Instant::now();
    let mut h = equal_parent(now);
    h.observe("a", "", false, true, now);
    assert_eq!((h.at, h.lin_at, urls(&h)), (1, 1, vec!["a", "a"]), "no spurious Back commit");
}

#[test]
fn a_script_back_is_inferred_and_a_link_to_the_parents_url_forks() {
    let now = Instant::now();
    // history.back(): the loading report (old flags) is discarded, the settled one has Forward set.
    let mut h = NavHistory::visited(&["a", "b"]);
    h.observe("a", "", true, false, now);
    assert_eq!((h.entries.len(), h.at, h.loading), (2, 1, false), "discarded: nothing changes");
    h.observe("a", "", false, true, now);
    assert_eq!((h.entries.len(), h.at, h.lin_at, h.entries[0].fwd), (2, 0, 0, Some(1)));
    // A link to the parent's URL: the settled report has no Forward flag.
    let mut h = NavHistory::visited(&["a", "b"]);
    h.observe("a", "", true, false, now);
    h.observe("a", "", false, false, now);
    assert_eq!((urls(&h), h.at, h.entries[2].parent), (vec!["a", "b", "a"], 2, Some(1)));
    // The same from a fresh cursor entry: the discarded report must not make the settled one b's redirect.
    let mut h = NavHistory::visited(&["a", "b", "c"]);
    back(&mut h, now);
    assert!(h.fresh);
    h.observe("a", "", true, true, now);
    h.observe("a", "", false, false, now);
    assert_eq!(urls(&h), ["a", "b", "c", "a"], "b is not rewritten");
    assert_eq!((h.at, h.entries[3].parent), (3, Some(1)));
}

#[test]
fn a_discarded_report_then_another_url_forks_or_rewrites_by_the_earlier_loading() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    h.observe("a", "", true, false, now);
    h.observe("x", "", true, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b", "x"], 2), "a fork from b; the discarded a never got an entry");
    // b was still loading when the discarded report came: x is b's redirect.
    let mut h = NavHistory::visited(&["a"]);
    h.observe("b", "", true, false, now);
    h.observe("a", "", true, false, now);
    h.observe("x", "", true, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "x"], 1));
}

#[test]
fn a_discarded_report_then_a_repeat_or_a_late_settled_report() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    h.observe("a", "", true, false, now);
    h.observe("a", "", true, false, now);
    h.observe("a", "Page A", false, true, now);
    assert_eq!((h.at, h.entries.len(), h.entries[0].title.as_str()), (0, 2, "Page A"));
    // The settled report decides alone: how long ago the discarded one came does not matter (plan D1).
    let mut h = NavHistory::visited(&["a", "b"]);
    h.observe("a", "", true, false, now);
    h.observe("a", "", false, true, now + Duration::from_secs(10));
    assert_eq!((h.at, h.entries.len()), (0, 2));
}

#[test]
fn an_inferred_forward_needs_the_flag_except_at_the_last_slot() {
    let now = Instant::now();
    // At the last slot a click and history.forward() leave the same tree and lin: no flag needed.
    let mut h = NavHistory::visited(&["a", "b"]);
    back(&mut h, now);
    h.observe("a", "", false, true, now);
    h.observe("b", "", true, true, now);
    h.observe("b", "", false, false, now);
    assert_eq!((h.at, h.lin_at, h.lin.len(), h.fresh), (1, 1, 2, true));
    // A slot before the last, a settled report only (CEF, Servo): the flag decides.
    let mut h = NavHistory::visited(&["a", "b", "c"]);
    back(&mut h, now);
    back(&mut h, now);
    // Within the move's window the origin's settled report is the old document finishing; after it, a script.
    let later = now + MOVE_WINDOW + Duration::from_secs(1);
    h.observe("b", "", false, true, later);
    assert_eq!((h.at, h.lin_at, h.lin.len()), (1, 1, 3), "history.forward()");
    let mut h = NavHistory::visited(&["a", "b", "c"]);
    back(&mut h, now);
    back(&mut h, now);
    h.observe("b", "", false, false, now + MOVE_WINDOW + Duration::from_secs(1));
    assert_eq!((h.at, h.lin_at, h.lin.len()), (1, 1, 2), "a click on a link to b: the engine dropped c");
    assert_eq!(h.entries.len(), 3, "the equal child is reused");
}

#[test]
fn go_after_a_discarded_report_moves_as_usual() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b", "c"]);
    back(&mut h, now);
    h.observe("a", "", true, true, now);
    assert_eq!(h.go(Dir::Forward, true, true, now), NavAction::GoForward);
    h.observe("c", "", false, false, now);
    assert_eq!((h.at, h.lin_at), (2, 2));
}

#[test]
fn the_real_engine_order_infers_both_ways() {
    let now = Instant::now();
    // WebView2: the loading report has the old position's flags, the settled one the new position's.
    let mut h = NavHistory::visited(&["a", "b", "c"]);
    h.observe("b", "", true, false, now);
    h.observe("b", "", false, true, now);
    assert_eq!((h.at, h.lin_at), (1, 1), "history.back() from the last slot");
    h.observe("c", "", true, true, now);
    h.observe("c", "", false, false, now);
    assert_eq!((h.at, h.lin_at, h.lin.len()), (2, 2, 3), "history.forward() to the last slot");
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
    h.observe("b", "", false, true, now);
    h.observe("a", "", true, true, now);
    h.observe("a", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b", "c", "a"], 3), "a link to the parent's URL at a non-last slot");
    assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(0), Some(1), Some(3)], 2));
}

#[test]
fn titles_follow_the_cursor_entry() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.observe("a", "A", false, false, now);
    h.observe("b", "A", true, false, now);
    assert_eq!(h.entries[1].title, "", "a loading report may carry the previous page's title");
    h.observe("b", "B", false, false, now);
    h.observe("c", "C", false, false, now);
    let titles: Vec<&str> = h.entries.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["A", "B", "C"]);
}

#[test]
fn a_branch_pick_navigates_once_and_points_fwd_at_it() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    back(&mut h, now);
    visit(&mut h, &["c"], now);
    assert_eq!(h.branches(Dir::Back), [(1, "b".to_string())]);
    assert!(h.back_fork() && !h.forward_fork());
    assert_eq!(h.go_to(1, now), NavAction::Navigate("b".into()), "never native");
    h.observe("b", "", true, false, now);
    h.observe("b", "", false, false, now);
    assert_eq!((h.entries.len(), h.at, h.entries[0].fwd), (3, 1, Some(1)), "no second entry for b");
    assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(0), Some(2), Some(1)], 2));
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::Navigate("a".into()), "the engine's slot before b is c");
}

#[test]
fn a_forward_pick_points_fwd_at_it() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    back(&mut h, now);
    visit(&mut h, &["c"], now);
    back(&mut h, now);
    assert!(h.forward_fork());
    assert_eq!(h.entries[0].fwd, Some(2), "Back from c");
    assert_eq!(h.branches(Dir::Forward), [(1, "b".to_string()), (2, "c".to_string())]);
    assert_eq!(h.go_to(1, now), NavAction::Navigate("b".into()));
    h.observe("b", "", false, false, now);
    assert_eq!((h.at, h.entries[0].fwd), (1, Some(1)));
    assert_eq!(h.go_to(1, now), NavAction::None, "the cursor entry itself");
}

/// Right after a typed navigation an engine can still report the page it left, from before it started the navigation
/// (CEF reports loading first): hidden, so the tab keeps the typed url and the trail keeps only reported pages.
#[test]
fn a_typed_navigation_hides_the_origins_leftover_reports_and_pushes_the_new_page() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    assert!(h.observe("a", "", false, false, now), "a finish from before the engine started");
    assert!(h.observe("a", "", true, false, now), "started, address not changed yet");
    assert!(!h.observe("b", "", true, false, now), "the arrival is not stale");
    h.observe("b", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b"], 1));
    assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(0), Some(1)], 1));
}

#[test]
fn a_typed_navigation_from_a_page_still_loading_is_a_new_page_not_a_redirect() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.observe("a", "", true, false, now);
    h.navigate("b", now);
    assert!(h.observe("a", "", true, false, now));
    h.observe("b", "", true, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b"], 1), "a kept, b pushed");
}

/// A typed navigation that never leaves the page (an attachment, a 204): CEF reports it started and finished, both
/// still at the old url, and nothing after. The finish is the page's real url.
#[test]
fn a_typed_navigation_that_ends_at_its_origin_shows_the_origin_and_adds_nothing() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    assert!(h.observe("a", "", false, false, now), "a finish from before it started");
    assert!(h.observe("a", "", true, false, now), "started");
    assert!(!h.observe("a", "", false, false, now), "finished without leaving");
    assert!(h.pending.is_none());
    assert_eq!((urls(&h), h.at), (vec!["a"], 0));
}

/// An engine still loading the origin (here a new one, woken at it) finishes that load after the navigation is sent:
/// the load it finished was not the navigation's.
#[test]
fn the_origins_own_load_finishing_does_not_end_a_typed_navigation() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.view_reset("a");
    h.navigate("b", now);
    assert!(h.observe("a", "", true, false, now));
    assert!(h.observe("a", "", false, false, now), "still hidden");
    h.observe("b", "", true, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b"], 1));
}

#[test]
fn after_a_typed_arrival_the_origins_late_finish_is_held_and_a_redirect_rewrites_the_new_page() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    h.observe("b", "", false, false, now);
    assert!(h.observe("a", "", false, false, now), "the old document finishing");
    assert_eq!((urls(&h), h.at), (vec!["a", "b"], 1));
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    h.observe("b", "", true, false, now);
    h.observe("c", "", true, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "c"], 1), "b redirected to c");
}

#[test]
fn a_typed_navigation_does_not_block_back_and_back_replaces_it() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    h.navigate("c", now);
    assert!(!h.move_pending(now));
    assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
    h.observe("a", "", false, true, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b"], 0));
}

#[test]
fn a_second_typed_url_replaces_the_first_and_the_origin_stays_hidden() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    h.navigate("c", now);
    assert!(h.observe("a", "", true, false, now));
    h.observe("c", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "c"], 1));
}

/// A move replaced before its hold ran out passes the hold on: the old document's late finish is still hidden.
#[test]
fn a_second_typed_url_after_the_first_arrived_still_holds_the_first_origin() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    h.observe("b", "", false, false, now);
    h.navigate("c", now);
    assert!(h.observe("a", "", false, false, now), "a's late finish, held");
    h.observe("c", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b", "c"], 2));
}

/// The same for a move that hadn't arrived: its arrival, if the engine still makes it, is not a page of the trail.
#[test]
fn a_typed_url_during_a_back_hides_the_backs_late_arrival() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    h.navigate("c", now);
    assert!(h.observe("a", "", false, false, now), "the Back's arrival, hidden");
    h.observe("c", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b", "c"], 2));
}

/// A move the page ends with a navigation of its own: the origin's late reports are the old document's.
#[test]
fn a_page_navigation_during_a_trail_move_hides_the_origins_late_reports() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    h.observe("a", "", false, true, now);
    assert_eq!(h.go(Dir::Forward, false, false, now), NavAction::Navigate("b".into()));
    h.observe("b2", "", true, false, now);
    assert!(h.observe("a", "", false, false, now), "the origin finishing, held");
    h.observe("b2", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["a", "b", "b2"], 2));
}

/// A typed url replaced by Back: the typed page, if the engine still reaches it, is not a page of the trail.
#[test]
fn back_during_a_typed_url_hides_the_typed_pages_late_arrival() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["p", "a"]);
    h.navigate("b", now);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("p".into()));
    assert!(h.observe("b", "", true, false, now), "the typed page loading, held");
    assert!(h.observe("b", "", false, false, now), "and finishing, held");
    h.observe("p", "", false, false, now);
    assert_eq!((urls(&h), h.at), (vec!["p", "a"], 0));
}

/// Typing the url shown ends the typed move without a new one; what it was waiting on is still held.
#[test]
fn typing_the_url_shown_during_a_typed_url_hides_the_first_typed_page() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    h.navigate("a", now);
    assert!(h.observe("b", "", true, false, now));
    assert!(h.observe("b", "", false, false, now));
    assert!(!h.observe("a", "", false, false, now), "the page shown is not held");
    assert_eq!((urls(&h), h.at), (vec!["a"], 0));
}

/// An old document still loading keeps its hold, as the origin's loading reports keep a move's.
#[test]
fn a_held_pages_loading_report_extends_its_hold() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    h.observe("b", "", false, false, now);
    h.navigate("c", now);
    assert!(h.observe("a", "", true, false, now + Duration::from_secs(1)));
    assert!(h.observe("a", "", false, false, now + Duration::from_secs(4)), "past MOVE_WINDOW, still held");
    h.observe("c", "", false, false, now + Duration::from_secs(5));
    assert_eq!((urls(&h), h.at), (vec!["a", "b", "c"], 2));
}

/// What a replaced move hid stops being hidden when the new move goes there.
#[test]
fn back_to_the_page_a_hold_covers_still_arrives() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a"]);
    h.navigate("b", now);
    h.observe("b", "", false, false, now);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    assert!(!h.observe("a", "", false, false, now), "the arrival");
    assert_eq!((urls(&h), h.at), (vec!["a", "b"], 0));
}

#[test]
fn typing_the_url_shown_or_into_an_empty_trail_starts_no_move() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    h.navigate("b", now);
    assert!(h.pending.is_none(), "the engine was sent b: the move to a is over");
    let mut h = NavHistory::default();
    h.navigate("b", now);
    assert!(h.pending.is_none());
    h.observe("a", "", true, false, now);
    assert_eq!(urls(&h), ["a"], "the engine's report makes the root");
}

/// Eviction renumbers entries: the push it follows has dropped the pending move, which then names its target anew.
#[test]
fn a_typed_navigation_from_a_full_trail_evicts_and_still_holds_the_origin() {
    let now = Instant::now();
    let names: Vec<String> = (0..MAX_ENTRIES).map(|i| format!("p{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut h = NavHistory::visited(&refs);
    h.navigate("new", now);
    h.observe("new", "", false, false, now);
    assert_eq!((h.entries.len(), h.entries[h.at].url.as_str()), (MAX_ENTRIES, "new"));
    assert!(h.observe(refs[MAX_ENTRIES - 1], "", false, false, now), "the origin's late finish, held");
    assert_eq!(h.entries[h.at].url, "new");
}

#[test]
fn a_trail_moves_origin_reports_before_the_arrival_are_hidden_and_a_204_ends_it() {
    let now = Instant::now();
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    assert!(h.observe("b", "", false, false, now), "the tab keeps showing a");
    h.observe("a", "", false, false, now);
    assert_eq!(h.at, 0);
    let mut h = NavHistory::visited(&["a", "b"]);
    assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
    assert!(h.observe("b", "", true, false, now));
    assert!(!h.observe("b", "", false, false, now), "a started load that ended at b");
    assert_eq!((urls(&h), h.at, h.pending.is_none()), (vec!["a", "b"], 1, true));
}
