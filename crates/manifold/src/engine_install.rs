//! On-demand install of the CEF runtime: the pin, what is on disk, and the states the UI shows.
//! Later tasks add the unpack.

use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha1::Sha1;
use sha2::{Digest, Sha256};

/// The CEF build the app downloads: where it lives and how to check it.
// Later install tasks read every field.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pin {
    pub(crate) version: &'static str,
    pub(crate) url: &'static str,
    pub(crate) size: u64,
    pub(crate) sha1: &'static str,
    pub(crate) sha256: &'static str,
}

/// How an engine is available, as every engine list shows it.
// Later tasks (lookup, UI) construct and match these.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EngineState {
    Installed,
    NotInstalled { size: u64 },
    Installing { received: u64, total: u64 },
    UpdateNeeded { size: u64 },
    Failed(String),
    Custom(PathBuf),
}

/// What the install directory holds, judged by its marker alone.
// Later tasks use it.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeOnDisk {
    Missing,
    Ready,
    Stale,
}

/// The file written last by an install; holds the pinned version it installed.
// Later tasks use it.
#[allow(dead_code)]
pub(crate) const MARKER: &str = "ready";

/// Ready when the marker names the pin's version, Stale when it names another, Missing without one.
// Later tasks use it.
#[allow(dead_code)]
pub(crate) fn runtime_state(dir: &Path, pin: &Pin) -> RuntimeOnDisk {
    match std::fs::read_to_string(dir.join(MARKER)) {
        Ok(v) if v.trim() == pin.version => RuntimeOnDisk::Ready,
        Ok(_) => RuntimeOnDisk::Stale,
        Err(_) => RuntimeOnDisk::Missing,
    }
}

/// Why an install step failed.
// Later tasks construct `Unpack` and show these.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InstallError {
    Network(String),
    Cancelled,
    BadHash { want: String, got: String },
    Unpack(String),
    Io(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(e) => write!(f, "download failed: {e}"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::BadHash { want, got } => write!(f, "checksum mismatch: expected {want}, got {got}"),
            Self::Unpack(e) => write!(f, "unpack failed: {e}"),
            Self::Io(e) => write!(f, "file error: {e}"),
        }
    }
}

const CHUNK: usize = 64 * 1024;

/// Fetch `url` into `part`, resuming from its current length with a `Range` request. `progress` gets
/// the total bytes in `part` so far (resumed bytes included). On failure `part` keeps what arrived, so
/// the next call resumes. A resume is restarted from zero, once, when the server answers 416 or a 206
/// whose `Content-Range` does not start at the resume point; a 200 means it sent the whole file, which
/// replaces `part`. A body that ends before `total` bytes is a `Network` error, never Ok.
///
/// An existing `part` already `total` bytes long returns Ok at once, so the caller (`install`) MUST
/// delete `part` when `verify` reports `BadHash`, or the next try would accept the same bad file.
///
/// There is no idle timeout on the body: a connection that stalls mid-body blocks the read until the OS
/// gives up, and `cancel` is only checked between reads. Run it on a worker thread.
// Later install tasks call it.
#[allow(dead_code)]
pub(crate) fn download(
    url: &str,
    part: &Path,
    total: u64,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), InstallError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    let mut have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > total {
        have = 0;
    }
    if have == total && have > 0 {
        progress(have);
        return Ok(());
    }
    if fetch(url, part, total, have, progress, cancel)? == Fetched::Done {
        return Ok(());
    }
    match std::fs::remove_file(part) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(InstallError::Io(e.to_string())),
    }
    match fetch(url, part, total, 0, progress, cancel)? {
        Fetched::Done => Ok(()),
        Fetched::Restart => Err(InstallError::Network("the server rejected the request".into())),
    }
}

#[derive(Debug, PartialEq)]
enum Fetched {
    Done,
    /// The server could not resume from `have`; try again from zero.
    Restart,
}

/// One request, appending to `part` from byte `have`.
fn fetch(
    url: &str,
    part: &Path,
    total: u64,
    mut have: u64,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<Fetched, InstallError> {
    let io = |e: std::io::Error| InstallError::Io(e.to_string());
    let mut request = ureq::get(url)
        .config()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build();
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = match request.call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(416)) if have > 0 => return Ok(Fetched::Restart),
        Err(e) => return Err(InstallError::Network(e.to_string())),
    };
    if response.status().as_u16() == 206 {
        let start = response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes "))
            .and_then(|v| v.split('-').next())
            .and_then(|v| v.trim().parse::<u64>().ok());
        if start != Some(have) {
            return Ok(Fetched::Restart);
        }
    } else {
        have = 0;
    }
    let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(part).map_err(io)?;
    file.set_len(have).map_err(io)?;
    file.seek(SeekFrom::Start(have)).map_err(io)?;
    let mut body = response.into_body().into_reader();
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        let n = body.read(&mut buf).map_err(|e| InstallError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(io)?;
        have += n as u64;
        progress(have);
    }
    if have != total {
        return Err(InstallError::Network(format!("the download ended at {have} of {total} bytes")));
    }
    Ok(Fetched::Done)
}

