//! Session-sync tests: swapping a tab's view kind.

use super::*;

/// A swap queues its target when it starts (`start_swap_to`), so a save before the old engine's state
/// arrives writes the target (as `swap_target.unwrap_or(kind)` did before), while the old engine still runs.
#[test]
fn a_swap_saves_its_target_before_the_state_snapshot() {
    let path = temp_dir("engine-swap-start").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Servo));
    app.start_swap_to(tab, ViewKind::Cef);
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef, "shown at once");
    assert!(app.doc.writes.field_queued(tab, field::VIEW_KIND), "queued when the swap starts");
    assert!(sent(&mut peer).iter().any(|m| matches!(m, HostToView::GetState)), "the swap asked for the state");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
    assert_eq!(app.nodes[&tab].as_tab().unwrap().tab.kind, ViewKind::Servo, "the old engine still runs");
    assert_eq!(swap_target(&app, tab), Some(ViewKind::Cef));
}

/// A switch whose new engine fails to start falls back to the old one, and writes it back when the
/// fallback starts; a fallback that fails too leaves a failed card on the old engine.
#[test]
fn a_failed_swap_writes_the_old_view_back() {
    let path = temp_dir("engine-swap-fails").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    app.start_swap_to(tab, ViewKind::Cef);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
    // The old engine's state arrived: its host is dropped and the new one spawns.
    app.finish_swap(tab, ViewKind::Cef, "https://s/".into());
    assert_eq!(app.test_spawns.len(), 1);
    fail_spawns(&mut app);
    let fallback = &app.test_spawns.first().expect("the fallback is in flight").0;
    assert_eq!((fallback.kind, fallback.fallback_kind), (ViewKind::Servo, None));
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Servo);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Servo), "written back as the fallback starts");
    fail_spawns(&mut app);
    assert!(matches!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Failed(_)));
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Servo);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Servo));
}

/// Sleeping mid-swap is reachable: `request_sleep` doesn't wait for a swap, and the page state can answer before the
/// swap's snapshot (the sleep then drops `swap_target` with the live payload). The card keeps the target the mirror
/// holds: it saves it and wakes into it.
#[test]
fn a_tab_put_to_sleep_mid_swap_keeps_the_target() {
    let path = temp_dir("engine-sleep-mid-swap").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    assert!(app.save_session());
    app.start_swap_to(tab, ViewKind::Cef);
    assert!(app.request_sleep(tab, crate::sleep_state::SleepWhy::Manual), "a sleep is not refused mid-swap");
    app.finish_sleep(tab, manifold_view_protocol::PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false });
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Sleeping, "asleep");
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Cef);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
    app.focus_tab(tab);
    assert_eq!(app.test_spawns[0].0.kind, ViewKind::Cef, "wakes into the target");
}

/// A crash mid-swap restarts the tab on the target the mirror holds, with the old engine as its fallback, so the
/// tab runs what the document says either way.
#[test]
fn a_crash_mid_swap_restarts_on_the_target_with_the_old_view_as_fallback() {
    let path = temp_dir("engine-crash-mid-swap").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    assert!(app.save_session());
    app.start_swap_to(tab, ViewKind::Cef);
    app.handle_view_message(tab, manifold_view_protocol::ViewToHost::Crashed { reason: "boom".into() });
    let spawn = &app.test_spawns.first().expect("restarting").0;
    assert_eq!((spawn.kind, spawn.fallback_kind), (ViewKind::Cef, Some(ViewKind::Servo)));
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Servo, "the target failed: back to the old engine");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Servo));
}

/// Once the old engine's snapshot arrives, `swap_target` is gone but the swap is still in flight, parked
/// until the host's cookies arrive (`hold_for_cookies`). Neither a local pick nor another replica's engine starts a
/// second swap then (whose snapshot would never come once the first drops the host), and the tab lands on the engine
/// the document holds.
#[test]
fn a_swap_waiting_for_its_cookies_is_still_the_one_swap() {
    let path = temp_dir("engine-swap-cookie-hold").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, ViewKind::Cef, "https://s/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.start_swap_to(tab, ViewKind::Webview2);
    assert!(app.save_session());
    let snapshot = manifold_view_protocol::TabState { url: "https://s/".into() };
    app.handle_view_message(tab, manifold_view_protocol::ViewToHost::StateSnapshot(snapshot));
    assert!(app.cookies.cookie_holds.contains_key(&tab), "parked for the cookies");
    assert_eq!(swap_target(&app, tab), None);
    app.start_swap_to(tab, ViewKind::Servo);
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Webview2, "a local pick is refused");
    remote_set(&mut other, tab, field::VIEW_KIND, view_kind(ViewKind::Servo));
    assert!(app.save_session());
    assert_eq!(swap_target(&app, tab), None, "no second swap");
    let asked = sent(&mut peer).iter().filter(|m| matches!(m, HostToView::GetState)).count();
    assert_eq!(asked, 1, "the old host was asked once");
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Webview2, "the remote engine is written over");
    app.handle_view_message(tab, manifold_view_protocol::ViewToHost::Cookies("[]".into()));
    let spawn = &app.test_spawns.first().expect("the swap goes on").0;
    assert_eq!((spawn.kind, spawn.fallback_kind), (ViewKind::Webview2, Some(ViewKind::Cef)));
    let _peers = land_spawns(&mut app);
    assert_eq!(app.nodes[&tab].as_tab().unwrap().tab.kind, ViewKind::Webview2, "landed on the target");
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Webview2, "the document holds what runs");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind(ViewKind::Webview2));
}
