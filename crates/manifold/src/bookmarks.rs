//! Bookmarks: a flat list of `(title, url, folder)` entries, persisted to
//! `%APPDATA%\manifold\bookmarks.json` — same "missing/corrupt
//! file just means empty, never fails to start" posture `settings.rs`/
//! `session.rs` already have (a corrupt one is moved aside first). `folder`
//! is a single optional level, not a
//! real tree -- the toolbar dropdown groups by it (unfoldered entries
//! first, then each named folder), but a folder can't contain another
//! folder.

use std::path::Path;

use crate::json_file::ProfileFile;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    /// `None` (or missing entirely in an older `bookmarks.json`, via
    /// `#[serde(default)]`) means unfoldered -- shown first in the
    /// dropdown, ahead of every named folder.
    #[serde(default)]
    pub folder: Option<String>,
}

const FILE: ProfileFile = ProfileFile { file: "bookmarks.json", what: "bookmarks" };

/// Missing file, unreadable/corrupt JSON, or no `%APPDATA%` at all all just
/// give an empty list (`ProfileFile::load`).
pub fn load() -> (Vec<Bookmark>, Option<String>) {
    FILE.load()
}

/// Same read as `load`, just from an arbitrary path — the "Import
/// Bookmarks..." settings-popup button reads back whatever "Export
/// Bookmarks..." wrote.
pub fn load_from(path: &Path) -> Result<Vec<Bookmark>, String> {
    crate::json_file::load(path, FILE.what)
}

pub fn save(bookmarks: &[Bookmark]) {
    FILE.save(bookmarks);
}

/// Same write as `save`, just to an arbitrary path.
pub fn save_to(bookmarks: &[Bookmark], path: &Path) {
    crate::json_file::save(bookmarks, path, FILE.what);
}
