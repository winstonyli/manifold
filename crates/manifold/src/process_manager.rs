//! Engine kinds and the view-host child process behind a tab: spawn, connect, read and reap. Its child modules hold
//! the kill-on-exit job (`job`), profile and cache folders (`profiles`), engine availability (`availability`), the
//! WSL-hosted WebKit spawn (`webkit`) and the control channel (`view_io`).

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::embedded_host;
use crate::view_install::{
    downloads, has_runtime, host_components, host_state, plan_for, runtime_state, verify_installed_host, OnDisk, Plan,
    RuntimeOnDisk, ViewStatus,
};
use ipc_protocol::transport::{self, LocalTransport};
use ipc_protocol::{HostToView, ViewToHost};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewKind {
    Servo,
    Cef,
    /// WebKit, via WebKitGTK — hosted inside WSL2 (no native Windows
    /// embedding story for WebKit exists; see project plan/memory).
    Webkit,
    /// Microsoft Edge WebView2 — native Windows API, same-machine process
    /// like Servo/Cef (no WSL2 relay needed).
    Webview2,
}

/// Where an engine's host executable comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostSource {
    /// Carried inside `manifold.exe` when the build embeds it (`MANIFOLD_WEBVIEW2_HOST`, see `embedded_host`) and
    /// extracted at spawn when no sibling host is there. A build that embeds nothing finds it like a sibling.
    Embedded,
    /// Pinned in `host_pin` and installed on demand into `engine-hosts/` (`view_install`), unless a sibling
    /// host (a dev tree's, or a Linux package's) is already there.
    Download,
}

/// What each engine is: the one place its name, store code, binary, label,
/// pixel order and jar use live. `desc` is an exhaustive `match`, so a new
/// `ViewKind` fails to compile until it has a row here.
pub(crate) struct ViewDesc {
    /// The string `settings.json`, session exports and the `view_kind` store field hold.
    pub(crate) name: &'static str,
    /// The store's `view_kind` field code: `store::model::ENGINES[code - 1]` is `name`.
    pub(crate) code: u64,
    pub(crate) binary: &'static str,
    pub(crate) label: &'static str,
    /// Frames arrive as BGRA (CEF, WebView2) rather than RGBA (Servo, WebKit).
    pub(crate) bgra: bool,
    /// Takes part in the cookie jar: waits for `SetCookies` before its first navigation (`VIEW_AWAIT_COOKIES`), and
    /// answers `GetCookies` (spec 2026-10-02-jars-design). The rest are neither seeded nor pulled, and a jar choice
    /// on their tabs shows "This view does not use jars yet".
    pub(crate) jar: bool,
    /// Opens its own DevTools window on `OpenDevTools` (CEF, WebView2). Servo has no inspector API; WebKit's
    /// inspector needs a window manager its Xvfb session doesn't have. Without it, F12 toasts instead.
    pub(crate) devtools: bool,
    /// Answers `FindStart` with `FindResult` (all but Servo, whose embedding API has no find). Without it, Ctrl+F
    /// toasts instead of opening a find bar that never gets a result.
    pub(crate) find: bool,
    pub(crate) source: HostSource,
}

impl ViewKind {
    pub(crate) const ALL: [ViewKind; 4] = [ViewKind::Servo, ViewKind::Cef, ViewKind::Webkit, ViewKind::Webview2];

    pub(crate) fn desc(self) -> ViewDesc {
        match self {
            ViewKind::Servo => ViewDesc {
                name: "servo",
                code: 1,
                binary: "manifold-engine-servo",
                label: "Servo",
                bgra: false,
                jar: false,
                devtools: false,
                find: false,
                source: HostSource::Download,
            },
            ViewKind::Cef => ViewDesc {
                name: "cef",
                code: 2,
                binary: "manifold-engine-cef",
                label: "Blink (CEF)",
                bgra: true,
                jar: true,
                devtools: true,
                find: true,
                source: HostSource::Download,
            },
            ViewKind::Webkit => ViewDesc {
                name: "webkit",
                code: 3,
                binary: "manifold-engine-webkit",
                label: "WebKit",
                bgra: false,
                jar: false,
                devtools: false,
                find: true,
                source: HostSource::Download,
            },
            ViewKind::Webview2 => ViewDesc {
                name: "webview2",
                code: 4,
                binary: "manifold-engine-webview2",
                label: "WebView2",
                bgra: true,
                jar: true,
                devtools: true,
                find: true,
                source: HostSource::Embedded,
            },
        }
    }

