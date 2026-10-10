use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::engine_install::{runtime_state, EngineState, Pin, RuntimeOnDisk, NO_PIN_REASON};
use ipc_protocol::transport::{self, LocalTransport};
use ipc_protocol::{EngineToHost, HostToEngine};

/// A Windows Job Object every engine-host child (`spawn_tab`/
/// `spawn_webkit_tab`, both call `assign_to_engine_job` right after
/// spawning) gets assigned to, configured with `JOB_OBJECT_LIMIT_KILL_ON_
/// JOB_CLOSE` -- Windows itself then guarantees every process in the job
/// dies the instant this job's last handle closes, which happens
/// automatically when *this* manifold.exe process terminates for *any*
/// reason (a clean exit, a crash, a Task Manager "End Task," anything
/// short of the whole OS going down) since Windows closes all of a
/// terminated process's own handles as part of teardown.
///
/// Exists because engine-hosts becoming orphaned was already a known, if
/// only ever partially worked around, problem: `spawn_tab`'s own comment
/// above already notes "a prior run that crashed... can leave a stale
/// engine-host-*.exe still holding tab_id's named pipe indefinitely," with
/// a fallback-socket-id workaround for *that* one symptom. Found live to
/// be worse than a stale pipe, though -- orphaned `manifold-engine-
/// webview2.exe` processes accumulating across several crashed/force-
/// killed sessions were confirmed to silently starve *new* engine-hosts'
/// own frame capture (`Windows.Graphics.Capture`, which every WebView2
/// engine-host's off-screen hosting window uses -- see that crate's own
/// `window.rs`), reproducing as tab content stuck on its loading spinner
/// forever, not a speed problem. Diagnosed by instrumenting the frame-
/// receive path directly: zero `FrameReady` messages arrived for over a
/// minute with stale processes present, then several hundred arrived
/// within seconds of killing them by hand.
///
/// A Job Object rather than a "scan for and kill anything that looks
/// orphaned" pass at startup on purpose -- the latter has no reliable way
/// to distinguish a genuine orphan from a second manifold.exe instance's
/// own perfectly healthy children by image name alone, and getting that
/// distinction wrong means one running copy of this app occasionally
/// kills another's tabs out from under it. A Job Object needs no such
/// heuristic: each manifold.exe process gets its own job, so there is
/// nothing to misidentify. It only prevents *future* orphaning, though --
/// it has no way to reach back and clean up engine-hosts already orphaned
/// by a session that ran before this existed; those still need a one-time
/// manual Task Manager cleanup.
#[cfg(target_os = "windows")]
struct EngineJobHandle(windows_sys::Win32::Foundation::HANDLE);
// Safety: a Win32 `HANDLE` is a plain kernel object reference, valid to
// use from any thread -- Rust's blanket `!Send`/`!Sync` on raw pointers is
// only being overly cautious here, not flagging a real hazard.
#[cfg(target_os = "windows")]
unsafe impl Send for EngineJobHandle {}
#[cfg(target_os = "windows")]
unsafe impl Sync for EngineJobHandle {}

#[cfg(target_os = "windows")]
static ENGINE_JOB: std::sync::OnceLock<EngineJobHandle> = std::sync::OnceLock::new();

/// Creates (once) and returns the Job Object every engine-host child gets
/// assigned to -- see `EngineJobHandle`'s own doc comment. A null handle
/// (creation failed) is a silent no-op everywhere it's used, and a job whose
/// kill-on-close setup failed is logged once and used anyway: this is a
/// crash-safety net, not a feature any spawn should fail over losing.
#[cfg(target_os = "windows")]
fn engine_job() -> windows_sys::Win32::Foundation::HANDLE {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    ENGINE_JOB
        .get_or_init(|| unsafe {
            let job = windows_sys::Win32::System::JobObjects::CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if !job.is_null() {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    // Engines then outlive a crashed manifold: say so once.
                    eprintln!("engine job: kill-on-close not set: {}", std::io::Error::last_os_error());
                }
            }
            EngineJobHandle(job as HANDLE)
        })
        .0
}

/// Adds a just-spawned engine-host child to `engine_job()` -- called right
/// after every successful `Command::spawn()` in this module. A no-op (not
/// an error) if the job itself failed to create/configure, or if this one
/// assignment fails (e.g. the child already exited) -- either way, the
/// worst case is falling back to this app's prior behavior (no automatic
/// cleanup), not a spawn failure over a crash-safety extra.
#[cfg(target_os = "windows")]
fn assign_to_engine_job(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
    let job = engine_job();
    if job.is_null() {
        return;
    }
    let process_handle = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    unsafe {
        let _ = AssignProcessToJobObject(job, process_handle);
    }
}
/// No kill-on-exit job off Windows: an engine-host sees the socket drop when manifold dies and shuts itself down
/// (`ipc_protocol::transport::spawn_host_reader`). Measured on Linux (WSL2, 2026-10-02): `kill -9` of manifold with a
/// live CEF tab left no CEF process after 1 s, so `PR_SET_PDEATHSIG` or a process group is not needed there. Said once
/// on stderr rather than silently, since other hosts (WebKit in WSL) are not covered by that measurement.
#[cfg(not(target_os = "windows"))]
fn assign_to_engine_job(_child: &Child) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "manifold: no kill-on-exit job on this platform; engine hosts exit when their socket to manifold drops"
        )
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    Servo,
    Cef,
    /// WebKit, via WebKitGTK — hosted inside WSL2 (no native Windows
    /// embedding story for WebKit exists; see project plan/memory).
    Webkit,
    /// Microsoft Edge WebView2 — native Windows API, same-machine process
    /// like Servo/Cef (no WSL2 relay needed).
    Webview2,
}

/// What each engine is: the one place its name, store code, binary, label and
/// pixel order live. `desc` is an exhaustive `match`, so a new `EngineKind`
/// fails to compile until it has a row here.
pub(crate) struct EngineDesc {
    /// The string `settings.json`, session exports and the `engine` store field hold.
    pub(crate) name: &'static str,
    /// The store's `engine` field code: `store::model::ENGINES[code - 1]` is `name`.
    pub(crate) code: u64,
    pub(crate) binary: &'static str,
    pub(crate) label: &'static str,
    /// Frames arrive as BGRA (CEF, WebView2) rather than RGBA (Servo, WebKit).
    pub(crate) bgra: bool,
}

impl EngineKind {
    pub(crate) const ALL: [EngineKind; 4] =
        [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit, EngineKind::Webview2];

    pub(crate) fn desc(self) -> EngineDesc {
        match self {
            EngineKind::Servo => {
                EngineDesc { name: "servo", code: 1, binary: "manifold-engine-servo", label: "Servo", bgra: false }
            }
            EngineKind::Cef => {
                EngineDesc { name: "cef", code: 2, binary: "manifold-engine-cef", label: "Blink (CEF)", bgra: true }
            }
            EngineKind::Webkit => {
                EngineDesc { name: "webkit", code: 3, binary: "manifold-engine-webkit", label: "WebKit", bgra: false }
            }
            EngineKind::Webview2 => EngineDesc {
                name: "webview2",
                code: 4,
                binary: "manifold-engine-webview2",
                label: "WebView2",
                bgra: true,
            },
        }
    }

    /// Case-insensitive, so the headless `SPLIT` argument and the stored
    /// lowercase names share one parser.
    pub(crate) fn from_name(name: &str) -> Option<EngineKind> {
        Self::ALL.into_iter().find(|k| k.desc().name.eq_ignore_ascii_case(name))
    }

    pub(crate) fn from_code(code: u64) -> Option<EngineKind> {
        Self::ALL.into_iter().find(|k| k.desc().code == code)
    }
}

/// `EngineKind` itself has no reason to derive serde (it's an internal
/// process-management type) — a small manual string mapping instead, written
/// by the two independent persistence formats that each need to store one
/// (`settings.rs`'s default-starting-engine, `session.rs`'s per-tab engine).
/// Serialize only: each format reads it back its own way (settings, leniently).
pub(crate) mod engine_kind_serde {
    use super::EngineKind;
    use serde::{Serialize, Serializer};

