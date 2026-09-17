//! Bookmarks: a flat list of `(title, url)` pairs, persisted to
//! `%APPDATA%\multiengine-browser\bookmarks.json` — same "missing/corrupt
//! file just means empty, never fails to start" posture `settings.rs`/
//! `session.rs` already have. No folders/tags in this first cut, just a
//! flat list; the toolbar dropdown that shows them is small enough that
//! organizing it further hasn't been worth the extra state yet.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
}

fn bookmarks_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("multiengine-browser").join("bookmarks.json"))
}

/// Missing file, unreadable/corrupt JSON, or no `%APPDATA%` at all all just
/// return an empty list — same fallback posture as `Session::load`.
pub fn load() -> Vec<Bookmark> {
    let Some(path) = bookmarks_path() else { return Vec::new() };
    load_from(&path).unwrap_or_default()
}

/// Same read as `load`, just from an arbitrary path — the "Import
/// Bookmarks..." settings-popup button reads back whatever "Export
/// Bookmarks..." wrote, same shape `Session::load_from`/`save_to` already
/// have for named sessions.
pub fn load_from(path: &Path) -> Option<Vec<Bookmark>> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
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
