//! Where the session lives on disk: the store (`session.mnf`, see
//! `session_sync`) and the named sessions in `sessions/`. The store holds
//! the graph's shape (which nodes, their url, engine, position, group, and
//! who opened whom); `App::restore_session` replays it at startup instead
//! of spawning a single fresh root tab.
//!
//! Only the graph's *shape* is portable across a restart — page state
//! (cookies, localStorage, IndexedDB, scroll position) is not, the same
//! "known v1 limitation" the README already documents for an engine swap;
//! a restored tab is a fresh navigation to its last known url, not a
//! resumed one.

use std::path::PathBuf;

fn session_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("manifold"))
}

/// Where the session store (spec "Journal") lives.
pub fn store_path() -> Option<PathBuf> {
    Some(session_dir()?.join("session.mnf"))
}

/// Where a named "Save Session As..." save lands — a subdirectory of the
/// same app-data folder the session store already lives in,
/// so both are covered by the same "just a file, back it up/delete it
/// yourself" posture. `name` is trusted to already be sanitized by the
/// caller (see `App::sanitize_session_name`) — this only joins it onto the
/// directory.
fn named_sessions_dir() -> Option<PathBuf> {
    Some(session_dir()?.join("sessions"))
}

fn named_session_path(name: &str) -> Option<PathBuf> {
    Some(named_sessions_dir()?.join(format!("{name}.json")))
}

/// Every saved name in `sessions/`, alphabetical — the "Open Session"
/// list. A missing directory (nothing saved yet) is just an empty
/// list, not an error.
pub fn list_named() -> Vec<String> {
    let Some(dir) = named_sessions_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

/// Deletes a named save — the "Open Session" list's own delete button.
pub fn delete_named(name: &str) {
    if let Some(path) = named_session_path(name) {
        let _ = std::fs::remove_file(path);
    }
}
