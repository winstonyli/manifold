//! The hosts carried inside `manifold.exe` (bare-exe plan Task 3, Decision 2): `build.rs` embeds each when its
//! `MANIFOLD_*_HOST` variable names a file, and `Embedded::ensure` puts it at a stable path under `engine-hosts/` so a
//! bare `manifold.exe` works offline. A build without the variable embeds nothing: `ensure` returns `None` and the
//! sibling exe is used, as before.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A host carried inside `manifold.exe`. Each has a folder of its own under `engine-hosts/`, so a host's lock file,
/// leftover `.old` and (WebView2) user-data folder stay apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Embedded {
    Webview2,
    Pdfium,
}

/// The host files' bytes (empty when this build embeds nothing) and their lowercase hex SHA-256 (`build.rs`).
static WEBVIEW2: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/webview2-host.bin"));
static PDFIUM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pdfium-host.bin"));

impl Embedded {
    fn folder(self) -> &'static str {
        match self {
            Self::Webview2 => "webview2",
            Self::Pdfium => "pdfium",
        }
    }
    /// Unchanged for WebView2: the exe name is how released builds and scripts know it.
    fn file(self) -> &'static str {
        match self {
            Self::Webview2 => "manifold-engine-webview2.exe",
            Self::Pdfium => "manifold-view-pdfium.exe",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Webview2 => "the WebView2 host",
            Self::Pdfium => "the PDF view",
        }
    }
    fn bytes(self) -> (&'static [u8], &'static str) {
        match self {
            Self::Webview2 => (WEBVIEW2, env!("MANIFOLD_WEBVIEW2_SHA256")),
            Self::Pdfium => (PDFIUM, env!("MANIFOLD_PDFIUM_SHA256")),
        }
    }
    /// Where the extracted host lives under `hosts_dir` (`engine-hosts/`): its own folder, so the `.WebView2`
    /// user-data folder WebView2 keys on the exe path, the lock file and the `.old` leftover all stay out of the flat
    /// host folder.
    pub(crate) fn host_path(self, hosts_dir: &Path) -> PathBuf {
        hosts_dir.join(self.folder()).join(self.file())
    }
    /// Makes `host_path(hosts_dir)` hold the embedded host and returns it; `None` when this build embeds nothing (and
    /// nothing is touched).
    pub(crate) fn ensure(self, hosts_dir: &Path) -> io::Result<Option<PathBuf>> {
        let (bytes, sha256) = self.bytes();
        ensure_with(self, hosts_dir, bytes, sha256)
    }
}

