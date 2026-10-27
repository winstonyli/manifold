use super::*;
use crate::card_id::testutil::node;
use crate::store::model::{ActorId, NodeId};
use crate::store::testutil::temp_dir;

/// A fixed jar id for tests that need one jar; its file is `000000000000000d-1.json`.
fn d() -> NodeId {
    NodeId { actor: ActorId(0xd), counter: 1 }
}

fn c(name: &str, value: &str) -> Cookie {
    Cookie {
        name: name.into(),
        value: value.into(),
        domain: "example.com".into(),
        path: "/".into(),
        expires: None,
        secure: false,
        http_only: true,
        same_site: SameSite::Lax,
    }
}

/// Wave 2 (Jar H3): one save of a 3000-cookie jar (serialize, DPAPI on Windows, temp file, rename). Timing,
/// not correctness, so ignored: `cargo test --release -j 8 -p manifold --bins jar_save_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn jar_save_cost() {
    use crate::bench::{report, timed};
    let dir = temp_dir("jar-save-cost");
    let cookies = (0..3000).map(|i| c(&format!("n{i}"), &format!("value-{i}-0123456789abcdef"))).collect();
    let jar = Jar { cookies, ..Jar::default() };
    report("save, 3000 cookies", timed(40, || jar.save(&dir, d()).unwrap()));
}

fn key(name: &str) -> Key {
    (name.into(), "example.com".into(), "/".into())
}

/// `merge` as it was before the index (4b833af), the reference the indexed one must match.
fn merge_reference(jar: &mut Vec<Cookie>, pulled: Vec<Cookie>, tab: &mut TabCookies) -> bool {
    let before = jar.clone();
    let t = now();
    let pulled_keys: HashSet<Key> = pulled.iter().map(Cookie::key).collect();
    for gone in tab.reported.difference(&pulled_keys) {
        jar.retain(|c| c.key() != *gone);
    }
    for p in &pulled {
        let k = p.key();
        if tab.baseline.get(&k).is_some_and(|b| b.same_as(p)) {
            continue;
        }
        match jar.iter_mut().position(|c| c.key() == k) {
            _ if p.expired(t) => jar.retain(|c| c.key() != k),
            Some(i) if jar[i].same_as(p) => {}
            Some(i) => jar[i] = p.clone(),
            None => jar.push(p.clone()),
        }
    }
    jar.retain(|c| !c.expired(t));
    let reported = std::mem::replace(&mut tab.reported, pulled_keys);
    tab.baseline.retain(|k, _| !reported.contains(k));
    tab.baseline.extend(pulled.into_iter().map(|p| (p.key(), p)));
    *jar != before
}

/// Pulls hold one cookie per key, as an engine's dump does. Every fourth starting jar repeats keys, which no code
/// path writes but a hand-edited or legacy file could hold: the index must match the reference there too.
#[test]
fn the_indexed_merge_matches_the_reference() {
    let mut rng = crate::store::testutil::XorShift(11);
    let cookie = |rng: &mut crate::store::testutil::XorShift| {
        let mut x = c(&format!("n{}", rng.below(12)), &format!("v{}", rng.below(3)));
        x.expires = match rng.below(4) {
            0 => Some(1.0),
            1 => Some(4_000_000_000.0),
            _ => None,
        };
        x
    };
    let unique = |v: Vec<Cookie>| {
        let mut seen = HashSet::new();
        v.into_iter().filter(|x| seen.insert(x.key())).collect::<Vec<_>>()
    };
    for round in 0..500 {
        let raw: Vec<Cookie> = (0..rng.below(10)).map(|_| cookie(&mut rng)).collect();
        let start = if round % 4 == 0 { raw } else { unique(raw) };
        let mut tab = TabCookies::seeded(&start);
        let (mut jar, mut reference) = (Jar { locked: false, cookies: start.clone() }, start.clone());
        let mut ref_tab = TabCookies::seeded(&start);
        for _ in 0..3 {
            let pulled = unique((0..rng.below(10)).map(|_| cookie(&mut rng)).collect());
            let got = jar.merge(pulled.clone(), &mut tab);
            let want = merge_reference(&mut reference, pulled, &mut ref_tab);
            assert_eq!((got, &jar.cookies), (want, &reference), "round {round}");
            assert_eq!(tab, ref_tab, "round {round}");
        }
    }
}

