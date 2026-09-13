use std::io;
use std::net::{TcpListener, TcpStream};

use interprocess::local_socket::{
    prelude::*, GenericNamespaced, Listener, ListenerOptions, Name, Stream as LocalStream,
    ToNsName,
};

use crate::channel::MessageChannel;

/// Control-channel connection between `browser-ui` and a same-machine
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

fn socket_name(tab_id: &str, suffix: &str) -> io::Result<Name<'static>> {
    format!("multiengine-browser-{tab_id}-{suffix}")
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

pub struct LocalListenerPair {
    /// browser-ui writes `HostToEngine` here; the engine-host reads it.
    pub host_to_engine: Listener,
    /// the engine-host writes `EngineToHost` here; browser-ui reads it.
    pub engine_to_host: Listener,
}

/// Called by browser-ui (the process manager) right before spawning an
/// engine-host child process, so both sockets exist before the child
/// tries to connect to them.
pub fn listen_local_pair(tab_id: &str) -> io::Result<LocalListenerPair> {
    Ok(LocalListenerPair {
        host_to_engine: ListenerOptions::new()
            .name(socket_name(tab_id, "h2e")?)
            .create_sync()?,
        engine_to_host: ListenerOptions::new()
            .name(socket_name(tab_id, "e2h")?)
            .create_sync()?,
    })
}

/// Called by browser-ui once the engine-host child process has connected.
/// Returns `(sender, receiver)`: send `HostToEngine` on the first, recv
/// `EngineToHost` on the second.
pub fn accept_local_pair(
    pair: &LocalListenerPair,
) -> io::Result<(LocalTransport, LocalTransport)> {
    let sender = MessageChannel::new(pair.host_to_engine.accept()?);
    let receiver = MessageChannel::new(pair.engine_to_host.accept()?);
    Ok((sender, receiver))
}

/// Called by an engine-host child process on startup to connect back to
/// the two control sockets its parent (`browser-ui`) already created.
/// Returns `(receiver, sender)`: recv `HostToEngine` on the first, send
/// `EngineToHost` on the second.
pub fn connect_local_pair(tab_id: &str) -> io::Result<(LocalTransport, LocalTransport)> {
    let receiver = MessageChannel::new(LocalStream::connect(socket_name(tab_id, "h2e")?)?);
    let sender = MessageChannel::new(LocalStream::connect(socket_name(tab_id, "e2h")?)?);
    Ok((receiver, sender))
}

/// Used only for the WSL2-hosted WebKit engine-host: browser-ui listens on
/// a TCP port, the WebKit host (spawned via `wsl.exe`) connects to it.
///
/// Bound on all interfaces (0.0.0.0), not just loopback: WSL2's NAT-mode
/// networking does NOT forward a WSL2 process's connection to
/// 127.0.0.1:port on the Windows side (verified directly — it comes back
/// `ECONNREFUSED`, not a timeout). The WSL2 guest instead reaches the
/// Windows host at its default-gateway IP (`ip route`'s `default via`
/// address, which is the host's own `vEthernet (WSL)` adapter) — see
/// `connect_remote`'s caller in engine-host-webkit for the matching side.
pub fn listen_remote(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port))
}

pub fn accept_remote(listener: &TcpListener) -> io::Result<RemoteTransport> {
    let (stream, _addr) = listener.accept()?;
    Ok(MessageChannel::new(stream))
}

/// `host` is the Windows side's address as reachable from inside WSL2 —
/// the default-gateway IP, not 127.0.0.1 (see `listen_remote`).
pub fn connect_remote(host: &str, port: u16) -> io::Result<RemoteTransport> {
    let stream = TcpStream::connect((host, port))?;
    Ok(MessageChannel::new(stream))
}
