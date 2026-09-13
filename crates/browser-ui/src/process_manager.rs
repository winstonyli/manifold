use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;

use ipc_protocol::transport::{self, LocalTransport, RemoteTransport};
use ipc_protocol::{EngineToHost, HostToEngine};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    Servo,
    Cef,
    /// WebKit, via WebKitGTK — hosted inside WSL2 (no native Windows
    /// embedding story for WebKit exists; see project plan/memory).
    Webkit,
}

impl EngineKind {
    fn binary_name(self) -> &'static str {
        match self {
            EngineKind::Servo => "engine-host-servo",
            EngineKind::Cef => "engine-host-cef",
            EngineKind::Webkit => "engine-host-webkit",
        }
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
/// browser-ui talks to it on. `browser-ui` is the process manager: it
/// owns spawning, tearing down, and (from M2 on) swapping the engine
/// backing a tab.
pub struct EngineTab {
    pub tab_id: String,
    pub kind: EngineKind,
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
}

impl Drop for EngineTab {
    fn drop(&mut self) {
        self.to_engine.send(&HostToEngine::Shutdown);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The engine-host binaries are sibling executables in the same
/// target/{debug,release} directory as browser-ui itself — standard
/// Cargo workspace layout, no separate install/discovery step needed.
fn sibling_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.set_file_name(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    });
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
/// engine-host-cef.exe and its full CEF runtime (libcef.dll, resources,
/// locales, its own ANGLE DLLs, ...) are isolated in `cef-runtime/` next to
/// the other binaries.
///
/// TODO: this isolation + the runtime file copying itself is currently a
/// manual post-build step (see project memory), not automated — a real
/// install/packaging step should do this, not a person running `cp`.
fn engine_binary_path(kind: EngineKind) -> PathBuf {
    match kind {
        EngineKind::Servo => sibling_binary(kind.binary_name()),
        EngineKind::Cef => {
            let mut path = sibling_binary(kind.binary_name());
            path.pop();
            path.push("cef-runtime");
            path.push(if cfg!(windows) {
                format!("{}.exe", kind.binary_name())
            } else {
                kind.binary_name().to_string()
            });
            path
        }
        EngineKind::Webkit => unreachable!("Webkit is spawned via wsl.exe, not a local path"),
    }
}

/// Converts this repo's Windows path (wherever browser-ui.exe actually is)
/// to the `/mnt/c/...` form WSL2 uses to see the same filesystem, so we can
/// hand `wsl.exe` a path to the engine-host-webkit binary it built.
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

/// engine-host-webkit is built *inside* WSL2 (it links against WebKitGTK, a
/// Linux-only library) into its own `target-linux/` directory rather than
/// the shared `target/` — that directory is also being written to by the
/// Windows-native build for every other crate, and a Linux host's "default
/// target" build and a Windows host's would otherwise collide there.
fn webkit_binary_path_in_repo() -> PathBuf {
    let mut path = sibling_binary("browser-ui"); // .../target/debug/browser-ui.exe
    path.pop(); // .../target/debug
    path.pop(); // .../target
    path.pop(); // repo root
    path.push("target-linux");
    path.push("debug");
    path.push("engine-host-webkit");
    path
}

/// During a swap, the old engine-host on this same `tab_id` was just killed
/// (see `App::finish_swap`'s `drop(self.tab.take())`) — but a named pipe's
/// OS-level cleanup after `TerminateProcess` isn't always instantaneous
/// relative to `child.wait()` returning, so creating a new listener with
/// the same name right away can transiently fail with "Access is denied".
/// A few short retries clears it without needing an arbitrary fixed delay
/// on every spawn (including the common non-swap case, where it's unused).
fn listen_local_pair_with_retry(tab_id: &str) -> std::io::Result<transport::LocalListenerPair> {
    let mut last_err = None;
    for attempt in 0..10 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        match transport::listen_local_pair(tab_id) {
            Ok(listeners) => return Ok(listeners),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap())
}

fn per_tab_log_files(tab_id: &str) -> std::io::Result<(std::fs::File, std::fs::File)> {
    // Explicit per-tab log file rather than relying on inherited
    // stdout/stderr: when browser-ui itself is launched with its stdio
    // redirected (e.g. `browser-ui.exe > file` from bash), that redirection
    // does not reliably propagate to a grandchild process spawned from
    // inside it, so an inherited engine-host produces no visible output at
    // all even though it runs fine.
    let log_path = std::env::temp_dir().join(format!("multiengine-browser-{tab_id}.log"));
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
        let _ = tx.send(EngineToHost::Crashed {
            reason: "engine-host connection lost".to_string(),
        });
    });
    rx
}

enum RemoteOrLocalReceiver {
    Local(LocalTransport),
    Remote(RemoteTransport),
}

/// Spawns an engine-host for `kind`, wires up its control channel(s), and
/// returns a handle the process manager (and the tab-swap logic) can drive.
///
/// `tab_id`/`initial_url` are passed via env vars, not argv, for
/// engine-host-cef specifically — see that crate's main.rs for why (CEF
/// parses this process's own argv as its own command line). We do the same
/// for the other engines too, for one consistent spawn path.
pub fn spawn_tab(kind: EngineKind, tab_id: &str, initial_url: &str) -> std::io::Result<EngineTab> {
    if kind == EngineKind::Webkit {
        return spawn_webkit_tab(tab_id, initial_url);
    }

    let listeners = listen_local_pair_with_retry(tab_id)?;
    let (stdout_file, stderr_file) = per_tab_log_files(tab_id)?;

    let child = Command::new(engine_binary_path(kind))
        .env("ENGINE_TAB_ID", tab_id)
        .env("ENGINE_INITIAL_URL", initial_url)
        .stdout(stdout_file)
        .stderr(stderr_file)
        .spawn()?;

    // The child connects to both sockets right after spawning; accept()
    // blocks until it does.
    let (to_engine, from_engine) = transport::accept_local_pair(&listeners)?;
    let events_rx = spawn_reader_thread(RemoteOrLocalReceiver::Local(from_engine));

    Ok(EngineTab {
        tab_id: tab_id.to_string(),
        kind,
        to_engine: ToEngine::Local(to_engine),
        events_rx,
        child,
    })
}

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
    let child = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--", "bash", "-c", &command])
        .stdout(stdout_file)
        .stderr(stderr_file)
        .spawn()?;

    // The child connects right after spawning; accept() blocks until it
    // does. Unlike the local case there's only one TCP connection, used
    // for both directions (see ToEngine::send / spawn_reader_thread) via
    // try_clone() rather than two separate sockets.
    let control = transport::accept_remote(&listener)?;
    let stream = control.into_inner();
    let read_stream = stream.try_clone()?;
    let to_engine = ipc_protocol::MessageChannel::new(stream);
    let from_engine = ipc_protocol::MessageChannel::new(read_stream);

    let events_rx = spawn_reader_thread(RemoteOrLocalReceiver::Remote(from_engine));

    Ok(EngineTab {
        tab_id: tab_id.to_string(),
        kind: EngineKind::Webkit,
        to_engine: ToEngine::Remote(to_engine),
        events_rx,
        child,
    })
}