#[test]
fn a_pulled_cookie_replaces_the_stored_one_with_the_same_key() {
    let mut jar = Jar { locked: false, cookies: vec![c("sid", "old")] };
    let mut tab = TabCookies::seeded(&jar.cookies);
    assert!(jar.merge(vec![c("sid", "new")], &mut tab));
    assert_eq!(jar.cookies, vec![c("sid", "new")]);
}

#[test]
fn a_cookie_missing_from_the_pull_is_deleted_only_if_the_tab_reported_it() {
    let mut jar = Jar { locked: false, cookies: vec![c("sid", "1"), c("fresh", "2")] };
    let mut tab = TabCookies { reported: [key("sid")].into(), ..TabCookies::default() };
    assert!(jar.merge(vec![], &mut tab));
    assert_eq!(jar.cookies, vec![c("fresh", "2")], "sid was a logout; fresh was never seen by the engine");
}

#[test]
fn an_expired_pulled_cookie_deletes_the_stored_one_with_its_key() {
    let mut jar = Jar { locked: false, cookies: vec![c("sid", "1"), c("other", "2")] };
    let mut gone = c("sid", "1");
    gone.expires = Some(1.0);
    assert!(jar.merge(vec![gone], &mut TabCookies::default()), "the engine's own expiry wins");
    assert_eq!(jar.cookies, vec![c("other", "2")]);
}

#[test]
fn a_stale_idle_tab_does_not_overwrite_a_rotated_token() {
    let mut jar = Jar { locked: false, cookies: vec![c("sid", "v1")] };
    let mut active = TabCookies::seeded(&jar.cookies);
    let mut idle = TabCookies::seeded(&jar.cookies);
    assert!(jar.merge(vec![c("sid", "v2")], &mut active), "the site rotated sid in the active tab");
    assert!(!jar.merge(vec![c("sid", "v1")], &mut idle), "the idle tab's v1 is what it was seeded with");
    assert_eq!(jar.cookies, vec![c("sid", "v2")]);
    assert!(!jar.merge(vec![c("sid", "v1")], &mut idle), "nor on its later pulls");
    assert_eq!(jar.cookies, vec![c("sid", "v2")]);
}

#[test]
fn a_seeded_cookie_the_tab_never_reported_survives_a_pull_without_it() {
    let mut jar = Jar { locked: false, cookies: vec![c("sid", "1"), c("pref", "2")] };
    let mut tab = TabCookies::seeded(&jar.cookies);
    // The engine rejected sid's seed: its first pull has only pref.
    assert!(!jar.merge(vec![c("pref", "2")], &mut tab));
    assert_eq!(jar.cookies, vec![c("sid", "1"), c("pref", "2")]);
    assert!(!jar.merge(vec![c("pref", "2")], &mut tab), "still not a logout on the next pull");
    assert_eq!(jar.cookies.len(), 2);
}

#[test]
fn a_cookie_one_pull_confirmed_and_the_next_lacks_is_deleted() {
    let mut jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
    let mut tab = TabCookies::seeded(&jar.cookies);
    assert!(!jar.merge(vec![c("sid", "1")], &mut tab));
    assert!(jar.merge(vec![], &mut tab), "the tab held sid and now does not: a logout");
    assert!(jar.cookies.is_empty());
    assert!(tab.baseline.is_empty() && tab.reported.is_empty());
}

#[test]
fn an_empty_pull_from_a_tab_that_never_reported_deletes_nothing() {
    let mut jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
    let mut tab = TabCookies::seeded(&jar.cookies);
    assert!(!jar.merge(vec![], &mut tab));
    assert_eq!(jar.cookies, vec![c("sid", "1")]);
}

fn expiring(name: &str, value: &str, expires: f64) -> Cookie {
    Cookie { expires: Some(expires), ..c(name, value) }
}

#[test]
fn expiries_within_a_millisecond_are_the_same_cookie() {
    let e = 4_102_444_800.123_456;
    assert!(expiring("sid", "1", e).same_as(&expiring("sid", "1", e + 0.000_001)), "a conversion wobble");
    assert!(!expiring("sid", "1", e).same_as(&expiring("sid", "1", e + 0.01)));
    assert!(!expiring("sid", "1", e).same_as(&expiring("sid", "2", e)), "the value still counts");
    assert!(!expiring("sid", "1", e).same_as(&c("sid", "1")), "a session cookie is not a persistent one");
    assert!(c("sid", "1").same_as(&c("sid", "1")));
}