    /// Case-insensitive, so the headless `SPLIT` argument and the stored
    /// lowercase names share one parser.
    pub(crate) fn from_name(name: &str) -> Option<ViewKind> {
        Self::ALL.into_iter().find(|k| k.desc().name.eq_ignore_ascii_case(name))
    }

    pub(crate) fn from_code(code: u64) -> Option<ViewKind> {
        Self::ALL.into_iter().find(|k| k.desc().code == code)
    }
}

/// `ViewKind` itself has no reason to derive serde (it's an internal
/// process-management type) — a small manual string mapping instead, written
/// by the two independent persistence formats that each need to store one
/// (`settings.rs`'s default-starting-engine, `session.rs`'s per-tab engine).
/// Serialize only: each format reads it back its own way (settings, leniently).
pub(crate) mod view_kind_serde {
    use super::ViewKind;
    use serde::{Serialize, Serializer};

    pub fn serialize<S: Serializer>(kind: &ViewKind, s: S) -> Result<S::Ok, S::Error> {
        kind.desc().name.serialize(s)
    }
}

/// A running view-host child process plus the control channel
/// manifold talks to it on. `manifold` is the process manager: it
/// owns spawning, tearing down, and (from M2 on) swapping the engine
/// backing a tab.
pub struct ViewTab {
    pub kind: ViewKind,
    /// The id used to name this tab's control-pipe/shared-memory socket —
    /// normally equal to its `tab_id`, but can diverge after the
    /// stale-pipe fallback in `spawn_tab`. manifold's own bookkeeping
    /// (the `TabNode` map, UI) keys off `tab_id`, never this; only
    /// low-level socket/shared-memory lookups (e.g. `SharedFrameBuffer`)
    /// need it.
    pub socket_id: String,
    /// Sends to the view-host off the UI thread (`view_io::Writer`), over its local pipe or, for the WSL2-hosted
    /// WebKit, its TCP connection (frames then travel inline in `FrameReady`).
    writer: view_io::Writer,
    pub events_rx: mpsc::Receiver<ViewToHost>,
    /// The newest frame its reader saw (`view_io::FrameSlot`), taken by `take_frame`.
    frame: view_io::FrameSlot,
    /// The mapping of the engine's current frame file (`view_io::FrameMap`), used by `frame_pixels`.
    frame_map: view_io::FrameMap,
    /// `None` once `Drop` has handed it to its reaper thread.
    child: Option<Child>,
    /// A directory only this host uses, deleted on drop: CEF's `root_cache_path`, or WebView2's named profile
    /// (`webview2_profile_dir`).
    cache_dir: Option<PathBuf>,
    /// A WebView2 host's profile slot (`claim_webview2_slot`), freed once `cache_dir` is gone.
    profile_slot: Option<usize>,
}

impl ViewTab {
    /// Queues `msg` for the engine and returns at once. A run of pointer moves, resizes or wheel steps the engine has
    /// not read yet merges into one (`view_io::coalesce`). Dropped when the engine has stopped reading and its queue
    /// is full (the tab then reads as unresponsive), and after `Shutdown`.
    pub fn send(&self, msg: &HostToView) {
        self.writer.send(msg);
    }

    /// M5: at most one tab is ever visible (actively capturing/sending
    /// frames) at a time — see `HostToView::SetVisible`'s doc comment.
    pub fn set_visible(&self, visible: bool) {
        self.send(&HostToView::SetVisible(visible));
    }

    /// Queues `Shutdown` now. An app exit calls this on every tab before dropping them, so `Drop` then waits one
    /// `view_io::SHUTDOWN_FLUSH` for all of them together.
    pub(crate) fn begin_shutdown(&self) {
        self.writer.shutdown();
    }

    /// The newest frame since the last call, if any.
    pub(crate) fn take_frame(&self) -> Option<view_io::Frame> {
        self.frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }

