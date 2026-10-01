//! Pure parts of two-phase sleep (`App::request_sleep` / `finish_sleep`) and
//! of scroll restore after a wake (`App::saved_scroll`).

// Nothing here is called until Task 5 wires it into `App`.
#![allow(dead_code)] // used by Task 5 wiring

use ipc_protocol::PageState;
use std::time::{Duration, Instant};

/// Why a tab is being put to sleep: the user asked, or the idle sweep did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SleepWhy {
    Manual,
    Idle,
}

/// A `QueryPageState` sent and not yet answered.
pub(crate) struct PendingSleep {
    pub(crate) why: SleepWhy,
    pub(crate) sent: Instant,
}

/// How long a query may go unanswered (engine busy, dead, or unable to answer).
pub(crate) const PENDING_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Sleep,
    Stay,
}

/// Idle sleep never discards a dirty form; a manual Sleep is an explicit act.
pub(crate) fn verdict(why: SleepWhy, state: &PageState) -> Verdict {
    if why == SleepWhy::Idle && state.dirty_form {
        Verdict::Stay
    } else {
        Verdict::Sleep
    }
}

pub(crate) fn expired(p: &PendingSleep, now: Instant) -> bool {
    now.duration_since(p.sent) >= PENDING_TIMEOUT
}

/// A sleeping tab's scroll position, waiting for its woken page's first load.
pub(crate) struct SavedScroll {
    pub(crate) x: f64,
    pub(crate) y: f64,
    seen_loading: bool,
}

impl SavedScroll {
    pub(crate) fn new(x: f64, y: f64) -> SavedScroll {
        SavedScroll { x, y, seen_loading: false }
    }

    /// Feed each `NavigationStateChanged`'s `loading`; true once a load has
    /// started and finished (a not-yet-started engine reports `false` too).
    pub(crate) fn on_loading(&mut self, loading: bool) -> bool {
        if loading {
            self.seen_loading = true;
            false
        } else {
            self.seen_loading
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_protocol::PageState;

    fn state(dirty: bool) -> PageState {
        PageState { scroll_x: 0.0, scroll_y: 10.0, dirty_form: dirty }
    }

    #[test]
    fn only_idle_sleep_stays_awake_for_a_dirty_form() {
        assert_eq!(verdict(SleepWhy::Idle, &state(true)), Verdict::Stay);
        assert_eq!(verdict(SleepWhy::Idle, &state(false)), Verdict::Sleep);
        assert_eq!(verdict(SleepWhy::Manual, &state(true)), Verdict::Sleep);
    }

    #[test]
    fn a_request_expires_after_the_timeout() {
        let t0 = Instant::now();
        let p = PendingSleep { why: SleepWhy::Manual, sent: t0 };
        assert!(!expired(&p, t0 + PENDING_TIMEOUT - Duration::from_millis(1)));
        assert!(expired(&p, t0 + PENDING_TIMEOUT));
    }

    #[test]
    fn restore_is_due_when_a_load_has_started_and_finished() {
        let mut s = SavedScroll::new(0.0, 300.0);
        assert!(!s.on_loading(false), "the engine's initial idle state is not the load");
        assert!(!s.on_loading(true));
        assert!(s.on_loading(false));
    }
}