#[test]
fn an_expiry_wobble_is_ignored_and_a_real_expiry_change_wins() {
    let e = 4_102_444_800.123_456;
    let mut jar = Jar { locked: false, cookies: vec![expiring("sid", "v1", e)] };
    let mut active = TabCookies::seeded(&jar.cookies);
    let mut idle = TabCookies::seeded(&jar.cookies);
    assert!(jar.merge(vec![expiring("sid", "v2", e + 3600.0)], &mut active), "rotated, with a new Max-Age");
    // The idle tab's copy of v1 came back through its engine a microsecond off: still its seed, not a change.
    assert!(!jar.merge(vec![expiring("sid", "v1", e + 0.000_001)], &mut idle));
    assert_eq!(jar.cookies, vec![expiring("sid", "v2", e + 3600.0)]);
    // The active tab's own copy wobbles too: no change, so no save.
    assert!(!jar.merge(vec![expiring("sid", "v2", e + 3_600.000_001)], &mut active));
    // A real expiry change (a sliding session) wins.
    assert!(jar.merge(vec![expiring("sid", "v2", e + 7200.0)], &mut active));
    assert_eq!(jar.cookies, vec![expiring("sid", "v2", e + 7200.0)]);
}

#[test]
fn an_unseeded_tab_reporting_the_jars_cookie_with_a_wobble_changes_nothing() {
    let e = 4_102_444_800.5;
    let mut jar = Jar { locked: false, cookies: vec![expiring("sid", "1", e)] };
    assert!(!jar.merge(vec![expiring("sid", "1", e - 0.000_001)], &mut TabCookies::default()));
    assert_eq!(jar.cookies, vec![expiring("sid", "1", e)]);
}

#[test]
fn debug_never_prints_a_cookie_value() {
    let jar = Jar { locked: false, cookies: vec![c("sid", "TOP-SECRET")] };
    let text = format!("{jar:?} {:?}", c("sid", "TOP-SECRET"));
    assert!(!text.contains("TOP-SECRET") && text.contains("<redacted>") && text.contains("sid"), "{text}");
}

#[test]
fn merging_the_same_cookies_changes_nothing() {
    let mut jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
    let mut tab = TabCookies::seeded(&jar.cookies);
    assert!(!jar.merge(vec![c("sid", "1")], &mut tab));
}

#[test]
fn an_expired_cookie_is_dropped_on_merge() {
    let mut gone = c("old", "x");
    gone.expires = Some(1.0);
    let mut jar = Jar::default();
    assert!(!jar.merge(vec![gone], &mut TabCookies::default()), "nothing was added, so nothing changed");
    assert!(jar.cookies.is_empty());
}

#[test]
fn the_jar_round_trips_through_its_file() {
    let dir = temp_dir("jar-roundtrip");
    let jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
    jar.save(&dir, d()).unwrap();
    assert_eq!(Jar::load(&dir, d()), jar);
}

fn asides(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("000000000000000d-1.json.unreadable-"))
        .collect();
    v.sort();
    v
}

#[test]
fn a_second_failure_keeps_the_first_aside() {
    let dir = temp_dir("jar-two-asides");
    std::fs::write(Jar::path(&dir, d()), "first").unwrap();
    Jar::load(&dir, d());
    std::fs::write(Jar::path(&dir, d()), "second").unwrap();
    Jar::load(&dir, d());
    let a = asides(&dir);
    assert_eq!(a.len(), 2);
    let mut contents: Vec<_> = a.iter().map(|p| std::fs::read(p).unwrap()).collect();
    contents.sort();
    assert_eq!(contents, vec![b"first".to_vec(), b"second".to_vec()]);
}

