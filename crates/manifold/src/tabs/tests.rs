use super::*;
use crate::session_sync::Remote;
use std::time::{Duration, Instant};

#[test]
fn the_third_crash_within_the_window_gives_up() {
    let t0 = Instant::now();
    let mut times = Vec::new();
    assert!(!crashes_too_often(&mut times, t0));
    assert!(!crashes_too_often(&mut times, t0 + Duration::from_secs(1)));
    assert!(crashes_too_often(&mut times, t0 + Duration::from_secs(2)));
}

#[test]
fn a_host_that_cannot_start_is_not_restarted_and_says_why() {
    let t0 = Instant::now();
    let mut times = Vec::new();
    let why = "WebView2 couldn't create its environment (0x80070005: Access is denied.)";
    let fatal = format!("{}{why}", ipc_protocol::HOST_FATAL);
    assert_eq!(after_crash(&fatal, &mut times, t0), AfterCrash::CannotStart(why));
    assert!(times.is_empty(), "not counted as a crash");
    let mismatch = format!("{}: host 2, engine 3", ipc_protocol::PROTOCOL_MISMATCH);
    assert_eq!(after_crash(&mismatch, &mut times, t0), AfterCrash::GiveUp);
    // A plain crash still restarts, up to the cap.
    let lost = "view-host connection lost";
    assert_eq!(after_crash(lost, &mut times, t0), AfterCrash::Restart);
    assert_eq!(after_crash(lost, &mut times, t0 + Duration::from_secs(1)), AfterCrash::Restart);
    assert_eq!(after_crash(lost, &mut times, t0 + Duration::from_secs(2)), AfterCrash::GiveUp);
}

#[test]
fn crashes_spread_past_the_window_keep_respawning() {
    let t0 = Instant::now();
    let mut times = Vec::new();
    for i in 0..10 {
        assert!(!crashes_too_often(&mut times, t0 + CRASH_WINDOW * i), "crash {i}");
    }
}

/// `SpawnTracker::forget` takes what `forget_id` cleared by hand (`skip_convert`, `navigated_from`,
/// `tab_crashes`, a matching `pending_respawn_focus`) and `reset` what `teardown_running_session` cleared (the
/// spawns in flight, `host_loads`, `navigated_from`) plus a generation bump; neither touches the rest.
#[test]
fn forget_and_reset_clear_what_the_old_sites_cleared() {
    let now = std::time::Instant::now();
    let filled = || {
        let mut s = SpawnTracker::default();
        s.skip_convert.insert("t".into(), true);
        s.navigated_from.insert("t".into(), ("u".into(), now, false));
        s.tab_crashes.insert("t".into(), vec![now]);
        s.pending_respawn_focus = Some("t".into());
        s.wakes_in_flight.insert("t".into());
        s.host_loads.insert("t".into(), now);
        s.remote_in_flight.insert("t".into(), Remote::Deleted);
        s.tab_spawns_in_flight = 2;
        s
    };
    let mut s = filled();
    s.forget("u");
    assert!(s.skip_convert.contains_key("t") && s.pending_respawn_focus.is_some(), "another id's state stays");
    s.forget("t");
    assert!(s.skip_convert.is_empty() && s.navigated_from.is_empty() && s.tab_crashes.is_empty());
    assert!(s.pending_respawn_focus.is_none());
    assert!(s.wakes_in_flight.contains("t") && s.host_loads.contains_key("t") && s.remote_in_flight.contains_key("t"));

    let mut s = filled();
    s.reset();
    assert!(s.wakes_in_flight.is_empty() && s.host_loads.is_empty() && s.remote_in_flight.is_empty());
    assert!(s.spawning.is_empty() && s.navigated_from.is_empty());
    assert_eq!(s.session_generation, 1);
    assert!(s.skip_convert.contains_key("t") && s.tab_crashes.contains_key("t") && s.pending_respawn_focus.is_some());
    assert_eq!(s.tab_spawns_in_flight, 2);
}
#[test]
fn finish_sleep_without_a_request_does_nothing() {
    let mut app = App::default();
    app.finish_sleep("t1", PageState { scroll_x: 0.0, scroll_y: 10.0, dirty_form: false });
    assert!(app.pending_sleeps.is_empty() && app.saved_scroll.is_empty());
}

#[test]
fn request_sleep_needs_a_live_tab() {
    let mut app = App::default();
    assert!(!app.request_sleep("missing", SleepWhy::Manual));
    assert!(app.pending_sleeps.is_empty());
}

#[test]
fn unanswered_requests_are_settled_after_the_timeout() {
    let mut app = App::default();
    let t0 = std::time::Instant::now();
    for (id, why) in [("m", SleepWhy::Manual), ("i", SleepWhy::Idle), ("j", SleepWhy::JarSwitch)] {
        app.pending_sleeps.insert(id.into(), PendingSleep { why, sent: t0 });
    }
    app.expire_pending_sleeps(t0 + sleep_state::PENDING_TIMEOUT - std::time::Duration::from_millis(1));
    assert_eq!(app.pending_sleeps.len(), 3);
    app.expire_pending_sleeps(t0 + sleep_state::PENDING_TIMEOUT);
    assert!(app.pending_sleeps.is_empty());
}
