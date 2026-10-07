//! Failure toasts that share a cause read as one: ten tabs failing because an engine is missing is one message with a
//! count, not ten toasts. Pure and clock-free (`now` is passed in) so the window is testable.

use std::time::{Duration, Instant};

/// How long after the first notice a burst is held before it is shown.
pub(crate) const WINDOW: Duration = Duration::from_millis(1500);

#[derive(Default)]
pub(crate) struct FailureNotices {
    first_at: Option<Instant>,
    /// `(cause, how many, the message when there was exactly one)`, first-seen order.
    held: Vec<(String, usize, String)>,
}

impl FailureNotices {
    pub(crate) fn push(&mut self, now: Instant, cause: &str, message: String) {
        self.first_at.get_or_insert(now);
        match self.held.iter_mut().find(|(c, ..)| c == cause) {
            Some((_, count, _)) => *count += 1,
            None => self.held.push((cause.to_owned(), 1, message)),
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
            .map(
                |(cause, count, message)| {
                    if count == 1 {
                        message
                    } else {
                        format!("{count} tabs couldn't start: {cause}")
                    }
                },
            )
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
        notices.push(t0, "no engine", "Couldn't start Servo for a: no engine".into());
        assert!(notices.flush(t0 + WINDOW / 2).is_empty(), "held inside the window");
        assert_eq!(notices.flush(t0 + WINDOW), ["Couldn't start Servo for a: no engine"]);
        assert!(!notices.is_pending());
    }

    #[test]
    fn same_cause_failures_are_one_message_with_a_count() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        for i in 0..10 {
            notices.push(t0 + Duration::from_millis(i * 50), "Blink (CEF) is not installed", format!("m{i}"));
        }
        assert_eq!(notices.flush(t0 + WINDOW), ["10 tabs couldn't start: Blink (CEF) is not installed"]);
    }

    #[test]
    fn two_causes_are_two_messages_in_first_seen_order() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "a", "ma".into());
        notices.push(t0, "b", "mb".into());
        notices.push(t0, "a", "ma2".into());
        assert_eq!(notices.flush(t0 + WINDOW), ["2 tabs couldn't start: a", "mb"]);
    }

    #[test]
    fn a_failure_after_a_flush_starts_a_new_window() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "a", "ma".into());
        assert_eq!(notices.flush(t0 + WINDOW).len(), 1);
        let later = t0 + WINDOW * 3;
        notices.push(later, "a", "again".into());
        assert!(notices.flush(later).is_empty());
        assert_eq!(notices.flush(later + WINDOW), ["again"]);
    }
}