/// Process-wide: spawns run on their own threads, and the file lock below does not exclude threads of one process.
static EXTRACT: std::sync::Mutex<()> = std::sync::Mutex::new(());
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `Embedded::ensure` for the given `host` bytes and their SHA-256 `sha256`.
///
/// Under a process mutex and a file lock (a second manifold must not race the first), it hashes the file already at
/// the path and leaves it alone when it matches, so there is no marker to go stale and a tampered or half-written
/// file is simply replaced. A replacement is written to `<host>.<pid>-<n>.tmp`, hashed again, then the old file is
/// renamed aside to `<host>.old` and the new one renamed in; a running host keeps its mapped, renamed-aside exe.
/// The `.old` is deleted, best effort, at the start of the next call (it fails while an older instance still runs it).
pub(crate) fn ensure_with(host: Embedded, hosts_dir: &Path, bytes: &[u8], sha256: &str) -> io::Result<Option<PathBuf>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let dest = host.host_path(hosts_dir);
    let wrap = |what: &str, e: io::Error| io::Error::new(e.kind(), format!("{what} {}: {e}", dest.display()));
    let dir = dest.parent().unwrap_or(hosts_dir);
    let _guard = EXTRACT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    std::fs::create_dir_all(dir).map_err(|e| wrap("could not create the folder for", e))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("extract.lock"))
        .map_err(|e| wrap("could not lock the extraction of", e))?;
    lock.lock().map_err(|e| wrap("could not lock the extraction of", e))?;

    let aside = sibling_with_suffix(&dest, ".old");
    let _ = std::fs::remove_file(&aside);
    if crate::view_install::sha256_file(&dest).is_ok_and(|have| have.eq_ignore_ascii_case(sha256)) {
        return Ok(Some(dest));
    }

    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = sibling_with_suffix(&dest, &format!(".{}-{n}.tmp", std::process::id()));
    let result = (|| {
        std::fs::write(&temp, bytes).map_err(|e| wrap(&format!("could not write {} for", host.label()), e))?;
        let written = crate::view_install::sha256_file(&temp)
            .map_err(|e| wrap("could not read back", io::Error::other(e.to_string())))?;
        if !written.eq_ignore_ascii_case(sha256) {
            return Err(wrap(
                &format!("{} written for", host.label()),
                io::Error::other(format!("has SHA-256 {written}, expected {sha256}")),
            ));
        }
        let had_old = dest.exists();
        if had_old {
            std::fs::rename(&dest, &aside).map_err(|e| wrap("could not replace", e))?;
        }
        if let Err(e) = std::fs::rename(&temp, &dest) {
            if had_old {
                let _ = std::fs::rename(&aside, &dest);
            }
            return Err(wrap(&format!("could not move the new {} to", host.label()), e));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map(|()| Some(dest))
}

/// `path` with `suffix` appended to its file name (`host.exe` -> `host.exe.old`).
fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Runs `op` up to `attempts` times, `delay` apart, while it fails with a sharing violation (32) or access denied (5).
/// A fresh exe can be held for a moment by a scanner or the indexer; the first spawn right after the write may lose.
pub(crate) fn retry_busy<T>(attempts: u32, delay: Duration, mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut left = attempts.max(1);
    loop {
        left -= 1;
        match op() {
            Err(e) if left > 0 && matches!(e.raw_os_error(), Some(32 | 5)) => std::thread::sleep(delay),
            other => return other,
        }
    }
}

/// `command.spawn()` for a host just extracted: 3 attempts, 100 ms apart (Task 0 saw 0 failures in 52 spawns with
/// Defender off; this is insurance for machines with real-time protection on).
pub(crate) fn spawn_fresh(command: &mut Command) -> io::Result<Child> {
    retry_busy(3, Duration::from_millis(100), || command.spawn())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;
    use sha2::{Digest, Sha256};

    fn hash(bytes: &[u8]) -> String {
        manifold_view_protocol::transport::to_hex(&Sha256::digest(bytes))
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> =
            std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    /// The host folder's files other than the lock file.
    fn files(hosts: &Path) -> Vec<String> {
        names(Embedded::Webview2.host_path(hosts).parent().unwrap())
            .into_iter()
            .filter(|n| n != "extract.lock")
            .collect()
    }

    #[test]
    fn an_empty_embed_returns_none_and_touches_nothing() {
        let hosts = temp_dir("embed_none");
        assert_eq!(ensure_with(Embedded::Webview2, &hosts, b"", "").unwrap(), None);
        assert!(names(&hosts).is_empty());
        // This test build has no env var set unless the dedicated run below does.
        for host in [Embedded::Webview2, Embedded::Pdfium] {
            if host.bytes().0.is_empty() {
                assert_eq!(host.ensure(&hosts).unwrap(), None);
                assert!(names(&hosts).is_empty());
            }
        }
    }

    #[test]
    fn extracts_when_missing() {
        let hosts = temp_dir("embed_missing");
        let got = ensure_with(Embedded::Webview2, &hosts, b"host-v1", &hash(b"host-v1")).unwrap();
        assert_eq!(got, Some(Embedded::Webview2.host_path(&hosts)));
        assert_eq!(std::fs::read(Embedded::Webview2.host_path(&hosts)).unwrap(), b"host-v1");
        assert_eq!(files(&hosts), [Embedded::Webview2.file()]);
    }

    #[test]
    fn a_matching_file_is_left_alone_even_when_it_is_locked() {
        let hosts = temp_dir("embed_match");
        let dest = ensure_with(Embedded::Webview2, &hosts, b"host-v1", &hash(b"host-v1")).unwrap().unwrap();
        // Held open without share-delete, as a running host holds its exe: a rename over it would fail.
        let mut held = std::fs::OpenOptions::new();
        held.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            held.share_mode(1);
        }
        let _held = held.open(&dest).unwrap();
        assert_eq!(ensure_with(Embedded::Webview2, &hosts, b"host-v1", &hash(b"host-v1")).unwrap(), Some(dest));
        assert_eq!(files(&hosts), [Embedded::Webview2.file()], "nothing written or renamed aside");
    }

    #[test]
    fn a_differing_file_is_renamed_aside_and_the_old_copy_deleted_on_the_next_call() {
        let hosts = temp_dir("embed_differs");
        let dest = Embedded::Webview2.host_path(&hosts);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"tampered").unwrap();
        ensure_with(Embedded::Webview2, &hosts, b"host-v1", &hash(b"host-v1")).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"host-v1");
        assert_eq!(std::fs::read(sibling_with_suffix(&dest, ".old")).unwrap(), b"tampered");
        assert_eq!(files(&hosts), [Embedded::Webview2.file(), &format!("{}.old", Embedded::Webview2.file())]);
        // The next call (the file now matches) clears the leftover.
        ensure_with(Embedded::Webview2, &hosts, b"host-v1", &hash(b"host-v1")).unwrap();
        assert_eq!(files(&hosts), [Embedded::Webview2.file()]);
        // A newer build replaces it again.
        ensure_with(Embedded::Webview2, &hosts, b"host-v2", &hash(b"host-v2")).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"host-v2");
    }

    #[test]
    fn a_bytes_hash_mismatch_writes_nothing() {
        let hosts = temp_dir("embed_badhash");
        let err = ensure_with(Embedded::Webview2, &hosts, b"host-v1", &hash(b"other")).unwrap_err();
        assert!(err.to_string().contains(Embedded::Webview2.file()), "{err}");
        assert_eq!(files(&hosts), Vec::<String>::new());
    }

    #[test]
    fn concurrent_spawns_extract_the_host_once_and_leave_no_temp_files() {
        let hosts = temp_dir("embed_race");
        let bytes = vec![7u8; 200_000];
        let want = hash(&bytes);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (hosts, bytes, want) = (hosts.clone(), bytes.clone(), want.clone());
                std::thread::spawn(move || ensure_with(Embedded::Webview2, &hosts, &bytes, &want))
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().unwrap(), Some(Embedded::Webview2.host_path(&hosts)));
        }
        assert_eq!(std::fs::read(Embedded::Webview2.host_path(&hosts)).unwrap(), bytes);
        assert_eq!(files(&hosts), [Embedded::Webview2.file()], "one file, no temp copies, no .old");
    }

    /// A running host maps its exe; Windows will not rename it, and the old file must survive the failed update.
    #[cfg(windows)]
    #[test]
    fn a_locked_file_that_differs_errors_naming_it_and_stays() {
        use std::os::windows::fs::OpenOptionsExt;
        let hosts = temp_dir("embed_locked");
        let dest = Embedded::Webview2.host_path(&hosts);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"running-old").unwrap();
        let _held = std::fs::OpenOptions::new().read(true).share_mode(1).open(&dest).unwrap();
        let err = ensure_with(Embedded::Webview2, &hosts, b"host-v1", &hash(b"host-v1")).unwrap_err();
        assert!(err.to_string().contains(Embedded::Webview2.file()), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"running-old");
        assert_eq!(files(&hosts), [Embedded::Webview2.file()], "the temp copy is removed");
    }

    #[test]
    fn retry_busy_retries_only_sharing_violation_and_access_denied() {
        let mut calls = 0;
        let got = retry_busy(3, Duration::ZERO, || {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(got.unwrap(), 3);
        calls = 0;
        let got: io::Result<()> = retry_busy(3, Duration::ZERO, || {
            calls += 1;
            Err(io::Error::from_raw_os_error(5))
        });
        assert!(got.is_err());
        assert_eq!(calls, 3, "gives up after the last attempt");
        calls = 0;
        let got: io::Result<()> = retry_busy(3, Duration::ZERO, || {
            calls += 1;
            Err(io::Error::from_raw_os_error(2))
        });
        assert!(got.is_err());
        assert_eq!(calls, 1, "any other error is returned at once");
    }

    /// Meaningful only for the dedicated run with `MANIFOLD_*_HOST` set at build time; skipped per host otherwise.
    #[test]
    fn the_embedded_hosts_extract_to_their_pinned_hashes() {
        for host in [Embedded::Webview2, Embedded::Pdfium] {
            let (embedded, embedded_sha) = host.bytes();
            if embedded.is_empty() {
                assert_eq!(embedded_sha, "");
                continue;
            }
            assert_eq!(hash(embedded), embedded_sha);
            let hosts = temp_dir("embed_real");
            let dest = host.ensure(&hosts).unwrap().unwrap();
            assert_eq!(dest, host.host_path(&hosts));
            assert_eq!(crate::view_install::sha256_file(&dest).unwrap(), embedded_sha);
            assert_eq!(std::fs::metadata(&dest).unwrap().len(), embedded.len() as u64);
        }
    }

    #[test]
    fn two_embedded_hosts_extract_side_by_side() {
        let dir = temp_dir("embedded-two");
        let a = ensure_with(Embedded::Webview2, &dir, b"AAAA", &hash(b"AAAA")).unwrap().unwrap();
        let b = ensure_with(Embedded::Pdfium, &dir, b"BBBB", &hash(b"BBBB")).unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), b"AAAA");
        assert_eq!(std::fs::read(&b).unwrap(), b"BBBB");
        assert!(b.ends_with("pdfium/manifold-view-pdfium.exe"), "{}", b.display());
    }
}