/// Stream `path` through SHA-256 and SHA-1 and require both to match the pin (hex, any case).
// Later install tasks call it.
#[allow(dead_code)]
pub(crate) fn verify(path: &Path, pin: &Pin) -> Result<(), InstallError> {
    let io = |e: std::io::Error| InstallError::Io(e.to_string());
    let mut file = std::fs::File::open(path).map_err(io)?;
    let (mut sha256, mut sha1) = (Sha256::new(), Sha1::new());
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        sha256.update(&buf[..n]);
        sha1.update(&buf[..n]);
    }
    for (want, got) in [(pin.sha256, hex(&sha256.finalize())), (pin.sha1, hex(&sha1.finalize()))] {
        if !want.eq_ignore_ascii_case(&got) {
            return Err(InstallError::BadHash { want: want.to_string(), got });
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::Arc;

    const PIN: Pin = Pin { version: "1.0", url: "http://x/a", size: 10, sha1: "aa", sha256: "bb" };

    #[test]
    fn a_missing_directory_is_missing() {
        assert_eq!(runtime_state(&temp_dir("ei1").join("nope"), &PIN), RuntimeOnDisk::Missing);
    }

    #[test]
    fn a_marker_with_the_pins_version_is_ready_and_another_is_stale() {
        let dir = temp_dir("ei2");
        std::fs::write(dir.join(MARKER), "1.0").unwrap();
        assert_eq!(runtime_state(&dir, &PIN), RuntimeOnDisk::Ready);
        std::fs::write(dir.join(MARKER), "0.9").unwrap();
        assert_eq!(runtime_state(&dir, &PIN), RuntimeOnDisk::Stale);
    }

    #[test]
    fn a_directory_without_a_marker_is_missing_not_ready() {
        let dir = temp_dir("ei3");
        std::fs::write(dir.join("libcef.dll"), b"x").unwrap();
        assert_eq!(runtime_state(&dir, &PIN), RuntimeOnDisk::Missing);
    }

    /// How the test server answers a request that carries `Range: bytes=N-`.
    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        /// 206 with a correct Content-Range.
        Honour,
        /// 200 with the whole body.
        Ignore,
        /// 206 whose Content-Range starts at 0 instead of N.
        WrongStart,
        /// 416.
        Refuse,
    }

    /// Serve `body` on a local port, answering ranged requests per `mode`. `cut_first` closes the first
    /// connection after that many body bytes (the headers still promise the full length).
    fn serve(body: Arc<Vec<u8>>, mode: Mode, cut_first: Option<usize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut cut = cut_first;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut start = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        start = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                }
                let len = body.len();
                let ranged = start > 0;
                if ranged && mode == Mode::Refuse {
                    let _ = stream.write_all(
                        b"HTTP/1.1 416 Range Not Satisfiable
Content-Length: 0
Connection: close

",
                    );
                    continue;
                }
                // The first byte the Content-Range claims; a wrong-start server then sends the whole body
                // from byte 0 under it.
                let claimed = start;
                if ranged && matches!(mode, Mode::Ignore | Mode::WrongStart) {
                    start = 0;
                }
                let head = if ranged && mode != Mode::Ignore {
                    let shown = if mode == Mode::WrongStart { 0 } else { claimed };
                    format!(
                        "HTTP/1.1 206 Partial Content
Content-Range: bytes {shown}-{}/{len}
Content-Length: {}
Connection: close

",
                        len - 1,
                        len - start
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK
Content-Length: {len}
Connection: close

"
                    )
                };
                let mut payload = &body[start..];
                if let Some(n) = cut.take() {
                    payload = &payload[..n.min(payload.len())];
                }
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(payload);
            }
        });
        url
    }

    fn body(n: usize) -> Arc<Vec<u8>> {
        Arc::new((0..n).map(|i| (i % 251) as u8).collect())
    }

    #[test]
    fn download_fetches_everything() {
        let b = body(300_000);
        let url = serve(b.clone(), Mode::Honour, None);
        let part = temp_dir("ei4").join("f.part");
        let mut last = 0;
        download(&url, &part, b.len() as u64, &mut |n| last = n, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), *b);
        assert_eq!(last, b.len() as u64);
    }

    #[test]
    fn download_resumes_after_a_cut_connection() {
        let b = body(300_000);
        let url = serve(b.clone(), Mode::Honour, Some(100_000));
        let part = temp_dir("ei5").join("f.part");
        let cancel = AtomicBool::new(false);
        let err = download(&url, &part, b.len() as u64, &mut |_| {}, &cancel).unwrap_err();
        assert!(matches!(err, InstallError::Network(_)), "{err:?}");
        let kept = std::fs::metadata(&part).unwrap().len();
        assert!(kept > 0 && kept < b.len() as u64, "kept {kept}");
        let mut first = 0;
        download(
            &url,
            &part,
            b.len() as u64,
            &mut |n| {
                if first == 0 {
                    first = n
                }
            },
            &cancel,
        )
        .unwrap();
        assert!(first > kept, "progress counts resumed bytes: {first} vs {kept}");
        assert_eq!(std::fs::read(&part).unwrap(), *b);
    }

    #[test]
    fn download_restarts_when_the_server_ignores_range() {
        let b = body(200_000);
        let url = serve(b.clone(), Mode::Ignore, None);
        let part = temp_dir("ei6").join("f.part");
        std::fs::write(&part, &b[..50_000]).unwrap();
        download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), *b);
    }

    #[test]
    fn download_stops_when_cancelled() {
        let b = body(300_000);
        let url = serve(b.clone(), Mode::Honour, None);
        let part = temp_dir("ei7").join("f.part");
        let cancel = AtomicBool::new(false);
        let err =
            download(&url, &part, b.len() as u64, &mut |_| cancel.store(true, Ordering::Relaxed), &cancel).unwrap_err();
        assert_eq!(err, InstallError::Cancelled);
        let kept = std::fs::metadata(&part).unwrap().len();
        assert!(kept > 0 && kept < b.len() as u64, "kept {kept}");
    }

    #[test]
    fn download_fails_on_a_short_body_without_content_length() {
        let b = body(100_000);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f", listener.local_addr().unwrap());
        let half = b[..40_000].to_vec();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK
Connection: close

",
            );
            let _ = stream.write_all(&half);
        });
        let part = temp_dir("ei9").join("f.part");
        let err = download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::Network(_)), "{err:?}");
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 40_000);
    }

    #[test]
    fn download_restarts_when_content_range_starts_elsewhere() {
        let b = body(200_000);
        let url = serve(b.clone(), Mode::WrongStart, None);
        let part = temp_dir("ei10").join("f.part");
        std::fs::write(&part, &b[..50_000]).unwrap();
        download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), *b);
    }

    #[test]
    fn download_retries_from_zero_after_416_and_fails_if_that_fails_too() {
        let b = body(200_000);
        let url = serve(b.clone(), Mode::Refuse, None);
        let part = temp_dir("ei11").join("f.part");
        std::fs::write(&part, &b[..50_000]).unwrap();
        download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), *b);
        // A server that answers every request 416 fails the retry too.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(
                    b"HTTP/1.1 416 Range Not Satisfiable
