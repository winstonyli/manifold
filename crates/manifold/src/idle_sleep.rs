//! Idle auto-sleep: which live tabs have been out of sight long enough to put
//! back to sleep (`App::request_sleep`). The clock is `IdleClock::seen`: each
//! tab's last moment on screen (or first sight, for a tab that never was),
//! kept up to date by `IdleClock::sweep` on every call, whether or not the
//! setting is on, so turning it on never sleeps a tab for time spent before.

use crate::store::model::NodeId;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How often `App::drain_tab_events` calls `IdleClock::sweep`.
pub(crate) const SWEEP_EVERY: Duration = Duration::from_secs(10);

/// What `IdleClock::sweep` needs to know about one live tab.
pub(crate) struct TabIdleView {
    pub(crate) id: NodeId,
    /// Shown in Tab view or the split pane right now.
    pub(crate) on_screen: bool,
    pub(crate) audio_playing: bool,
    pub(crate) pinned: bool,
    pub(crate) loading: bool,
    /// A download this tab started has not finished (its card is still filling).
    pub(crate) downloading: bool,
}

impl TabIdleView {
    /// On screen, playing audio, pinned, loading or downloading: never put to sleep by a sweep or a jar switch.
    pub(crate) fn busy(&self) -> bool {
        self.on_screen || self.audio_playing || self.pinned || self.loading || self.downloading
    }
}

#[derive(Default)]
pub(crate) struct IdleClock {
    seen: HashMap<NodeId, Instant>,
    last_sweep: Option<Instant>,
}

impl IdleClock {
    /// Whether `SWEEP_EVERY` has passed since the last `sweep` (always, the first time).
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.last_sweep.is_none_or(|t| now.duration_since(t) >= SWEEP_EVERY)
    }

    /// Updates every tab's clock and returns the ids idle for at least
    /// `limit` (none when `limit` is `None`). A tab on screen, playing
    /// audio, pinned, still loading or downloading counts as seen now, so it never
    /// qualifies; tabs no longer in `tabs` are forgotten.
    pub(crate) fn sweep(
        &mut self,
        now: Instant,
        limit: Option<Duration>,
        tabs: impl IntoIterator<Item = TabIdleView>,
    ) -> Vec<NodeId> {
        self.last_sweep = Some(now);
        let mut live = std::collections::HashSet::new();
        let mut idle = Vec::new();
        for tab in tabs {
            live.insert(tab.id);
            let seen = self.seen.entry(tab.id).or_insert(now);
            if tab.busy() {
                *seen = now;
            } else if limit.is_some_and(|l| now.duration_since(*seen) >= l) {
                idle.push(tab.id);
            }
        }
        self.seen.retain(|id, _| live.contains(id));
        idle
    }

    /// Restarts `id`'s clock at `now` (a tab that was kept awake, so it is
    /// not asked again every sweep).
    pub(crate) fn restart(&mut self, id: NodeId, now: Instant) {
        self.seen.insert(id, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::testutil::node;

    fn tab(id: NodeId) -> TabIdleView {
        TabIdleView { id, on_screen: false, audio_playing: false, pinned: false, loading: false, downloading: false }
    }

    const LIMIT: Option<Duration> = Some(Duration::from_secs(60));

    #[test]
    fn a_tab_sleeps_only_after_the_limit_out_of_sight() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        assert!(clock.sweep(t0, LIMIT, [tab(node(1))]).is_empty());
        assert!(clock.sweep(t0 + Duration::from_secs(59), LIMIT, [tab(node(1))]).is_empty());
        assert_eq!(clock.sweep(t0 + Duration::from_secs(60), LIMIT, [tab(node(1))]), vec![node(1)]);
    }

    #[test]
    fn restart_resets_the_clock() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        clock.sweep(t0, LIMIT, [tab(node(1))]);
        clock.restart(node(1), t0 + Duration::from_secs(50));
        assert!(clock.sweep(t0 + Duration::from_secs(100), LIMIT, [tab(node(1))]).is_empty());
        assert_eq!(clock.sweep(t0 + Duration::from_secs(110), LIMIT, [tab(node(1))]), vec![node(1)]);
    }

    #[test]
    fn busy_tabs_restart_their_clock() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        clock.sweep(t0, LIMIT, [tab(node(2)), tab(node(3)), tab(node(4)), tab(node(5)), tab(node(6))]);
        let later = t0 + Duration::from_secs(90);
        let views = [
            TabIdleView { on_screen: true, ..tab(node(2)) },
            TabIdleView { audio_playing: true, ..tab(node(3)) },
            TabIdleView { pinned: true, ..tab(node(4)) },
            TabIdleView { loading: true, ..tab(node(5)) },
            TabIdleView { downloading: true, ..tab(node(6)) },
        ];
        assert!(clock.sweep(later, LIMIT, views).is_empty());
        // The clock restarted at `later`, not at `t0`.
        assert!(clock.sweep(later + Duration::from_secs(30), LIMIT, [tab(node(2))]).is_empty());
        assert_eq!(clock.sweep(later + Duration::from_secs(60), LIMIT, [tab(node(2))]), vec![node(2)]);
    }

    #[test]
    fn busy_is_any_reason_to_stay_awake() {
        let quiet = tab(node(1));
        assert!(!quiet.busy());
        assert!(TabIdleView { on_screen: true, ..tab(node(1)) }.busy());
        assert!(TabIdleView { audio_playing: true, ..tab(node(1)) }.busy());
        assert!(TabIdleView { pinned: true, ..tab(node(1)) }.busy());
        assert!(TabIdleView { loading: true, ..tab(node(1)) }.busy());
        assert!(TabIdleView { downloading: true, ..tab(node(1)) }.busy());
    }

    #[test]
    fn off_never_sleeps_but_keeps_the_clock_so_turning_on_counts_prior_idle_time() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        clock.sweep(t0, None, [tab(node(1))]);
        assert!(clock.sweep(t0 + Duration::from_secs(600), None, [tab(node(1))]).is_empty());
        assert_eq!(clock.sweep(t0 + Duration::from_secs(601), LIMIT, [tab(node(1))]), vec![node(1)]);
    }

    #[test]
    fn a_tab_that_left_and_came_back_starts_over() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        clock.sweep(t0, LIMIT, [tab(node(1))]);
        // Asleep (not a live tab) for a sweep, then woken: first seen again.
        clock.sweep(t0 + Duration::from_secs(30), LIMIT, []);
        assert!(clock.sweep(t0 + Duration::from_secs(100), LIMIT, [tab(node(1))]).is_empty());
    }

    #[test]
    fn due_every_sweep_interval() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        assert!(clock.due(t0));
        clock.sweep(t0, None, []);
        assert!(!clock.due(t0 + SWEEP_EVERY - Duration::from_millis(1)));
        assert!(clock.due(t0 + SWEEP_EVERY));
    }
}