#[test]
fn a_header_only_or_truncated_file_goes_aside() {
    let dir = temp_dir("jar-header-only");
    std::fs::write(Jar::path(&dir, d()), b"MJR1").unwrap();
    assert_eq!(Jar::load(&dir, d()), Jar::default());
    assert_eq!(asides(&dir).len(), 1);
    Jar { locked: false, cookies: vec![c("sid", "1")] }.save(&dir, d()).unwrap();
    let bytes = std::fs::read(Jar::path(&dir, d())).unwrap();
    std::fs::write(Jar::path(&dir, d()), &bytes[..bytes.len() / 2]).unwrap();
    assert_eq!(Jar::load(&dir, d()), Jar::default());
    assert_eq!(asides(&dir).len(), 2);
}

#[test]
fn an_unreadable_path_locks_the_jar_and_save_leaves_it_alone() {
    let dir = temp_dir("jar-locked");
    // A directory where the file should be: reading it fails with something other than NotFound.
    std::fs::create_dir(Jar::path(&dir, d())).unwrap();
    let jar = Jar::load(&dir, d());
    assert!(jar.locked);
    Jar { cookies: vec![c("sid", "1")], ..jar }.save(&dir, d()).unwrap();
    assert!(Jar::path(&dir, d()).is_dir());
}

#[cfg(windows)]
#[test]
fn a_failed_aside_rename_locks_the_jar_and_the_file_survives() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = temp_dir("jar-rename-fails");
    std::fs::write(Jar::path(&dir, d()), "not json").unwrap();
    // Opened without FILE_SHARE_DELETE, so the rename fails while it is held (read allowed).
    let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(Jar::path(&dir, d())).unwrap();
    let jar = Jar::load(&dir, d());
    assert!(jar.locked);
    jar.save(&dir, d()).unwrap();
    drop(held);
    assert_eq!(std::fs::read(Jar::path(&dir, d())).unwrap(), b"not json");
}

#[test]
fn load_leaves_a_live_instances_tmp_alone_and_save_leaves_no_tmp() {
    let dir = temp_dir("jar-tmp");
    // This test's own process stands in for another running instance mid-save.
    let live = dir.join(format!("000000000000000d-1.json.{}.tmp", std::process::id()));
    std::fs::write(&live, "theirs").unwrap();
    Jar::load(&dir, d());
    assert_eq!(std::fs::read(&live).unwrap(), b"theirs", "a live instance may be mid-save");
    std::fs::remove_file(&live).unwrap();
    Jar { locked: false, cookies: vec![c("sid", "1")] }.save(&dir, d()).unwrap();
    let names: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(names, ["000000000000000d-1.json"], "the save's own temp file is gone");
}

#[test]
fn load_deletes_a_tmp_whose_process_is_gone() {
    let dir = temp_dir("jar-dead-tmp");
    // A pid no process has (Windows pids are multiples of 4; Linux caps them far below this).
    let dead = dir.join("000000000000000d-1.json.4294967295.tmp");
    let other = dir.join("000000000000000d-1.json.notapid.tmp");
    std::fs::write(&dead, "crashed save").unwrap();
    std::fs::write(&other, "x").unwrap();
    Jar::load(&dir, d());
    assert!(!dead.exists(), "a crashed save's copy of the jar");
    assert!(other.exists(), "only 000000000000000d-1.json.<pid>.tmp files are touched");
    assert!(process_alive(std::process::id()) && !process_alive(4_294_967_295));
}

#[test]
fn a_dead_counter_form_tmp_is_deleted_too() {
    let dir = temp_dir("jar-dead-tmp-n");
    let dead = dir.join("000000000000000d-1.json.4294967295-3.tmp");
    std::fs::write(&dead, "crashed save").unwrap();
    Jar::load(&dir, d());
    assert!(!dead.exists());
}

#[test]
fn only_the_five_newest_unreadable_files_are_kept() {
    let dir = temp_dir("jar-aside-cap");
    for i in 0..7 {
        std::fs::write(Jar::path(&dir, d()), format!("bad{i}")).unwrap();
        Jar::load(&dir, d());
    }
    let mut kept: Vec<_> = asides(&dir).iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
    kept.sort();
    assert_eq!(kept, ["bad2", "bad3", "bad4", "bad5", "bad6"]);
}

#[test]
fn a_missing_file_loads_as_an_empty_jar() {
    assert_eq!(Jar::load(&temp_dir("jar-missing"), d()), Jar::default());
}

