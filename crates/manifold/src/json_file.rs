//! Loading and saving the app's JSON files (`settings.json`, session exports,
//! `bookmarks.json`, and imported copies of them) with errors that
//! say which file failed and why. The returned error names just the file,
//! short enough for a toast; the full paths go to stderr.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// `path`'s file name, for a toast -- the full path is logged instead.
pub fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

/// Reads and parses `path`. `what` names the file's kind in the error
/// ("session", "settings", ...). A UTF-8 byte order mark is skipped, as
/// `content::passes` does.
pub fn load<T: DeserializeOwned>(path: &Path, what: &str) -> Result<T, String> {
    let detail = match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str(contents.strip_prefix('\u{feff}').unwrap_or(&contents)) {
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
/// file this run has loaded. Only a file known to be missing is a fresh
/// start: one whose metadata can't be read (`try_exists` errs, say access
/// denied) goes on to the load, so its error is reported, not hidden.
pub fn load_or_move_aside<T: DeserializeOwned>(path: &Path, what: &str) -> Result<Option<T>, String> {
    if matches!(path.try_exists(), Ok(false)) {
        return Ok(None);
    }
    let err = match load(path, what) {
        Ok(value) => return Ok(Some(value)),
        Err(err) => err,
    };
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
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

/// Writes `value` as pretty JSON over `path` (creating its directory) so a
/// crash mid-write leaves the old file whole rather than a truncated one
/// that the next launch moves aside. Failures go to stderr: these files
/// are rewritten on the next change anyway.
pub fn save<T: Serialize + ?Sized>(value: &T, path: &Path, what: &str) {
    if let Some(dir) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(dir) {
            eprintln!("{what}: failed to create {}: {err}", dir.display());
            return;
        }
    }
    let json = match serde_json::to_string_pretty(value) {
        Ok(json) => json,
        Err(err) => return eprintln!("{what}: failed to serialize: {err}"),
    };
    if let Err(err) = crate::store::atomic::replace(path, json.as_bytes()) {
        eprintln!("{what}: failed to write {}: {err}", path.display());
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

    /// A path whose existence can't be told (here a name too long for any file system) isn't read as missing:
    /// the error comes back rather than a fresh start that a later save would write over the file with.
    #[test]
    fn a_path_that_cant_be_checked_is_not_missing() {
        let dir = crate::store::testutil::temp_dir("json-unknown");
        let path = dir.join(format!("{}.json", "x".repeat(300)));
        assert!(path.try_exists().is_err(), "the premise: no metadata for this name");
        let err = load_or_move_aside::<Vec<u32>>(&path, "thing").unwrap_err();
        assert!(err.starts_with("Couldn't load thing "), "{err}");
    }

    #[test]
    fn save_replaces_whole_or_not_at_all() {
        let dir = crate::store::testutil::temp_dir("json-save");
        let path = dir.join("sub").join("thing.json");
        save(&[1, 2], &path, "thing");
        save(&[3], &path, "thing");
        assert_eq!(load::<Vec<u32>>(&path, "thing"), Ok(vec![3]));
        // The failed-save half needs a rename that fails: the Windows read-only attribute does that, while a
        // Unix rename replaces a read-only file regardless of its mode (only the directory's mode counts).
        if cfg!(not(windows)) {
            return;
        }
        let set_readonly = |readonly: bool| {
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "the Windows read-only attribute makes the rename over it fail; on Unix this is a temp file this test owns"
            )]
            perms.set_readonly(readonly);
            std::fs::set_permissions(&path, perms).unwrap();
        };
        set_readonly(true);
        save(&[4, 5, 6], &path, "thing");
        set_readonly(false);
        assert_eq!(load::<Vec<u32>>(&path, "thing"), Ok(vec![3]), "a failed save leaves the old file");
        let names: Vec<_> =
            std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["thing.json"], "no temp file left behind");
    }
}