    pub fn serialize<S: Serializer>(kind: &EngineKind, s: S) -> Result<S::Ok, S::Error> {
        kind.desc().name.serialize(s)
    }
}

/// A running engine-host child process plus the control channel
/// manifold talks to it on. `manifold` is the process manager: it
/// owns spawning, tearing down, and (from M2 on) swapping the engine
/// backing a tab.
pub struct EngineTab {
    pub kind: EngineKind,
    /// The id used to name this tab's control-pipe/shared-memory socket —
    /// normally equal to its `tab_id`, but can diverge after the
    /// stale-pipe fallback in `spawn_tab`. manifold's own bookkeeping
    /// (the `TabNode` map, UI) keys off `tab_id`, never this; only
    /// low-level socket/shared-memory lookups (e.g. `SharedFrameBuffer`)
    /// need it.
    pub socket_id: String,
    /// Sends to the engine-host off the UI thread (`engine_io::Writer`), over its local pipe or, for the WSL2-hosted
    /// WebKit, its TCP connection (frames then travel inline in `FrameReady`).
    writer: engine_io::Writer,
    pub events_rx: mpsc::Receiver<EngineToHost>,
    /// The newest frame its reader saw (`engine_io::FrameSlot`), taken by `take_frame`.
    frame: engine_io::FrameSlot,
    /// `None` once `Drop` has handed it to its reaper thread.
    child: Option<Child>,
    /// A directory only this host uses, deleted on drop: CEF's `root_cache_path`, or WebView2's named profile
    /// (`webview2_profile_dir`).
    cache_dir: Option<PathBuf>,
    /// A WebView2 host's profile slot (`claim_webview2_slot`), freed once `cache_dir` is gone.
    profile_slot: Option<usize>,
}

impl EngineTab {
    /// Queues `msg` for the engine and returns at once. A run of pointer moves, resizes or wheel steps the engine has
    /// not read yet merges into one (`engine_io::coalesce`). Dropped when the engine has stopped reading and its queue
    /// is full (the tab then reads as unresponsive), and after `Shutdown`.
    pub fn send(&self, msg: &HostToEngine) {
        self.writer.send(msg);
    }

    /// M5: at most one tab is ever visible (actively capturing/sending
    /// frames) at a time — see `HostToEngine::SetVisible`'s doc comment.
    pub fn set_visible(&self, visible: bool) {
        self.send(&HostToEngine::SetVisible(visible));
    }

    /// Queues `Shutdown` now. An app exit calls this on every tab before dropping them, so `Drop` then waits one
    /// `engine_io::SHUTDOWN_FLUSH` for all of them together.
    pub(crate) fn begin_shutdown(&self) {
        self.writer.shutdown();
    }

    /// The newest frame since the last call, if any.
    pub(crate) fn take_frame(&self) -> Option<engine_io::Frame> {
        self.frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }

    /// The engine has stopped reading: a send was refused while a write had been blocked for
    /// `engine_io::REFUSED_BLOCKED_AFTER`, or one write has been stuck for `engine_io::UNRESPONSIVE_AFTER` (the card
    /// badge and the menu's Restart and End rows).
    pub(crate) fn is_unresponsive(&self, now: std::time::Instant) -> bool {
        self.writer.is_unresponsive(now)
    }

    /// `is_unresponsive`, true once per stall (the toast).
    pub(crate) fn take_newly_unresponsive(&self, now: std::time::Instant) -> bool {
        self.writer.take_newly_unresponsive(now)
    }

    /// Makes a fake tab unresponsive: fills its queue (its peer must never be read).
    #[cfg(test)]
    pub(crate) fn wedge(&self) {
        self.writer.wedge();
    }

    /// A live tab for unit tests: no process, its control channel a loopback TCP connection. The returned peer
    /// reads what the app sends it (`HostToEngine`), timing out after 200 ms when nothing more comes; the sender
    /// feeds the tab's events as its reader would.
    #[cfg(test)]
    pub(crate) fn fake_fed(
        kind: EngineKind,
    ) -> (EngineTab, ipc_protocol::MessageChannel<std::net::TcpStream>, mpsc::SyncSender<EngineToHost>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let host = std::net::TcpStream::connect(listener.local_addr().expect("local addr")).expect("connect");
        let (peer, _) = listener.accept().expect("accept");
        peer.set_read_timeout(Some(Duration::from_millis(200))).expect("read timeout");
        let (feed, events_rx) = mpsc::sync_channel(engine_io::EVENTS_CAP);
        let tab = EngineTab {
            kind,
            socket_id: String::new(),
            writer: engine_io::Writer::spawn(transport::RemoteTransport::new(host), "engine writer (fake)"),
            events_rx,
            frame: engine_io::FrameSlot::default(),
            child: None,
            cache_dir: None,
            profile_slot: None,
        };
        (tab, ipc_protocol::MessageChannel::new(peer), feed)
    }

    /// `fake_fed` without the feed: its events channel is closed and empty.
    #[cfg(test)]
    pub(crate) fn fake(kind: EngineKind) -> (EngineTab, ipc_protocol::MessageChannel<std::net::TcpStream>) {
        let (tab, peer, _) = Self::fake_fed(kind);
        (tab, peer)
    }
}

impl Drop for EngineTab {
    fn drop(&mut self) {
        // Read before `shutdown`, after which a writer never reads as unresponsive.
        let stuck = self.writer.is_unresponsive(std::time::Instant::now());
        self.writer.shutdown();
        let Some(child) = self.child.take() else { return };
        let (cache_dir, slot) = (self.cache_dir.take(), self.profile_slot.take());
        if self.kind == EngineKind::Webkit {
            // Off the UI thread: a WebKit host gets up to 3 s to exit on its own,
            // and dropping a session of N of them used to freeze the window for 3N s.
            // It has no frame file (frames arrive over TCP) and no cache dir.
            // A writer stuck on its socket stays blocked until the Linux side dies: closing the
            // socket on this side does not unblock a send on Windows (measured, wave 3b).
            let reaper = std::thread::spawn(move || reap_host(EngineKind::Webkit, child));
            CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()).push(reaper);
            return;
        }
        // The queue and `Shutdown` get until `SHUTDOWN_FLUSH` after the first `shutdown` to go out.
        // A stuck engine is not waited for: the kill below breaks its pipe, which ends the blocked
        // write too (measured: about 0.2 ms after the engine's end closes).
        if !stuck {
            self.writer.wait_finished(engine_io::SHUTDOWN_FLUSH);
        }
        // A local host is killed at once, so this stays inline. It must: a respawn
        // of the same tab reuses `socket_id`, and Windows deletes a file even while
        // another process maps it (measured), so a delete that ran late on another
        // thread could take the new host's frame file.
        reap_host(self.kind, child);
        // The host has exited, so its frame file is no longer mapped by it.
        ipc_protocol::remove_frame_file(&self.socket_id);
        // The profile slot is freed after the folder's deletion is tried.
        if let Some(dir) = cache_dir {
            remove_cache_dir(dir, slot);
        }
    }
}