    /// The pixels of the engine's frame file of `generation` (`view_io::FrameMap::pixels`).
    pub(crate) fn frame_pixels(&mut self, generation: u32, width: u32, height: u32) -> std::io::Result<&[u8]> {
        self.frame_map.pixels(&self.socket_id, generation, width, height)
    }

    /// The engine has stopped reading: a send was refused while a write had been blocked for
    /// `view_io::REFUSED_BLOCKED_AFTER`, or one write has been stuck for `view_io::UNRESPONSIVE_AFTER` (the card
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
    /// reads what the app sends it (`HostToView`), timing out after 200 ms when nothing more comes; the sender
    /// feeds the tab's events as its reader would.
    #[cfg(test)]
    pub(crate) fn fake_fed(kind: ViewKind) -> (ViewTab, FakePeer, mpsc::SyncSender<ViewToHost>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let host = std::net::TcpStream::connect(listener.local_addr().expect("local addr")).expect("connect");
        let (peer, _) = listener.accept().expect("accept");
        peer.set_read_timeout(Some(Duration::from_millis(200))).expect("read timeout");
        let (feed, events_rx) = mpsc::sync_channel(view_io::EVENTS_CAP);
        let tab = ViewTab {
            kind,
            socket_id: String::new(),
            writer: view_io::Writer::spawn(transport::RemoteTransport::new(host), "engine writer (fake)"),
            events_rx,
            frame: view_io::FrameSlot::default(),
            frame_map: view_io::FrameMap::default(),
            child: None,
            cache_dir: None,
            profile_slot: None,
        };
        let writer = tab.writer.clone();
        (tab, FakePeer { channel: ipc_protocol::MessageChannel::new(peer), writer }, feed)
    }

    /// `fake_fed` without the feed: its events channel is closed and empty.
    #[cfg(test)]
    pub(crate) fn fake(kind: ViewKind) -> (ViewTab, FakePeer) {
        let (tab, peer, _) = Self::fake_fed(kind);
        (tab, peer)
    }
}

/// The engine end of a `fake_fed` tab: its channel, plus the tab's writer to know when everything sent has arrived.
#[cfg(test)]
pub(crate) struct FakePeer {
    channel: ipc_protocol::MessageChannel<std::net::TcpStream>,
    writer: view_io::Writer,
}

#[cfg(test)]
impl FakePeer {
    /// Everything the app has sent so far: waits for the writer thread to get it all out (under load it can lag
    /// further behind than the channel's 200 ms read timeout), then reads until nothing more comes.
    pub(crate) fn sent(&mut self) -> Vec<ipc_protocol::HostToView> {
        assert!(self.writer.wait_idle(Duration::from_secs(10)), "the writer never went idle");
        std::iter::from_fn(|| self.channel.recv().ok()).collect()
    }
}

#[cfg(test)]
impl std::ops::Deref for FakePeer {
    type Target = ipc_protocol::MessageChannel<std::net::TcpStream>;
    fn deref(&self) -> &Self::Target {
        &self.channel
    }
}

#[cfg(test)]
impl std::ops::DerefMut for FakePeer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.channel
    }
}

impl Drop for ViewTab {
    fn drop(&mut self) {
        // Read before `shutdown`, after which a writer never reads as unresponsive.
        let stuck = self.writer.is_unresponsive(std::time::Instant::now());
        self.writer.shutdown();
        let Some(child) = self.child.take() else { return };
        let (cache_dir, slot) = (self.cache_dir.take(), self.profile_slot.take());
        if self.kind == ViewKind::Webkit {
            // Off the UI thread: a WebKit host gets up to 3 s to exit on its own,
            // and dropping a session of N of them used to freeze the window for 3N s.
            // It has no frame file (frames arrive over TCP) and no cache dir.
            // A writer stuck on its socket stays blocked until the Linux side dies: closing the
            // socket on this side does not unblock a send on Windows (measured, wave 3b).
            match std::thread::Builder::new()
                .name("reap-webkit".into())
                .spawn(move || reap_host(ViewKind::Webkit, child))
            {
                Ok(reaper) => CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()).push(reaper),
                // Out of threads: the host got `Shutdown` and exits on its own; this is a drop, it cannot panic.
                Err(e) => eprintln!("couldn't start the WebKit host reaper: {e}"),
            }
            return;
        }
        // The queue and `Shutdown` get until `SHUTDOWN_FLUSH` after the first `shutdown` to go out.
        // A stuck engine is not waited for: the kill below breaks its pipe, which ends the blocked
        // write too (measured: about 0.2 ms after the engine's end closes).
        if !stuck {
            self.writer.wait_finished(view_io::SHUTDOWN_FLUSH);
        }
        // A local host is killed at once, so this stays inline. It must: a respawn
        // of the same tab reuses `socket_id`, and Windows deletes a file even while
        // another process maps it (measured), so a delete that ran late on another
        // thread could take the new host's frame file.
        reap_host(self.kind, child);
        // The host has exited, so its frame file is no longer mapped by it; ours is dropped first.
        self.frame_map.clear();
        ipc_protocol::remove_frame_file(&self.socket_id);
        // The profile slot is freed after the folder's deletion is tried.
        if let Some(dir) = cache_dir {
            remove_cache_dir(dir, slot);
        }
    }
}

