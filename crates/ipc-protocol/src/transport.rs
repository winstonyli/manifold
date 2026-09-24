use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use interprocess::local_socket::{
    prelude::*, GenericNamespaced, Listener, ListenerOptions, Name, Stream as LocalStream,
    ToNsName,
};

use crate::channel::MessageChannel;
use crate::messages::HostToEngine;

/// Control-channel connection between `manifold` and a same-machine
/// engine-host (CEF, Servo). Frame pixels travel separately via
/// `SharedFrameBuffer`; this only carries `HostToEngine`/`EngineToHost`
/// messages. Each `LocalTransport` is one-directional by convention (see
/// `listen_local_pair`/`connect_local_pair`) — one socket per tab carries
/// host-to-engine messages, a second carries engine-to-host messages, so
/// neither side ever needs to duplicate a stream handle to read and write
/// concurrently from different threads.
pub type LocalTransport = MessageChannel<LocalStream>;

/// Control-channel connection to a WSL2-hosted engine-host (WPE). No
/// shared memory crosses the Windows/WSL2 boundary, so frame bytes are
/// sent inline on this same TCP stream via `send_raw`/`recv_raw`.
pub type RemoteTransport = MessageChannel<TcpStream>;

/// An engine's reader thread: forwards each `HostToEngine` from
/// `from_host` into the returned channel, so the engine's main thread (which
/// has to keep driving its browser's event loop) only ever polls it. Stops
/// after forwarding `Shutdown`. If the connection drops instead,
/// manifold.exe died without its normal teardown (force-killed, crashed):
/// the thread sends a `Shutdown` of its own, so the engine exits through
/// its usual path instead of running orphaned. Before this was shared, only
/// the WebKit and Servo engines did that; WebKit's orphan was seen live at
/// ~45% CPU after `taskkill /F /IM manifold.exe`.
pub fn spawn_host_reader<S: Read + Write + Send + 'static>(
    mut from_host: MessageChannel<S>,
) -> mpsc::Receiver<HostToEngine> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let msg = from_host.recv::<HostToEngine>().unwrap_or(HostToEngine::Shutdown);
        let is_shutdown = matches!(msg, HostToEngine::Shutdown);
        if tx.send(msg).is_err() || is_shutdown {
            break;
        }
    });
    rx
}

fn socket_name(socket_id: &str, suffix: &str) -> io::Result<Name<'static>> {
    format!("manifold-{socket_id}-{suffix}")
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

pub struct LocalListenerPair {
    /// manifold writes `HostToEngine` here; the engine-host reads it.
    pub host_to_engine: Listener,
    /// the engine-host writes `EngineToHost` here; manifold reads it.
    pub engine_to_host: Listener,
}

/// Called by manifold (the process manager) right before spawning an
/// engine-host child process, so both sockets exist before the child
/// tries to connect to them.
///
/// `socket_id` names the pipes and is normally the tab's `tab_id`, but
/// callers may pass a distinct disambiguating id instead (e.g. when a
/// stale engine-host from a prior crashed run still holds `tab_id`'s
/// pipes) — it never needs to match manifold's own tab bookkeeping.
pub fn listen_local_pair(socket_id: &str) -> io::Result<LocalListenerPair> {
    Ok(LocalListenerPair {
        host_to_engine: ListenerOptions::new()
            .name(socket_name(socket_id, "h2e")?)
            .create_sync()?,
        engine_to_host: ListenerOptions::new()
            .name(socket_name(socket_id, "e2h")?)
            .create_sync()?,
    })
}

/// Bounded wait for a single same-machine engine-host (CEF/Servo) to dial
/// back on one of the two sockets, rather than `Listener::accept`'s
/// unbounded block — see `accept_remote`'s doc comment for the sibling bug
/// this mirrors (there, a cold WSL2 boot; here, in principle a
/// same-machine child process that fails to reach `connect_local_pair` at
/// all — e.g. a corrupted/mismatched binary that exits before connecting —
/// would otherwise hang manifold's main/event-loop thread just the same,
/// just without the "it can take literal hours" cold-boot severity that
/// made the remote case the first one actually hit in practice).
fn accept_one(listener: &Listener, timeout: Duration) -> io::Result<LocalStream> {
    listener.set_nonblocking(interprocess::local_socket::ListenerNonblockingMode::Accept)?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok(stream) => return Ok(stream),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out after {timeout:?} waiting for engine-host to connect"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Called by manifold once the engine-host child process has connected.
/// Returns `(sender, receiver)`: send `HostToEngine` on the first, recv
/// `EngineToHost` on the second. `timeout` applies to each socket in turn
/// (so the real worst case is up to `2 * timeout`, not shared/split between
/// them) — the two accepts are cheap and effectively sequential in
/// practice (`connect_local_pair` dials both back within microseconds of
/// each other), so this favors giving each one its own full budget over
/// the bookkeeping of a single shared deadline.
pub fn accept_local_pair(
    pair: &LocalListenerPair,
    timeout: Duration,
) -> io::Result<(LocalTransport, LocalTransport)> {
    let sender = MessageChannel::new(accept_one(&pair.host_to_engine, timeout)?);
    let receiver = MessageChannel::new(accept_one(&pair.engine_to_host, timeout)?);
    Ok((sender, receiver))
}

/// Called by an engine-host child process on startup to connect back to
/// the two control sockets its parent (`manifold`) already created.
/// Returns `(receiver, sender)`: recv `HostToEngine` on the first, send
/// `EngineToHost` on the second.
///
/// `socket_id` must match whatever id `listen_local_pair` was called
/// with for this tab (manifold passes it down as `ENGINE_SOCKET_ID`,
/// separate from the user-visible `ENGINE_TAB_ID`).
pub fn connect_local_pair(socket_id: &str) -> io::Result<(LocalTransport, LocalTransport)> {
    let receiver = MessageChannel::new(LocalStream::connect(socket_name(socket_id, "h2e")?)?);
    let sender = MessageChannel::new(LocalStream::connect(socket_name(socket_id, "e2h")?)?);
    Ok((receiver, sender))
}

/// Used only for the WSL2-hosted WebKit engine-host: manifold listens on
/// a TCP port, the WebKit host (spawned via `wsl.exe`) connects to it.
///
/// Bound on all interfaces (0.0.0.0), not just loopback: WSL2's NAT-mode
/// networking does NOT forward a WSL2 process's connection to
/// 127.0.0.1:port on the Windows side (verified directly — it comes back
/// `ECONNREFUSED`, not a timeout). The WSL2 guest instead reaches the
/// Windows host at its default-gateway IP (`ip route`'s `default via`
/// address, which is the host's own `vEthernet (WSL)` adapter) — see
/// `connect_remote`'s caller in manifold-engine-webkit for the matching side.
pub fn listen_remote(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port))
}