/// Waits for an engine host to exit after `Shutdown`, killing it if needed.
fn reap_host(kind: EngineKind, mut child: Child) {
    if kind == EngineKind::Webkit {
        // For the WSL2-hosted case, `child` is only the Windows-side
        // `wsl.exe` relay — force-killing it immediately (the
        // local-engine path below) does NOT terminate the actual
        // Linux-side process tree (fish/bash/xvfb-run/Xvfb/
        // manifold-engine-webkit), which WSL2 keeps running independently
        // of any Windows-side process state: confirmed live, a killed
        // relay leaves that whole tree orphaned and running
        // indefinitely, invisible to any further Windows-side cleanup.
        // The graceful `Shutdown` queued above (the writer sends it before the reaper gives up) already makes
        // manifold-engine-webkit return from `main()` on its own, which
        // cascades correctly on the Linux side (xvfb-run's own EXIT
        // trap kills its Xvfb child once its wrapped command exits, and
        // so on up through bash/fish to `wsl.exe` itself returning) —
        // also confirmed live, leaving zero orphaned processes. So wait
        // for that natural exit first, bounded in case the message
        // never lands (network hiccup, or the process was already
        // wedged for some unrelated reason), and only fall back to a
        // force-kill — with its known orphan risk — if it doesn't
        // happen in time.
        for _ in 0..30 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Parent of every CEF engine-host's own `root_cache_path`. Temp when
/// there's no profile (always under `cargo test`), so CEF never falls back
/// to its machine-wide default, which only one CEF process can use at once.
fn cef_cache_root() -> PathBuf {
    crate::profile::cef_cache_dir().unwrap_or_else(|| std::env::temp_dir().join("manifold-cef-cache"))
}

/// Names each CEF engine's cache dir `<manifold pid>-<n>`: unique across
/// manifold instances sharing a profile, and across respawns of one tab,
/// so a swap's new engine never shares a dir the old one's
/// `remove_cache_dir` is still deleting.
static CEF_CACHE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Deletes an exited CEF engine's cache dir. Its renderer/GPU subprocesses
/// die with it (its own kill-on-close job) but not instantly, and Windows
/// won't delete files they still hold, so this retries for a few seconds
/// on a background thread. Whatever is left `sweep_cef_cache` removes at
/// the next startup.
///
/// The same for an exited WebView2 host's profile folder, and then its
/// `slot` is freed either way: the shared browser process keeps a closed
/// profile's folder open while it lives, but its InPrivate cookies went with
/// the host (`take_webview2_slot`).
fn remove_cache_dir(dir: PathBuf, slot: Option<usize>) {
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

static CACHE_REMOVALS: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// Waits for every WebKit host reap (`Drop for EngineTab`) and
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
    let removed = ipc_protocol::sweep_stale_frames_in(&std::env::temp_dir(), Duration::from_secs(600));
    if removed > 0 {
        eprintln!("removed {removed} stale frame buffer file(s)");
    }
}

/// A per-tab engine log over this size is emptied at the next spawn (`per_tab_log_files`).
const MAX_TAB_LOG_BYTES: u64 = 8 << 20;
const MAX_TAB_LOG_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
const KEEP_CRASH_REPORTS: usize = 10;

/// Startup sweep of `manifold-*.log` in the temp dir: per-tab engine logs untouched for 14 days go, and of the
/// `manifold-crash-*.log` reports only the newest 10 stay.
pub fn sweep_engine_logs() {
    let removed = sweep_engine_logs_in(&std::env::temp_dir(), MAX_TAB_LOG_AGE, KEEP_CRASH_REPORTS);
    if removed > 0 {
        eprintln!("removed {removed} old engine log(s) and crash report(s)");
    }
}

fn sweep_engine_logs_in(dir: &Path, max_age: Duration, keep_crashes: usize) -> usize {
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
    sweep_cache_dirs(&cef_cache_root(), "", Duration::from_secs(60));
}

/// What a WebView2 host's profile directory is named: manifold hands each host a profile name `manifold_<slot>`
/// (`ENGINE_WEBVIEW2_PROFILE`; manifold-engine-webview2's `create_composition_controller`), and WebView2 keeps a named
/// profile in `WV2Profile_<name>` (observed, not documented: runtime 154.0.4258.53, 2026-10-03). InPrivate holds no
/// cookies there, but caches and empty databases (~2 MB) stay behind.
const WEBVIEW2_PROFILE_PREFIX: &str = "WV2Profile_manifold_";

/// How many profile names WebView2 hosts take turns with. WebView2 lists every named profile it has seen in its
/// `Local State` (about 1 KB each) and never forgets one, so names are reused rather than made per host. More
/// concurrent WebView2 tabs than this still each get their own name, beyond the pool, and the log says so.
const WEBVIEW2_PROFILE_POOL: usize = 8;

fn webview2_profile_name(slot: usize) -> String {
    format!("manifold_{slot}")
}

/// WebView2's default user-data folder for hosts run from `host_exe`, `<host exe>.WebView2`, which the host does not
/// override.
fn webview2_user_data(host_exe: &Path) -> PathBuf {
    let mut udf = host_exe.as_os_str().to_owned();
    udf.push(".WebView2");
    PathBuf::from(udf)
}

/// Where WebView2 keeps the profiles of hosts run from `host_exe`.
fn webview2_profiles_root(host_exe: &Path) -> PathBuf {
    webview2_user_data(host_exe).join("EBWebView")
}

fn webview2_profile_dir(host_exe: &Path, slot: usize) -> PathBuf {
    webview2_profiles_root(host_exe).join(format!("{WEBVIEW2_PROFILE_PREFIX}{slot}"))
}

/// The profile slots this manifold's WebView2 hosts hold, each with its lock file (`take_webview2_slot`).
static WEBVIEW2_SLOTS: std::sync::Mutex<std::collections::BTreeMap<usize, Option<std::fs::File>>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Records and returns the lowest slot not in `busy` that `take` can claim (`None`: in use outside `busy`). Past every
/// claimable slot it keeps counting, so it always returns one; a result `>= WEBVIEW2_PROFILE_POOL` is beyond the pool.
fn claim_slot<G>(busy: &mut std::collections::BTreeMap<usize, G>, mut take: impl FnMut(usize) -> Option<G>) -> usize {
    let mut n = 0;
    loop {
        if let std::collections::btree_map::Entry::Vacant(free) = busy.entry(n) {
            if let Some(guard) = take(n) {
                free.insert(guard);
                return n;
            }
        }
        n += 1;
    }
}

/// Claims slot `n` of the user-data folder `udf` against everything outside this process: its lock file
/// (`manifold-profile-<n>.lock`, held open without sharing, so another manifold instance using the same folder skips
/// it). Its leftover profile folder is deleted if it can be; one that cannot is still open in the browser process
/// all hosts share, which keeps a profile loaded after its last host exits. That does not block reuse: the cookies
/// are InPrivate, in memory, and a new host with the name starts with an empty store (shown by `scripts/jar-probe.sh`'s
/// reuse run, runtime 154.0.4258.53). A lock file that cannot be created at all (not a sharing violation) gives a
/// claim without a lock.
fn take_webview2_slot(udf: &Path, n: usize) -> Option<Option<std::fs::File>> {
    let _ = std::fs::create_dir_all(udf);
    let mut open = std::fs::OpenOptions::new();
    open.create(true).truncate(false).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open.share_mode(0);
    }
    const ERROR_SHARING_VIOLATION: i32 = 32;
    let lock = match open.open(udf.join(format!("manifold-profile-{n}.lock"))) {
        Ok(file) => Some(file),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => return None,
        Err(_) => None,
    };
    let _ = std::fs::remove_dir_all(udf.join("EBWebView").join(format!("{WEBVIEW2_PROFILE_PREFIX}{n}")));
    Some(lock)
}

/// The lowest free profile slot for a new WebView2 host run from `host_exe`. Freed by `free_webview2_slot` only once
/// the host process has exited and its folder's deletion has been tried, so a live host's profile is never handed to
/// another.
fn claim_webview2_slot(host_exe: &Path) -> usize {
    let udf = webview2_user_data(host_exe);
    let mut slots = WEBVIEW2_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let n = claim_slot(&mut slots, |n| take_webview2_slot(&udf, n));
    if n >= WEBVIEW2_PROFILE_POOL {
        eprintln!(
            "webview2: all {WEBVIEW2_PROFILE_POOL} pooled profiles are in use; using {} (adds one Local State entry)",
            webview2_profile_name(n)
        );
    }
    n
}

fn free_webview2_slot(n: usize) {
    WEBVIEW2_SLOTS.lock().unwrap_or_else(|e| e.into_inner()).remove(&n);
}

/// Deletes WebView2 host profiles `remove_cache_dir` could not (the shared browser process still held them, or
/// manifold was killed), as `sweep_cef_cache` does; only `WEBVIEW2_PROFILE_PREFIX` dirs are touched.
pub fn sweep_webview2_profiles() {
    let root = webview2_profiles_root(&engine_binary_path(EngineKind::Webview2));
    sweep_cache_dirs(&root, WEBVIEW2_PROFILE_PREFIX, Duration::from_secs(60));
}

/// Sweeps the dirs in `root` whose names start with `prefix`.
fn sweep_cache_dirs(root: &Path, prefix: &str, min_age: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(prefix) || !entry.path().is_dir() {
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

/// The engine-host binaries are sibling executables in the same
/// target/{debug,release} directory as manifold itself — standard
/// Cargo workspace layout, no separate install/discovery step needed.
fn sibling_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.set_file_name(crate::cef_platform::PLATFORM.host_exe(name));
    path
}

/// Where to find each engine-host's executable.
///
/// CEF is the one exception to the plain-sibling rule: it ships its own
/// `libEGL.dll`/`libGLESv2.dll` (ANGLE), which collide with Servo's
/// differently-built copies of the same-named DLLs if both engine-hosts
/// live in the same directory — Windows resolves an implicit DLL
/// dependency from the loading EXE's own directory first, so whichever
/// engine's ANGLE build was copied there last wins for *both* engines.
/// manifold-engine-cef.exe and its full CEF runtime (libcef.dll, resources,
/// locales, its own ANGLE DLLs, ...) are isolated in `cef-runtime/` next to
/// the other binaries.
///
/// TODO: this isolation + the runtime file copying itself is currently a
/// manual post-build step (see project memory), not automated — a real
/// install/packaging step should do this, not a person running `cp`.
///
/// Falls back to `engine_runtime_dir` (under `profile::engine_data_dir()`)
/// when the sibling binary isn't there — for an engine fetched on demand
/// after install rather than bundled next to `manifold.exe`.
fn engine_binary_path(kind: EngineKind) -> PathBuf {
    let file_name = crate::cef_platform::PLATFORM.host_exe(kind.desc().binary);
    let sibling = match kind {
        EngineKind::Servo => sibling_binary(kind.desc().binary),
        EngineKind::Cef => cef_sibling_exe(),
        // WebView2Loader is a single small loader (statically linked by
        // the `webview2-com` crate, or a lone DLL at worst) — no ANGLE-style
        // filename collision with Servo/CEF's runtime files, so no isolation
        // directory needed like CEF's `cef-runtime/` above.
        EngineKind::Webview2 => sibling_binary(kind.desc().binary),
        EngineKind::Webkit => unreachable!("Webkit is spawned via wsl.exe, not a local path"),
    };
    if sibling.exists() {
        return sibling;
    }
    let Some(mut fallback) = engine_hosts_dir() else { return sibling };
    // Same isolation as the sibling-dir case above: CEF's own ANGLE DLLs
    // would collide with Servo's if both engines' fallback files ever
    // landed flat in the same `engine-hosts/` directory.
    if let Some(runtime) = engine_runtime_dir(kind) {
        fallback = runtime;
    }
    fallback.push(file_name);
    fallback
}

pub(crate) fn engine_hosts_dir() -> Option<PathBuf> {
    crate::profile::engine_data_dir().map(|mut dir| {
        dir.push("engine-hosts");
        dir
    })
}

/// `cef-runtime/<exe>` next to `manifold.exe`: where `scripts/sync-runtime-deps.sh` puts it for dev builds.
fn cef_sibling_exe() -> PathBuf {
    let mut path = sibling_binary(EngineKind::Cef.desc().binary);
    let file_name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
    path.pop();
    path.push("cef-runtime");
    path.push(file_name);
    path
}

/// The directory a downloaded runtime unpacks into (and whose `ready` marker
/// `runtime_state` reads); `None` for engines that download nothing, or
/// without a data directory.
pub(crate) fn engine_runtime_dir(kind: EngineKind) -> Option<PathBuf> {
    match kind {
        EngineKind::Cef => engine_hosts_dir().map(|dir| dir.join("cef-runtime")),
        _ => None,
    }
}

/// What CEF's runtime looks like on disk right now: a bundled sibling
/// `cef-runtime/` (dev builds) is Ready; otherwise the data-dir runtime is
/// judged by its marker.
pub(crate) fn cef_on_disk() -> RuntimeOnDisk {
    if cef_sibling_exe().exists() {
        return RuntimeOnDisk::Ready;
    }
    match (engine_runtime_dir(EngineKind::Cef), crate::cef_pin::pin_for_platform()) {
        (Some(dir), Some(pin)) => runtime_state(&dir, pin),
        _ => RuntimeOnDisk::Missing,
    }
}

/// Why Servo is Unavailable when its host exe is not where `engine_binary_path` looks.
const SERVO_HOST_MISSING: &str = "the Servo host wasn't found";

/// Whether Servo's host exe is where `engine_binary_path` finds it: Ready, or Missing.
fn servo_on_disk() -> RuntimeOnDisk {
    if engine_binary_path(EngineKind::Servo).exists() {
        RuntimeOnDisk::Ready
    } else {
        RuntimeOnDisk::Missing
    }
}

/// How long `engine_on_disk` reuses its Servo answer: the engine lists read it every frame they draw.
const SERVO_CHECK_TTL: Duration = Duration::from_secs(5);

/// The `on_disk` that `engine_availability` takes for `kind`, for the engine lists: CEF's runtime
/// (`cef_on_disk`), Servo's host exe (`servo_on_disk`, reread at most every `SERVO_CHECK_TTL`), Ready for the rest.
pub(crate) fn engine_on_disk(kind: EngineKind) -> RuntimeOnDisk {
    static SERVO: std::sync::Mutex<Option<(std::time::Instant, RuntimeOnDisk)>> = std::sync::Mutex::new(None);
    match kind {
        EngineKind::Cef => cef_on_disk(),
        EngineKind::Servo => {
            let mut check = SERVO.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match *check {
                Some((at, on_disk)) if at.elapsed() <= SERVO_CHECK_TTL => on_disk,
                _ => {
                    let on_disk = servo_on_disk();
                    *check = Some((std::time::Instant::now(), on_disk));
                    on_disk
                }
            }
        }
        EngineKind::Webview2 | EngineKind::Webkit => RuntimeOnDisk::Ready,
    }
}

/// How `kind` is available, in one place. `on_disk` is what the engine needs on disk (`engine_on_disk`):
/// Servo is Installed when its host exe is there, else Unavailable (`SERVO_HOST_MISSING`); WebView2 and
/// WebKit ignore it and are Installed; CEF follows its runtime. A custom path and a running install
/// override CEF's; a failed install overrides Missing and Stale but not a current runtime.
pub(crate) fn engine_availability(
    kind: EngineKind,
    on_disk: RuntimeOnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> EngineState {
    availability_with(
        std::env::consts::OS,
        crate::cef_pin::pin_for_platform(),
        kind,
        on_disk,
        installing,
        failed,
        custom,
    )
}

/// Why `kind` cannot run on `os` (a `std::env::consts::OS` value) at all, or `None` when it can. The one
/// place that knows: availability (every engine list) and the spawn refusal both read it.
/// WebView2 is Windows-only. WebKit runs through WSL on Windows and is hosted natively on Linux; macOS has
/// no WebKit host wired yet. (WSL is not probed: on Windows it is only found missing when a tab spawns.)
fn platform_unavailable_reason(os: &str, kind: EngineKind) -> Option<&'static str> {
    match kind {
        EngineKind::Webview2 if os != "windows" => Some("needs Windows"),
        EngineKind::Webkit if !matches!(os, "windows" | "linux") => Some("needs Windows or Linux"),
        _ => None,
    }
}

/// The engine a fresh profile starts new tabs in on `os`: the first of WebView2, Servo that the platform
/// can run. CEF is never the default (an opt-in download) and WebKit needs setup.
pub(crate) fn platform_default_engine(os: &str) -> EngineKind {
    [EngineKind::Webview2, EngineKind::Servo]
        .into_iter()
        .find(|kind| platform_unavailable_reason(os, *kind).is_none())
        .unwrap_or(EngineKind::Servo)
}

/// `engine_availability` for a given OS and pin. `None` means this platform has no CEF build, so there is
/// nothing to download: `Unavailable`, unless a runtime is already there (a dev tree's) or a custom one is set.
fn availability_with(
    os: &str,
    pin: Option<&Pin>,
    kind: EngineKind,
    on_disk: RuntimeOnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> EngineState {
    if let Some(reason) = platform_unavailable_reason(os, kind) {
        return EngineState::Unavailable(reason.to_string());
    }
    match kind {
        EngineKind::Cef => {}
        EngineKind::Servo if on_disk != RuntimeOnDisk::Ready => {
            return EngineState::Unavailable(SERVO_HOST_MISSING.to_string());
        }
        EngineKind::Servo | EngineKind::Webview2 | EngineKind::Webkit => return EngineState::Installed,
    }
    if let Some(path) = custom {
        return EngineState::Custom(path.to_path_buf());
    }
    if let Some((received, total)) = installing {
        return EngineState::Installing { received, total };
    }
    // A runtime that is there and current outranks a recorded failure (the failure was an update or a
    // retry that did not matter in the end); a failure outranks Missing and Stale.
    if on_disk == RuntimeOnDisk::Ready {
        return EngineState::Installed;
    }
    let Some(pin) = pin else { return EngineState::Unavailable(NO_PIN_REASON.to_string()) };
    if let Some(message) = failed {
        return EngineState::Failed(message.to_string());
    }
    match on_disk {
        RuntimeOnDisk::Ready => EngineState::Installed,
        RuntimeOnDisk::Missing => EngineState::NotInstalled { size: pin.size },
        RuntimeOnDisk::Stale => EngineState::UpdateNeeded { size: pin.size },
    }
}

/// Why `spawn_tab` failed.
#[derive(Debug)]
pub enum SpawnError {
    /// The engine's runtime isn't on disk; nothing was launched.
    EngineMissing(EngineKind),
    /// The engine's runtime is out of date (Update needed); nothing was launched.
    EngineOutdated(EngineKind),
    /// The engine's state is Unavailable (the reason is the state's), so nothing was launched.
    EngineUnavailable(EngineKind, String),
    Io(std::io::Error),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::EngineMissing(kind) => {
                write!(f, "{} is not installed", crate::engine_info::engine_label(*kind))
            }
            SpawnError::EngineOutdated(kind) => {
                write!(f, "{} needs an update", crate::engine_info::engine_label(*kind))
            }
            SpawnError::EngineUnavailable(kind, reason) => {
                f.write_str(&crate::engine_info::PickOutcome::unavailable_toast(*kind, reason))
            }
            SpawnError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for SpawnError {}

impl From<std::io::Error> for SpawnError {
    fn from(e: std::io::Error) -> Self {
        SpawnError::Io(e)
    }
}

/// Where the host runs from when a custom CEF directory is in use: a copy of the host exe inside it.
///
/// Windows resolves the host's implicit `libcef.dll` from the exe's own directory before `PATH`, so the
/// only way to make the host load the custom runtime is for the host to live there. (A PATH entry would
/// lose to any `libcef.dll` beside the original host, and a clean machine has no host beside a bring-your-own
/// runtime at all.)
fn custom_host_path(custom_dir: &Path, host_exe: &Path) -> PathBuf {
    custom_dir.join(host_exe.file_name().unwrap_or_default())
}

/// Spawns run on their own threads, so two can reach `ensure_host_copy` at once; one copy at a time.
static HOST_COPY: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Makes `custom_host_path` hold the current host exe: copied when missing, replaced when the source's
/// size or modification time differs, left alone otherwise. Returns the path to launch.
///
/// The copy goes to a uniquely named file in the same directory and is renamed into place, under a
/// process-wide lock, so a launching host never sees a half-written exe and concurrent spawns copy once.
/// A destination that already matches is current even when a running host has it locked.
fn ensure_host_copy(custom_dir: &Path, host_exe: &Path) -> std::io::Result<PathBuf> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _guard = HOST_COPY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = std::fs::metadata(host_exe).map_err(|e| {
        std::io::Error::new(e.kind(), format!("the CEF host {} was not found: {e}", host_exe.display()))
    })?;
    let dest = custom_host_path(custom_dir, host_exe);
    let same = std::fs::metadata(&dest)
        .map(|m| m.len() == source.len() && m.modified().ok() == source.modified().ok())
        .unwrap_or(false);
    if !same {
        let wrap = |e: std::io::Error| {
            std::io::Error::new(e.kind(), format!("could not copy the CEF host into {}: {e}", custom_dir.display()))
        };
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = custom_dir.join(format!(
            "{}.{}-{n}.tmp",
            dest.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        std::fs::copy(host_exe, &temp).map_err(wrap)?;
        // `fs::copy` carries no mtime on every platform; set it so the next launch can skip.
        if let Ok(modified) = source.modified() {
            if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&temp) {
                let _ = file.set_modified(modified);
            }
        }
        if let Err(e) = std::fs::rename(&temp, &dest) {
            let _ = std::fs::remove_file(&temp);
            return Err(wrap(e));
        }
    }
    Ok(dest)
}

/// The CEF host exe that ships with the app: beside `manifold.exe` in a packaged build, else the
/// `cef-runtime/` copy a dev build keeps. This is what gets copied next to a runtime that has no host.
fn shipped_cef_host() -> PathBuf {
    let flat = sibling_binary(EngineKind::Cef.desc().binary);
    if flat.exists() {
        flat
    } else {
        cef_sibling_exe()
    }
}

/// The directory a CEF host copy must live in (see `custom_host_path`), or None to run the host as
/// `engine_binary_path` finds it. A custom directory wins; a dev build's own `cef-runtime/` already holds
/// its host; otherwise the downloaded runtime, which holds libcef but never a host, so it gets a copy too.
fn host_home(custom: Option<&Path>, dev_runtime_has_host: bool, downloaded: Option<PathBuf>) -> Option<PathBuf> {
    match custom {
        Some(dir) => Some(dir.to_path_buf()),
        None if dev_runtime_has_host => None,
        None => downloaded,
    }
}

/// Refuses to launch an engine whose state (from disk alone) isn't Installed or Custom: by its reason when
/// Unavailable (Servo without its host exe), as needing an update when Update needed, else as not installed.
fn check_spawnable(kind: EngineKind, on_disk: RuntimeOnDisk, custom: Option<&Path>) -> Result<(), SpawnError> {
    match engine_availability(kind, on_disk, None, None, custom) {
        EngineState::Installed | EngineState::Custom(_) => Ok(()),
        EngineState::Unavailable(reason) => Err(SpawnError::EngineUnavailable(kind, reason)),
        EngineState::UpdateNeeded { .. } => Err(SpawnError::EngineOutdated(kind)),
        _ => Err(SpawnError::EngineMissing(kind)),
    }
}

/// The spawn refusal for an engine this OS cannot run, from the same `platform_unavailable_reason` the
/// engine lists show, so a spawn fails by name instead of a bare "file not found".
fn unsupported_on(os: &str, kind: EngineKind) -> Option<String> {
    platform_unavailable_reason(os, kind).map(|reason| crate::engine_info::PickOutcome::unavailable_toast(kind, reason))
}

/// Why a WebKit spawn cannot start on `os` although WebKit counts as available there, or `None`. The only
/// WebKit host is reached through `wsl.exe` (`spawn_webkit_tab`); on Linux, where it would run natively,
/// that launch is not wired yet, so the spawn says so instead of failing on a missing `wsl.exe`.
fn webkit_host_missing(os: &str) -> Option<&'static str> {
    (os == "linux").then_some("WebKit can't start yet: a native Linux WebKit host is not wired up")
}

/// Converts this repo's Windows path (wherever manifold.exe actually is)
/// to the `/mnt/c/...` form WSL2 uses to see the same filesystem, so we can
/// hand `wsl.exe` a path to the manifold-engine-webkit binary it built.
fn windows_path_to_wsl(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    // "C:/Users/..." -> "/mnt/c/Users/..."
    if let Some(drive) = s.chars().next() {
        if s.as_bytes().get(1) == Some(&b':') {
            let rest = &s[2..];
            return format!("/mnt/{}{}", drive.to_ascii_lowercase(), rest);
        }
    }
    s
}

/// POSIX single-quote escaping for the one path `webkit_shell_command` splices into its `bash -c`
/// string: derived from the running exe's location, never from a page.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// manifold-engine-webkit is built *inside* WSL2 (it links against WebKitGTK, a
/// Linux-only library) into its own `target-linux/` directory rather than
/// the shared `target/` — that directory is also being written to by the
/// Windows-native build for every other crate, and a Linux host's "default
/// target" build and a Windows host's would otherwise collide there.
fn webkit_binary_path_in_repo() -> PathBuf {
    let mut path = sibling_binary("manifold"); // .../target/debug/manifold.exe
    path.pop(); // .../target/debug
    path.pop(); // .../target
    path.pop(); // repo root
    path.push("target-linux");
    path.push("debug");
    path.push("manifold-engine-webkit");
    path
}

/// During a swap, the old engine-host on this same `tab_id` was just killed
/// (see `App::finish_swap`'s `drop(self.tab.take())`) — but a named pipe's
/// OS-level cleanup after `TerminateProcess` isn't always instantaneous
/// relative to `child.wait()` returning, so creating a new listener with
/// the same name right away can transiently fail with "Access is denied".
/// A few short retries clears it without needing an arbitrary fixed delay
/// on every spawn (including the common non-swap case, where it's unused).
fn listen_local_pair_with_retry(socket_id: &str) -> std::io::Result<transport::LocalListenerPair> {
    const ATTEMPTS: u32 = 10;
    let mut last_err = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        match transport::listen_local_pair(socket_id) {
            Ok(listeners) => return Ok(listeners),
            Err(err) => last_err = Some(err),
        }
    }
    let err = last_err.unwrap();
    // A collision this persistent (as opposed to the transient
    // "Access is denied" this retry loop is normally for, see the doc
    // comment above) usually means some *other* process — often a stale
    // engine-host left over from a previous crashed/killed run — is still
    // holding `socket_id`'s named pipe. Surface the OS error code and the
    // name we tried, since the bare `err` alone (e.g. just "os error 5")
    // gives no hint of which pipe or that a leftover process is the likely
    // cause. `spawn_tab` retries this once more with a fresh `socket_id`
    // before giving up for good — see its stale-pipe fallback.
    Err(std::io::Error::new(
        err.kind(),
        format!(
            "failed to create named pipe for '{socket_id}' after {ATTEMPTS} attempts \
             ({:.1}s): {err} (raw OS error: {:?}). A leftover engine-host process from a \
             previous run may still be holding this pipe.",
            (ATTEMPTS.saturating_sub(1)) as f64 * 0.05,
            err.raw_os_error(),
        ),
    ))
}

/// Disambiguator for the stale-pipe fallback below — distinct per call
/// within this process, so repeated respawns of the same `tab_id` (e.g. a
/// crash-restart loop) never collide with an earlier fallback attempt
/// that's itself still stuck.
static SOCKET_ID_FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

fn fresh_fallback_socket_id(tab_id: &str) -> String {
    let n = SOCKET_ID_FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{tab_id}-r{}-{n}", std::process::id())
}

fn per_tab_log_files(tab_id: &str) -> std::io::Result<(std::fs::File, std::fs::File)> {
    // Explicit per-tab log file rather than relying on inherited
    // stdout/stderr: when manifold itself is launched with its stdio
    // redirected (e.g. `manifold.exe > file` from bash), that redirection
    // does not reliably propagate to a grandchild process spawned from
    // inside it, so an inherited engine-host produces no visible output at
    // all even though it runs fine.
    let mut stdout_file = open_capped_log(&per_tab_log_path(tab_id), MAX_TAB_LOG_BYTES)?;
    // Appended, not truncated: a second manifold, or a respawn racing the old
    // host's last writes, used to wipe the log that explains a crash.
    use std::io::Write as _;
    let _ = writeln!(stdout_file, "{}", spawn_header(tab_id));
    let stderr_file = stdout_file.try_clone()?;
    Ok((stdout_file, stderr_file))
}

/// Opens `path` for appending, emptying it first if it is over `cap` bytes (a verbose engine respawned for weeks
/// would otherwise fill %TEMP%).
fn open_capped_log(path: &Path, cap: u64) -> std::io::Result<std::fs::File> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > cap) {
        let _ = std::fs::remove_file(path);
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path)
}

