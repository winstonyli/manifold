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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::local_state::LocalState;
use crate::store::journal::Journal;

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
pub(crate) fn named_sessions_dir() -> Option<PathBuf> {
    Some(crate::profile::dir()?.join("sessions"))
}

/// What the named sessions in `dir` still use of the content dir, for
/// `content::kept`: each one's live nodes' ids and `path` file names, and
/// its Recently Closed entries' `path` file names. `None` when any save or
/// its personal state can't be read whole, so pruning never runs against
/// a partial list; a missing `dir` names nothing.
pub(crate) fn content_names(dir: &Path) -> Option<HashSet<String>> {
    let mut names = HashSet::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Some(names),
        Err(_) => return None,
    };
    for entry in entries {
        let path = entry.ok()?.path();
        if path.extension().is_none_or(|ext| ext != "mnf") {
            continue;
        }
        let doc = Journal::read_intact(&std::fs::read(&path).ok()?).ok()??;
        let closed = match crate::store::local::read(&path, doc.id).ok()? {
            Some(bytes) => LocalState::from_bytes(&bytes)?.closed,
            None => Vec::new(),
        };
        let live = crate::store::json::doc_to_json(&doc).nodes;
        for node in &live {
            names.insert(node.id.clone());
        }
        let paths = live.iter().chain(closed.iter().flat_map(|c| &c.nodes)).filter_map(|n| n.fields.get("path"));
        names.extend(paths.filter_map(|p| p.as_str()).filter_map(crate::content::file_name));
    }
    Some(names)
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
