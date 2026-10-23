//! The WebKit host, which runs inside WSL2: translating the host path, building the `bash -c` command and the spawn.

use super::*;

/// Why a WebKit spawn cannot start on `os` although WebKit counts as available there, or `None`. The only
/// WebKit host is reached through `wsl.exe` (`spawn_webkit_tab`); on Linux, where it would run natively,
/// that launch is not wired yet, so the spawn says so instead of failing on a missing `wsl.exe`.
pub(super) fn webkit_host_missing(os: &str) -> Option<&'static str> {
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

/// Generous upper bound for a cold WSL2 VM boot + Xvfb readiness + GTK/
/// WebKit init, not a tuned-tight value — see `accept_remote`'s doc comment
/// for why this exists at all. Failing loudly (a failed card the user
/// can see and retry) well within a minute beats a tab stuck starting forever.
const WEBKIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

/// The `bash -c` string `spawn_webkit_tab` hands to `wsl.exe -d Ubuntu -- bash -c`. It crosses two
/// shells there (the distro's login shell parses what `wsl.exe` re-joins, then bash), so text from a
/// page must not appear in it as itself.
pub(super) fn webkit_shell_command(
    initial_url: &str,
    port: u16,
    token: &transport::Token,
    binary_path: &str,
    dark: bool,
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
    let dark = if dark { format!("{}=1 ", host_env::ENV_DARK) } else { String::new() };
    Ok(format!(
        "{dark}VIEW_INITIAL_URL_HEX={url_hex} VIEW_TCP_PORT={port} VIEW_TOKEN={} \
         VIEW_HOST_IP=$(ip route | awk '/^default/ {{print $3}}') xvfb-run -a {}",
        transport::to_hex(token),
        shell_quote(binary_path),
    ))
}

pub(super) fn spawn_webkit_tab(tab_id: NodeId, initial_url: &str, wake: view_io::Wake) -> std::io::Result<ViewTab> {
    // Bind an ephemeral port (0 = "pick one for me") so concurrent tabs
    // don't collide. Bound on all interfaces, not just loopback: WSL2's
    // NAT-mode networking refuses (ECONNREFUSED, verified directly) a
    // WSL2-side connection to 127.0.0.1:port on the Windows host — the
    // guest must instead reach the host at its own default-gateway IP,
    // which the command string below discovers at spawn time (see
    // VIEW_HOST_IP).
    let listener = TcpListener::bind(("0.0.0.0", 0))?;
    let port = listener.local_addr()?.port();

    let (stdout_file, stderr_file) = per_tab_log_files(tab_id)?;
    // The file `spawn_tab` just hashed when it came from a download: WSL runs it straight from the Windows drive
    // (drvfs gives every file the exec bit; plan "Spike results" 2), so there is no copy into the distro.
    let host = webkit_host_path().ok_or_else(|| std::io::Error::other("no data directory for the WebKit host"))?;
    let binary_path = windows_path_to_wsl(&host);

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
    let command = webkit_shell_command(initial_url, port, &token, &binary_path, manifold_pages::is_dark())?;
    let mut child = Command::new("wsl.exe")
        .args(["-d", "Ubuntu", "--", "bash", "-c", &command])
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
    let events_rx = spawn_reader_thread(from_view, Arc::clone(&frame), wake);

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
    })
}
