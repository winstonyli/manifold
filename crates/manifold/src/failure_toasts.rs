//! Failure toasts that share a cause read as one: ten tabs failing because an engine is missing is one message with a
//! count, not ten toasts. A cause is an engine plus an error text; the count is distinct intents (the url, engine
//! and group asked for), so one intent failing again and again is one failure, not many tabs. Pure and clock-free
//! (`now` is passed in) so the window is testable.

use std::time::{Duration, Instant};

/// How long after the first notice a burst is held before it is shown.
pub(crate) const WINDOW: Duration = Duration::from_millis(1500);

/// The toast line (`App::toasts`): the one showing, the ones waiting, and spawn failures held to merge by cause.
#[derive(Default)]
pub(crate) struct Toasts {
    /// A brief toast message (full text, not just a path -- see `build_
    /// save_toast`'s own doc comment) for the most recent completed
    /// download/save or crash/respawn notice, and when it was first drawn —
    /// cleared once `SAVE_TOAST_DURATION` has passed, at which point the
    /// next queued message (`save_toast_queue`) takes over, if any. Was
    /// "latest wins" with no queue at all until this session, when a
    /// second toast (e.g. a crash notice) arriving while a download's own
    /// "Saved <path>" toast was still showing was confirmed to silently
    /// replace it, unlike `FindState::result`'s own "latest wins" (correct
    /// there -- a stale search count would be actively wrong, whereas a
    /// dropped one-time notification like "tab crashed" is just gone).
    pub(crate) save_toast: Option<(String, std::time::Instant)>,
    /// See `save_toast`'s own doc comment -- messages waiting to be shown,
    /// one at a time in order, each after the previous one's
    /// `SAVE_TOAST_DURATION` elapses. Pushed to by `queue_toast` (and
    /// directly by `DownloadCompleted`, for borrow reasons).
    pub(crate) save_toast_queue: std::collections::VecDeque<String>,
    /// Spawn-failure toasts held to merge by cause (`failure_toasts`).
    pub(crate) failure_notices: FailureNotices,
    /// A toast with an Undo (or Redo) button: the message it shows on, the history entry it steps, and whether it
    /// redoes (`App::step_history`).
    pub(crate) undo: Option<(String, u64, bool)>,
}

/// One cause's burst: its intents (distinct, first-seen order) and the first message, shown when there is one intent.
struct Held {
    view_kind: String,
    error: String,
    intents: Vec<String>,
    message: String,
}

#[derive(Default)]
pub(crate) struct FailureNotices {
    first_at: Option<Instant>,
    /// First-seen order.
    held: Vec<Held>,
}

impl FailureNotices {
    /// Notes a failure of `intent` on `view_kind` with `error`; `message` is what one such failure reads as.
    pub(crate) fn push(&mut self, now: Instant, view_kind: &str, error: &str, intent: &str, message: String) {
        self.first_at.get_or_insert(now);
        match self.held.iter_mut().find(|h| h.view_kind == view_kind && h.error == error) {
            Some(h) if !h.intents.iter().any(|i| i == intent) => h.intents.push(intent.to_owned()),
            Some(_) => {}
            None => self.held.push(Held {
                view_kind: view_kind.to_owned(),
                error: error.to_owned(),
                intents: vec![intent.to_owned()],
                message,
            }),
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.first_at.is_some()
    }

    /// The messages to show now: empty until `WINDOW` has passed since the first held notice.
    pub(crate) fn flush(&mut self, now: Instant) -> Vec<String> {
        let Some(first) = self.first_at else { return Vec::new() };
        if now.duration_since(first) < WINDOW {
            return Vec::new();
        }
        self.first_at = None;
        std::mem::take(&mut self.held)
            .into_iter()
            .map(|h| match h.intents.len() {
                1 => h.message,
                n => format!("{n} tabs couldn't start: {}", h.error),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_failure_reads_as_its_own_message_after_the_window() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "Servo", "no engine", "a", "Couldn't start Servo for a: no engine".into());
        assert!(notices.flush(t0 + WINDOW / 2).is_empty(), "held inside the window");
        assert_eq!(notices.flush(t0 + WINDOW), ["Couldn't start Servo for a: no engine"]);
        assert!(!notices.is_pending());
    }

    #[test]
    fn distinct_intents_of_one_cause_are_one_message_with_a_count() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        for i in 0..10 {
            let intent = format!("u{i}");
            notices.push(
                t0 + Duration::from_millis(i * 50),
                "CEF",
                "Blink (CEF) is not installed",
                &intent,
                format!("m{i}"),
            );
        }
        assert_eq!(notices.flush(t0 + WINDOW), ["10 tabs couldn't start: Blink (CEF) is not installed"]);
    }

    #[test]
    fn one_intent_failing_three_times_is_its_own_message() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        for _ in 0..3 {
            notices.push(t0, "Servo", "no engine", "a", "Couldn't start Servo for a: no engine".into());
        }
        assert_eq!(notices.flush(t0 + WINDOW), ["Couldn't start Servo for a: no engine"]);
    }

    #[test]
    fn the_same_text_on_two_view_kinds_is_two_messages() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "Servo", "boom", "a", "ma".into());
        notices.push(t0, "CEF", "boom", "a", "mb".into());
        assert_eq!(notices.flush(t0 + WINDOW), ["ma", "mb"]);
    }

    #[test]
    fn three_distinct_intents_read_as_three_tabs() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        for intent in ["a", "b", "c"] {
            notices.push(t0, "Servo", "boom", intent, format!("m{intent}"));
        }
        assert_eq!(notices.flush(t0 + WINDOW), ["3 tabs couldn't start: boom"]);
    }

    #[test]
    fn two_causes_are_two_messages_in_first_seen_order() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "Servo", "a", "u1", "ma".into());
        notices.push(t0, "Servo", "b", "u1", "mb".into());
        notices.push(t0, "Servo", "a", "u2", "ma2".into());
        assert_eq!(notices.flush(t0 + WINDOW), ["2 tabs couldn't start: a", "mb"]);
    }

    #[test]
    fn a_failure_after_a_flush_starts_a_new_window() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "Servo", "a", "u", "ma".into());
        assert_eq!(notices.flush(t0 + WINDOW).len(), 1);
        let later = t0 + WINDOW * 3;
        notices.push(later, "Servo", "a", "u", "again".into());
        assert!(notices.flush(later).is_empty());
        assert_eq!(notices.flush(later + WINDOW), ["again"]);
    }
}
