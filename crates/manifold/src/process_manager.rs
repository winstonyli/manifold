use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

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
#[cfg(not(target_os = "windows"))]
fn assign_to_engine_job(_child: &Child) {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl EngineKind {
    fn binary_name(self) -> &'static str {
        match self {
            EngineKind::Servo => "manifold-engine-servo",
            EngineKind::Cef => "manifold-engine-cef",
            EngineKind::Webkit => "manifold-engine-webkit",
            EngineKind::Webview2 => "manifold-engine-webview2",
        }
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
        let name = match kind {
            EngineKind::Servo => "servo",
            EngineKind::Cef => "cef",
            EngineKind::Webkit => "webkit",
            EngineKind::Webview2 => "webview2",
        };
        name.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<EngineKind, D::Error> {
        let name = String::deserialize(d)?;
        Ok(match name.as_str() {
            "cef" => EngineKind::Cef,
            "webkit" => EngineKind::Webkit,
            "webview2" => EngineKind::Webview2,
            _ => EngineKind::Servo,
        })
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
    }
}

/// The engine-host binaries are sibling executables in the same
/// target/{debug,release} directory as manifold itself — standard
/// Cargo workspace layout, no separate install/discovery step needed.
fn sibling_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.set_file_name(if cfg!(windows) { format!("{name}.exe") } else { name.to_string() });
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
/// Falls back to `profile::engine_data_dir()` when the sibling binary isn't there —
/// for an engine fetched on demand after install rather than bundled next
/// to `manifold.exe`. No download step writes there yet; the fallback
/// just means adding one later won't require touching this lookup.
fn engine_binary_path(kind: EngineKind) -> PathBuf {
    let file_name = if cfg!(windows) { format!("{}.exe", kind.binary_name()) } else { kind.binary_name().to_string() };
    let sibling = match kind {
        EngineKind::Servo => sibling_binary(kind.binary_name()),
        EngineKind::Cef => {
            let mut path = sibling_binary(kind.binary_name());
            path.pop();
            path.push("cef-runtime");
            path.push(&file_name);
            path
        }
        // WebView2Loader is a single small loader (statically linked by
        // the `webview2-com` crate, or a lone DLL at worst) — no ANGLE-style
        // filename collision with Servo/CEF's runtime files, so no isolation
        // directory needed like CEF's `cef-runtime/` above.
        EngineKind::Webview2 => sibling_binary(kind.binary_name()),
        EngineKind::Webkit => unreachable!("Webkit is spawned via wsl.exe, not a local path"),
    };
    if sibling.exists() {
        return sibling;
    }
    let Some(mut fallback) = crate::profile::engine_data_dir() else { return sibling };
    fallback.push("engine-hosts");
    // Same isolation as the sibling-dir case above: CEF's own ANGLE DLLs
    // would collide with Servo's if both engines' fallback files ever
    // landed flat in the same `engine-hosts/` directory.
    if matches!(kind, EngineKind::Cef) {
        fallback.push("cef-runtime");
    }
    fallback.push(file_name);
    fallback
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
pub fn spawn_tab(kind: EngineKind, tab_id: &str, initial_url: &str) -> std::io::Result<EngineTab> {
    if kind == EngineKind::Webkit {
        return spawn_webkit_tab(tab_id, initial_url);
    }

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
                Err(_) => return Err(primary_err),
            }
        }
    };
    let (stdout_file, stderr_file) = per_tab_log_files(tab_id)?;

    let mut child = Command::new(engine_binary_path(kind))
        .env("ENGINE_TAB_ID", tab_id)
        .env("ENGINE_SOCKET_ID", &socket_id)
        .env("ENGINE_INITIAL_URL", initial_url)
        .stdout(stdout_file)
        .stderr(stderr_file)
        .spawn()?;
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
            return Err(e);
        }
    };
    let events_rx = spawn_reader_thread(RemoteOrLocalReceiver::Local(from_engine));

    Ok(EngineTab { kind, socket_id, to_engine: ToEngine::Local(to_engine), events_rx, child })
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
    let command = format!(
        "ENGINE_TAB_ID={} ENGINE_INITIAL_URL={} ENGINE_TCP_PORT={} ENGINE_HOST_IP=$(ip route | awk '/^default/ {{print $3}}') xvfb-run -a {}",
        shell_quote(tab_id),
        shell_quote(initial_url),
        port,
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
    let control = match transport::accept_remote(&listener, WEBKIT_CONNECT_TIMEOUT) {
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
    })
}
