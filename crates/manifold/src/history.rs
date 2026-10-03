//! Browsing history: a list of `(title, url)` entries, persisted to
//! `%APPDATA%\manifold\history.json` — same "missing/corrupt
//! file just means empty, never fails to start" posture `settings.rs`/
//! `bookmarks.rs` already have (a corrupt one is moved aside first).

use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub title: String,
    pub url: String,
}

/// How many history entries to keep before dropping the oldest.
pub const MAX_HISTORY_ENTRIES: usize = 200;

fn history_path() -> Option<PathBuf> {
    Some(crate::profile::dir()?.join("history.json"))
}

/// Missing file, unreadable/corrupt JSON, or no `%APPDATA%` at all all just
/// give an empty list — same fallback as `bookmarks::load`, including moving
/// an unreadable file aside and reporting it as the error half.
pub fn load() -> (Vec<HistoryEntry>, Option<String>) {
    let Some(path) = history_path() else { return (Vec::new(), None) };
    match crate::json_file::load_or_move_aside(&path, "history") {
        Ok(history) => {
            let mut entries: Vec<HistoryEntry> = history.unwrap_or_default();
            cap(&mut entries);
            (entries, None)
        }
        Err(err) => (Vec::new(), Some(err)),
    }
}

/// Drops the oldest entries past `MAX_HISTORY_ENTRIES`, in case the file
/// grew beyond the limit. The list is most-recent-last, like `App::history`.
fn cap(entries: &mut Vec<HistoryEntry>) {
    if entries.len() > MAX_HISTORY_ENTRIES {
        entries.drain(..entries.len() - MAX_HISTORY_ENTRIES);
    }
}

pub fn save(history: &[HistoryEntry]) {
    let Some(path) = history_path() else { return };
    crate::json_file::save(history, &path, "history");
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

    /// The list is most-recent-last, so a cap keeps the newest entries.
    #[test]
    fn capping_keeps_the_newest_entries() {
        let entry = |i: usize| HistoryEntry { title: String::new(), url: format!("https://{i}/") };
        let mut entries: Vec<HistoryEntry> = (0..MAX_HISTORY_ENTRIES + 5).map(entry).collect();
        cap(&mut entries);
        assert_eq!(entries.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(entries[0].url, entry(5).url);
        assert_eq!(entries.last().unwrap().url, entry(MAX_HISTORY_ENTRIES + 4).url);
    }
}