/// Bounded wait for the WSL2-hosted engine-host to dial back, rather than
/// `TcpListener::accept`'s unbounded block — a cold/idle WSL2 instance can
/// take an extremely long, unpredictable time to boot its lightweight VM,
/// start Xvfb, and get far enough into GTK/WebKit init to connect, and this
/// call happens on manifold's own main/event-loop thread (`spawn_root_tab`
/// runs from winit's `resumed()`), so an unbounded wait here freezes the
/// entire app — window redraw, OS message pump, and headless stdin command
/// dispatch alike — for as long as that takes (confirmed live: from
/// sub-second on an already-warm WSL2 instance up to several minutes on a
/// cold one, and per an earlier investigation, occasionally hours). Polling
/// via `set_nonblocking` rather than a dedicated accept-with-deadline API,
/// since `std::net::TcpListener` has none.
pub fn accept_remote(listener: &TcpListener, timeout: Duration) -> io::Result<RemoteTransport> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                stream.set_nonblocking(false)?;
                return Ok(MessageChannel::new(stream));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out after {timeout:?} waiting for engine-host to connect"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
}

/// `host` is the Windows side's address as reachable from inside WSL2 —
/// the default-gateway IP, not 127.0.0.1 (see `listen_remote`).
pub fn connect_remote(host: &str, port: u16) -> io::Result<RemoteTransport> {
    let stream = TcpStream::connect((host, port))?;
    Ok(MessageChannel::new(stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the WebKit-engine-host hang: `accept_remote`
    /// used to call `TcpListener::accept()` directly, which blocks
    /// indefinitely if nobody ever connects — on manifold's main/
    /// event-loop thread, that meant the *entire app* froze (window,
    /// input, headless stdin commands) for as long as a WSL2-hosted
    /// engine-host took to dial back, unbounded, confirmed live to reach
    /// multiple hours in one real incident. This doesn't reproduce a cold
    /// WSL2 boot (out of scope for a unit test), but it does pin the
    /// actual behavioral contract the fix promises: given nobody ever
    /// connects at all, `accept_remote` must return within (not much
    /// more than) its `timeout` argument, as a clean `TimedOut` error, not
    /// hang — the exact shape of the bug that let the freeze happen.
    #[test]
    fn accept_remote_times_out_when_nobody_connects() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
        let timeout = Duration::from_millis(200);

        let start = Instant::now();
        let result = accept_remote(&listener, timeout);
        let elapsed = start.elapsed();

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("accept_remote should time out, not succeed"),
        };
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        // Generous upper bound (not a tight timing assertion) — this just
        // needs to prove it didn't block indefinitely, not measure exact
        // poll-loop overhead.
        assert!(
            elapsed < timeout * 5,
            "accept_remote took {elapsed:?}, expected close to the {timeout:?} timeout"
        );
    }

    /// Same regression, for `accept_local_pair`'s bounded wait (added
    /// alongside the WebKit fix, hardening the same unbounded-`accept()`
    /// pattern on the same-machine CEF/Servo path) — a listener with no
    /// client ever connecting must time out cleanly rather than hang.
    #[test]
    fn accept_local_pair_times_out_when_nobody_connects() {
        let pair =
            listen_local_pair("test-accept-local-pair-timeout").expect("listen_local_pair");
        let timeout = Duration::from_millis(200);

        let start = Instant::now();
        let result = accept_local_pair(&pair, timeout);
        let elapsed = start.elapsed();

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("accept_local_pair should time out, not succeed"),
        };
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        // Two sequential per-socket timeouts (see accept_local_pair's doc
        // comment) — worst case is up to ~2x timeout, not 1x.
        assert!(
            elapsed < timeout * 10,
            "accept_local_pair took {elapsed:?}, expected roughly 2x the {timeout:?} timeout"
        );
    }
}
