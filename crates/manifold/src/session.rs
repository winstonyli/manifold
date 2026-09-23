//! Session persistence: the tab graph's shape (which tabs, their url,
//! engine, position, and who opened whom), which tab is focused, and which
//! view is active are saved to disk on every change that matters and
//! reloaded at startup — `App::restore_session` replays it instead of
//! always spawning a single fresh root tab, so quitting and relaunching
//! picks up roughly where it left off. Like `settings.rs`'s own file, a
//! missing or corrupt one just falls back to the old single-root-tab
//! behavior rather than failing to start.
//!
//! Only the graph's *shape* is portable across a restart — page state
//! (cookies, localStorage, IndexedDB, scroll position) is not, the same
//! "known v1 limitation" the README already documents for an engine swap;
//! a restored tab is a fresh navigation to its last known url, not a
//! resumed one.

use std::path::{Path, PathBuf};

use crate::process_manager::{engine_kind_serde, EngineKind};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionTab {
    pub id: String,
    pub parent: Option<String>,
    pub url: String,
    #[serde(with = "engine_kind_serde")]
    pub engine: EngineKind,
    /// Graph-view layout position — restoring it keeps a familiar-looking
    /// layout across a restart instead of every tab re-jittering from the
    /// same spawn point and re-settling from scratch.
    pub pos_x: f32,
    pub pos_y: f32,
}

/// A sticky note's own saved data -- see `crate::NotePayload`'s own doc
/// comment for why a note is persisted at all (unlike most other per-node
/// display state, which is session-only).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionNote {
    pub id: String,
    pub text: String,
    pub pos_x: f32,
    pub pos_y: f32,
}

/// An image node's own saved data -- see `crate::ImagePayload`'s own doc
/// comment for why only the file `path` is kept (not the picture's own
/// bytes, and not `width`/`height` -- those are re-derived by decoding the
/// file again at restore, same as a note's `SessionNote` doesn't duplicate
/// anything `NotePayload` can hold directly).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionImage {
    pub id: String,
    pub path: String,
    pub pos_x: f32,
    pub pos_y: f32,
}

/// A PDF node's own saved data -- same "just the file path" posture
/// `SessionImage` already has, for the same reason: `width`/`height`/
/// `page_count` are all re-derived by rendering the file's first page
/// again at restore, not duplicated here.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionPdf {
    pub id: String,
    pub path: String,
    pub pos_x: f32,
    pub pos_y: f32,
}

/// A JSON node's own saved data -- same "just the file path" posture
/// `SessionImage`/`SessionPdf` already have, for the same reason: the
/// parsed `serde_json::Value` is re-derived by reading and reparsing the
/// file again at restore, not duplicated here.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionJson {
    pub id: String,
    pub path: String,
    pub pos_x: f32,
    pub pos_y: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    /// `#[serde(default)]` so a `session.json` written before notes existed
    /// (an empty `Vec`, not a missing field) still loads cleanly.
    #[serde(default)]
    pub notes: Vec<SessionNote>,
    #[serde(default)]
    pub images: Vec<SessionImage>,
    #[serde(default)]
    pub pdfs: Vec<SessionPdf>,
    #[serde(default)]
    pub jsons: Vec<SessionJson>,
    pub focused: Option<String>,
    /// `true` = `ViewMode::Item` (named before that variant was renamed
    /// from `Tab`; kept so existing `session.json` files still load).
    pub view_mode_tab: bool,
    pub next_tab_id: u64,
    #[serde(default)]
    pub next_note_id: u64,
    #[serde(default)]
    pub next_image_id: u64,
    #[serde(default)]
    pub next_pdf_id: u64,
    #[serde(default)]
    pub next_json_id: u64,
}

/// Cheap, process-local "did this file's bytes change" check -- not a
/// cryptographic or cross-run-stable hash, just `DefaultHasher` (SipHash)
/// over the raw file contents, good enough to notice that some *other*
/// process touched `session.json` since this one last read or wrote it.
/// See `fingerprint`'s own doc comment for why this exists at all.
fn content_fingerprint(contents: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

/// A fingerprint of the auto-restore `session.json` exactly as it sits on
/// disk right now -- `None` if it doesn't exist or can't be read. `App`
/// compares this against what it saw at last load/save, right before
/// `save()` would otherwise overwrite the file, so a second process (or a
/// second `manifold.exe` instance) that's touched this same file in the
/// meantime is noticed instead of silently clobbered -- confirmed live:
/// this project's own concurrent testing sessions did exactly that to each
/// other, with zero warning, before this existed.
pub fn fingerprint() -> Option<u64> {
    let contents = std::fs::read_to_string(session_path()?).ok()?;
    Some(content_fingerprint(&contents))
}

/// Where a save that lost a fingerprint race lands instead of overwriting
/// the real `session.json` -- see `App::save_session`'s own doc comment.
/// Unix-seconds-suffixed, the same collision-avoidance convention this
/// project's own screenshot (`manifold-<unix-seconds>.png`) and crash-log
/// (`manifold-crash-<unix-seconds>.log`) paths already use -- a fixed name
/// would let a *second* conflict later in the same session silently
/// overwrite the first one's still-unreviewed data, exactly the failure
/// mode this whole mechanism exists to prevent.
pub fn conflict_path() -> Option<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(session_dir()?.join(format!("session.conflict-{secs}.json")))
}

fn session_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("manifold"))
}

fn session_path() -> Option<PathBuf> {
    Some(session_dir()?.join("session.json"))
}

/// Where a named "Save Session As..." save lands — a subdirectory of the
/// same app-data folder the auto-restore `session.json` already lives in,
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

impl Session {
    /// Missing file, unreadable/corrupt JSON, or no `%APPDATA%` at all all
    /// just return `None` — the caller falls back to spawning a single
    /// fresh root tab, same posture as `Settings::load`'s own fallback.
    pub fn load() -> Option<Self> {
        Self::load_from(&session_path()?)
    }

    pub fn save(&self) {
        let Some(path) = session_path() else {
            return;
        };
        self.save_to(&path);
    }

    /// Same read as `load`, just from an arbitrary path — the named-session
    /// "Open Session" list reads back whatever `save_as` wrote.
    pub fn load_from(path: &Path) -> Option<Self> {
        let contents = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&contents).ok()
    }

    /// Same write as `save`, just to an arbitrary path.
    pub fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(dir) {
                eprintln!("session: failed to create {}: {err}", dir.display());
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(err) = std::fs::write(path, json) {
                    eprintln!("session: failed to write {}: {err}", path.display());
                }
            }
            Err(err) => eprintln!("session: failed to serialize: {err}"),
        }
    }

    /// "Save Session As `name`" — writes to `sessions/<name>.json` rather
    /// than the auto-restore `session.json`, so a named save never gets
    /// silently overwritten by ordinary "every change that matters"
    /// auto-saving the way `session.json` itself is.
    pub fn save_as(&self, name: &str) {
        let Some(path) = named_session_path(name) else { return };
        self.save_to(&path);
    }

    /// Loads a previously named-saved session back, by name.
    pub fn load_named(name: &str) -> Option<Self> {
        Self::load_from(&named_session_path(name)?)
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
}
