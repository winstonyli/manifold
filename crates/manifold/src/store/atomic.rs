//! The one "write beside, fsync, rename over" helper. Every file manifold
//! replaces goes through it, so all of them get the same durability (the data
//! reaches the disk before the rename can) and the same cleanup (a failed
//! replace leaves no temp file, which for a jar is a copy of its credentials).

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::journal::with_suffix;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// `<path>.<pid>-<n>.tmp`, new on every call. Unique across threads (a
/// shared name let two fetches write into one file) and across instances
/// (the pid; `jar::remove_dead_tmps` reads it to find crashed saves).
pub(crate) fn temp_path(path: &Path) -> PathBuf {
    with_suffix(path, &format!(".{}-{}.tmp", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)))
}

/// The pid in a temp suffix written by `temp_path` (`<pid>-<n>.tmp`) or by
/// builds before it (`<pid>.tmp`).
pub(crate) fn temp_pid(suffix: &str) -> Option<u32> {
    let stem = suffix.strip_suffix(".tmp")?;
    let (pid, n) = stem.split_once('-').unwrap_or((stem, "0"));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(pid) && digits(n)).then(|| pid.parse().ok()).flatten()
}

pub(crate) fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, bytes, false, || {})
}

/// `replace` for a file only the current user may read (0600 on Unix; on
/// Windows it inherits the profile folder's ACL).
pub(crate) fn replace_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, bytes, true, || {})
}

/// Writes `bytes` to a fresh temp beside `path` and fsyncs it, runs
/// `before_rename` (the journal keeps `.prev` there), then renames over
/// `path`, and on Unix fsyncs the directory. On any error `path` is as it was and the temp is gone.
pub(crate) fn replace_with(path: &Path, bytes: &[u8], private: bool, before_rename: impl FnOnce()) -> io::Result<()> {
    #[cfg(test)]
    if blocked(path) {
        return Err(io::Error::other("replace blocked by a test"));
    }
    let tmp = temp_path(path);
    let result = write_new(&tmp, bytes, private).and_then(|()| {
        before_rename();
        std::fs::rename(&tmp, path)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    // The rename itself is durable only once its directory is (Unix; Windows has no directory handle to sync).
    #[cfg(unix)]
    if result.is_ok() {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
        }
    }
    result
}

fn write_new(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Targets whose `replace_with` fails as if its temp couldn't be created.
/// Temp names differ per call, so a test can't force the failure by blocking a fixed `<path>.tmp`.
/// Keyed by the target path, which each test makes unique with `temp_dir`.
#[cfg(test)]
static BLOCKED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn blocked(path: &Path) -> bool {
    BLOCKED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p == path)
}

/// Makes replacing `path` fail (`block`) or work again.
#[cfg(test)]
pub(crate) fn block_for_test(path: &Path, block: bool) {
    let mut blocked = BLOCKED.lock().unwrap_or_else(|e| e.into_inner());
    blocked.retain(|p| p != path);
    if block {
        blocked.push(path.to_path_buf());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;

    fn tmp_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn replace_writes_whole_and_leaves_no_temp() {
        let dir = temp_dir("atomic-ok");
        let path = dir.join("f.json");
        replace(&path, b"one").unwrap();
        replace(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(tmp_files(&dir).is_empty());
    }

    #[test]
    fn a_failed_rename_removes_the_temp() {
        let dir = temp_dir("atomic-fail");
        let path = dir.join("target");
        std::fs::create_dir(&path).unwrap(); // a directory can't be replaced by a file
        assert!(replace(&path, b"x").is_err());
        assert!(tmp_files(&dir).is_empty(), "{:?}", tmp_files(&dir));
    }

    #[test]
    fn before_rename_runs_between_write_and_rename() {
        let dir = temp_dir("atomic-hook");
        let path = dir.join("f");
        std::fs::write(&path, b"old").unwrap();
        let mut seen = None;
        replace_with(&path, b"new", false, || seen = Some(std::fs::read(&path).unwrap())).unwrap();
        assert_eq!(seen.as_deref(), Some(&b"old"[..]), "the hook sees the old file");
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn temp_names_are_unique_per_call_and_carry_the_pid() {
        let path = Path::new("x/default.json");
        let (a, b) = (temp_path(path), temp_path(path));
        assert_ne!(a, b);
        let suffix = a.file_name().unwrap().to_str().unwrap().strip_prefix("default.json.").unwrap();
        assert_eq!(temp_pid(suffix), Some(std::process::id()));
        assert_eq!(temp_pid("1234.tmp"), Some(1234), "the pre-counter form");
        assert_eq!(temp_pid("12-3.tmp"), Some(12));
        assert_eq!(temp_pid("notapid.tmp"), None);
        assert_eq!(temp_pid("12-.tmp"), None);
        assert_eq!(temp_pid("12"), None);
    }

    #[test]
    fn a_blocked_target_fails_until_unblocked() {
        let dir = temp_dir("atomic-blocked");
        let path = dir.join("f");
        block_for_test(&path, true);
        assert!(replace(&path, b"x").is_err());
        assert!(!path.exists() && tmp_files(&dir).is_empty());
        block_for_test(&path, false);
        replace(&path, b"x").unwrap();
    }
}