fn per_tab_log_path(tab_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("manifold-{tab_id}.log"))
}

/// Waits for a just-spawned same-machine host to connect, giving up early if
/// it exits. On failure the child is killed and reaped, and the error says
/// which way it failed: "exited with code N before connecting: <the last
/// lines of its log since this spawn's header>", or "still running but did not connect within ...".
/// A host that exited after writing an `ipc_protocol::HOST_FATAL` line (among
/// those last lines) gets that line's reason alone, e.g. "CEF runtime doesn't support CEF API version ...".
/// The log is the per-tab file the child writes its stdout and stderr to.
fn connect_host(
    listeners: &transport::LocalListenerPair,
    child: &mut Child,
    log_path: &Path,
    timeout: Duration,
) -> std::io::Result<(LocalTransport, LocalTransport)> {
    let mut exited = None;
    let result = transport::accept_local_pair_while(listeners, timeout, &mut || match child.try_wait() {
        Ok(Some(status)) => {
            exited = Some(status);
            false
        }
        _ => true,
    });
    let err = match result {
        Ok(pair) => return Ok(pair),
        Err(err) => err,
    };
    // It may have exited during the final poll interval, after the last check.
    let exited = exited.or_else(|| child.try_wait().ok().flatten());
    let _ = child.kill();
    let _ = child.wait();
    let message = match exited {
        Some(status) => {
            let mut lines = log_tail_lines(log_path, 5);
            // This spawn's lines only (from its header on): the log is appended to across spawns.
            if let Some(header) = lines.iter().rposition(|l| is_spawn_header(l)) {
                lines.drain(..header);
            }
            // The host said why in a line of its own (`HOST_FATAL`): that line alone is the reason.
            if let Some(reason) = host_fatal(&lines) {
                return Err(std::io::Error::new(err.kind(), reason));
            }
            let how = match status.code() {
                Some(code) if code < 0 => format!("with code {:#x}", code as u32),
                Some(code) => format!("with code {code}"),
                None => format!("({status})"),
            };
            let tail = lines.join(" | ");
            let tail = if tail.is_empty() { String::from(" (its log is empty)") } else { format!(": {tail}") };
            format!("engine-host exited {how} before connecting{tail}")
        }
        None if err.kind() == std::io::ErrorKind::TimedOut => {
            format!("engine-host is still running but did not connect within {timeout:?} (log: {})", log_path.display())
        }
        None => return Err(err),
    };
    Err(std::io::Error::new(err.kind(), message))
}

