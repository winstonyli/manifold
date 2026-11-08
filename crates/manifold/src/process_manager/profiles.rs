//! Engine cache and profile folders: where CEF and WebView2 hosts keep them, handing out the WebView2 profile
//! slots, and the startup sweeps that delete what an exited or killed host left behind.

use super::*;

/// Parent of every CEF view-host's own `root_cache_path`. Temp when
/// there's no profile (always under `cargo test`), so CEF never falls back
/// to its machine-wide default, which only one CEF process can use at once.
pub(super) fn cef_cache_root() -> PathBuf {
    crate::profile::cef_cache_dir().unwrap_or_else(|| std::env::temp_dir().join("manifold-cef-cache"))
}

/// Names each CEF engine's cache dir `<manifold pid>-<n>`: unique across
/// manifold instances sharing a profile, and across respawns of one tab,
/// so a swap's new engine never shares a dir the old one's
/// `remove_cache_dir` is still deleting.
pub(super) static CEF_CACHE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Deletes an exited CEF engine's cache dir. Its renderer/GPU subprocesses
/// die with it (its own kill-on-close job) but not instantly, and Windows
/// won't delete files they still hold, so this retries for a few seconds
/// on a background thread. Whatever is left `sweep_cef_cache` removes at
/// the next startup.
///
/// The same for an exited WebView2 host's profile folder, and then its
/// `slot` is freed either way: the shared browser process keeps a closed
/// profile's folder open while it lives, but its InPrivate cookies went with
/// the host (`clear_webview2_slot`).
pub(super) fn remove_cache_dir(dir: PathBuf, slot: Option<usize>) {
    let removal = std::thread::spawn(move || {
        for _ in 0..50 {
            if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(n) = slot {
            free_webview2_slot(n);
        }
    });
    CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()).push(removal);
}

pub(super) static CACHE_REMOVALS: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> =
    std::sync::Mutex::new(Vec::new());

/// Waits for every WebKit host reap (`Drop for ViewTab`) and
/// `remove_cache_dir` so far, at most their few seconds each: `main` calls
/// this after dropping the app's tabs, since the process exiting would
/// otherwise cut the waits and deletes short. A WebKit host's wait for its
/// own exit can make this take up to 3 s, as it did when `Drop` waited
/// inline. Loops until the list stays empty, so a thread pushed while
/// earlier ones are joined is waited for too.
pub fn finish_cache_removals() {
    loop {
        let removals = std::mem::take(&mut *CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()));
        if removals.is_empty() {
            return;
        }
        for removal in removals {
            let _ = removal.join();
        }
    }
}

/// Frame files left by hosts that never got reaped (manifold killed or
/// crashed): any `manifold-frame-*.bin` in temp that no running host holds
/// (mapped on Windows, `flock`ed on Unix; `sweep_stale_frames_in`) and that is
/// over ten minutes old. Another manifold's live files are spared the same way.
pub fn sweep_frame_files() {
    let removed = manifold_view_protocol::sweep_stale_frames_in(&std::env::temp_dir(), Duration::from_secs(600));
    if removed > 0 {
        eprintln!("removed {removed} stale frame buffer file(s)");
    }
}

/// A per-tab engine log over this size is emptied at the next spawn (`per_tab_log_files`).
pub(super) const MAX_TAB_LOG_BYTES: u64 = 8 << 20;
const MAX_TAB_LOG_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
const KEEP_CRASH_REPORTS: usize = 10;

/// Startup sweep of `manifold-*.log` in the temp dir: per-tab engine logs untouched for 14 days go, and of the
/// `manifold-crash-*.log` reports only the newest 10 stay.
pub fn sweep_view_logs() {
    let removed = sweep_view_logs_in(&std::env::temp_dir(), MAX_TAB_LOG_AGE, KEEP_CRASH_REPORTS);
    if removed > 0 {
        eprintln!("removed {removed} old engine log(s) and crash report(s)");
    }
}

pub(super) fn sweep_view_logs_in(dir: &Path, max_age: Duration, keep_crashes: usize) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let (mut crashes, mut logs) = (Vec::new(), Vec::new());
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("manifold-") && name.ends_with(".log")) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else { continue };
        let list = if name.starts_with("manifold-crash-") { &mut crashes } else { &mut logs };
        list.push((modified, entry.path()));
    }
    crashes.sort_by_key(|c| std::cmp::Reverse(c.0));
    let old = std::time::SystemTime::now().checked_sub(max_age);
    let doomed = crashes.into_iter().skip(keep_crashes).chain(logs.into_iter().filter(|(m, _)| Some(*m) < old));
    doomed.filter(|(_, path)| std::fs::remove_file(path).is_ok()).count()
}

/// Deletes CEF cache dirs left by engines that `remove_cache_dir` never
/// finished (manifold killed or crashed). (`sweep_webview2_profiles` does the
/// same for WebView2's per-host profiles.) Another manifold on the same
/// profile may be using some: Windows won't rename a dir while a running
/// CEF holds files in it, so each is renamed aside first and deleted only
/// if that works. Dirs touched in the last minute are skipped, since their
/// CEF may not have opened its files yet.
pub fn sweep_cef_cache() {
    sweep_cache_dirs(&cef_cache_root(), Duration::from_secs(60));
}

