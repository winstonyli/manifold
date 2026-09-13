use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Serialize};

/// Length-prefixed postcard framing over any duplex byte stream. One
/// `MessageChannel` implementation covers both `LocalTransport` (a
/// named-pipe / Unix-socket stream from `interprocess`) and
/// `RemoteTransport` (a `TcpStream`) — both are just `Read + Write`.
///
/// Uses `postcard`, not `bincode`: bincode is unmaintained as of
/// RUSTSEC-2025-0141 (the whole crate, not just a bad 3.0.0 release) and
/// its own advisory recommends postcard as a replacement.
pub struct MessageChannel<S> {
    stream: S,
}

impl<S: Read + Write> MessageChannel<S> {
    pub fn new(stream: S) -> Self {
        Self { stream }
    }

    pub fn send<T: Serialize>(&mut self, msg: &T) -> io::Result<()> {
        let bytes = postcard::to_allocvec(msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.stream.write_all(&bytes)?;
        self.stream.flush()
    }

    pub fn recv<T: DeserializeOwned>(&mut self) -> io::Result<T> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf)?;
        postcard::from_bytes(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Used only by `RemoteTransport`: read `len` raw frame bytes sent
    /// immediately after an `EngineToHost::FrameReady` message, since a
    /// WSL2-hosted engine-host has no shared memory segment with the
    /// Windows host process.
    pub fn recv_raw(&mut self, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn send_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }

    pub fn into_inner(self) -> S {
        self.stream
    }
}
