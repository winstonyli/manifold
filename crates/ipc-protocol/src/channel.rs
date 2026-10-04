use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Serialize};

/// The largest message either side accepts; a longer length prefix is an error, not an allocation.
pub const MAX_MESSAGE_BYTES: usize = 128 * 1024 * 1024;

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
        let bytes = postcard::to_allocvec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message larger than MAX_MESSAGE_BYTES"));
        }
        self.stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.stream.write_all(&bytes)?;
        self.stream.flush()
    }

    pub fn recv<T: DeserializeOwned>(&mut self) -> io::Result<T> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message larger than MAX_MESSAGE_BYTES"));
        }
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf)?;
        postcard::from_bytes(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Writes `PROTOCOL_VERSION`; the first thing each side sends.
    pub fn send_version(&mut self) -> io::Result<()> {
        self.stream.write_all(&crate::PROTOCOL_VERSION.to_le_bytes())?;
        self.stream.flush()
    }

    /// Reads the peer's `send_version`.
    pub fn recv_version(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.stream.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    pub fn into_inner(self) -> S {
        self.stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_huge_length_prefix_is_refused_before_allocating() {
        let mut bytes = u32::MAX.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 8]);
        let err = MessageChannel::new(Cursor::new(bytes)).recv::<u8>().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_message_round_trips() {
        let mut out = MessageChannel::new(Cursor::new(Vec::new()));
        out.send(&"hello".to_string()).unwrap();
        let mut back = MessageChannel::new(Cursor::new(out.into_inner().into_inner()));
        assert_eq!(back.recv::<String>().unwrap(), "hello");
    }

    #[test]
    fn a_version_round_trips_ahead_of_messages() {
        let mut sender = MessageChannel::new(Cursor::new(Vec::new()));
        sender.send_version().unwrap();
        sender.send(&7u32).unwrap();
        let mut receiver = MessageChannel::new(Cursor::new(sender.into_inner().into_inner()));
        assert_eq!(receiver.recv_version().unwrap(), crate::PROTOCOL_VERSION);
        assert_eq!(receiver.recv::<u32>().unwrap(), 7);
    }

    #[test]
    fn a_truncated_payload_is_an_eof_not_a_hang() {
        let mut bytes = 10u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[1, 2, 3]);
        let err = MessageChannel::new(Cursor::new(bytes)).recv::<u32>().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn garbage_payload_is_invalid_data_not_a_panic() {
        let mut bytes = 4u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        let err = MessageChannel::new(Cursor::new(bytes)).recv::<crate::HostToEngine>().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
