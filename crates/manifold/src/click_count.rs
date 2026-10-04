//! Click counting for page input: engines need a press's click count
//! (double-click selects a word, fires `dblclick`), and the window system
//! only delivers single presses.

use std::time::{Duration, Instant};

use ipc_protocol::MouseButton;

/// Windows' default double-click time.
pub(crate) const MULTI_CLICK_TIME: Duration = Duration::from_millis(500);
/// How far (physical px) a press may land from the last and still count.
pub(crate) const MULTI_CLICK_SLOP: f64 = 4.0;

#[derive(Debug, Default)]
pub(crate) struct ClickCounter {
    last: Option<(MouseButton, (f64, f64), Instant)>,
    count: u8,
}

impl ClickCounter {
    /// Records a press and returns its click count: 1, or one more than the
    /// last press when it was the same button, close by and soon after.
    pub(crate) fn press(&mut self, button: MouseButton, pos: (f64, f64), now: Instant) -> u8 {
        let continues = self.last.is_some_and(|(b, p, t)| {
            b == button
                && now.duration_since(t) <= MULTI_CLICK_TIME
                && (p.0 - pos.0).abs() <= MULTI_CLICK_SLOP
                && (p.1 - pos.1).abs() <= MULTI_CLICK_SLOP
        });
        self.count = if continues { self.count.saturating_add(1) } else { 1 };
        self.last = Some((button, pos, now));
        self.count
    }

    /// The count of the last press, for its release.
    pub(crate) fn current(&self) -> u8 {
        self.count.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_presses_in_one_place_count_up() {
        let mut clicks = ClickCounter::default();
        let t = Instant::now();
        assert_eq!(clicks.press(MouseButton::Left, (10.0, 10.0), t), 1);
        assert_eq!(clicks.press(MouseButton::Left, (12.0, 9.0), t + Duration::from_millis(200)), 2);
        assert_eq!(clicks.press(MouseButton::Left, (12.0, 9.0), t + Duration::from_millis(400)), 3);
        assert_eq!(clicks.current(), 3);
    }

    #[test]
    fn slow_far_or_other_button_presses_start_over() {
        let mut clicks = ClickCounter::default();
        let t = Instant::now();
        clicks.press(MouseButton::Left, (0.0, 0.0), t);
        assert_eq!(clicks.press(MouseButton::Left, (0.0, 0.0), t + Duration::from_millis(600)), 1, "slow");
        assert_eq!(clicks.press(MouseButton::Left, (20.0, 0.0), t + Duration::from_millis(700)), 1, "far");
        assert_eq!(clicks.press(MouseButton::Right, (20.0, 0.0), t + Duration::from_millis(800)), 1, "other button");
    }
}
