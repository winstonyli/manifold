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

/// Where the session store (spec "Journal") lives.
pub fn store_path() -> Option<PathBuf> {
    Some(crate::profile::dir()?.join("session.mnf"))
}

/// Where a named "Save Session As..." save lands — a subdirectory of the
/// same app-data folder the session store already lives in,
/// so both are covered by the same "just a file, back it up/delete it
/// yourself" posture. `name` is trusted to already be sanitized by the
/// caller (see `App::sanitize_session_name`) — this only joins it onto the
/// directory. A named save is a full store file (spec "Named sessions"):
/// same `.mnf`/`.mnf.local`/`.mnf.prev` layout as the running session's
/// own `session.mnf`, just under a name instead of the fixed one.
fn named_sessions_dir() -> Option<PathBuf> {
    Some(crate::profile::dir()?.join("sessions"))
}

pub(crate) fn named_session_path(name: &str) -> Option<PathBuf> {
    Some(named_sessions_dir()?.join(format!("{name}.mnf")))
}

/// Every saved name in `sessions/`, alphabetical — the "Open Session"
/// list. A missing directory (nothing saved yet) is just an empty
/// list, not an error. Only `.mnf` stems count, so a stray `.mnf.local`/
/// `.mnf.prev` beside one doesn't show up as its own entry.
pub fn list_named() -> Vec<String> {
    let Some(dir) = named_sessions_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "mnf"))
        .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

/// Deletes a named save — the "Open Session" list's own delete button.
/// Removes the store file and its personal-state/previous-copy
/// companions; a missing one is not an error.
pub fn delete_named(name: &str) {
    let Some(path) = named_session_path(name) else { return };
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(crate::store::journal::with_suffix(&path, ".local"));
    let _ = std::fs::remove_file(crate::store::journal::with_suffix(&path, ".prev"));
}