Content-Length: 0
Connection: close

",
                );
            }
        });
        let part = temp_dir("ei12").join("f.part");
        std::fs::write(&part, &b[..50_000]).unwrap();
        let err = download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::Network(_)), "{err:?}");
    }

    #[test]
    fn download_cancelled_beforehand_opens_nothing() {
        let part = temp_dir("ei13").join("f.part");
        let err = download("http://127.0.0.1:1/f", &part, 10, &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(err, InstallError::Cancelled);
        assert!(!part.exists());
    }

    #[test]
    fn verify_rejects_a_wrong_hash_and_accepts_the_right_one() {
        let path = temp_dir("ei8").join("f");
        std::fs::write(&path, b"hello").unwrap();
        let sha256: &'static str = Box::leak(hex(&Sha256::digest(b"hello")).into_boxed_str());
        let sha1: &'static str = Box::leak(hex(&Sha1::digest(b"hello")).into_boxed_str());
        let good = Pin { version: "1", url: "u", size: 5, sha1, sha256 };
        verify(&path, &good).unwrap();
        let bad256 = Pin { sha256: "00", ..good.clone() };
        assert!(matches!(verify(&path, &bad256), Err(InstallError::BadHash { .. })));
        let bad1 = Pin { sha1: "00", ..good };
        assert!(matches!(verify(&path, &bad1), Err(InstallError::BadHash { .. })));
    }
}
