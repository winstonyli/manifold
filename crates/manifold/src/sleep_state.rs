//! Pure parts of two-phase sleep (`App::request_sleep` / `finish_sleep`) and
//! of scroll restore after a wake (`App::saved_scroll`).

use crate::store::model::NodeId;
use manifold_view_protocol::PageState;
use std::time::{Duration, Instant};

/// Why a tab is being put to sleep: the user asked, the idle sweep did, or its jar changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SleepWhy {
    Manual,
    Idle,
    /// The tab's jar changed while it ran (spec "Live tabs when `jar` changes"); follows the idle rules.
    JarSwitch,
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

/// Only a manual sleep discards a dirty form: it is an explicit act; idle sleep and a jar switch wait.
pub(crate) fn verdict(why: SleepWhy, state: &PageState) -> Verdict {
    if why != SleepWhy::Manual && state.dirty_form {
        Verdict::Stay
    } else {
        Verdict::Sleep
    }
}

/// What a live tab does about its jar (`jar_switch`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JarSwitch {
    Nothing,
    SleepNow,
    Wait,
}

/// The spec's "Live tabs when `jar` changes" table, before the dirty-form check (`verdict`). `spawned_with` is the
/// running host's jar (`None`: no host, so it will spawn into `wanted`). `reseed`: the host must be replaced although
/// its key is unchanged (its jar was cleared).
pub(crate) fn jar_switch(
    spawned_with: Option<NodeId>,
    wanted: NodeId,
    reseed: bool,
    busy: bool,
    focused: bool,
) -> JarSwitch {
    match spawned_with {
        Some(had) if had != wanted || reseed => {
            if busy || focused {
                JarSwitch::Wait
            } else {
                JarSwitch::SleepNow
            }
        }
        _ => JarSwitch::Nothing,
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
    use manifold_view_protocol::PageState;

    fn state(dirty: bool) -> PageState {
        PageState { scroll_x: 0.0, scroll_y: 10.0, dirty_form: dirty }
    }

    /// Only a manual sleep discards a dirty form.
    #[test]
    fn only_idle_sleep_stays_awake_for_a_dirty_form() {
        assert_eq!(verdict(SleepWhy::Idle, &state(true)), Verdict::Stay);
        assert_eq!(verdict(SleepWhy::Idle, &state(false)), Verdict::Sleep);
        assert_eq!(verdict(SleepWhy::Manual, &state(true)), Verdict::Sleep);
    }

    #[test]
    fn the_jar_switch_table() {
        use crate::store::model::{ActorId, NodeId};
        let work = NodeId { actor: ActorId(1), counter: 1 };
        let d = NodeId { actor: ActorId(1), counter: 2 };
        assert_eq!(jar_switch(None, work, false, false, false), JarSwitch::Nothing, "asleep or not spawned yet");
        assert_eq!(jar_switch(None, work, true, false, false), JarSwitch::Nothing, "no host to reseed");
        assert_eq!(jar_switch(Some(work), work, false, false, false), JarSwitch::Nothing, "already on it");
        assert_eq!(jar_switch(Some(work), work, true, false, false), JarSwitch::SleepNow, "its jar was cleared");
        assert_eq!(jar_switch(Some(d), work, false, false, false), JarSwitch::SleepNow);
        assert_eq!(jar_switch(Some(d), work, false, true, false), JarSwitch::Wait, "busy");
        assert_eq!(jar_switch(Some(d), work, false, false, true), JarSwitch::Wait, "focused");
        assert_eq!(jar_switch(Some(work), work, true, true, false), JarSwitch::Wait, "a busy reseed waits too");
    }

    #[test]
    fn a_jar_switch_stays_awake_for_a_dirty_form() {
        assert_eq!(verdict(SleepWhy::JarSwitch, &state(true)), Verdict::Stay);
        assert_eq!(verdict(SleepWhy::JarSwitch, &state(false)), Verdict::Sleep);
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
