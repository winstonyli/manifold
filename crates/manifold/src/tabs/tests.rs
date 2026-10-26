use super::*;
use crate::card_id::testutil::node;
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
    let fatal = format!("{}{why}", manifold_view_protocol::HOST_FATAL);
    assert_eq!(after_crash(&fatal, &mut times, t0), AfterCrash::CannotStart(why));
    assert!(times.is_empty(), "not counted as a crash");
    let mismatch = format!("{}: host 2, engine 3", manifold_view_protocol::PROTOCOL_MISMATCH);
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
        s.skip_convert.insert(node(1), true);
        s.navigated_from.insert(node(1), ("u".into(), now, false));
        s.tab_crashes.insert(node(1), vec![now]);
        s.pending_respawn_focus = Some(node(1));
        s.wakes_in_flight.insert(node(1));
        s.host_loads.insert(node(1), now);
        s.remote_in_flight.insert(node(1), Remote::Deleted);
        s.tab_spawns_in_flight = 2;
        s
    };
    let mut s = filled();
    s.forget(node(2));
    assert!(s.skip_convert.contains_key(&node(1)) && s.pending_respawn_focus.is_some(), "another id's state stays");
    s.forget(node(1));
    assert!(s.skip_convert.is_empty() && s.navigated_from.is_empty() && s.tab_crashes.is_empty());
    assert!(s.pending_respawn_focus.is_none());
    assert!(
        s.wakes_in_flight.contains(&node(1))
            && s.host_loads.contains_key(&node(1))
            && s.remote_in_flight.contains_key(&node(1))
    );

    let mut s = filled();
    s.reset();
    assert!(s.wakes_in_flight.is_empty() && s.host_loads.is_empty() && s.remote_in_flight.is_empty());
    assert!(s.spawning.is_empty() && s.navigated_from.is_empty());
    assert_eq!(s.session_generation, 1);
    assert!(
        s.skip_convert.contains_key(&node(1))
            && s.tab_crashes.contains_key(&node(1))
            && s.pending_respawn_focus.is_some()
    );
    assert_eq!(s.tab_spawns_in_flight, 2);
}
#[test]
fn finish_sleep_without_a_request_does_nothing() {
    let mut app = App::default();
    app.finish_sleep(node(1), PageState { scroll_x: 0.0, scroll_y: 10.0, dirty_form: false });
    assert!(app.pending_sleeps.is_empty() && app.saved_scroll.is_empty());
}

#[test]
fn request_sleep_needs_a_live_tab() {
    let mut app = App::default();
    assert!(!app.request_sleep(node(1), SleepWhy::Manual));
    assert!(app.pending_sleeps.is_empty());
}

#[test]
fn unanswered_requests_are_settled_after_the_timeout() {
    let mut app = App::default();
    let t0 = std::time::Instant::now();
    for (id, why) in [(node(1), SleepWhy::Manual), (node(2), SleepWhy::Idle), (node(3), SleepWhy::JarSwitch)] {
        app.pending_sleeps.insert(id, PendingSleep { why, sent: t0 });
    }
    app.expire_pending_sleeps(t0 + sleep_state::PENDING_TIMEOUT - std::time::Duration::from_millis(1));
    assert_eq!(app.pending_sleeps.len(), 3);
    app.expire_pending_sleeps(t0 + sleep_state::PENDING_TIMEOUT);
    assert!(app.pending_sleeps.is_empty());
}

mod page_text {
    use super::*;
    use crate::process_manager::ViewKind;
    use crate::session_sync::tests::{add_jar, live_tab, Peer};
    use crate::tabs::page_text::{ASK_TIMEOUT, TEXT_DELAY};
    use manifold_view_protocol::HostToView;

    const URL: &str = "https://a.test/";

    fn tab(app: &mut App, url: &str) -> (NodeId, Peer) {
        live_tab(app, ViewKind::Servo, url)
    }

    fn set_url(app: &mut App, id: NodeId, url: &str) {
        app.nodes.get_mut(&id).unwrap().as_tab_mut().unwrap().url = url.into();
    }

    fn text_queries(peer: &mut Peer) -> usize {
        peer.sent().iter().filter(|m| matches!(m, HostToView::QueryPageText)).count()
    }

    #[test]
    fn load_end_arms_a_timer_and_it_fires_after_text_delay() {
        let mut app = App::default();
        let (id, mut peer) = tab(&mut app, URL);
        let t0 = Instant::now();
        app.note_load_end(id, URL, t0);
        app.poll_text_timers(t0 + TEXT_DELAY - Duration::from_millis(1));
        assert_eq!(text_queries(&mut peer), 0);
        app.poll_text_timers(t0 + TEXT_DELAY);
        app.poll_text_timers(t0 + TEXT_DELAY * 2);
        assert_eq!(text_queries(&mut peer), 1, "asked once, then the timer is spent");
    }

