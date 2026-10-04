use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use interprocess::local_socket::{
    prelude::*, GenericNamespaced, Listener, ListenerOptions, Name, Stream as LocalStream, ToNsName,
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
/// Reads the host's `PROTOCOL_VERSION` first; a mismatch is a `Shutdown`.
pub fn spawn_host_reader<S: Read + Write + Send + 'static>(
    mut from_host: MessageChannel<S>,
) -> mpsc::Receiver<HostToEngine> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        match from_host.recv_version().map(crate::check_version) {
            Ok(Ok(())) => {}
            Ok(Err(mismatch)) => {
                eprintln!("{mismatch}");
                let _ = tx.send(HostToEngine::Shutdown);
                return;
            }
            Err(_) => {
                let _ = tx.send(HostToEngine::Shutdown);
                return;
            }
        }
        loop {
            let msg = from_host.recv::<HostToEngine>().unwrap_or(HostToEngine::Shutdown);
            let is_shutdown = matches!(msg, HostToEngine::Shutdown);
            if tx.send(msg).is_err() || is_shutdown {
                break;
            }
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
        host_to_engine: ListenerOptions::new().name(socket_name(socket_id, "h2e")?).create_sync()?,
        engine_to_host: ListenerOptions::new().name(socket_name(socket_id, "e2h")?).create_sync()?,
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
fn accept_one(listener: &Listener, timeout: Duration, waiting: &mut dyn FnMut() -> bool) -> io::Result<LocalStream> {
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
                if !waiting() {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped waiting for engine-host"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Called by manifold once the engine-host child process has connected.
/// Writes `PROTOCOL_VERSION` on the sender before returning.
/// Returns `(sender, receiver)`: send `HostToEngine` on the first, recv
/// `EngineToHost` on the second. `timeout` applies to each socket in turn
/// (so the real worst case is up to `2 * timeout`, not shared/split between
/// them) — the two accepts are cheap and effectively sequential in
/// practice (`connect_local_pair` dials both back within microseconds of
/// each other), so this favors giving each one its own full budget over
/// the bookkeeping of a single shared deadline.
pub fn accept_local_pair(pair: &LocalListenerPair, timeout: Duration) -> io::Result<(LocalTransport, LocalTransport)> {
    accept_local_pair_while(pair, timeout, &mut || true)
}

/// `accept_local_pair` that also gives up, with `ErrorKind::Interrupted`, as
/// soon as `waiting` returns false (checked between accept polls): manifold
/// passes "the child is still running", so a host that dies on start is
/// reported at once rather than after the whole timeout.
pub fn accept_local_pair_while(
    pair: &LocalListenerPair,
    timeout: Duration,
    waiting: &mut dyn FnMut() -> bool,
) -> io::Result<(LocalTransport, LocalTransport)> {
    let mut sender = MessageChannel::new(accept_one(&pair.host_to_engine, timeout, waiting)?);
    sender.send_version()?;
    let receiver = MessageChannel::new(accept_one(&pair.engine_to_host, timeout, waiting)?);
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
/// Writes `PROTOCOL_VERSION` on the sender; `spawn_host_reader` reads the
/// host's from the receiver.
pub fn connect_local_pair(socket_id: &str) -> io::Result<(LocalTransport, LocalTransport)> {
    let receiver = MessageChannel::new(LocalStream::connect(socket_name(socket_id, "h2e")?)?);
    let mut sender = MessageChannel::new(LocalStream::connect(socket_name(socket_id, "e2h")?)?);
    sender.send_version()?;
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
/// The secret a spawned WSL engine-host must present (`ENGINE_TOKEN`, as hex).
pub const TOKEN_LEN: usize = 16;
pub type Token = [u8; TOKEN_LEN];

pub fn token_to_hex(token: &Token) -> String {
    token.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn token_from_hex(hex: &str) -> Option<Token> {
    if hex.len() != TOKEN_LEN * 2 || !hex.is_ascii() {
        return None;
    }
    let mut token = [0u8; TOKEN_LEN];
    for (i, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(token)
}

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
/// A connection is accepted only once it sends `token` (see `connect_remote`): the
/// listener is on every interface, so without it the first host on the network to
/// connect would become the engine. Connections that send another token, or none
/// within a second (or within what is left of `timeout`, if less), are dropped and
/// the wait continues until `timeout`; the deadline holds however many such peers
/// arrive. Writes `PROTOCOL_VERSION` once the token is accepted.
pub fn accept_remote(listener: &TcpListener, timeout: Duration, token: &Token) -> io::Result<RemoteTransport> {
    let timed_out = || {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out after {timeout:?} waiting for engine-host to connect"),
        )
    };
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _addr)) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(timed_out());
                }
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(left.min(Duration::from_secs(1))))?;
                let mut sent = [0u8; TOKEN_LEN];
                if stream.read_exact(&mut sent).is_ok() && &sent == token {
                    stream.set_read_timeout(None)?;
                    let mut channel = MessageChannel::new(stream);
                    channel.send_version()?;
                    return Ok(channel);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(timed_out());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
}

/// `host` is the Windows side's address as reachable from inside WSL2 —
/// the default-gateway IP, not 127.0.0.1 (see `listen_remote`).
/// Sends `token` first, which `accept_remote` checks, then `PROTOCOL_VERSION`.
pub fn connect_remote(host: &str, port: u16, token: &Token) -> io::Result<RemoteTransport> {
    let mut stream = TcpStream::connect((host, port))?;
    stream.write_all(token)?;
    let mut channel = MessageChannel::new(stream);
    channel.send_version()?;
    Ok(channel)
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
        let result = accept_remote(&listener, timeout, &[7; TOKEN_LEN]);
        let elapsed = start.elapsed();

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("accept_remote should time out, not succeed"),
        };
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        // Generous upper bound (not a tight timing assertion) — this just
        // needs to prove it didn't block indefinitely, not measure exact
        // poll-loop overhead.
        assert!(elapsed < timeout * 5, "accept_remote took {elapsed:?}, expected close to the {timeout:?} timeout");
    }

    /// Same regression, for `accept_local_pair`'s bounded wait (added
    /// alongside the WebKit fix, hardening the same unbounded-`accept()`
    /// pattern on the same-machine CEF/Servo path) — a listener with no
    /// client ever connecting must time out cleanly rather than hang.
    #[test]
    fn accept_local_pair_times_out_when_nobody_connects() {
        let pair = listen_local_pair("test-accept-local-pair-timeout").expect("listen_local_pair");
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

    #[test]
    fn a_dropped_host_becomes_shutdown() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&crate::PROTOCOL_VERSION.to_le_bytes()).unwrap();
            // dropped: the engine's reader must turn this into Shutdown
        });
        let rx = spawn_host_reader(MessageChannel::new(TcpStream::connect(("127.0.0.1", port)).unwrap()));
        host.join().unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), HostToEngine::Shutdown));
    }

    #[test]
    fn a_wrong_version_from_the_host_becomes_shutdown() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&(crate::PROTOCOL_VERSION + 1).to_le_bytes()).unwrap();
            // A well-formed Navigate after it must never be delivered. Best
            // effort: the reader may already have hung up on the bad version.
            let _ =
                MessageChannel::new(stream).send(&HostToEngine::Navigate { url: "https://x".into(), referrer: None });
        });
        let rx = spawn_host_reader(MessageChannel::new(TcpStream::connect(("127.0.0.1", port)).unwrap()));
        host.join().unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), HostToEngine::Shutdown));
    }

    /// An intruder that keeps reconnecting with a bad token must not keep
    /// `accept_remote` past its deadline.
    #[test]
    fn accept_remote_times_out_under_a_stream_of_bad_peers() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let intruder = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
                        let _ = s.write_all(&[0u8; TOKEN_LEN]);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        let start = Instant::now();
        let result = accept_remote(&listener, Duration::from_millis(300), &[7; TOKEN_LEN]);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        intruder.join().unwrap();
        assert_eq!(result.err().map(|e| e.kind()), Some(io::ErrorKind::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
    }

    /// Peers that connect and then say nothing each used to cost a full 1 s
    /// read timeout, deadline or not, so a queue of them kept `accept_remote`
    /// (on the UI thread) busy for as many seconds as there were peers.
    #[test]
    fn accept_remote_times_out_under_silent_peers() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        // Held open (not dropped) so each read waits out its timeout, not EOF.
        let silent: Vec<TcpStream> = (0..5).map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap()).collect();
        let start = Instant::now();
        let result = accept_remote(&listener, Duration::from_millis(300), &[7; TOKEN_LEN]);
        drop(silent);
        assert_eq!(result.err().map(|e| e.kind()), Some(io::ErrorKind::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn the_token_round_trips_through_hex() {
        let token: Token = std::array::from_fn(|i| (i * 17) as u8);
        assert_eq!(token_from_hex(&token_to_hex(&token)), Some(token));
        assert_eq!(token_from_hex("zz"), None);
        assert_eq!(token_from_hex(&"é".repeat(16)), None);
    }

    #[test]
    fn only_a_peer_with_the_token_is_accepted() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token: Token = [9; TOKEN_LEN];
        let intruder = std::thread::spawn(move || {
            let _ = connect_remote("127.0.0.1", port, &[1; TOKEN_LEN]);
        });
        let engine = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            connect_remote("127.0.0.1", port, &[9; TOKEN_LEN]).unwrap()
        });
        let mut accepted = accept_remote(&listener, Duration::from_secs(5), &token).expect("the real engine");
        let mut engine = engine.join().unwrap();
        intruder.join().unwrap();
        // Each side wrote its version as soon as the token was through.
        assert_eq!(accepted.recv_version().unwrap(), crate::PROTOCOL_VERSION);
        assert_eq!(engine.recv_version().unwrap(), crate::PROTOCOL_VERSION);
        engine.send(&1u8).unwrap();
        assert_eq!(accepted.recv::<u8>().unwrap(), 1);
    }
}