/// The last `lines` non-empty lines of a log, trimmed, read from at most its
/// last 16 KiB. Empty when there is no such file.
fn log_tail_lines(path: &Path, lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut bytes = Vec::new();
    if let Ok(mut file) = std::fs::File::open(path) {
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(len.saturating_sub(16 * 1024)));
        let _ = file.read_to_end(&mut bytes);
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut tail: Vec<String> =
        text.lines().map(str::trim).filter(|l| !l.is_empty()).rev().take(lines).map(String::from).collect();
    tail.reverse();
    tail
}

/// The reason in the last `ipc_protocol::HOST_FATAL` line among `lines`, without the prefix. Only lines
/// after the last spawn header (`spawn_header`) count: the log is appended to, so a line above it is an
/// earlier host's.
fn host_fatal(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .rev()
        .take_while(|l| !is_spawn_header(l))
        .find_map(|l| l.strip_prefix(ipc_protocol::HOST_FATAL))
        .map(|r| r.trim().to_string())
}

/// Starts the line `per_tab_log_files` writes before each spawn.
const SPAWN_HEADER_START: &str = "---- manifold pid ";

/// The line `per_tab_log_files` writes to a tab's log before each spawn.
fn spawn_header(tab_id: &str) -> String {
    format!("{SPAWN_HEADER_START}{} spawning a host for {tab_id} ----", std::process::id())
}

