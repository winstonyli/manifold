//! Idle auto-sleep: which live tabs have been out of sight long enough to put
//! back to sleep (`App::request_sleep`). The clock is `IdleClock::seen`: each
//! tab's last moment on screen (or first sight, for a tab that never was),
//! kept up to date by `IdleClock::sweep` on every call, whether or not the
//! setting is on, so turning it on never sleeps a tab for time spent before.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How often `App::drain_tab_events` calls `IdleClock::sweep`.
pub(crate) const SWEEP_EVERY: Duration = Duration::from_secs(10);

/// What `IdleClock::sweep` needs to know about one live tab.
pub(crate) struct TabIdleView<'a> {
    pub(crate) id: &'a str,
    /// Shown in Tab view or the split pane right now.
    pub(crate) on_screen: bool,
    pub(crate) audio_playing: bool,
    pub(crate) pinned: bool,
    pub(crate) loading: bool,
}

#[derive(Default)]
pub(crate) struct IdleClock {
    seen: HashMap<String, Instant>,
    last_sweep: Option<Instant>,
}

impl IdleClock {
    /// Whether `SWEEP_EVERY` has passed since the last `sweep` (always, the first time).
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.last_sweep.is_none_or(|t| now.duration_since(t) >= SWEEP_EVERY)
    }

    /// Updates every tab's clock and returns the ids idle for at least
    /// `limit` (none when `limit` is `None`). A tab on screen, playing
    /// audio, pinned or still loading counts as seen now, so it never
    /// qualifies; tabs no longer in `tabs` are forgotten.
    pub(crate) fn sweep<'a>(
        &mut self,
        now: Instant,
        limit: Option<Duration>,
        tabs: impl IntoIterator<Item = TabIdleView<'a>>,
    ) -> Vec<String> {
        self.last_sweep = Some(now);
        let mut live = std::collections::HashSet::new();
        let mut idle = Vec::new();
        for tab in tabs {
            live.insert(tab.id.to_string());
            let busy = tab.on_screen || tab.audio_playing || tab.pinned || tab.loading;
            let seen = self.seen.entry(tab.id.to_string()).or_insert(now);
            if busy {
                *seen = now;
            } else if limit.is_some_and(|l| now.duration_since(*seen) >= l) {
                idle.push(tab.id.to_string());
            }
        }
        self.seen.retain(|id, _| live.contains(id));
        idle
    }

    /// Restarts `id`'s clock at `now` (a tab that was kept awake, so it is
    /// not asked again every sweep).
    pub(crate) fn restart(&mut self, id: &str, now: Instant) {
        self.seen.insert(id.to_string(), now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str) -> TabIdleView<'_> {
        TabIdleView { id, on_screen: false, audio_playing: false, pinned: false, loading: false }
    }

    const LIMIT: Option<Duration> = Some(Duration::from_secs(60));

    #[test]
    fn a_tab_sleeps_only_after_the_limit_out_of_sight() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        assert!(clock.sweep(t0, LIMIT, [tab("a")]).is_empty());
        assert!(clock.sweep(t0 + Duration::from_secs(59), LIMIT, [tab("a")]).is_empty());
        assert_eq!(clock.sweep(t0 + Duration::from_secs(60), LIMIT, [tab("a")]), vec!["a"]);
    }

    #[test]
    fn restart_resets_the_clock() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        clock.sweep(t0, LIMIT, [tab("a")]);
        clock.restart("a", t0 + Duration::from_secs(50));
        assert!(clock.sweep(t0 + Duration::from_secs(100), LIMIT, [tab("a")]).is_empty());
        assert_eq!(clock.sweep(t0 + Duration::from_secs(110), LIMIT, [tab("a")]), vec!["a"]);
    }

    #[test]
    fn busy_tabs_restart_their_clock() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        clock.sweep(t0, LIMIT, [tab("on"), tab("audio"), tab("pin"), tab("load")]);
        let later = t0 + Duration::from_secs(90);
        let views = [
            TabIdleView { on_screen: true, ..tab("on") },
            TabIdleView { audio_playing: true, ..tab("audio") },
            TabIdleView { pinned: true, ..tab("pin") },
            TabIdleView { loading: true, ..tab("load") },
        ];
        assert!(clock.sweep(later, LIMIT, views).is_empty());
        // The clock restarted at `later`, not at `t0`.
        assert!(clock.sweep(later + Duration::from_secs(30), LIMIT, [tab("on")]).is_empty());
        assert_eq!(clock.sweep(later + Duration::from_secs(60), LIMIT, [tab("on")]), vec!["on"]);
    }

    #[test]
    fn off_never_sleeps_but_keeps_the_clock_so_turning_on_counts_prior_idle_time() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        clock.sweep(t0, None, [tab("a")]);
        assert!(clock.sweep(t0 + Duration::from_secs(600), None, [tab("a")]).is_empty());
        assert_eq!(clock.sweep(t0 + Duration::from_secs(601), LIMIT, [tab("a")]), vec!["a"]);
    }

    #[test]
    fn a_tab_that_left_and_came_back_starts_over() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        clock.sweep(t0, LIMIT, [tab("a")]);
        // Asleep (not a live tab) for a sweep, then woken: first seen again.
        clock.sweep(t0 + Duration::from_secs(30), LIMIT, []);
        assert!(clock.sweep(t0 + Duration::from_secs(100), LIMIT, [tab("a")]).is_empty());
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