#[test]
fn a_corrupt_file_loads_empty_and_is_moved_aside() {
    let dir = temp_dir("jar-corrupt");
    std::fs::write(Jar::path(&dir, d()), "not json").unwrap();
    assert_eq!(Jar::load(&dir, d()), Jar::default());
    assert!(!Jar::path(&dir, d()).exists());
    assert_eq!(std::fs::read(&asides(&dir)[0]).unwrap(), b"not json");
}

#[test]
fn protect_and_unprotect_round_trip() {
    let plain = b"{\"cookies\":[]} some bytes";
    let blob = protect(plain).unwrap();
    assert_eq!(unprotect(&blob).unwrap(), plain);
}

#[test]
fn a_saved_jar_is_protected_on_disk() {
    let dir = temp_dir("jar-protected");
    let jar = Jar { locked: false, cookies: vec![c("sid", "SECRET123")] };
    jar.save(&dir, d()).unwrap();
    let bytes = std::fs::read(Jar::path(&dir, d())).unwrap();
    assert!(bytes.starts_with(b"MJR1"));
    if cfg!(windows) {
        assert!(!String::from_utf8_lossy(&bytes).contains("SECRET123"), "the value must not appear on disk");
    }
    assert_eq!(Jar::load(&dir, d()), jar);
}

#[test]
fn a_legacy_plaintext_jar_loads_and_the_next_save_protects_it() {
    let dir = temp_dir("jar-legacy");
    let jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
    std::fs::write(Jar::path(&dir, d()), serde_json::to_vec(&jar).unwrap()).unwrap();
    assert_eq!(Jar::load(&dir, d()), jar);
    jar.save(&dir, d()).unwrap();
    assert!(std::fs::read(Jar::path(&dir, d())).unwrap().starts_with(b"MJR1"));
}

#[test]
fn a_tampered_protected_file_loads_empty_and_is_kept_aside() {
    let dir = temp_dir("jar-tamper");
    Jar { locked: false, cookies: vec![c("sid", "1")] }.save(&dir, d()).unwrap();
    let mut bytes = std::fs::read(Jar::path(&dir, d())).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(Jar::path(&dir, d()), &bytes).unwrap();
    assert_eq!(Jar::load(&dir, d()), Jar::default());
    assert_eq!(std::fs::read(&asides(&dir)[0]).unwrap(), bytes);
    // A save after the failed load writes a fresh file and leaves the aside copy alone.
    Jar::default().save(&dir, d()).unwrap();
    assert_eq!(asides(&dir).len(), 1);
}

#[test]
fn a_jar_file_is_named_by_its_node_id() {
    let dir = temp_dir("jar-named-file");
    let named = NodeId { actor: ActorId(0xab), counter: 3 };
    assert_eq!(Jar::path(&dir, d()), dir.join("000000000000000d-1.json"));
    assert_eq!(Jar::path(&dir, named), dir.join("00000000000000ab-3.json"));
    let a = Jar { locked: false, cookies: vec![c("sid", "alice")] };
    let b = Jar { locked: false, cookies: vec![c("sid", "bob")] };
    a.save(&dir, d()).unwrap();
    b.save(&dir, named).unwrap();
    assert_eq!(Jar::load(&dir, d()), a);
    assert_eq!(Jar::load(&dir, named), b);
    Jar::delete_files(&dir, named).unwrap();
    assert!(!Jar::path(&dir, named).exists());
    Jar::delete_files(&dir, named).unwrap(); // already gone: fine
    assert_eq!(Jar::load(&dir, d()), a, "the default jar is untouched");
}

/// Every copy of a deleted jar goes, and only that jar's: `…-12` shares `…-1`'s prefix.
#[test]
fn delete_files_takes_only_that_jars_copies() {
    let dir = temp_dir("jar-delete-files");
    let one = NodeId { actor: ActorId(7), counter: 1 };
    let twelve = NodeId { actor: ActorId(7), counter: 12 };
    let (a, b) = (file_name(one), file_name(twelve));
    let gone = [a.clone(), format!("{a}.unreadable-5"), format!("{a}.unreadable-5-1"), format!("{a}.4294967295.tmp")];
    let kept =
        [format!("{a}.{}.tmp", std::process::id()), b.clone(), format!("{b}.unreadable-5"), "default.json".into()];
    for name in gone.iter().chain(&kept) {
        std::fs::write(dir.join(name), "x").unwrap();
    }
    Jar::delete_files(&dir, one).unwrap();
    for name in &gone {
        assert!(!dir.join(name).exists(), "{name}");
    }
    for name in &kept {
        assert!(dir.join(name).exists(), "{name}");
    }
}