fn is_spawn_header(line: &str) -> bool {
    line.starts_with(SPAWN_HEADER_START) && line.contains(" spawning a host for ")
}

/// Synthesizes `EngineToHost::Crashed` when the connection to an
/// engine-host drops, since a hard-killed/panicked process cannot send a
/// message about its own death. Also fires on an *intentional* teardown
/// (a swap's old tab getting dropped, which sends `Shutdown` then kills
/// the child) — harmless there: `tx`'s paired `rx` was owned by that same
/// now-dropped `EngineTab`, so the `send` below simply fails and is
/// ignored; only a tab that's still the *current* one has a live `rx` to
/// actually receive this.
///
/// Reads the engine's `PROTOCOL_VERSION` first. A stale host (built from an
/// older `messages.rs`) gets a `Crashed` whose reason starts with
/// `PROTOCOL_MISMATCH`, so no message of it is ever misread.
///
/// Frames go to `frame` (newest wins); everything else goes through a channel of `engine_io::EVENTS_CAP`, whose
/// `send` waits while the app is that far behind (survey E3).
fn spawn_reader_thread<S: std::io::Read + std::io::Write + Send + 'static>(
    mut from_engine: ipc_protocol::MessageChannel<S>,
    frame: engine_io::FrameSlot,
    wake: engine_io::Wake,
) -> mpsc::Receiver<EngineToHost> {
    let (tx, rx) = mpsc::sync_channel(engine_io::EVENTS_CAP);
    std::thread::spawn(move || {
        let lost = || EngineToHost::Crashed { reason: "engine-host connection lost".to_string() };
        match from_engine.recv_version().map(ipc_protocol::check_version) {
            Ok(Ok(())) => {}
            Ok(Err(mismatch)) => {
                eprintln!("{mismatch}");
                let _ = tx.send(EngineToHost::Crashed { reason: mismatch });
                wake();
                return;
            }
            Err(_) => {
                let _ = tx.send(lost());
                wake();
                return;
            }
        }
        while let Ok(msg) = from_engine.recv::<EngineToHost>() {
            match msg {
                EngineToHost::FrameReady { width, height, pixels, .. } => {
                    *frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(engine_io::Frame { width, height, pixels });
                }
                // Fails once the tab is gone (its receiver dropped).
                msg => {
                    let sent = tx.send(msg);
                    wake();
                    if sent.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(lost());
        wake();
    });
    rx
}

/// Same-machine (CEF/Servo) equivalent of `WEBKIT_CONNECT_TIMEOUT`, much
/// shorter since there's no cold-VM-boot equivalent to wait out here — a
/// same-machine child that hasn't connected within a few seconds of
/// spawning has failed (crashed, or a missing/mismatched binary), not
/// merely slow to start.
const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawns an engine-host for `kind`, wires up its control channel(s), and
/// returns a handle the process manager (and the tab-swap logic) can drive.
///
/// `tab_id`/`initial_url` are passed via env vars, not argv, for
/// manifold-engine-cef specifically — see that crate's main.rs for why (CEF
/// parses this process's own argv as its own command line). We do the same
/// for the other engines too, for one consistent spawn path.
///
/// `custom_cef` is a validated bring-your-own CEF directory: it satisfies the availability check, and the
/// host runs from a copy inside it so Windows loads that directory's `libcef.dll` (`ensure_host_copy`).
///
/// `wake` is called by the tab's reader thread after each engine message (`event_loop::engine_wake`).
pub fn spawn_tab(
    kind: EngineKind,
    tab_id: &str,
    initial_url: &str,
    custom_cef: Option<&Path>,
    jar_json: &dyn Fn() -> String,
    wake: engine_io::Wake,
) -> Result<EngineTab, SpawnError> {
    if let Some(reason) = unsupported_on(std::env::consts::OS, kind) {
        return Err(SpawnError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, reason)));
    }
    if kind == EngineKind::Webkit {
        if let Some(reason) = webkit_host_missing(std::env::consts::OS) {
            return Err(SpawnError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, reason)));
        }
        return Ok(spawn_webkit_tab(tab_id, initial_url, wake)?);
    }
    let custom_cef = custom_cef.filter(|_| kind == EngineKind::Cef);
    // Read fresh, not through `engine_on_disk`'s cache: a Servo host built a moment ago should launch.
    let on_disk = match kind {
        EngineKind::Servo => servo_on_disk(),
        _ => engine_on_disk(kind),
    };
    check_spawnable(kind, on_disk, custom_cef)?;

    // Normally the socket/shared-memory id is just tab_id. But a prior run
    // that crashed (rather than exiting cleanly) can leave a stale
    // engine-host-*.exe still holding tab_id's named pipe indefinitely —
    // listen_local_pair_with_retry's short retries won't outlast that. In
    // that case, fall back once to a fresh disambiguating id for
    // socket/shared-memory naming only; tab_id itself keeps meaning what it
    // already means everywhere else in manifold (TabNode map key, UI).
    let (socket_id, listeners) = match listen_local_pair_with_retry(tab_id) {
        Ok(listeners) => (tab_id.to_string(), listeners),
        Err(primary_err) => {
            let fallback_id = fresh_fallback_socket_id(tab_id);
            match listen_local_pair_with_retry(&fallback_id) {
                Ok(listeners) => (fallback_id, listeners),
                Err(_) => return Err(primary_err.into()),
            }
        }
    };
    let (stdout_file, stderr_file) = per_tab_log_files(tab_id)?;
    let host_exe = match (kind, host_home(custom_cef, cef_sibling_exe().exists(), engine_runtime_dir(kind))) {
        (EngineKind::Cef, Some(dir)) => ensure_host_copy(&dir, &shipped_cef_host())?,
        _ => engine_binary_path(kind),
    };

    // Each CEF engine-host gets its own throwaway cache dir, so CEF tabs
    // don't share cookies or logins and keep none across a restart (see
    // manifold-engine-cef's main.rs for why they can't share one).
    let cache_dir = (kind == EngineKind::Cef).then(|| {
        let n = CEF_CACHE_COUNTER.fetch_add(1, Ordering::Relaxed);
        cef_cache_root().join(format!("{}-{n}", std::process::id()))
    });

    let mut command = Command::new(&host_exe);
    command
        .env("ENGINE_TAB_ID", tab_id)
        .env("ENGINE_SOCKET_ID", &socket_id)
        .env("ENGINE_INITIAL_URL", initial_url)
        .stdout(stdout_file)
        .stderr(stderr_file);
    // These hosts wait for the jar (`SetCookies`) before their first navigation.
    if matches!(kind, EngineKind::Webview2 | EngineKind::Cef) {
        command.env("ENGINE_AWAIT_COOKIES", "1");
    }
    if let Some(dir) = &cache_dir {
        command.env("ENGINE_CACHE_DIR", dir);
    }
    // Each WebView2 host gets a profile name of its own among the live hosts, reused once a host is gone.
    let profile_slot = (kind == EngineKind::Webview2).then(|| claim_webview2_slot(&host_exe));
    if let Some(n) = profile_slot {
        command.env("ENGINE_WEBVIEW2_PROFILE", webview2_profile_name(n));
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            if let Some(n) = profile_slot {
                free_webview2_slot(n);
            }
            return Err(e.into());
        }
    };
    assign_to_engine_job(&child);
    let cache_dir = cache_dir.or_else(|| profile_slot.map(|n| webview2_profile_dir(&host_exe, n)));

    // The child connects to both sockets right after spawning — bounded by
    // LOCAL_CONNECT_TIMEOUT rather than an unbounded accept() (see its own
    // doc comment, and accept_remote's, for why an unbounded wait here on
    // manifold's main/event-loop thread is a real hazard, not just
    // theoretical caution). On timeout (or the child crashing/exiting
    // before connecting, e.g. missing runtime DLLs), kill it explicitly —
    // `Child`'s own `Drop` does not, so leaving this `?` to just propagate
    // would leak the process, exactly the kind of leftover
    // `listen_local_pair_with_retry`'s own error message above warns
    // about. Same cleanup `spawn_webkit_tab`'s own accept-timeout path
    // already does. `connect_host` does the kill, stops waiting as soon as
    // the child exits, and names its exit code and last log lines.
    let (to_engine, from_engine) =
        match connect_host(&listeners, &mut child, &per_tab_log_path(tab_id), LOCAL_CONNECT_TIMEOUT) {
            Ok(pair) => pair,
            Err(e) => {
                if let Some(dir) = cache_dir {
                    remove_cache_dir(dir, profile_slot);
                }
                return Err(e.into());
            }
        };
    let frame = engine_io::FrameSlot::default();
    let events_rx = spawn_reader_thread(from_engine, Arc::clone(&frame), wake);

    let writer = engine_io::Writer::spawn(to_engine, &format!("engine writer {socket_id}"));
    let tab = EngineTab { kind, socket_id, writer, events_rx, frame, child: Some(child), cache_dir, profile_slot };
    if matches!(kind, EngineKind::Webview2 | EngineKind::Cef) {
        // Serialised now that the host is connected, so cookies merged while it started are in the seed.
        tab.send(&HostToEngine::SetCookies(jar_json()));
    }
    Ok(tab)
}