/// What a WebView2 host's profile directory is named: manifold hands each host a profile name `manifold_<slot>`
/// (`VIEW_WEBVIEW2_PROFILE`; manifold-view-webview2's `create_composition_controller`), and WebView2 keeps a named
/// profile in `WV2Profile_<name>` (observed, not documented: runtime 154.0.4258.53, 2026-10-03). InPrivate holds no
/// cookies there, but caches and empty databases (~2 MB) stay behind.
pub(super) const WEBVIEW2_PROFILE_PREFIX: &str = "WV2Profile_manifold_";

/// How many profile names WebView2 hosts take turns with. WebView2 lists every named profile it has seen in its
/// `Local State` (about 1 KB each) and never forgets one, so names are reused rather than made per host. More
/// concurrent WebView2 tabs than this still each get their own name, beyond the pool, and the log says so.
pub(super) const WEBVIEW2_PROFILE_POOL: usize = 8;

pub(super) fn webview2_profile_name(slot: usize) -> String {
    format!("manifold_{slot}")
}

/// Parent of every manifold process's WebView2 user-data folder. Temp when there's no profile (always under `cargo
/// test`), as `cef_cache_root`.
fn webview2_data_root() -> PathBuf {
    crate::profile::webview2_data_dir().unwrap_or_else(|| std::env::temp_dir().join("manifold-webview2"))
}

/// This manifold's WebView2 user-data folder, `<root>/<manifold pid>`, which its hosts get in
/// `VIEW_WEBVIEW2_USER_DATA`: one per process, so two instances, on one profile or two, never share a browser process,
/// its profile names or its `Local State`. WebView2's own default, `<host exe>.WebView2`, was shared by every instance
/// running that host.
pub(super) fn webview2_user_data() -> PathBuf {
    webview2_data_root().join(std::process::id().to_string())
}

pub(super) fn webview2_profile_dir(udf: &Path, slot: usize) -> PathBuf {
    udf.join("EBWebView").join(format!("{WEBVIEW2_PROFILE_PREFIX}{slot}"))
}

/// The profile slots this manifold's WebView2 hosts hold.
static WEBVIEW2_SLOTS: std::sync::Mutex<std::collections::BTreeSet<usize>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Records and returns the lowest slot not in `busy`; a result `>= WEBVIEW2_PROFILE_POOL` is beyond the pool.
pub(super) fn claim_slot(busy: &mut std::collections::BTreeSet<usize>) -> usize {
    (0..).find(|&n| busy.insert(n)).expect("a free slot")
}

/// Deletes slot `n`'s leftover profile folder in `udf` if it can. One that cannot is still open in the browser
/// process all hosts share, which keeps a profile loaded after its last host exits. That does not block reuse: the
/// cookies are InPrivate, in memory, and a new host with the name starts with an empty store (shown by `gate jar`'s
/// reuse run, runtime 154.0.4258.53); while the name is still closing the host tries again (`retry_while_busy`).
pub(super) fn clear_webview2_slot(udf: &Path, n: usize) {
    let _ = std::fs::remove_dir_all(webview2_profile_dir(udf, n));
}

/// The lowest free profile slot for a new WebView2 host in the user-data folder `udf`. Freed by `free_webview2_slot`
/// only once the host process has exited and its folder's deletion has been tried, so a live host's profile is never
/// handed to another.
pub(super) fn claim_webview2_slot(udf: &Path) -> usize {
    let mut slots = WEBVIEW2_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let n = claim_slot(&mut slots);
    clear_webview2_slot(udf, n);
    if n >= WEBVIEW2_PROFILE_POOL {
        eprintln!(
            "webview2: all {WEBVIEW2_PROFILE_POOL} pooled profiles are in use; using {} (adds one Local State entry)",
            webview2_profile_name(n)
        );
    }
    n
}

pub(super) fn free_webview2_slot(n: usize) {
    WEBVIEW2_SLOTS.lock().unwrap_or_else(|e| e.into_inner()).remove(&n);
}

/// Deletes the WebView2 user-data folders of manifold processes that are gone (each leaves its own,
/// `webview2_user_data`), as `sweep_cef_cache` does: a live one's browser process holds its files, so it can't be
/// renamed aside. Profiles inside a live folder that `remove_cache_dir` could not delete go when their slot is reused.
pub fn sweep_webview2_profiles() {
    sweep_cache_dirs(&webview2_data_root(), Duration::from_secs(60));
}

/// Sweeps the dirs in `root`.
pub(super) fn sweep_cache_dirs(root: &Path, min_age: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let path = entry.path();
        // Renamed aside by an earlier sweep that then couldn't finish.
        let aside = if path.extension().is_some_and(|ext| ext == "old") {
            path
        } else {
            let age = entry.metadata().and_then(|meta| meta.modified()).ok().and_then(|time| time.elapsed().ok());
            if age.is_none_or(|age| age < min_age) {
                continue;
            }
            let aside = path.with_extension("old");
            if std::fs::rename(&path, &aside).is_err() {
                continue;
            }
            aside
        };
        let _ = std::fs::remove_dir_all(&aside);
    }
}
