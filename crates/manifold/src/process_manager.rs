use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::engine_install::{runtime_state, EngineState, Pin, RuntimeOnDisk, NO_PIN_REASON};
use ipc_protocol::transport::{self, LocalTransport, RemoteTransport};
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
/// (creation or configuration failed) is a silent no-op everywhere it's
/// used: this is a crash-safety net, not a feature any spawn should fail
/// over losing.
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
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
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
/// No kill-on-exit job on other platforms yet (Linux would use `PR_SET_PDEATHSIG` or a process group): an
/// engine-host outlives a manifold that is force-killed, until its reader thread sees the socket drop and
/// shuts it down (`ipc_protocol::transport::spawn_host_reader`). Said once on stderr rather than silently.
#[cfg(not(target_os = "windows"))]
fn assign_to_engine_job(_child: &Child) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!("manifold: engine hosts are not tied to manifold's lifetime on this platform yet (no job object)")
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
/// process-management type) — a small manual string mapping instead, shared
/// by the two independent persistence formats that each need to store one
/// (`settings.rs`'s default-starting-engine, `session.rs`'s per-tab engine).
pub(crate) mod engine_kind_serde {
    use super::EngineKind;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(kind: &EngineKind, s: S) -> Result<S::Ok, S::Error> {
        kind.desc().name.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<EngineKind, D::Error> {
        let name = String::deserialize(d)?;
        EngineKind::from_name(&name).ok_or_else(|| serde::de::Error::custom(format!("unknown engine {name:?}")))
    }
}

/// Either control channel an `EngineTab` might have, depending on whether
/// its engine-host is a same-machine process (`Local`, shared memory for
/// frames) or WSL2-hosted (`Remote`, TCP loopback, frames travel inline in
/// `FrameReady` — see `ipc_protocol::messages::EngineToHost`).
enum ToEngine {
    Local(LocalTransport),
    Remote(RemoteTransport),
}

impl ToEngine {
    fn send(&mut self, msg: &HostToEngine) {
        match self {
            ToEngine::Local(t) => {
                let _ = t.send(msg);
            }
            ToEngine::Remote(t) => {
                let _ = t.send(msg);
            }
        }
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
    to_engine: ToEngine,
    pub events_rx: mpsc::Receiver<EngineToHost>,
    child: Child,
    /// CEF's `root_cache_path` for this process, deleted on drop.
    cache_dir: Option<PathBuf>,
}

impl EngineTab {
    pub fn send(&mut self, msg: &HostToEngine) {
        // A dead/crashed engine-host makes this an inert no-op for now;
        // crash detection + auto-restart is M4 (`Crashed` handling).
        self.to_engine.send(msg);
    }