/// Generous upper bound for a cold WSL2 VM boot + Xvfb readiness + GTK/
/// WebKit init, not a tuned-tight value — see `accept_remote`'s doc comment
/// for why this exists at all. Failing loudly (a failed card the user
/// can see and retry) well within a minute beats an unbounded app freeze.
const WEBKIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

/// The `bash -c` string `spawn_webkit_tab` hands to `wsl.exe -d Ubuntu -- bash -c`. It crosses two
/// shells there (the distro's login shell parses what `wsl.exe` re-joins, then bash), so text from a
/// page must not appear in it as itself.
fn webkit_shell_command(
    initial_url: &str,
    port: u16,
    token: &transport::Token,
    binary_path: &str,
) -> std::io::Result<String> {
    /// Windows' command line holds 32 767 characters; leave the rest of this one room.
    const MAX_URL_HEX: usize = 16_384;
    let url_hex = transport::to_hex(initial_url.as_bytes());
    if url_hex.len() > MAX_URL_HEX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the URL is too long to start a WebKit tab with",
        ));
    }
    Ok(format!(
        "ENGINE_INITIAL_URL_HEX={url_hex} ENGINE_TCP_PORT={port} ENGINE_TOKEN={} \
         ENGINE_HOST_IP=$(ip route | awk '/^default/ {{print $3}}') xvfb-run -a {}",
        transport::to_hex(token),
        shell_quote(binary_path),
    ))
}

