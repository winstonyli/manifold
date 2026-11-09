use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use interprocess::local_socket::{
    prelude::*, GenericNamespaced, Listener, ListenerOptions, Name, Stream as LocalStream, ToNsName,
};

use crate::channel::MessageChannel;
use crate::messages::HostToView;

/// Control-channel connection between `manifold` and a same-machine
/// view-host (CEF, Servo, WebView2). Frame pixels travel separately via
/// `SharedFrameBuffer`; this only carries `HostToView`/`ViewToHost`
/// messages. Each `LocalTransport` is one-directional by convention (see
/// `listen_local_pair`/`connect_local_pair`) — one socket per tab carries
/// host-to-engine messages, a second carries engine-to-host messages, so
/// neither side ever needs to duplicate a stream handle to read and write
/// concurrently from different threads.
pub type LocalTransport = MessageChannel<LocalStream>;

/// Control-channel connection to a WSL2-hosted view-host (WebKit). No
/// shared memory crosses the Windows/WSL2 boundary.
pub type RemoteTransport = MessageChannel<TcpStream>;

/// `HostToView` messages an engine has not taken yet (`spawn_host_reader`). Full, the reader stops reading, so an
/// engine whose main loop is stuck backs up the app's writer for it, and the app shows the tab as not responding;
/// unbounded, the engine's memory grew instead and the app never knew (wave 3b).
pub const VIEW_INBOX: usize = 256;

/// What this view host (speaking `PROTOCOL_VERSION`) makes of manifold's number `app`: `None` when they agree,
/// else the `Skew` naming which side to rebuild.
fn skew_with_app(app: u32) -> Option<crate::Skew> {
    crate::Skew::between(Some(crate::PROTOCOL_VERSION), app)
}

/// An engine's reader thread: forwards each `HostToView` from
/// `from_host` into the returned channel, so the engine's main thread (which
/// has to keep driving its browser's event loop) only ever polls it. Stops
/// after forwarding `Shutdown`. If the connection drops instead,
/// manifold.exe died without its normal teardown (force-killed, crashed):
/// the thread sends a `Shutdown` of its own, so the engine exits through
/// its usual path instead of running orphaned. Before this was shared, only
/// the WebKit and Servo engines did that; WebKit's orphan was seen live at
/// ~45% CPU after `taskkill /F /IM manifold.exe`.
/// Reads manifold's `PROTOCOL_VERSION` first; a mismatch is logged naming which side to rebuild (`Skew`)
/// and is a `Shutdown`.
/// The channel holds `VIEW_INBOX` messages; past that the reader waits for the engine to take one.
pub fn spawn_host_reader<S: Read + Write + Send + 'static>(
    mut from_host: MessageChannel<S>,
) -> mpsc::Receiver<HostToView> {
    let (tx, rx) = mpsc::sync_channel(VIEW_INBOX);
    std::thread::spawn(move || {
        match from_host.recv_version().map(skew_with_app) {
            Ok(None) => {}
            Ok(Some(skew)) => {
                eprintln!("{skew}");
                let _ = tx.send(HostToView::Shutdown);
                return;
            }
            Err(_) => {
                let _ = tx.send(HostToView::Shutdown);
                return;
            }
        }
        loop {
            let msg = from_host.recv::<HostToView>().unwrap_or(HostToView::Shutdown);
            let is_shutdown = matches!(msg, HostToView::Shutdown);
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
    /// manifold writes `HostToView` here; the view-host reads it.
    pub host_to_view: Listener,
    /// the view-host writes `ViewToHost` here; manifold reads it.
    pub view_to_host: Listener,
}

/// Called by manifold (the process manager) right before spawning an
/// view-host child process, so both sockets exist before the child
/// tries to connect to them.
///
/// `socket_id` names the pipes and is normally the tab's `tab_id`, but
/// callers may pass a distinct disambiguating id instead (e.g. when a
/// stale view-host from a prior crashed run still holds `tab_id`'s
/// pipes) — it never needs to match manifold's own tab bookkeeping.
pub fn listen_local_pair(socket_id: &str) -> io::Result<LocalListenerPair> {
    Ok(LocalListenerPair {
        host_to_view: ListenerOptions::new().name(socket_name(socket_id, "h2e")?).create_sync()?,
        view_to_host: ListenerOptions::new().name(socket_name(socket_id, "e2h")?).create_sync()?,
    })
}

/// Bounded wait for a single same-machine view-host (CEF/Servo/WebView2) to dial
/// back on one of the two sockets, rather than `Listener::accept`'s
/// unbounded block — see `accept_remote`'s doc comment for the sibling case
/// (there, a cold WSL2 boot; here, a same-machine child process that fails
/// to reach `connect_local_pair` at all, e.g. a corrupted/mismatched binary
/// that exits before connecting). Callers run on a spawn worker thread, so
/// the bound keeps that thread, its tab's "starting" card and the
/// half-started child from lingering, rather than keeping the UI alive.
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
                        format!("timed out after {timeout:?} waiting for view-host to connect"),
                    ));
                }
                if !waiting() {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped waiting for view-host"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Called by manifold once the view-host child process has connected.
/// Writes `PROTOCOL_VERSION` on the sender before returning.
/// Returns `(sender, receiver)`: send `HostToView` on the first, recv
/// `ViewToHost` on the second. `timeout` applies to each socket in turn
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
    let mut sender = MessageChannel::new(accept_one(&pair.host_to_view, timeout, waiting)?);
    sender.send_version()?;
    let receiver = MessageChannel::new(accept_one(&pair.view_to_host, timeout, waiting)?);
    Ok((sender, receiver))
}

