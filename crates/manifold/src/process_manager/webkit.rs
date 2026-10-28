//! The WebKit host, a TCP-linked Linux program: started through WSL2 on Windows (translating the host path and
//! building the `bash -c` command) or directly on Linux, where it gets a display from the session or Xvfb.

use super::runtime::WSL_DISTRO;
use super::*;

/// POSIX single-quote escaping for the one path `webkit_shell_command` splices into its `bash -c`
/// string: derived from the running exe's location, never from a page.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Generous upper bound for a cold WSL2 VM boot + Xvfb readiness + GTK/
/// WebKit init, not a tuned-tight value — see `accept_remote`'s doc comment
/// for why this exists at all. Failing loudly (a failed card the user
/// can see and retry) well within a minute beats a tab stuck starting forever.
const WEBKIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

/// What the host reads (`manifold-view-webkitgtk`'s `main`) besides `VIEW_HOST_IP`, the address it dials back, which
/// differs by runtime. Page text (the URL) appears only as hex.
pub(super) fn webkit_env(
    initial_url: &str,
    port: u16,
    token: &transport::Token,
    dark: bool,
) -> std::io::Result<Vec<(&'static str, String)>> {
    /// Windows' command line holds 32 767 characters; leave the rest of this one room.
    const MAX_URL_HEX: usize = 16_384;
    let url_hex = transport::to_hex(initial_url.as_bytes());
    if url_hex.len() > MAX_URL_HEX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the URL is too long to start a WebKit tab with",
        ));
    }
    let mut env = Vec::new();
    if dark {
        env.push((host_env::ENV_DARK, "1".to_string()));
    }
    env.extend([
        ("VIEW_INITIAL_URL_HEX", url_hex),
        ("VIEW_TCP_PORT", port.to_string()),
        ("VIEW_TOKEN", transport::to_hex(token)),
    ]);
    Ok(env)
}

/// The `bash -c` string `spawn_webkit_tab` hands to `wsl.exe -d <WSL_DISTRO> -- bash -c`. It crosses two
/// shells there (the distro's login shell parses what `wsl.exe` re-joins, then bash), so text from a
/// page must not appear in it as itself.
pub(super) fn webkit_shell_command(
    initial_url: &str,
    port: u16,
    token: &transport::Token,
    binary_path: &str,
    dark: bool,
) -> std::io::Result<String> {
    let assignments: String =
        webkit_env(initial_url, port, token, dark)?.iter().map(|(name, value)| format!("{name}={value} ")).collect();
    Ok(format!(
        "{assignments}VIEW_HOST_IP=$(ip route | awk '/^default/ {{print $3}}') xvfb-run -a {}",
        shell_quote(binary_path),
    ))
}

/// Starts the WebKit host the way `runtime` says and accepts its connection back. A `Wsl` host dials the Windows
/// host's default-gateway address, so its listener is on every interface; a `Native` one dials loopback.
pub(super) fn spawn_webkit_tab(
    runtime: HostRuntime,
    tab_id: NodeId,
    initial_url: &str,
    wake: view_io::Wake,
) -> std::io::Result<ViewTab> {
    // Bind an ephemeral port (0 = "pick one for me") so concurrent tabs don't collide. WSL2's NAT-mode networking
    // refuses (ECONNREFUSED, verified directly) a WSL2-side connection to 127.0.0.1:port on the Windows host, the
    // guest must instead reach the host at its own default-gateway IP, which the command string below discovers at
    // spawn time (see VIEW_HOST_IP).
    let bind_ip = match runtime {
        HostRuntime::Wsl => "0.0.0.0",
        HostRuntime::Native => "127.0.0.1",
    };
    let listener = TcpListener::bind((bind_ip, 0))?;
    let port = listener.local_addr()?.port();

    let (stdout_file, stderr_file) = per_tab_log_files(tab_id)?;
    // The file `spawn_tab` just hashed when it came from a download: WSL runs it straight from the Windows drive
    // (drvfs gives every file the exec bit; plan "Spike results" 2), so there is no copy into the distro.
    let host = webkit_host_path().ok_or_else(|| std::io::Error::other("no data directory for the WebKit host"))?;

    // A random secret the engine must send before it is accepted: the listener may be on every interface
    // (`listen_remote`). It sits in the host's command line or environment, as hex, visible to the same user's
    // processes only.
    let mut token = [0u8; transport::TOKEN_LEN];
    getrandom::fill(&mut token).map_err(|e| std::io::Error::other(e.to_string()))?;
    let dark = manifold_pages::is_dark();
    let mut command = match runtime {
        HostRuntime::Wsl => {
            // `wsl.exe` does NOT forward this Command's environment into the Linux process (verified: env vars set
            // via `.env()` here simply don't appear on the other side) -- WSL has its own opt-in `WSLENV`
            // passthrough, more trouble than it is worth. Pass values as inline shell assignments in the command
            // string instead, and run under `xvfb-run` for the virtual display GTK's offscreen rendering needs.
            let binary = runtime
                .host_path(&host)
                .ok_or_else(|| std::io::Error::other("the WebKit host is not on a drive WSL can see"))?;
            let shell = webkit_shell_command(initial_url, port, &token, &binary, dark)?;
            let mut command = Command::new("wsl.exe");
            command.args(["-d", WSL_DISTRO, "--", "bash", "-c", &shell]);
            command
        }
        HostRuntime::Native => {
            // manifold itself needs a display on Linux, so the host inherits it.
            let mut command = Command::new(&host);
            command.envs(webkit_env(initial_url, port, &token, dark)?).env("VIEW_HOST_IP", "127.0.0.1");
            command
        }
    };
    let mut child = command
        // Not manifold's stdin: `wsl.exe` forwards what it reads, and would take the debug commands piped to manifold.
        .stdin(std::process::Stdio::null())
        .stdout(stdout_file)
        .stderr(stderr_file)
        .spawn()?;
    // Only reaches the Windows-side `wsl.exe` relay, same limitation
    // `Drop for ViewTab`'s own doc comment already calls out for
    // explicit `Shutdown`-then-kill -- the actual Linux-side process tree
    // isn't part of this job and survives a crash regardless. Still a
    // strict improvement over today (the relay itself no longer orphans).
    assign_to_view_job(&child);

    // The child connects right after spawning. Bounded, not the unbounded
    // wait `TcpListener::accept` alone would give: this runs on a spawn worker
    // thread (`App::spawn_tab_async`; the UI thread is free), but an
    // view-host that never connects — a cold WSL2 VM boot, Xvfb taking
    // unusually long to come up, etc.; observed minutes-to-hours rather than
    // the sub-second common case — would otherwise leave the tab on its
    // "starting" card with no error or retry, forever. On timeout, kill the
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
    let to_view = manifold_view_protocol::MessageChannel::new(stream);
    let from_view = manifold_view_protocol::MessageChannel::new(read_stream);

    let frame = view_io::FrameSlot::default();
    let events_rx = spawn_reader_thread(from_view, Arc::clone(&frame), runtime, wake);

    Ok(ViewTab {
        kind: ViewKind::Webkit,
        // No local named pipe/shared memory for the WSL2-hosted case (see
        // `RemoteTransport`), so there's nothing to
        // disambiguate — tab_id is fine as-is.
        socket_id: tab_id.to_string(),
        writer: view_io::Writer::spawn(to_view, &format!("engine writer {tab_id}")),
        events_rx,
        frame,
        frame_map: view_io::FrameMap::default(),
        child: Some(child),
        cache_dir: None,
        profile_slot: None,
        shown: true,
        first_frame: FirstFrame::Waiting,
    })
}