    #[test]
    fn a_settled_state_with_no_text_arms_the_timer_without_a_load_end() {
        let mut app = App::default();
        let (id, mut peer) = tab(&mut app, URL);
        let t0 = Instant::now();
        app.note_settled(id, URL, false, t0);
        app.note_settled(id, URL, false, t0 + Duration::from_secs(1));
        app.poll_text_timers(t0 + TEXT_DELAY);
        assert_eq!(text_queries(&mut peer), 1, "armed once; a second settled state does not push it back");
        app.accept_page_text(id, "hello".into());
        app.note_settled(id, URL, false, t0 + TEXT_DELAY * 3);
        app.poll_text_timers(t0 + TEXT_DELAY * 6);
        assert_eq!(text_queries(&mut peer), 0, "text for this url is stored, so a title update asks nothing");
    }

    #[test]
    fn a_navigation_before_the_timer_cancels_it() {
        let mut app = App::default();
        let (id, mut peer) = tab(&mut app, URL);
        let t0 = Instant::now();
        app.note_load_end(id, URL, t0);
        set_url(&mut app, id, "https://b.test/");
        app.poll_text_timers(t0 + TEXT_DELAY);
        assert_eq!(text_queries(&mut peer), 0);
    }

    #[test]
    fn page_text_is_stored_with_the_url_it_was_asked_at() {
        let mut app = App::default();
        let (id, _peer) = tab(&mut app, URL);
        app.ask_page_text(id);
        app.accept_page_text(id, "hello world".into());
        let rec = app.search.texts.get(id).unwrap();
        assert_eq!((rec.text.as_str(), rec.url.as_str()), ("hello world", URL));
    }

    #[test]
    fn page_text_for_a_changed_url_is_dropped() {
        let mut app = App::default();
        let (id, _peer) = tab(&mut app, URL);
        app.ask_page_text(id);
        set_url(&mut app, id, "https://b.test/");
        app.accept_page_text(id, "x".into());
        assert!(app.search.texts.get(id).is_none());
    }

    #[test]
    fn an_unanswered_ask_and_a_closed_tabs_timer_are_forgotten() {
        let mut app = App::default();
        let (silent, _peer) = tab(&mut app, URL);
        let (closed, _peer2) = tab(&mut app, "https://c.test/");
        let t0 = Instant::now();
        app.ask_page_text(silent);
        app.note_load_end(closed, "https://c.test/", t0 + Duration::from_secs(3600));
        app.nodes.remove(&closed);
        app.poll_text_timers(t0);
        assert!(!app.page_text.is_quiet(), "the ask is still awaited inside its window");
        app.poll_text_timers(Instant::now() + ASK_TIMEOUT);
        assert!(app.page_text.is_quiet(), "an empty or non-HTML page that never answers stops holding it");
    }

    #[test]
    fn an_unasked_answer_is_dropped() {
        let mut app = App::default();
        let (id, _peer) = tab(&mut app, URL);
        app.accept_page_text(id, "x".into());
        assert!(app.search.texts.get(id).is_none());
    }

    #[test]
    fn manifold_pages_and_non_http_urls_are_not_wanted() {
        let mut app = App::default();
        for (url, wanted) in [
            ("manifold://start", false),
            ("file:///x", false),
            ("about:blank", false),
            (URL, true),
            ("HTTP://A.TEST/", true),
        ] {
            let (id, _peer) = tab(&mut app, url);
            assert_eq!(app.text_wanted(id), wanted, "{url}");
        }
    }

    #[test]
    fn an_ephemeral_jar_tab_is_not_wanted() {
        let mut app = App::default();
        let (id, mut peer) = tab(&mut app, URL);
        let jar = add_jar(&mut app, "Tmp");
        app.set_jar_edge(id, Some(jar));
        assert!(app.text_wanted(id));
        app.set_jar_ephemeral(jar, true);
        assert!(!app.text_wanted(id));
        app.ask_page_text(id);
        app.accept_page_text(id, "x".into());
        assert!(app.search.texts.get(id).is_none());
        assert_eq!(text_queries(&mut peer), 0);
    }

    #[test]
    fn sleep_asks_for_text_first_when_the_stored_text_is_for_another_url() {
        let mut app = App::default();
        let (id, mut peer) = tab(&mut app, URL);
        assert!(app.request_sleep(id, SleepWhy::Manual));
        let sent = peer.sent();
        let text = sent.iter().position(|m| matches!(m, HostToView::QueryPageText));
        let state = sent.iter().position(|m| matches!(m, HostToView::QueryPageState));
        assert!(text.is_some() && text < state, "{sent:?}");
    }

    #[test]
    fn sleep_does_not_ask_when_the_stored_text_is_current() {
        let mut app = App::default();
        let (id, mut peer) = tab(&mut app, URL);
        app.ask_page_text(id);
        app.accept_page_text(id, "x".into());
        let _ = peer.sent();
        assert!(app.request_sleep(id, SleepWhy::Manual));
        assert_eq!(text_queries(&mut peer), 0);
    }
}