/// Called by a view host child process on startup to connect back to
/// the two control sockets its parent (`manifold`) already created.
/// Returns `(receiver, sender)`: recv `HostToView` on the first, send
/// `ViewToHost` on the second.
///
/// `socket_id` must match whatever id `listen_local_pair` was called
/// with for this tab (manifold passes it down as `VIEW_SOCKET_ID`).
/// Writes `PROTOCOL_VERSION` on the sender; `spawn_host_reader` reads the
/// host's from the receiver.
pub fn connect_local_pair(socket_id: &str) -> io::Result<(LocalTransport, LocalTransport)> {
    let receiver = MessageChannel::new(LocalStream::connect(socket_name(socket_id, "h2e")?)?);
    let mut sender = MessageChannel::new(LocalStream::connect(socket_name(socket_id, "e2h")?)?);
    sender.send_version()?;
    Ok((receiver, sender))
}

/// Bytes in the one-time token a remote view host sends first.
pub const TOKEN_LEN: usize = 16;
/// The secret a spawned WSL view-host must present (`VIEW_TOKEN`, as hex).
pub type Token = [u8; TOKEN_LEN];

/// Lowercase hex of `bytes`. The only form in which text that came from a page crosses a shell boundary.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes `hex` spells, or `None` unless it is an even number of ASCII hex digits (`from_str_radix`
/// alone would also take a leading `+`).
pub fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect()
}

pub fn token_from_hex(hex: &str) -> Option<Token> {
    from_hex(hex)?.try_into().ok()
}

/// The `TOKEN_LEN` bytes a connecting peer sends, read within one second of now or `deadline`, whichever is first:
/// the limit covers the whole token, so a peer dripping a byte at a time cannot hold the wait past it.
fn read_token(stream: &mut TcpStream, deadline: Instant) -> Option<Token> {
    let end = (Instant::now() + Duration::from_secs(1)).min(deadline);
    let mut token = [0u8; TOKEN_LEN];
    let mut got = 0;
    while got < TOKEN_LEN {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        stream.set_read_timeout(Some(left)).ok()?;
        match stream.read(&mut token[got..]) {
            Ok(0) | Err(_) => return None,
            Ok(n) => got += n,
        }
    }
    Some(token)
}

/// Bounded wait for the WSL2-hosted view-host to dial back, rather than
/// `TcpListener::accept`'s unbounded block — a cold/idle WSL2 instance can
/// take an extremely long, unpredictable time to boot its lightweight VM,
/// start Xvfb, and get far enough into GTK/WebKit init to connect (from
/// sub-second on an already-warm WSL2 instance up to several minutes on a
/// cold one, and occasionally hours). The call runs on a spawn worker thread
/// (`App::spawn_tab_async`; the UI thread is not blocked), but an unbounded
/// wait would leave the tab on its "starting" card with no error or retry
/// and its `wsl.exe` launcher running, forever. Polling
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
        io::Error::new(io::ErrorKind::TimedOut, format!("timed out after {timeout:?} waiting for view-host to connect"))
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
                if read_token(&mut stream, deadline).as_ref() == Some(token) {
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

/// `host` is the Windows side's address as reachable from inside WSL2 — the default-gateway IP (`ip route`'s
/// `default via`, the host's `vEthernet (WSL)` adapter), not 127.0.0.1: WSL2's NAT mode answers a guest connection to
/// the Windows side's 127.0.0.1 with `ECONNREFUSED`. The listener is bound on all interfaces for that reason.
/// Sends `token` first, which `accept_remote` checks, then `PROTOCOL_VERSION`.
pub fn connect_remote(host: &str, port: u16, token: &Token) -> io::Result<RemoteTransport> {
    let mut stream = TcpStream::connect((host, port))?;
    stream.write_all(token)?;
    let mut channel = MessageChannel::new(stream);
    channel.send_version()?;
    Ok(channel)
}

/// The first message from manifold, waiting up to `timeout` and calling
/// `pump` between polls so the engine's own loop keeps running (CEF work,
/// window messages). `None` on timeout or a closed channel.
pub fn wait_for_first_message(
    rx: &mpsc::Receiver<HostToView>,
    timeout: Duration,
    mut pump: impl FnMut(),
) -> Option<HostToView> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        pump();
        match rx.try_recv() {
            Ok(msg) => return Some(msg),
            Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(8)),
            Err(mpsc::TryRecvError::Disconnected) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests;
