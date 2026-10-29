use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Serialize};

/// The largest message either side accepts; a longer length prefix is an error, not an allocation.
/// Room for a 4096x4096 RGBA frame (64 MiB) twice over; the length is refused before anything is
/// allocated, so a corrupt or hostile prefix can't make the reader reserve gigabytes.
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
        // One buffer, one write: the 4-byte length is reserved up front and filled in after the body.
        let mut frame =
            postcard::to_extend(msg, vec![0u8; 4]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let len = frame.len() - 4;
        if len > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message larger than MAX_MESSAGE_BYTES"));
        }
        frame[..4].copy_from_slice(&(len as u32).to_le_bytes());
        self.stream.write_all(&frame)?;
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

    /// Writes this exe's protocol number, read from the record it carries on disk (`stamp`), so the number on the wire
    /// is the one `host_protocol` reads; the first thing each side sends.
    pub fn send_version(&mut self) -> io::Result<()> {
        let version = std::hint::black_box(&crate::stamp::STAMP).version;
        self.stream.write_all(&version.to_le_bytes())?;
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

    /// Counts `write` calls: one per message, so a writer thread blocked on a full pipe never leaves a length prefix
    /// written without its body (survey E1).
    struct CountWrites {
        writes: usize,
        bytes: Vec<u8>,
    }

    impl Write for CountWrites {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for CountWrites {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    #[test]
    fn a_message_is_one_write() {
        let mut out = MessageChannel::new(CountWrites { writes: 0, bytes: Vec::new() });
        out.send(&"hello".to_string()).unwrap();
        let sink = out.into_inner();
        assert_eq!(sink.writes, 1, "the length prefix and the body go out in one write");
        assert_eq!(MessageChannel::new(Cursor::new(sink.bytes)).recv::<String>().unwrap(), "hello");
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
        let err = MessageChannel::new(Cursor::new(bytes)).recv::<crate::HostToView>().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
