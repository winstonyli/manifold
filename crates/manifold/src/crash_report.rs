//! Minimal crash reporting -- no external crate needed. `std::panic::
//! set_hook` plus `std::backtrace::Backtrace::force_capture()` (stable
//! since Rust 1.65) already give a real, symbolized backtrace, since this
//! workspace already keeps `debug = "line-tables-only"` for exactly this
//! purpose (see the root `Cargo.toml`'s own comment). A dedicated crate
//! (`human-panic`, `color-eyre`, etc.) would just be dressing this up for
//! a one-file local log nobody but a developer reads.

use std::sync::Mutex;

/// A cheap, human-readable snapshot of what the app was doing, refreshed
/// once per frame (see `record_state`) so the panic hook -- which has no
/// access to `App` itself, panic hooks are global -- has *something*
/// better than a bare backtrace to go on. Deliberately just a formatted
/// `String`, not a struct: nothing here is read back programmatically,
/// only ever pasted into a bug report by a human.
static LAST_STATE: Mutex<Option<String>> = Mutex::new(None);

/// Call once at startup, before the event loop runs.
pub fn install() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Still prints the normal message to the console -- this adds a
        // file alongside it, it doesn't replace the existing behavior.
        default_hook(info);
        write_report(info);
    }));
}

/// Refreshes `LAST_STATE` -- call once per frame. Cheap (one string
/// format), matching this app's own "already redraws every frame, so cost
/// per frame must stay negligible" posture elsewhere.
pub fn record_state(state: impl std::fmt::Display) {
    if let Ok(mut guard) = LAST_STATE.lock() {
        *guard = Some(state.to_string());
    }
}

fn write_report(info: &std::panic::PanicHookInfo<'_>) {
    let backtrace = std::backtrace::Backtrace::force_capture();
    let state = LAST_STATE
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| "(no state captured yet)".to_string());
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let report = format!(
        "Manifold crash report -- unix time {timestamp}\n\n{info}\n\nLast known state:\n{state}\n\nBacktrace:\n{backtrace}\n"
    );
    let path = std::env::temp_dir().join(format!("manifold-crash-{timestamp}.log"));
    let _ = std::fs::write(&path, report);
}
