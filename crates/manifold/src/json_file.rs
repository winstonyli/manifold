//! Loading the app's JSON files (`session.json`, `settings.json`,
//! `bookmarks.json`, and imported/named copies of them) with errors that
//! say which file failed and why, ready for a toast.

use std::path::Path;

use serde::de::DeserializeOwned;

/// Reads and parses `path`. `what` names the file's kind in the error
/// ("session", "settings", ...).
pub fn load<T: DeserializeOwned>(path: &Path, what: &str) -> Result<T, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|err| format!("Couldn't read {what} {}: {err}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|err| format!("Couldn't load {what} {}: {err}", path.display()))
}

/// For a file the app auto-saves over: `Ok(None)` when it doesn't exist
/// (a fresh start). One that exists but can't be loaded is renamed to
/// `<stem>.unreadable-<unix-seconds>.json` and comes back as `Err`. Left in
/// place, the first save after the fresh start would overwrite it: the
/// fingerprint guards in `App::save_session`/`save_settings` only cover a
/// file this run has loaded.
pub fn load_or_move_aside<T: DeserializeOwned>(path: &Path, what: &str) -> Result<Option<T>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let err = match load(path, what) {
        Ok(value) => return Ok(Some(value)),
        Err(err) => err,
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(what);
    let aside = path.with_file_name(format!("{stem}.unreadable-{secs}.json"));
    match std::fs::rename(path, &aside) {
        Ok(()) => Err(format!("{err} -- moved it to {}", aside.display())),
        Err(rename_err) => Err(format!("{err} (and couldn't move it aside: {rename_err})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_file_is_moved_aside() {
        let dir = std::env::temp_dir().join(format!("manifold-json-file-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("thing.json");
        assert_eq!(load_or_move_aside::<Vec<u32>>(&path, "thing"), Ok(None));
        std::fs::write(&path, "[1, 2").unwrap();
        let err = load_or_move_aside::<Vec<u32>>(&path, "thing").unwrap_err();
        assert!(err.contains("moved it to"), "{err}");
        assert!(!path.exists());
        let aside: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].to_string_lossy().starts_with("thing.unreadable-"));
        std::fs::write(&path, "[1, 2]").unwrap();
        assert_eq!(load_or_move_aside::<Vec<u32>>(&path, "thing"), Ok(Some(vec![1, 2])));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
