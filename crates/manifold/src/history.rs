//! Browsing history: a list of `(title, url)` entries, persisted to
//! `%APPDATA%\manifold\history.json` — same "missing/corrupt
//! file just means empty, never fails to start" posture `settings.rs`/
//! `bookmarks.rs` already have (a corrupt one is moved aside first).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub title: String,
    pub url: String,
}

/// How many history entries to keep before dropping the oldest.
pub const MAX_HISTORY_ENTRIES: usize = 200;

fn history_path() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        let appdata = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(appdata).join("manifold").join("history.json"))
    }
    #[cfg(test)]
    {
        // Tests must not read or write the real %APPDATA%\manifold\history.json.
        // Callers use round-trip tests on JSON strings, not the file path.
        None
    }
}

/// Missing file, unreadable/corrupt JSON, or no `%APPDATA%` at all all just
/// give an empty list — same fallback as `bookmarks::load`, including moving
/// an unreadable file aside and reporting it as the error half.
pub fn load() -> (Vec<HistoryEntry>, Option<String>) {
    let Some(path) = history_path() else { return (Vec::new(), None) };
    match crate::json_file::load_or_move_aside(&path, "history") {
        Ok(history) => {
            let mut entries: Vec<HistoryEntry> = history.unwrap_or_default();
            // Truncate to MAX_HISTORY_ENTRIES in case it grew beyond our limit.
            if entries.len() > MAX_HISTORY_ENTRIES {
                entries.truncate(MAX_HISTORY_ENTRIES);
            }
            (entries, None)
        }
        Err(err) => (Vec::new(), Some(err)),
    }
}

pub fn save(history: &[HistoryEntry]) {
    let Some(path) = history_path() else { return };
    save_to(history, &path);
}

fn save_to(history: &[HistoryEntry], path: &Path) {
    if let Some(dir) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(dir) {
            eprintln!("history: failed to create {}: {err}", dir.display());
            return;
        }
    }
    match serde_json::to_string_pretty(history) {
        Ok(json) => {
            if let Err(err) = std::fs::write(path, json) {
                eprintln!("history: failed to write {}: {err}", path.display());
            }
        }
        Err(err) => eprintln!("history: failed to serialize: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_through_json() {
        let entries = vec![HistoryEntry { title: "A".into(), url: "https://a.example/".into() }];
        let text = serde_json::to_string(&entries).unwrap();
        let back: Vec<HistoryEntry> = serde_json::from_str(&text).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].url, "https://a.example/");
    }
}