#[test]
fn an_unreadable_named_jar_goes_aside_under_its_own_name() {
    let dir = temp_dir("jar-named-aside");
    let named = NodeId { actor: ActorId(1), counter: 2 };
    std::fs::write(Jar::path(&dir, named), "garbage").unwrap();
    assert!(Jar::load(&dir, named).cookies.is_empty());
    let names: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert!(names.iter().any(|n| n.starts_with("0000000000000001-2.json.unreadable-")), "{names:?}");
    assert!(asides(&dir).is_empty(), "no aside for the other jar");
}

#[test]
fn view_json_parses_into_cookies() {
    let json = r#"[{"name":"a","value":"b","domain":"x.com","path":"/","expires":null,"secure":true,"http_only":false,"same_site":"Strict"}]"#;
    let cookies = from_json(json).unwrap();
    assert_eq!(cookies[0].same_site, SameSite::Strict);
    assert!(from_json("nope").is_err());
}

/// M3: a dump that doesn't parse says where, never what. serde's own message would quote the input ("unknown
/// variant `SECRET`"), and a cookie value is a credential. It stays all or nothing: a cookie skipped from a pull
/// would read as a logout and be deleted from the jar.
#[test]
fn a_bad_dump_reports_where_without_echoing_a_value() {
    let good = r#"{"name":"a","value":"b","domain":"x.com","path":"/","expires":null,"secure":true,"http_only":false,"same_site":"Strict"}"#;
    let bad = r#"{"name":"sid","value":"SECRET","domain":"a","path":"/","expires":null,"secure":true,"http_only":true,"same_site":"SECRET"}"#;
    let err = from_json(&format!("[{bad}]")).unwrap_err();
    assert!(err.contains("line 1"), "{err}");
    assert!(!err.contains("SECRET"), "{err}");
    assert!(from_json(&format!("[{good},{bad}]")).is_err(), "all or nothing");
    assert_eq!(from_json(&format!("[{good}]")).map(|c| c.len()), Ok(1));
}

#[test]
fn cookie_state_forget_drops_one_tabs_entries_and_returns_its_hold() {
    let now = std::time::Instant::now();
    let mut state = CookieState::default();
    for (id, url) in [(node(1), "https://a/"), (node(2), "https://b/")] {
        state.jar_tabs.insert(id, (d(), TabCookies::default()));
        state.jar_reseed.insert(id);
        state.cookie_holds.insert(id, (now, crate::tabs::Held::Snapshot(crate::ViewKind::Webview2, url.into())));
        state.jar_pulls.insert(id, now);
        state.jar_silent.insert(id, 1);
    }
    state.jar_dirty.insert(d(), now);
    state.jar_last_pull = Some(now);

    let held = state.forget(node(1));

    assert!(matches!(held, Some((_, crate::tabs::Held::Snapshot(_, s))) if s == "https://a/"));
    assert!(
        !state.jar_tabs.contains_key(&node(1))
            && !state.jar_reseed.contains(&node(1))
            && !state.cookie_holds.contains_key(&node(1))
    );
    assert!(!state.jar_pulls.contains_key(&node(1)) && !state.jar_silent.contains_key(&node(1)));
    assert!(
        state.jar_tabs.contains_key(&node(2))
            && state.jar_reseed.contains(&node(2))
            && state.cookie_holds.contains_key(&node(2))
    );
    assert!(state.jar_pulls.contains_key(&node(2)) && state.jar_silent.contains_key(&node(2)));
    assert!(state.jar_dirty.len() == 1 && state.jar_last_pull.is_some(), "jar-wide state stays");
    assert!(state.forget(node(3)).is_none());
}

#[test]
fn a_jar_file_is_named_by_its_node_id_alone() {
    let id = NodeId { actor: ActorId(3), counter: 7 };
    assert_eq!(Jar::path(Path::new("d"), id), Path::new("d").join(format!("{id}.json")));
}
