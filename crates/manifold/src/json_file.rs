//! Loading the app's JSON files (`session.json`, `settings.json`,
//! `bookmarks.json`, and imported/named copies of them) with errors that
//! say which file failed and why. The returned error names just the file,
//! short enough for a toast; the full paths go to stderr.

use std::path::Path;

use serde::de::DeserializeOwned;

/// `path`'s file name, for a toast -- the full path is logged instead.
pub fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

/// Reads and parses `path`. `what` names the file's kind in the error
/// ("session", "settings", ...).
pub fn load<T: DeserializeOwned>(path: &Path, what: &str) -> Result<T, String> {
    let detail = match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(value) => return Ok(value),
            Err(err) => err.to_string(),
        },
        Err(err) => err.to_string(),
    };
    eprintln!("json_file: couldn't load {what} {}: {detail}", path.display());
    Err(format!("Couldn't load {what} {}: {detail}", file_name(path)))
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
        Ok(()) => {
            eprintln!("json_file: moved it to {}", aside.display());
            Err(format!("{err}. Kept it as {}", file_name(&aside)))
        }
        Err(rename_err) => {
            eprintln!("json_file: couldn't move it aside: {rename_err}");
            Err(format!("{err}. Couldn't move it aside: {rename_err}"))
        }
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
        assert!(err.starts_with("Couldn't load thing thing.json: "), "{err}");
        assert!(err.contains("Kept it as thing.unreadable-"), "{err}");
        assert!(!path.exists());
        let aside: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].to_string_lossy().starts_with("thing.unreadable-"));
        std::fs::write(&path, "[1, 2]").unwrap();
        assert_eq!(load_or_move_aside::<Vec<u32>>(&path, "thing"), Ok(Some(vec![1, 2])));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