    /// M5: at most one tab is ever visible (actively capturing/sending
    /// frames) at a time — see `HostToEngine::SetVisible`'s doc comment.
    pub fn set_visible(&mut self, visible: bool) {
        self.send(&HostToEngine::SetVisible(visible));
    }
}

impl Drop for EngineTab {
    fn drop(&mut self) {
        self.to_engine.send(&HostToEngine::Shutdown);
        if self.kind == EngineKind::Webkit {
            // For the WSL2-hosted case, `child` is only the Windows-side
            // `wsl.exe` relay — force-killing it immediately (the
            // local-engine path below) does NOT terminate the actual
            // Linux-side process tree (fish/bash/xvfb-run/Xvfb/
            // manifold-engine-webkit), which WSL2 keeps running independently
            // of any Windows-side process state: confirmed live, a killed
            // relay leaves that whole tree orphaned and running
            // indefinitely, invisible to any further Windows-side cleanup.
            // The graceful `Shutdown` sent above already makes
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
            let mut exited = false;
            for _ in 0..30 {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    exited = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !exited {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        } else {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(dir) = self.cache_dir.take() {
            remove_cache_dir(dir);
        }
    }
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
fn remove_cache_dir(dir: PathBuf) {
    let removal = std::thread::spawn(move || {
        for _ in 0..50 {
            if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()).push(removal);
}

static CACHE_REMOVALS: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// Waits for every `remove_cache_dir` so far, at most its few seconds of
/// retries: `main` calls this after dropping the app's tabs, since the
/// process exiting would otherwise cut the deletes short.
pub fn finish_cache_removals() {
    let removals = std::mem::take(&mut *CACHE_REMOVALS.lock().unwrap_or_else(|e| e.into_inner()));
    for removal in removals {
        let _ = removal.join();
    }
}

/// Deletes CEF cache dirs left by engines that `remove_cache_dir` never
/// finished (manifold killed or crashed). Another manifold on the same
/// profile may be using some: Windows won't rename a dir while a running
/// CEF holds files in it, so each is renamed aside first and deleted only
/// if that works. Dirs touched in the last minute are skipped, since their
/// CEF may not have opened its files yet.
pub fn sweep_cef_cache() {
    sweep_cache_dirs(&cef_cache_root(), Duration::from_secs(60));
}

fn sweep_cache_dirs(root: &Path, min_age: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
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

/// How `kind` is available, in one place. Engines that ship with the app are
/// always Installed; CEF follows the disk (`cef_on_disk`). A custom path and a running install
/// override it; a failed install overrides Missing and Stale but not a current runtime.
pub(crate) fn engine_availability(
    kind: EngineKind,
    cef_on_disk: RuntimeOnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> EngineState {
    availability_with(
        std::env::consts::OS,
        crate::cef_pin::pin_for_platform(),
        kind,
        cef_on_disk,
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
    cef_on_disk: RuntimeOnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> EngineState {
    if let Some(reason) = platform_unavailable_reason(os, kind) {
        return EngineState::Unavailable(reason.to_string());
    }
    if kind != EngineKind::Cef {
        return EngineState::Installed;
    }
    if let Some(path) = custom {
        return EngineState::Custom(path.to_path_buf());
    }
    if let Some((received, total)) = installing {
        return EngineState::Installing { received, total };
    }
    // A runtime that is there and current outranks a recorded failure (the failure was an update or a
    // retry that did not matter in the end); a failure outranks Missing and Stale.
    if cef_on_disk == RuntimeOnDisk::Ready {
        return EngineState::Installed;
    }
    let Some(pin) = pin else { return EngineState::Unavailable(NO_PIN_REASON.to_string()) };
    if let Some(message) = failed {
        return EngineState::Failed(message.to_string());
    }
    match cef_on_disk {
        RuntimeOnDisk::Ready => EngineState::Installed,
        RuntimeOnDisk::Missing => EngineState::NotInstalled { size: pin.size },
        RuntimeOnDisk::Stale => EngineState::UpdateNeeded { size: pin.size },
    }
}

/// Why `spawn_tab` failed.
#[derive(Debug)]
pub enum SpawnError {
    /// The engine's runtime isn't on disk (or is out of date); nothing was launched.
    EngineMissing(EngineKind),
    Io(std::io::Error),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::EngineMissing(kind) => {
                write!(f, "{} is not installed", crate::engine_info::engine_label(*kind))
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

/// Refuses to launch an engine whose state (from disk alone) isn't Installed or Custom.
fn check_spawnable(kind: EngineKind, cef_on_disk: RuntimeOnDisk, custom: Option<&Path>) -> Result<(), SpawnError> {
    match engine_availability(kind, cef_on_disk, None, None, custom) {
        EngineState::Installed | EngineState::Custom(_) => Ok(()),
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

/// POSIX single-quote escaping for building the `bash -c` command string
/// `spawn_webkit_tab` passes to `wsl.exe` (env vars there since `wsl.exe`
/// doesn't forward this process's own environment — see that function).
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
    let log_path = std::env::temp_dir().join(format!("manifold-{tab_id}.log"));
    let stdout_file = std::fs::File::create(&log_path)?;
    let stderr_file = stdout_file.try_clone()?;
    Ok((stdout_file, stderr_file))
}

/// Synthesizes `EngineToHost::Crashed` when the connection to an
/// engine-host drops, since a hard-killed/panicked process cannot send a
/// message about its own death. Also fires on an *intentional* teardown
/// (a swap's old tab getting dropped, which sends `Shutdown` then kills
/// the child) — harmless there: `tx`'s paired `rx` was owned by that same
/// now-dropped `EngineTab`, so the `send` below simply fails and is
/// ignored; only a tab that's still the *current* one has a live `rx` to
/// actually receive this.
fn spawn_reader_thread(mut from_engine: RemoteOrLocalReceiver) -> mpsc::Receiver<EngineToHost> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let msg = match &mut from_engine {
                RemoteOrLocalReceiver::Local(t) => t.recv::<EngineToHost>(),
                RemoteOrLocalReceiver::Remote(t) => t.recv::<EngineToHost>(),
            };
            match msg {
                Ok(msg) => {
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(EngineToHost::Crashed { reason: "engine-host connection lost".to_string() });
    });
    rx
}

enum RemoteOrLocalReceiver {
    Local(LocalTransport),
    Remote(RemoteTransport),
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
pub fn spawn_tab(
    kind: EngineKind,
    tab_id: &str,
    initial_url: &str,
    custom_cef: Option<&Path>,
) -> Result<EngineTab, SpawnError> {
    if let Some(reason) = unsupported_on(std::env::consts::OS, kind) {
        return Err(SpawnError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, reason)));
    }
    if kind == EngineKind::Webkit {
        if let Some(reason) = webkit_host_missing(std::env::consts::OS) {
            return Err(SpawnError::Io(std::io::Error::new(std::io::ErrorKind::Unsupported, reason)));
        }
        return Ok(spawn_webkit_tab(tab_id, initial_url)?);
    }
    let custom_cef = custom_cef.filter(|_| kind == EngineKind::Cef);
    check_spawnable(kind, if kind == EngineKind::Cef { cef_on_disk() } else { RuntimeOnDisk::Ready }, custom_cef)?;

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

    let mut command = Command::new(host_exe);
    command
        .env("ENGINE_TAB_ID", tab_id)
        .env("ENGINE_SOCKET_ID", &socket_id)
        .env("ENGINE_INITIAL_URL", initial_url)
        .stdout(stdout_file)
        .stderr(stderr_file);
    if let Some(dir) = &cache_dir {
        command.env("ENGINE_CACHE_DIR", dir);
    }
    let mut child = command.spawn()?;
    assign_to_engine_job(&child);

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
    // already does.
    let (to_engine, from_engine) = match transport::accept_local_pair(&listeners, LOCAL_CONNECT_TIMEOUT) {
        Ok(pair) => pair,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(dir) = cache_dir {
                remove_cache_dir(dir);
            }
            return Err(e.into());
        }
    };
    let events_rx = spawn_reader_thread(RemoteOrLocalReceiver::Local(from_engine));

    Ok(EngineTab { kind, socket_id, to_engine: ToEngine::Local(to_engine), events_rx, child, cache_dir })
}

/// Generous upper bound for a cold WSL2 VM boot + Xvfb readiness + GTK/
/// WebKit init, not a tuned-tight value — see `accept_remote`'s doc comment
/// for why this exists at all. Failing loudly (a `tab_spawn_error` the user
/// can see and retry) well within a minute beats an unbounded app freeze.
const WEBKIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

fn spawn_webkit_tab(tab_id: &str, initial_url: &str) -> std::io::Result<EngineTab> {
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
    // every interface (`listen_remote`). It sits in this command line, visible to the
    // same user's processes only.
    let mut token = [0u8; transport::TOKEN_LEN];
    getrandom::fill(&mut token).map_err(|e| std::io::Error::other(e.to_string()))?;
    let command = format!(
        "ENGINE_TAB_ID={} ENGINE_INITIAL_URL={} ENGINE_TCP_PORT={} ENGINE_TOKEN={} ENGINE_HOST_IP=$(ip route | awk '/^default/ {{print $3}}') xvfb-run -a {}",
        shell_quote(tab_id),
        shell_quote(initial_url),
        port,
        transport::token_to_hex(&token),
        shell_quote(&binary_path),
    );
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

    let events_rx = spawn_reader_thread(RemoteOrLocalReceiver::Remote(from_engine));

    Ok(EngineTab {
        kind: EngineKind::Webkit,
        // No local named pipe/shared memory for the WSL2-hosted case (see
        // ToEngine::Remote/RemoteTransport above), so there's nothing to
        // disambiguate — tab_id is fine as-is.
        socket_id: tab_id.to_string(),
        to_engine: ToEngine::Remote(to_engine),
        events_rx,
        child,
        cache_dir: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn bundled_engines_are_always_installed_where_the_os_can_run_them() {
        let a = |os, kind, disk, installing, failed| availability_with(os, None, kind, disk, installing, failed, None);
        for (os, kind) in [
            ("windows", EngineKind::Servo),
            ("linux", EngineKind::Servo),
            ("macos", EngineKind::Servo),
            ("windows", EngineKind::Webview2),
            ("windows", EngineKind::Webkit),
            ("linux", EngineKind::Webkit),
        ] {
            assert_eq!(a(os, kind, RuntimeOnDisk::Missing, None, None), EngineState::Installed, "{os} {kind:?}");
            assert_eq!(
                a(os, kind, RuntimeOnDisk::Stale, Some((1, 2)), Some("x")),
                EngineState::Installed,
                "{os} {kind:?}"
            );
        }
    }

    /// Every OS case, from any host: the reason text is also what the spawn refusal and the lists show.
    #[test]
    fn engines_the_os_cannot_run_are_unavailable_with_a_reason() {
        let unavailable = |os, kind| availability_with(os, None, kind, RuntimeOnDisk::Ready, None, None, None);
        let reason = |text: &str| EngineState::Unavailable(text.to_string());
        for os in ["linux", "macos", "freebsd"] {
            assert_eq!(unavailable(os, EngineKind::Webview2), reason("needs Windows"), "{os}");
        }
        assert_eq!(unavailable("macos", EngineKind::Webkit), reason("needs Windows or Linux"));
        assert_eq!(unavailable("freebsd", EngineKind::Webkit), reason("needs Windows or Linux"));
        assert_eq!(unavailable("windows", EngineKind::Webview2), EngineState::Installed);
        assert_eq!(unavailable("windows", EngineKind::Webkit), EngineState::Installed);
        assert_eq!(unavailable("linux", EngineKind::Webkit), EngineState::Installed);
        // Servo runs everywhere; CEF follows its pin, not the OS table.
        assert_eq!(unavailable("macos", EngineKind::Servo), EngineState::Installed);
        assert_eq!(unavailable("macos", EngineKind::Cef), EngineState::Installed);
        // The spawn refusal reads the same table.
        assert_eq!(unsupported_on("windows", EngineKind::Webview2), None);
        assert_eq!(unsupported_on("linux", EngineKind::Servo), None);
        assert_eq!(unsupported_on("linux", EngineKind::Webkit), None);
        let refusal = unsupported_on("macos", EngineKind::Webview2).unwrap();
        assert!(refusal.contains("WebView2") && refusal.contains("needs Windows"), "{refusal}");
        assert!(unsupported_on("macos", EngineKind::Webkit).unwrap().contains("needs Windows or Linux"));
    }

    #[test]
    fn a_linux_webkit_spawn_names_the_missing_native_host() {
        // Available on Linux, but the only host is reached through wsl.exe: refused by name, not by a
        // raw "No such file or directory" from a wsl.exe that isn't there.
        assert!(webkit_host_missing("linux").unwrap().contains("native Linux WebKit host"));
        assert_eq!(webkit_host_missing("windows"), None);
    }

    #[test]
    fn the_platform_default_is_the_first_engine_the_os_can_run() {
        assert_eq!(platform_default_engine("windows"), EngineKind::Webview2);
        for os in ["linux", "macos", "freebsd"] {
            assert_eq!(platform_default_engine(os), EngineKind::Servo, "{os}");
        }
        for os in ["windows", "linux", "macos"] {
            assert!(platform_unavailable_reason(os, platform_default_engine(os)).is_none());
        }
    }

    #[test]
    fn cef_state_follows_disk_then_overrides() {
        let pin = crate::cef_pin::pin_for_platform().expect("a pin on a platform with tests");
        let a =
            |disk, installing, failed, custom| engine_availability(EngineKind::Cef, disk, installing, failed, custom);
        assert_eq!(a(RuntimeOnDisk::Ready, None, None, None), EngineState::Installed);
        assert_eq!(a(RuntimeOnDisk::Missing, None, None, None), EngineState::NotInstalled { size: pin.size });
        assert_eq!(a(RuntimeOnDisk::Stale, None, None, None), EngineState::UpdateNeeded { size: pin.size });
        assert_eq!(
            a(RuntimeOnDisk::Missing, Some((5, 9)), Some("old"), None),
            EngineState::Installing { received: 5, total: 9 }
        );
        assert_eq!(a(RuntimeOnDisk::Missing, None, Some("boom"), None), EngineState::Failed("boom".into()));
        assert_eq!(a(RuntimeOnDisk::Stale, None, Some("boom"), None), EngineState::Failed("boom".into()));
        // A current runtime outranks a recorded failure, so the list agrees with what a spawn would do.
        assert_eq!(a(RuntimeOnDisk::Ready, None, Some("boom"), None), EngineState::Installed);
        let p = Path::new("C:/my/cef");
        assert_eq!(a(RuntimeOnDisk::Missing, Some((1, 2)), Some("x"), Some(p)), EngineState::Custom(p.to_path_buf()));
    }

    #[test]
    fn a_platform_without_a_pin_offers_no_install() {
        let a = |disk, installing, failed, custom| {
            availability_with("windows", None, EngineKind::Cef, disk, installing, failed, custom)
        };
        let unavailable = EngineState::Unavailable(NO_PIN_REASON.to_string());
        for disk in [RuntimeOnDisk::Missing, RuntimeOnDisk::Stale] {
            assert_eq!(a(disk, None, None, None), unavailable);
            // A recorded failure cannot exist without a pin, and would not change the answer.
            assert_eq!(a(disk, None, Some("boom"), None), unavailable);
        }
        // What is already on disk (a dev tree) or configured still works, and other engines are untouched.
        assert_eq!(a(RuntimeOnDisk::Ready, None, None, None), EngineState::Installed);
        let p = Path::new("C:/my/cef");
        assert_eq!(a(RuntimeOnDisk::Missing, None, None, Some(p)), EngineState::Custom(p.to_path_buf()));
        assert_eq!(
            availability_with("windows", None, EngineKind::Servo, RuntimeOnDisk::Missing, None, None, None),
            EngineState::Installed
        );
        // The pick says so rather than asking to download, and the label gives the reason.
        assert_eq!(
            crate::engine_info::pick_outcome(EngineKind::Cef, &unavailable),
            crate::engine_info::PickOutcome::Unavailable(EngineKind::Cef, NO_PIN_REASON.to_string())
        );
        assert!(crate::engine_info::engine_state_label(&unavailable).0.contains(NO_PIN_REASON));
    }

    #[test]
    fn a_stale_marker_is_update_needed_not_a_spawn() {
        let pin = crate::cef_pin::pin_for_platform().expect("a pin on a platform with tests");
        let state = engine_availability(EngineKind::Cef, RuntimeOnDisk::Stale, None, None, None);
        assert_eq!(state, EngineState::UpdateNeeded { size: pin.size });
        assert!(matches!(
            check_spawnable(EngineKind::Cef, RuntimeOnDisk::Stale, None),
            Err(SpawnError::EngineMissing(EngineKind::Cef))
        ));
        assert!(matches!(
            check_spawnable(EngineKind::Cef, RuntimeOnDisk::Missing, None),
            Err(SpawnError::EngineMissing(EngineKind::Cef))
        ));
        assert!(check_spawnable(EngineKind::Cef, RuntimeOnDisk::Ready, None).is_ok());
        assert!(check_spawnable(EngineKind::Servo, RuntimeOnDisk::Missing, None).is_ok());
    }

    #[test]
    fn a_custom_directory_allows_a_spawn_the_disk_would_refuse() {
        let custom = Path::new("D:/cef");
        for disk in [RuntimeOnDisk::Missing, RuntimeOnDisk::Stale] {
            assert!(check_spawnable(EngineKind::Cef, disk, Some(custom)).is_ok());
        }
    }

    #[test]
    fn the_host_copy_is_made_when_missing_skipped_when_unchanged_and_replaced_when_the_source_changes() {
        use crate::store::testutil::temp_dir;
        let src_dir = temp_dir("host_src");
        let custom = temp_dir("host_custom");
        let host = src_dir.join("manifold-engine-cef.exe");
        std::fs::write(&host, b"v1").unwrap();

        let dest = ensure_host_copy(&custom, &host).unwrap();
        assert_eq!(dest, custom_host_path(&custom, &host));
        assert_eq!(dest.parent().unwrap(), custom);
        assert_eq!(std::fs::read(&dest).unwrap(), b"v1", "copied when missing");

        // Unchanged: a marker written into the copy survives only if it was not recopied.
        let stamp = std::fs::metadata(&dest).unwrap().modified().unwrap();
        ensure_host_copy(&custom, &host).unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().modified().unwrap(), stamp);

        std::fs::write(&host, b"version 2").unwrap();
        ensure_host_copy(&custom, &host).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"version 2", "replaced when the source changed");

        let err = ensure_host_copy(&custom, &src_dir.join("nope.exe")).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn the_cef_host_is_copied_beside_a_runtime_that_lacks_one() {
        let (custom, runtime) = (Path::new("D:/mine"), PathBuf::from("D:/data/cef-runtime"));
        assert_eq!(host_home(Some(custom), true, Some(runtime.clone())), Some(custom.to_path_buf()));
        assert_eq!(host_home(None, true, Some(runtime.clone())), None, "a dev tree runs its own host");
        assert_eq!(host_home(None, false, Some(runtime.clone())), Some(runtime), "a downloaded runtime has none");
        assert_eq!(host_home(None, false, None), None);
    }

    #[test]
    fn concurrent_spawns_copy_the_host_once_and_leave_no_temp_files() {
        use crate::store::testutil::temp_dir;
        let src_dir = temp_dir("host_race_src");
        let custom = temp_dir("host_race_dest");
        let host = src_dir.join("manifold-engine-cef.exe");
        std::fs::write(&host, vec![7u8; 200_000]).unwrap();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (custom, host) = (custom.clone(), host.clone());
                std::thread::spawn(move || ensure_host_copy(&custom, &host))
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().unwrap(), custom.join("manifold-engine-cef.exe"));
        }
        assert_eq!(std::fs::read(custom.join("manifold-engine-cef.exe")).unwrap(), vec![7u8; 200_000]);
        let names: Vec<_> = std::fs::read_dir(&custom).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names.len(), 1, "no temp copies left: {names:?}");
    }

    #[test]
    fn a_matching_host_copy_is_current_even_when_it_is_locked() {
        use crate::store::testutil::temp_dir;
        let src_dir = temp_dir("host_lock_src");
        let custom = temp_dir("host_lock_dest");
        let host = src_dir.join("manifold-engine-cef.exe");
        std::fs::write(&host, b"host").unwrap();
        let dest = ensure_host_copy(&custom, &host).unwrap();
        // Held open without share-delete, as a running host holds its exe: a rename over it would fail.
        let mut held = std::fs::OpenOptions::new();
        held.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            held.share_mode(1);
        }
        let _held = held.open(&dest).unwrap();
        assert_eq!(ensure_host_copy(&custom, &host).unwrap(), dest);
    }

    #[test]
    fn a_missing_engine_error_reads_plainly() {
        assert_eq!(SpawnError::EngineMissing(EngineKind::Cef).to_string(), "Blink (CEF) is not installed");
    }

    #[test]
    fn only_cef_has_a_downloaded_runtime_dir() {
        // `engine_data_dir()` is None under cfg(test), so no real profile is touched.
        assert_eq!(engine_runtime_dir(EngineKind::Cef), None);
        assert_eq!(engine_runtime_dir(EngineKind::Servo), None);
    }

    /// A dir whose files a live CEF holds (opened without share-delete, as
    /// Chromium's SQLite does) survives the sweep untouched; a dead one and
    /// one an earlier sweep renamed aside are deleted.
    #[cfg(windows)]
    #[test]
    fn sweep_skips_dirs_still_in_use() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;

        let root = crate::store::testutil::temp_dir("cef-sweep");
        for name in ["1-0", "1-1", "1-2.old"] {
            std::fs::create_dir(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("Cookies"), b"x").unwrap();
        }
        let _held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ_WRITE)
            .open(root.join("1-1").join("Cookies"))
            .unwrap();

        super::sweep_cache_dirs(&root, Duration::from_secs(3600));
        assert!(root.join("1-0").exists(), "a dir touched within min_age is kept");

        super::sweep_cache_dirs(&root, Duration::ZERO);
        let mut left: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
        left.sort();
        assert_eq!(left, ["1-1"]);
        assert!(root.join("1-1").join("Cookies").exists());
    }
}
