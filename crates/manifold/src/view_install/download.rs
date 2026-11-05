//! Fetching an archive or host file and checking its hash.

use crate::view_install::host_pin::Component as HostFile;
use manifold_view_protocol::transport::to_hex;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::*;

const CHUNK: usize = 64 * 1024;

/// Fetch `url` into `part`, resuming from its current length with a `Range` request. `progress` gets
/// the total bytes in `part` so far (resumed bytes included). On failure `part` keeps what arrived, so
/// the next call resumes. A resume is restarted from zero, once, when the server answers 416 or a 206
/// whose `Content-Range` does not start at the resume point; a 200 means it sent the whole file, which
/// replaces `part`. A body that ends before `total` bytes is a `Network` error, never Ok.
/// A body that would pass `total` is a `Network` error too, and deletes `part`.
///
/// An existing `part` already `total` bytes long returns Ok at once, so the caller (`install`) MUST
/// delete `part` when `verify` reports `BadHash`, or the next try would accept the same bad file.
///
/// The body is read on a helper thread so that a connection that goes quiet is noticed: no data for
/// `IDLE_TIMEOUT` is a `Network` error (the `.part` is kept, so a retry resumes), and `cancel` is seen
/// within about 100 ms even while a read is blocked. (ureq's body timeout is a total, not an idle
/// time, so it only bounds the leaked helper thread: see the body budget in `fetch`.) The helper
/// thread ends when the socket does, or at the latest when that budget runs out.
/// Run it on a worker thread.
pub(crate) fn download(
    url: &str,
    part: &Path,
    total: u64,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), InstallError> {
    download_with_idle(url, part, total, progress, cancel, IDLE_TIMEOUT)
}

/// How long a download may receive nothing before it fails.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// `download` with the idle timeout given (tests use a short one).
pub(super) fn download_with_idle(
    url: &str,
    part: &Path,
    total: u64,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
    idle: Duration,
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
    if fetch(url, part, total, have, progress, cancel, idle)? == Fetched::Done {
        return Ok(());
    }
    remove_path(part)?;
    match fetch(url, part, total, 0, progress, cancel, idle)? {
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

/// The slowest link a body read allows for: see the body budget in `fetch`.
const SLOW_BYTES_PER_SEC: u64 = 32 * 1024;

/// One request, appending to `part` from byte `have`.
fn fetch(
    url: &str,
    part: &Path,
    total: u64,
    mut have: u64,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
    idle: Duration,
) -> Result<Fetched, InstallError> {
    // The most the body read may take in total: the time the remaining bytes need at
    // `SLOW_BYTES_PER_SEC`, at least two minutes. The idle timeout is what ends a quiet connection
    // from the caller's side; this total only bounds the helper thread left blocked on a dead socket,
    // which would otherwise leak for ever. A link slower than the floor fails with a `Network` error
    // and resumes from the `.part`.
    let body_budget = crate::net::body_budget(total - have.min(total), SLOW_BYTES_PER_SEC, Duration::from_secs(120));
    let mut request = crate::net::agent()
        .get(url)
        .config()
        .timeout_resolve(Some(Duration::from_secs(15)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(body_budget))
        .build();
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = match request.call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(416)) if have > 0 => return Ok(Fetched::Restart),
        Err(e) => return Err(InstallError::Network(crate::net::describe(&e))),
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
    let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(part)?;
    file.set_len(have)?;
    file.seek(SeekFrom::Start(have))?;
    let mut body = response.into_body().into_reader();
    let (tx, rx) = std::sync::mpsc::sync_channel::<std::io::Result<Vec<u8>>>(4);
    std::thread::spawn(move || loop {
        let mut buf = vec![0u8; CHUNK];
        match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                buf.truncate(n);
                if tx.send(Ok(buf)).is_err() {
                    break;
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e));
                break;
            }
        }
    });
    let mut last_data = std::time::Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        match rx.recv_timeout(idle.min(Duration::from_millis(100))) {
            Ok(Ok(chunk)) => {
                last_data = std::time::Instant::now();
                if have + chunk.len() as u64 > total {
                    // The pin says how big the file is. More than that is a wrong or hostile stream: stop
                    // before it fills the disk, and drop what arrived (a resume would ask the same server).
                    drop(file);
                    let _ = remove_path(part);
                    return Err(InstallError::Network(format!("the server sent more than the {total} bytes expected")));
                }
                file.write_all(&chunk)?;
                have += chunk.len() as u64;
                progress(have);
            }
            Ok(Err(e)) => return Err(InstallError::Network(crate::net::describe_io(e))),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if last_data.elapsed() >= idle => {
                return Err(InstallError::Network(format!("no data for {} s", idle.as_secs_f32())));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if have != total {
        return Err(InstallError::Network(format!("the download ended at {have} of {total} bytes")));
    }
    Ok(Fetched::Done)
}

/// Stream `path` through SHA-256 and SHA-1 and require both to match the pin (hex, any case).
pub(crate) fn verify(path: &Path, pin: &Pin) -> Result<(), InstallError> {
    let mut file = std::fs::File::open(path)?;
    let (mut sha256, mut sha1) = (Sha256::new(), Sha1::new());
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha256.update(&buf[..n]);
        sha1.update(&buf[..n]);
    }
    for (want, got) in [(pin.sha256, to_hex(&sha256.finalize())), (pin.sha1, to_hex(&sha1.finalize()))] {
        if !want.eq_ignore_ascii_case(&got) {
            return Err(InstallError::BadHash { want: want.to_string(), got });
        }
    }
    Ok(())
}

/// The lowercase hex SHA-256 of the file at `path`.
pub(crate) fn sha256_file(path: &Path) -> Result<String, InstallError> {
    #[cfg(test)]
    HASHED.with(|n| n.set(n.get() + 1));
    let mut file = std::fs::File::open(path)?;
    let mut sha256 = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha256.update(&buf[..n]);
    }
    Ok(to_hex(&sha256.finalize()))
}

/// Require the file at `path` to have SHA-256 `want` (hex, any case). A host has no SHA-1: that is the CEF
/// archive's cross-check (`verify`).
pub(crate) fn verify_sha256(path: &Path, want: &str) -> Result<(), InstallError> {
    let got = sha256_file(path)?;
    if want.eq_ignore_ascii_case(&got) {
        Ok(())
    } else {
        Err(InstallError::BadHash { want: want.to_string(), got })
    }
}

/// Hash every host file in `dir` against its pin. Run when a tab is about to spawn the host, so a file changed
/// after the install (the folder is user-writable) is refused instead of run. Not part of the on-disk poll.
pub(crate) fn verify_installed_host(dir: &Path, files: &[HostFile]) -> Result<(), InstallError> {
    files.iter().try_for_each(|file| verify_sha256(&dir.join(file.file), file.sha256))
}