fn spawn_webkit_tab(tab_id: &str, initial_url: &str, wake: engine_io::Wake) -> std::io::Result<EngineTab> {
    // Bind an ephemeral port (0 = "pick one for me") so concurrent tabs
    // don't collide. Bound on all interfaces, not just loopback: WSL2's
    // NAT-mode networking refuses (ECONNREFUSED, verified directly) a
    // WSL2-side connection to 127.0.0.1:port on the Windows host — the
    // guest must instead reach the host at its own default-gateway IP,
    // which the command string below discovers at spawn time (see
    // ENGINE_HOST_IP).
    let listener = TcpListener::bind(("0.0.0.0", 0))?;
    let port = listener.local_addr()?.port();

    let (stdout_file, stderr_file) = per_tab_log_files(tab_id)?;
    let binary_path = windows_path_to_wsl(&webkit_binary_path_in_repo());

    // `wsl.exe` does NOT forward this Command's environment into the
    // Linux process (verified: env vars set via `.env()` here simply don't
    // appear on the other side) — WSL has its own opt-in `WSLENV`
    // passthrough mechanism instead, which is more trouble than it's
    // worth here. Pass values as inline shell assignments in the command
    // string instead, and run under `xvfb-run` for the virtual display
    // GTK's offscreen rendering needs.
    // The Windows host isn't reachable from WSL2 at 127.0.0.1 (see
    // listen_remote's doc comment) — only at its default-gateway IP, which
    // changes across WSL2 restarts, so it's discovered fresh here rather
    // than hardcoded.
    // A random secret the engine must send before it is accepted: the listener is on
    // every interface (`listen_remote`). It sits in this command line, as hex, visible to the
    // same user's processes only.
    let mut token = [0u8; transport::TOKEN_LEN];
    getrandom::fill(&mut token).map_err(|e| std::io::Error::other(e.to_string()))?;
    let command = webkit_shell_command(initial_url, port, &token, &binary_path)?;
    let mut child = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--", "bash", "-c", &command])
        .stdout(stdout_file)
        .stderr(stderr_file)
        .spawn()?;
    // Only reaches the Windows-side `wsl.exe` relay, same limitation
    // `Drop for EngineTab`'s own doc comment already calls out for
    // explicit `Shutdown`-then-kill -- the actual Linux-side process tree
    // isn't part of this job and survives a crash regardless. Still a
    // strict improvement over today (the relay itself no longer orphans).
    assign_to_engine_job(&child);

    // The child connects right after spawning. Bounded, not the unbounded
    // wait `TcpListener::accept` alone would give: this runs on manifold's
    // own main/event-loop thread (`spawn_root_tab` runs from winit's
    // `resumed()`), so an engine-host that never connects — a cold WSL2 VM
    // boot, Xvfb taking unusually long to come up, etc. — would otherwise
    // freeze the entire app (redraw, input, headless stdin commands, all of
    // it) indefinitely; confirmed live to occasionally take minutes-to-hours
    // rather than the sub-second common case. On timeout, kill the
    // half-started `wsl.exe` launcher before surfacing the error — leaving
    // it running would just orphan a second attempt at the same port/tab
    // the next time this is retried.
    let control = match transport::accept_remote(&listener, WEBKIT_CONNECT_TIMEOUT, &token) {
        Ok(control) => control,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    let stream = control.into_inner();
    let read_stream = stream.try_clone()?;
    let to_engine = ipc_protocol::MessageChannel::new(stream);
    let from_engine = ipc_protocol::MessageChannel::new(read_stream);

    let frame = engine_io::FrameSlot::default();
    let events_rx = spawn_reader_thread(from_engine, Arc::clone(&frame), wake);

    Ok(EngineTab {
        kind: EngineKind::Webkit,
        // No local named pipe/shared memory for the WSL2-hosted case (see
        // `RemoteTransport`), so there's nothing to
        // disambiguate — tab_id is fine as-is.
        socket_id: tab_id.to_string(),
        writer: engine_io::Writer::spawn(to_engine, &format!("engine writer {tab_id}")),
        events_rx,
        frame,
        child: Some(child),
        cache_dir: None,
        profile_slot: None,
    })
}

pub(crate) mod engine_io;
#[cfg(test)]
mod tests;
