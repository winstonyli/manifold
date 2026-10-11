//! The scripts' `check` and `timing_mark`: `PASS:`/`FAIL:` lines, a failure count that becomes the exit code, and
//! `TIMING` lines for `scripts/gate-timing.sh`.
//!
//! `scripts/gate.sh` counts the lines containing `PASS` or `FAIL` in a stage's log, so nothing else the runner prints
//! may contain either word (section names included).

use std::fmt::Display;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static FAILURES: AtomicU32 = AtomicU32::new(0);

/// `PASS: label` (scripts/lib.sh `check`).
pub fn pass_line(label: &str) -> String {
    format!("PASS: {label}")
}

/// `FAIL: label: got [got], wanted [want]` (scripts/lib.sh `check`).
pub fn fail_eq_line(label: &str, want: &dyn Display, got: &dyn Display) -> String {
    format!("FAIL: {label}: got [{got}], wanted [{want}]")
}

/// `FAIL: label` (headless-smoke-test.sh `failed`, a check with no got/wanted pair).
pub fn fail_line(label: &str) -> String {
    format!("FAIL: {label}")
}

/// Prints `PASS: label` or `FAIL: label`; a failure is counted and the run goes on (the scripts use `set -u`, not
/// `-e`).
pub fn check(label: &str, ok: bool) -> bool {
    if ok {
        println!("{}", pass_line(label));
    } else {
        println!("{}", fail_line(label));
        FAILURES.fetch_add(1, Ordering::SeqCst);
    }
    ok
}

/// `check` of `want == got`, printing both on a failure.
pub fn check_eq<T: PartialEq + Display>(label: &str, want: T, got: T) -> bool {
    let ok = want == got;
    if ok {
        println!("{}", pass_line(label));
    } else {
        println!("{}", fail_eq_line(label, &want, &got));
        FAILURES.fetch_add(1, Ordering::SeqCst);
    }
    ok
}

/// Counts a failure the runner detected itself (a timeout, a launch error) after printing its own `FAIL:` line.
pub fn note_failure() {
    FAILURES.fetch_add(1, Ordering::SeqCst);
}

pub fn failures() -> u32 {
    FAILURES.load(Ordering::SeqCst)
}

/// The process exit code: 0 when no check failed.
pub fn exit_code() -> std::process::ExitCode {
    std::process::ExitCode::from(u8::from(failures() > 0))
}

/// `TIMING <name> <secs>.<ms> <start epoch>`, the format of scripts/timing.sh `timing_mark`: whole seconds, then
/// the milliseconds truncated (not rounded) to 3 digits, then the step's start in whole epoch seconds.
pub fn timing_line(name: &str, elapsed: Duration, start_epoch: u64) -> String {
    let us = elapsed.as_micros();
    format!("TIMING {name} {}.{:03} {start_epoch}", us / 1_000_000, us % 1_000_000 / 1000)
}

/// A timed step: prints its `TIMING` line when dropped (also on a panic or an early return, like the scripts'
/// EXIT-trap `timing_mark`). `Section::start("a")` then `.next("b")` ends "a" and opens "b".
pub struct Section {
    name: String,
    t0: Instant,
    epoch: u64,
}

impl Section {
    /// `name` is one word and must not contain PASS or FAIL.
    pub fn start(name: &str) -> Section {
        debug_assert!(!name.contains("PASS") && !name.contains("FAIL") && !name.contains(char::is_whitespace));
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Section { name: name.to_string(), t0: Instant::now(), epoch }
    }

    /// Ends this step (printing it) and opens `name`.
    pub fn next(self, name: &str) -> Section {
        drop(self);
        Section::start(name)
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        println!("{}", timing_line(&self.name, self.t0.elapsed(), self.epoch));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_match_the_scripts() {
        assert_eq!(pass_line("a card"), "PASS: a card");
        assert_eq!(fail_line("a card"), "FAIL: a card");
        assert_eq!(fail_eq_line("count", &3, &5), "FAIL: count: got [5], wanted [3]");
        assert_eq!(fail_eq_line("url", &"a", &""), "FAIL: url: got [], wanted [a]");
    }

    #[test]
    fn timing_line_truncates_to_milliseconds() {
        assert_eq!(
            timing_line("setup", Duration::from_micros(1_234_999), 1_760_000_000),
            "TIMING setup 1.234 1760000000"
        );
        assert_eq!(timing_line("run1-link", Duration::from_micros(61_000_000), 5), "TIMING run1-link 61.000 5");
        assert_eq!(timing_line("x", Duration::from_micros(999), 0), "TIMING x 0.000 0");
        assert_eq!(timing_line("x", Duration::from_micros(5_000), 0), "TIMING x 0.005 0");
    }

    #[test]
    fn a_failed_check_is_counted() {
        let before = failures();
        assert!(check("passes", true));
        assert_eq!(failures(), before);
        assert!(!check_eq("fails on purpose in the unit test", 1, 2));
        assert!(failures() > before);
    }
}