/// Waits for an engine host to exit after `Shutdown`, killing it if needed.
fn reap_host(kind: ViewKind, mut child: Child) {
    if kind == ViewKind::Webkit {
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

/// Why `spawn_tab` failed.
#[derive(Debug)]
pub enum SpawnError {
    /// The engine's runtime isn't on disk; nothing was launched.
    ViewMissing(ViewKind),
    /// The engine's runtime is out of date (Update needed); nothing was launched.
    ViewOutdated(ViewKind),
    /// The engine's state is Unavailable (the reason is the state's), so nothing was launched.
    ViewUnavailable(ViewKind, String),
    Io(std::io::Error),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::ViewMissing(kind) => {
                write!(f, "{} is not installed", crate::view_info::view_label(*kind))
            }
            SpawnError::ViewOutdated(kind) => {
                write!(f, "{} needs an update", crate::view_info::view_label(*kind))
            }
            SpawnError::ViewUnavailable(kind, reason) => {
                f.write_str(&crate::view_info::PickOutcome::unavailable_toast(*kind, reason))
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

/// The CEF host exe to copy next to a runtime that has no host: beside `manifold.exe` in a packaged build, else the
/// `cef-runtime/` copy a dev build keeps, else the one `view_install::install_host` downloaded into `engine-hosts/`.
fn shipped_cef_host() -> PathBuf {
    sibling_host(ViewKind::Cef)
        .or_else(|| {
            let file = crate::cef_platform::PLATFORM.host_exe(ViewKind::Cef.desc().binary);
            view_hosts_dir().map(|dir| dir.join(file)).filter(|path| path.exists())
        })
        .unwrap_or_else(cef_sibling_exe)
}

/// The directory a CEF host copy must live in (see `custom_host_path`), or None to run the host as
/// `view_binary_path` finds it. A custom directory wins; a dev build's own `cef-runtime/` already holds
/// its host; otherwise the downloaded runtime, which holds libcef but never a host, so it gets a copy too.
fn host_home(custom: Option<&Path>, dev_runtime_has_host: bool, downloaded: Option<PathBuf>) -> Option<PathBuf> {
    match custom {
        Some(dir) => Some(dir.to_path_buf()),
        None if dev_runtime_has_host => None,
        None => downloaded,
    }
}

/// Refuses to launch an engine whose state (from disk alone) isn't Installed or Custom: by its reason when
/// Unavailable (no host and nothing pinned to download), as needing an update when Update needed, else as not
/// installed.
fn check_spawnable(
    kind: ViewKind,
    plan: &Result<Plan, String>,
    on_disk: OnDisk,
    custom: Option<&Path>,
) -> Result<(), SpawnError> {
    match view_availability(kind, plan, on_disk, None, None, custom) {
        ViewStatus::Installed | ViewStatus::Custom(_) => Ok(()),
        ViewStatus::Unavailable(reason) => Err(SpawnError::ViewUnavailable(kind, reason)),
        ViewStatus::UpdateNeeded { .. } => Err(SpawnError::ViewOutdated(kind)),
        _ => Err(SpawnError::ViewMissing(kind)),
    }
}

/// The spawn refusal for an engine this OS cannot run, from the same `platform_unavailable_reason` the
/// engine lists show, so a spawn fails by name instead of a bare "file not found".
fn unsupported_on(os: &str, kind: ViewKind) -> Option<String> {
    platform_unavailable_reason(os, kind).map(|reason| crate::view_info::PickOutcome::unavailable_toast(kind, reason))
}

/// During a swap, the old view-host on this same `tab_id` was just killed
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
    // view-host left over from a previous crashed/killed run — is still
    // holding `socket_id`'s named pipe. Surface the OS error code and the
    // name we tried, since the bare `err` alone (e.g. just "os error 5")
    // gives no hint of which pipe or that a leftover process is the likely
    // cause. `spawn_tab` retries this once more with a fresh `socket_id`
    // before giving up for good — see its stale-pipe fallback.
    Err(std::io::Error::new(
        err.kind(),
        format!(
            "failed to create named pipe for '{socket_id}' after {ATTEMPTS} attempts \
             ({:.1}s): {err} (raw OS error: {:?}). A leftover view-host process from a \
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
    // inside it, so an inherited view-host produces no visible output at
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
            format!("view-host exited {how} before connecting{tail}")
        }
        None if err.kind() == std::io::ErrorKind::TimedOut => {
            format!("view-host is still running but did not connect within {timeout:?} (log: {})", log_path.display())
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

/// Synthesizes `ViewToHost::Crashed` when the connection to an
/// view-host drops, since a hard-killed/panicked process cannot send a
/// message about its own death. Also fires on an *intentional* teardown
/// (a swap's old tab getting dropped, which sends `Shutdown` then kills
/// the child) — harmless there: `tx`'s paired `rx` was owned by that same
/// now-dropped `ViewTab`, so the `send` below simply fails and is
/// ignored; only a tab that's still the *current* one has a live `rx` to
/// actually receive this.
///
/// Reads the engine's `PROTOCOL_VERSION` first. A stale host (built from an
/// older `messages.rs`) gets a `Crashed` whose reason starts with
/// `PROTOCOL_MISMATCH`, so no message of it is ever misread.
///
/// Frames go to `frame` (newest wins); everything else goes through a channel of `view_io::EVENTS_CAP`, whose
/// `send` waits while the app is that far behind (survey E3).
fn spawn_reader_thread<S: std::io::Read + std::io::Write + Send + 'static>(
    mut from_view: ipc_protocol::MessageChannel<S>,
    frame: view_io::FrameSlot,
    wake: view_io::Wake,
) -> mpsc::Receiver<ViewToHost> {
    let (tx, rx) = mpsc::sync_channel(view_io::EVENTS_CAP);
    std::thread::spawn(move || {
        let lost = || ViewToHost::Crashed { reason: "view-host connection lost".to_string() };
        match from_view.recv_version().map(ipc_protocol::check_version) {
            Ok(Ok(())) => {}
            Ok(Err(mismatch)) => {
                eprintln!("{mismatch}");
                let _ = tx.send(ViewToHost::Crashed { reason: mismatch });
                wake();
                return;
            }
            Err(_) => {
                let _ = tx.send(lost());
                wake();
                return;
            }
        }
        while let Ok(msg) = from_view.recv::<ViewToHost>() {
            match msg {
                ViewToHost::FrameReady { generation, width, height, damage, pixels } => {
                    let mut slot = frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    // A frame the app never took folds its rows into this one's; a new size or file makes it whole.
                    let rows = view_io::frame_rows(damage, height);
                    let rows = match slot.take() {
                        Some(prev) if (prev.generation, prev.width, prev.height) == (generation, width, height) => {
                            view_io::merge_rows(prev.rows, rows)
                        }
                        Some(_) => None,
                        None => rows,
                    };
                    *slot = Some(view_io::Frame { generation, width, height, rows, pixels });
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

/// Spawns an view-host for `kind`, wires up its control channel(s), and
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
/// `wake` is called by the tab's reader thread after each engine message (`event_loop::view_wake`).
pub fn spawn_tab(
    kind: ViewKind,
    tab_id: &str,
    initial_url: &str,
    custom_cef: Option<&Path>,
    jar_json: &dyn Fn() -> String,
    wake: view_io::Wake,
) -> Result<ViewTab, SpawnError> {
    if let Some(reason) = unsupported_on(std::env::consts::OS, kind) {
        return Err(SpawnError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, reason)));
    }
    if kind == ViewKind::Webkit {
        if let Some(reason) = webkit_host_missing(std::env::consts::OS) {
            return Err(SpawnError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, reason)));
        }
    }
    let custom_cef = custom_cef.filter(|_| kind == ViewKind::Cef);
    let on_disk = view_on_disk_fresh(kind);
    check_spawnable(kind, &plan_for(kind), on_disk, custom_cef)?;
    // A host that came from a download, not from beside manifold.exe, is hashed again here: the folder it sits in is
    // writable, and the on-disk poll that said Ready only read a marker.
    if downloads(kind) && sibling_host(kind).is_none() {
        if let Some(dir) = view_hosts_dir() {
            verify_installed_host(&dir, host_components(kind)).map_err(|_| {
                let reason = format!("{} host changed on disk; reinstall", crate::view_info::view_label(kind));
                SpawnError::Io(std::io::Error::other(reason))
            })?;
        }
    }
    if kind == ViewKind::Webkit {
        return Ok(spawn_webkit_tab(tab_id, initial_url, wake)?);
    }
    // An embedded WebView2 host is put in place before its path is used (the profile slot is keyed on it). A sibling
    // host wins as it does everywhere else; a build that embeds nothing returns `None` and touches no file.
    let extracted_host = match view_hosts_dir() {
        Some(dir) if kind == ViewKind::Webview2 && sibling_host(kind).is_none() => {
            embedded_host::Embedded::Webview2.ensure(&dir)?
        }
        _ => None,
    };

    // Normally the socket/shared-memory id is just tab_id. But a prior run
    // that crashed (rather than exiting cleanly) can leave a stale
    // view-host-*.exe still holding tab_id's named pipe indefinitely —
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
    let host_exe = match (kind, host_home(custom_cef, cef_sibling_exe().exists(), view_runtime_dir(kind))) {
        (ViewKind::Cef, Some(dir)) => ensure_host_copy(&dir, &shipped_cef_host())?,
        _ => view_binary_path(kind),
    };

    // Each CEF view-host gets its own throwaway cache dir, so CEF tabs
    // don't share cookies or logins and keep none across a restart (see
    // manifold-engine-cef's main.rs for why they can't share one).
    let cache_dir = (kind == ViewKind::Cef).then(|| {
        let n = CEF_CACHE_COUNTER.fetch_add(1, Ordering::Relaxed);
        cef_cache_root().join(format!("{}-{n}", std::process::id()))
    });

    let mut command = Command::new(&host_exe);
    command
        .env("VIEW_TAB_ID", tab_id)
        .env("VIEW_SOCKET_ID", &socket_id)
        .env("VIEW_INITIAL_URL", initial_url)
        // A host never reads manifold's stdin (the debug-command pipe); `wsl.exe` would, and take commands from it.
        .stdin(Stdio::null())
        .stdout(stdout_file)
        .stderr(stderr_file);
    // Jar engines wait for the jar (`SetCookies`) before their first navigation.
    if kind.desc().jar {
        command.env("VIEW_AWAIT_COOKIES", "1");
    }
    if let Some(dir) = &cache_dir {
        command.env("VIEW_CACHE_DIR", dir);
    }
    // Each WebView2 host gets a profile name of its own among the live hosts, reused once a host is gone.
    let profile_slot = (kind == ViewKind::Webview2).then(|| claim_webview2_slot(&host_exe));
    if let Some(n) = profile_slot {
        command.env("VIEW_WEBVIEW2_PROFILE", webview2_profile_name(n));
    }
    let spawned = if extracted_host.is_some() { embedded_host::spawn_fresh(&mut command) } else { command.spawn() };
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            if let Some(n) = profile_slot {
                free_webview2_slot(n);
            }
            return Err(e.into());
        }
    };
    assign_to_view_job(&child);
    let cache_dir = cache_dir.or_else(|| profile_slot.map(|n| webview2_profile_dir(&host_exe, n)));

    // The child connects to both sockets right after spawning — bounded by
    // LOCAL_CONNECT_TIMEOUT rather than an unbounded accept() (see its own
    // doc comment, and accept_remote's, for why an unbounded wait here would
    // strand the tab on its "starting" card; this runs on a spawn worker
    // thread (`App::spawn_tab_async`), not the UI thread). On timeout (or the child crashing/exiting
    // before connecting, e.g. missing runtime DLLs), kill it explicitly —
    // `Child`'s own `Drop` does not, so leaving this `?` to just propagate
    // would leak the process, exactly the kind of leftover
    // `listen_local_pair_with_retry`'s own error message above warns
    // about. Same cleanup `spawn_webkit_tab`'s own accept-timeout path
    // already does. `connect_host` does the kill, stops waiting as soon as
    // the child exits, and names its exit code and last log lines.
    let (to_view, from_view) =
        match connect_host(&listeners, &mut child, &per_tab_log_path(tab_id), LOCAL_CONNECT_TIMEOUT) {
            Ok(pair) => pair,
            Err(e) => {
                if let Some(dir) = cache_dir {
                    remove_cache_dir(dir, profile_slot);
                }
                return Err(e.into());
            }
        };
    let frame = view_io::FrameSlot::default();
    let events_rx = spawn_reader_thread(from_view, Arc::clone(&frame), wake);

    let writer = view_io::Writer::spawn(to_view, &format!("engine writer {socket_id}"));
    let frame_map = view_io::FrameMap::default();
    let tab =
        ViewTab { kind, socket_id, writer, events_rx, frame, frame_map, child: Some(child), cache_dir, profile_slot };
    if kind.desc().jar {
        // Serialised now that the host is connected, so cookies merged while it started are in the seed.
        tab.send(&HostToView::SetCookies(jar_json()));
    }
    Ok(tab)
}

mod availability;
mod job;
mod profiles;
pub(crate) mod view_io;
mod webkit;
#[cfg(test)]
use crate::view_install::host_platform;
#[cfg(test)]
use availability::{
    availability_with, built_webkit_host, on_disk_cached, pick_webkit_host, OnDiskCache, ON_DISK_CHECK_TTL,
};
use availability::{
    cef_sibling_exe, platform_unavailable_reason, sibling_binary, sibling_host, view_binary_path, view_on_disk_fresh,
    webkit_host_path,
};
pub(crate) use availability::{
    forget_on_disk, platform_default_view_kind, view_availability, view_hosts_dir, view_on_disk, view_runtime_dir,
};
use job::assign_to_view_job;
use profiles::{
    cef_cache_root, claim_webview2_slot, free_webview2_slot, remove_cache_dir, webview2_profile_dir,
    webview2_profile_name, CACHE_REMOVALS, CEF_CACHE_COUNTER, MAX_TAB_LOG_BYTES,
};
#[cfg(test)]
use profiles::{
    claim_slot, sweep_cache_dirs, sweep_view_logs_in, take_webview2_slot, WEBVIEW2_PROFILE_POOL,
    WEBVIEW2_PROFILE_PREFIX,
};
pub use profiles::{
    finish_cache_removals, sweep_cef_cache, sweep_frame_files, sweep_view_logs, sweep_webview2_profiles,
};
#[cfg(test)]
use webkit::webkit_shell_command;
use webkit::{spawn_webkit_tab, webkit_host_missing};
#[cfg(test)]
mod tests;

/// The PDF view exe: beside `manifold.exe` when built there (dev trees, the gate), else the copy embedded in this
/// build, extracted on first use. `None` when neither exists.
pub(crate) fn pdf_view_exe() -> std::io::Result<Option<std::path::PathBuf>> {
    let sibling = sibling_binary("manifold-view-pdfium");
    if sibling.exists() {
        return Ok(Some(sibling));
    }
    match view_hosts_dir() {
        Some(dir) => embedded_host::Embedded::Pdfium.ensure(&dir),
        None => Ok(None),
    }
}

/// Puts a spawned view process in the kill-on-close job, so it dies with manifold.
pub(crate) fn contain_view_process(child: &std::process::Child) {
    assign_to_view_job(child);
}
