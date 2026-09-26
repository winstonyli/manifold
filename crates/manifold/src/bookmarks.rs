//! Bookmarks: a flat list of `(title, url, folder)` entries, persisted to
//! `%APPDATA%\manifold\bookmarks.json` — same "missing/corrupt
//! file just means empty, never fails to start" posture `settings.rs`/
//! `session.rs` already have (a corrupt one is moved aside first). `folder`
//! is a single optional level, not a
//! real tree -- the toolbar dropdown groups by it (unfoldered entries
//! first, then each named folder), but a folder can't contain another
//! folder.

use std::path::{Path, PathBuf};

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

fn bookmarks_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("manifold").join("bookmarks.json"))
}

/// Missing file, unreadable/corrupt JSON, or no `%APPDATA%` at all all just
/// give an empty list — same fallback as `Settings::load`, including moving
/// an unreadable file aside and reporting it as the `Err` half.
pub fn load() -> (Vec<Bookmark>, Option<String>) {
    let Some(path) = bookmarks_path() else { return (Vec::new(), None) };
    match crate::json_file::load_or_move_aside(&path, "bookmarks") {
        Ok(bookmarks) => (bookmarks.unwrap_or_default(), None),
        Err(err) => (Vec::new(), Some(err)),
    }
}

/// Same read as `load`, just from an arbitrary path — the "Import
/// Bookmarks..." settings-popup button reads back whatever "Export
/// Bookmarks..." wrote.
pub fn load_from(path: &Path) -> Result<Vec<Bookmark>, String> {
    crate::json_file::load(path, "bookmarks")
}

pub fn save(bookmarks: &[Bookmark]) {
    let Some(path) = bookmarks_path() else { return };
    save_to(bookmarks, &path);
}

/// Same write as `save`, just to an arbitrary path.
pub fn save_to(bookmarks: &[Bookmark], path: &Path) {
    if let Some(dir) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(dir) {
            eprintln!("bookmarks: failed to create {}: {err}", dir.display());
            return;
        }
    }
    match serde_json::to_string_pretty(bookmarks) {
        Ok(json) => {
            if let Err(err) = std::fs::write(path, json) {
                eprintln!("bookmarks: failed to write {}: {err}", path.display());
            }
        }
        Err(err) => eprintln!("bookmarks: failed to serialize: {err}"),
    }
}
