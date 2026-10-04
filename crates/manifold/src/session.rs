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
/// yourself" posture. A name only joins onto it through
/// `named_session_path`, which refuses one that could leave it
/// (`session_name_is_safe`). A named save is a full store file (spec "Named sessions"):
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

/// The longest name `session_name_is_safe` accepts, in chars; with `sessions\`
/// and `.mnf.<suffix>` it stays well inside Windows' 255-unit file name.
const MAX_SESSION_NAME: usize = 200;

/// Whether `name` can be a file stem in `sessions/` on every platform: not
/// empty, `.`/`..` or over `MAX_SESSION_NAME` chars; no separator, drive
/// or stream colon, control character or other character Windows refuses
/// (`<>"|?*`); not a Windows device name (`is_windows_device_name`).
/// "Save As" names are already filtered by `sanitize_session_name`, which
/// lets a device name through; this guards every way in (`OPENSESSION`).
pub(crate) fn session_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().count() <= MAX_SESSION_NAME
        && !name.chars().any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
        && !is_windows_device_name(name)
}

/// `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, or `COM`/`LPT` with a
/// digit or superscript digit, in any case, judged on the part before the
/// first dot with trailing spaces dropped (`nul.txt` and `CON ` open the
/// device too).
fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    matches!(prefix.as_str(), "COM" | "LPT")
        && matches!(rest.as_slice(), [c] if c.is_ascii_digit() || matches!(c, '\u{b9}' | '\u{b2}' | '\u{b3}'))
}

/// `None` without a profile, or for a name that isn't `session_name_is_safe`.
pub(crate) fn named_session_path(name: &str) -> Option<PathBuf> {
    if !session_name_is_safe(name) {
        return None;
    }
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

/// Deletes a named save: the store file and every file the store keeps
/// beside it (`<name>.mnf.*`: personal state, `.prev`, lock, temps and
/// `.damaged-*` copies, which are whole sessions too). A missing one is
/// not an error. A `<name>.mnf.<more>.mnf` is another named save
/// (`list_named` shows it), so it and its own `.mnf.*` files stay.
pub(crate) fn delete_session_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let (Some(dir), Some(file)) = (path.parent(), path.file_name().and_then(|f| f.to_str())) else { return };
    let ours = |n: &str| {
        n.strip_prefix(file)
            .is_some_and(|tail| tail.starts_with('.') && !tail.ends_with(".mnf") && !tail.contains(".mnf."))
    };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_str().is_some_and(ours) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The "Open Session" list's delete, after its confirm
/// (`App::build_session_delete_popup`).
pub fn delete_named(name: &str) {
    if let Some(path) = named_session_path(name) {
        delete_session_files(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;

    #[test]
    fn deleting_a_session_removes_every_copy_and_nothing_else() {
        let dir = temp_dir("delete-session");
        let names = [
            "a.mnf",
            "a.mnf.local",
            "a.mnf.prev",
            "a.mnf.lock",
            "a.mnf.123-4.tmp",
            "a.mnf.damaged-00ff",
            "ab.mnf",
            "a b.mnf",
            "a.mnf.old.mnf",
            "a.mnf.old.mnf.local",
            "a.mnf.mnf",
        ];
        for n in names {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        delete_session_files(&dir.join("a.mnf"));
        let left: std::collections::BTreeSet<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        let kept = ["a b.mnf", "a.mnf.mnf", "a.mnf.old.mnf", "a.mnf.old.mnf.local", "ab.mnf"];
        assert_eq!(left, kept.map(String::from).into(), "other saves and their files are kept");
    }

    #[test]
    fn a_session_name_cannot_leave_the_sessions_folder() {
        let longest = "x".repeat(200);
        for ok in ["work", "a b", "x-1_2", "名前", "console", "COM10", "LPTX", "nul-ish", "CONIN", "a.con", &longest]
        {
            assert!(session_name_is_safe(ok), "{ok}");
        }
        let too_long = "x".repeat(201);
        let bad_names = [
            "",
            ".",
            "..",
            "..\\..\\x",
            "../x",
            "a/b",
            "c:x",
            "a\0b",
            "a\tb",
            "a<b",
            "a>b",
            "a\"b",
            "a|b",
            "a?b",
            "a*b",
            "CON",
            "con",
            "Prn",
            "aux",
            "NUL",
            "nul.txt",
            "CON.backup",
            "com1",
            "COM9",
            "lpt1",
            "LPT9",
            "COM\u{b9}",
            "lpt\u{b2}",
            "COM\u{b3}",
            "CON ",
            "LPT0",
            "conin$",
            "CONOUT$",
            &too_long,
        ];
        for bad in bad_names {
            assert!(!session_name_is_safe(bad), "{bad:?}");
        }
    }
}
