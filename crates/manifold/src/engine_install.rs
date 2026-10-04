//! On-demand install of the CEF runtime: the pin, what is on disk, and the states the UI shows.

use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::process_manager::EngineKind;

use sha1::Sha1;
use sha2::{Digest, Sha256};

/// The CEF build the app downloads: where it lives and how to check it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pin {
    pub(crate) version: &'static str,
    pub(crate) url: &'static str,
    pub(crate) size: u64,
    pub(crate) sha1: &'static str,
    pub(crate) sha256: &'static str,
}

/// How an engine is available, as every engine list shows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EngineState {
    Installed,
    NotInstalled {
        size: u64,
    },
    Installing {
        received: u64,
        total: u64,
    },
    UpdateNeeded {
        size: u64,
    },
    Failed(String),
    Custom(PathBuf),
    /// No CEF build is pinned for this platform (`cef_pin::pin_for_platform`): nothing to download, no
    /// offer made. Holds the reason to show.
    Unavailable(String),
}

/// Why CEF cannot be installed where `cef_pin::pin_for_platform` is `None`.
pub(crate) const NO_PIN_REASON: &str = "no CEF build for this platform yet";

/// What the install directory holds, judged by its marker alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeOnDisk {
    Missing,
    Ready,
    Stale,
}

/// The file written last by an install; holds the pinned version it installed.
pub(crate) const MARKER: &str = "ready";

/// Ready when the marker names the pin's version, Stale when it names another, Missing without one.
pub(crate) fn runtime_state(dir: &Path, pin: &Pin) -> RuntimeOnDisk {
    match std::fs::read_to_string(dir.join(MARKER)) {
        Ok(v) if v.trim() == pin.version => RuntimeOnDisk::Ready,
        Ok(_) => RuntimeOnDisk::Stale,
        Err(_) => RuntimeOnDisk::Missing,
    }
}

/// A bring-your-own CEF directory's last validation: the configured path, when it was checked, the verdict.
pub(crate) type DirCheck = (String, std::time::Instant, Result<(), String>);

/// How long a bring-your-own CEF directory's verdict is trusted before the disk is read again.
const CEF_DIR_CHECK_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// The CEF version a bring-your-own directory names. Two sources, on every platform:
/// - `archive.json` (a runtime exported by the `cef` crate's tool): its `name` is the archive the build
///   came from (`cef_binary_<version>_<platform>_<distribution>.tar.bz2`); the version has no `_`.
/// - our own `MARKER`, which holds the pin's version (a runtime this app installed; the CDN archives on
///   Windows and Linux carry no `archive.json`).
///
/// `archive.json` wins when present (an invalid one is an error, not a fall-through); with neither, it
/// is refused.
fn custom_version(dir: &Path) -> Result<String, String> {
    match std::fs::read_to_string(dir.join("archive.json")) {
        Ok(text) => {
            let json: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("archive.json is not valid ({e})"))?;
            let name = json.get("name").and_then(|n| n.as_str()).ok_or("archive.json has no \"name\"")?;
            let rest = name.strip_prefix("cef_binary_").unwrap_or(name);
            Ok(rest.split('_').next().unwrap_or(rest).to_string())
        }
        Err(archive_err) => match std::fs::read_to_string(dir.join(MARKER)) {
            Ok(v) => Ok(v.trim().to_string()),
            Err(_) => {
                Err(format!("no archive.json or \"{MARKER}\" marker to read the CEF version from ({archive_err})"))
            }
        },
    }
}

/// Whether `dir` is a CEF runtime of the pinned version: it holds the platform's runtime library
/// (`CefPlatform::runtime_lib`) and its version (`custom_version`: `archive.json` or our `MARKER`) is
/// `pin.version`. An `Err` says what is wrong, naming both versions on a mismatch.
pub(crate) fn validate_custom(dir: &Path, pin: &Pin) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let lib = crate::cef_platform::PLATFORM.runtime_lib;
    if !dir.join(lib).is_file() {
        return Err(format!("no {lib} in that directory"));
    }
    let found = custom_version(dir)?;
    if found != pin.version {
        return Err(format!("found CEF {found}, expected {}", pin.version));
    }
    Ok(())
}

/// Why an install step failed.
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
/// The body is read on a helper thread so that a connection that goes quiet is noticed: no data for
/// `IDLE_TIMEOUT` is a `Network` error (the `.part` is kept, so a retry resumes), and `cancel` is seen
/// within about 100 ms even while a read is blocked. (ureq's body timeout is a total, not an idle
/// time, so it only bounds the leaked helper thread: see the body budget in `fetch`.) The helper thread ends when the socket does, or at the latest when that budget runs out.
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
fn download_with_idle(
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
    match std::fs::remove_file(part) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(InstallError::Io(e.to_string())),
    }
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
    let io = |e: std::io::Error| InstallError::Io(e.to_string());
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
    let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(part).map_err(io)?;
    file.set_len(have).map_err(io)?;
    file.seek(SeekFrom::Start(have)).map_err(io)?;
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
                file.write_all(&chunk).map_err(io)?;
                have += chunk.len() as u64;
                progress(have);
            }
            Ok(Err(e)) => return Err(InstallError::Network(e.to_string())),
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

/// Where the runtime entry `path` goes under `into`, or None to skip it. The path is `<top>/Release/..`
/// or `<top>/Resources/..`; both folders flatten into `into`. Errors on any component that could escape:
/// `..`, a root or prefix, and any name containing `:` (on Windows `C:evil.dll` is a drive-relative name
/// that `join` would let replace the whole destination, and `a:b` names an NTFS stream) or `\`. A backslash
/// is a separator on Windows, so no component holds one there; on Linux it is an ordinary character, so
/// `Release/..\evil` is one odd name rather than an escape, refused anyway so an archive unpacks the same
/// (or fails the same) on every platform. CEF's archives use neither character.
fn target(path: &Path, into: &Path) -> Result<Option<PathBuf>, InstallError> {
    let mut names = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(n) if n.to_string_lossy().contains([':', '\\']) => {
                return Err(InstallError::Unpack(format!("unsafe path in archive: {}", path.display())));
            }
            Component::Normal(n) => names.push(n),
            Component::CurDir => {}
            _ => return Err(InstallError::Unpack(format!("unsafe path in archive: {}", path.display()))),
        }
    }
    let [_top, dir, rest @ ..] = names.as_slice() else { return Ok(None) };
    let platform = &crate::cef_platform::PLATFORM;
    if !platform.runtime_dirs.iter().any(|d| *dir == *d) || rest.is_empty() {
        return Ok(None);
    }
    let last = rest[rest.len() - 1].to_string_lossy().to_ascii_lowercase();
    let skipped = rest.iter().any(|n| platform.skip_dirs.iter().any(|s| *n == *s))
        || platform.skip_files.iter().any(|s| last == s.to_ascii_lowercase())
        || platform.skip_suffixes.iter().any(|s| last.ends_with(s));
    Ok((!skipped).then(|| rest.iter().fold(into.to_path_buf(), |p, n| p.join(n))))
}

/// Extract the runtime from the CEF `tar.bz2` at `archive` into `into`: the files under `<top>/Release/`
/// and `<top>/Resources/` (`CefPlatform::runtime_dirs`), flattened into `into`, without the build leftovers
/// (`CefPlatform::skip_*`; on Windows `*.lib`, `include`, `cmake`, `libcef_dll`, `CMakeLists.txt`,
/// `debug.log`). Any entry that is a link, or whose path has a `..`,
/// root or prefix component, fails the whole unpack. `MARKER` is written last, holding the pin's version.
/// `cancel` is checked between entries (`Cancelled`). On error `into` may be half-filled; the caller
/// discards it.
#[cfg(test)] // production goes through `install`, which calls `unpack_with` directly
pub(crate) fn unpack(archive: &Path, into: &Path, pin: &Pin, cancel: &AtomicBool) -> Result<(), InstallError> {
    unpack_with(archive, into, pin, cancel, &mut || {})
}

/// The permission bits an unpacked file gets: the archive's own, minus
/// setuid/setgid/sticky and group/other write. `chrome-sandbox` needs setuid
/// root only for the setuid sandbox, which manifold doesn't use (the CEF host
/// passes `--no-sandbox`, see `manifold-engine-cef/src/app.rs`).
#[cfg_attr(not(unix), allow(dead_code))] // only applied on Unix; tested everywhere
fn unpacked_mode(header_mode: u32) -> u32 {
    header_mode & 0o755
}

/// `unpack` that calls `on_entry` for each entry, after the cancel check (tests flip `cancel` from it at an
/// exact point).
fn unpack_with(
    archive: &Path,
    into: &Path,
    pin: &Pin,
    cancel: &AtomicBool,
    on_entry: &mut dyn FnMut(),
) -> Result<(), InstallError> {
    let io = |e: std::io::Error| InstallError::Io(e.to_string());
    // A bad archive (corrupt bz2, truncated tar) is `Unpack`; any other failure (disk full, access denied)
    // is `Io`, which `install` treats as retryable without downloading again.
    let bad = |e: std::io::Error| {
        use std::io::ErrorKind::{InvalidData, InvalidInput, UnexpectedEof};
        if matches!(e.kind(), InvalidData | InvalidInput | UnexpectedEof) {
            InstallError::Unpack(e.to_string())
        } else {
            InstallError::Io(e.to_string())
        }
    };
    let file = std::fs::File::open(archive).map_err(io)?;
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(std::io::BufReader::new(file)));
    std::fs::create_dir_all(into).map_err(io)?;
    let mut files = 0usize;
    // Reading the tar itself failing (a bad header checksum is `ErrorKind::Other`) is a bad archive whatever the kind.
    let corrupt = |e: std::io::Error| InstallError::Unpack(e.to_string());
    for entry in tar.entries().map_err(corrupt)? {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        on_entry();
        let mut entry = entry.map_err(corrupt)?;
        let path = entry.path().map_err(corrupt)?.into_owned();
        // Check every entry, even ones that will be skipped.
        let dest = target(&path, into)?;
        match entry.header().entry_type() {
            t if t.is_symlink() || t.is_hard_link() => {
                return Err(InstallError::Unpack(format!("link in archive: {}", path.display())));
            }
            t if t.is_file() => {}
            _ => continue,
        }
        let Some(dest) = dest else { continue };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut out = std::fs::File::create(&dest).map_err(io)?;
        std::io::copy(&mut entry, &mut out).map_err(bad)?;
        // Without this every file lands 0644 and the CEF host can't exec its helpers.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.header().mode().map_err(corrupt)?;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(unpacked_mode(mode))).map_err(io)?;
        }
        files += 1;
    }
    if files == 0 {
        return Err(InstallError::Unpack("the archive held no runtime files".into()));
    }
    std::fs::write(into.join(MARKER), pin.version).map_err(io)
}

fn remove_dir(dir: &Path) -> Result<(), InstallError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(InstallError::Io(e.to_string())),
    }
}

fn remove_file(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Download `pin`, verify it, unpack it beside the final directory and swap it in; returns
/// `data_dir/cef-runtime`. The final directory only changes at the end (old one removed, new one
/// renamed in). A bad hash or a corrupt archive deletes the `.part` (so a retry downloads afresh); a
/// network failure, a cancel (checked while downloading, between archive entries and before the swap) or
/// a file error (disk full) keeps it. Every failure removes the `.tmp` and leaves the previous install
/// alone. Blocking: run it on a worker thread.
pub(crate) fn install(
    pin: &Pin,
    data_dir: &Path,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, InstallError> {
    install_with(pin, data_dir, progress, cancel, &mut || {})
}

/// `install` with `on_entry` passed to the unpack (see `unpack_with`).
fn install_with(
    pin: &Pin,
    data_dir: &Path,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
    on_entry: &mut dyn FnMut(),
) -> Result<PathBuf, InstallError> {
    let io = |e: std::io::Error| InstallError::Io(e.to_string());
    std::fs::create_dir_all(data_dir).map_err(io)?;
    let part = data_dir.join("cef-runtime.part");
    let tmp = data_dir.join("cef-runtime.tmp");
    let live = data_dir.join("cef-runtime");
    download(pin.url, &part, pin.size, progress, cancel)?;
    if let Err(e) = verify(&part, pin) {
        if matches!(e, InstallError::BadHash { .. }) {
            remove_file(&part);
            let _ = remove_dir(&tmp);
        }
        return Err(e);
    }
    let cancelled = || cancel.load(Ordering::Relaxed);
    // A cancel that arrives after the download is seen by the first unpack entry, so no separate check here.
    remove_dir(&tmp)?;
    if let Err(e) = unpack_with(&part, &tmp, pin, cancel, on_entry) {
        let _ = remove_dir(&tmp);
        // The archive itself is bad: a retry must download afresh. Cancel and disk errors keep it.
        if matches!(e, InstallError::Unpack(_)) {
            remove_file(&part);
        }
        return Err(e);
    }
    if cancelled() {
        let _ = remove_dir(&tmp);
        return Err(InstallError::Cancelled);
    }
    if let Err(e) = swap(&tmp, &live) {
        let _ = remove_dir(&tmp);
        return Err(e);
    }
    remove_file(&part);
    Ok(live)
}

/// Put `new` at `live`, keeping the previous `live` aside as `live.old` until the new one is in place.
/// If the second rename fails, the previous install is moved back, so `live` is never half-deleted.
fn swap(new: &Path, live: &Path) -> Result<(), InstallError> {
    let io = |e: std::io::Error| InstallError::Io(e.to_string());
    let old = live.with_extension("old");
    remove_dir(&old)?;
    let had_old = match std::fs::rename(live, &old) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(io(e)),
    };
    if let Err(e) = std::fs::rename(new, live) {
        if had_old {
            let _ = std::fs::rename(&old, live);
        }
        return Err(io(e));
    }
    let _ = remove_dir(&old);
    Ok(())
}

/// The files an install leaves beside the runtime while it works (and after some failures).
const LEFTOVERS: [&str; 3] = ["cef-runtime.part", "cef-runtime.tmp", "cef-runtime.old"];

/// Whether any install leftover (`.part`, `.tmp`, `.old`) is in `data_dir`.
pub(crate) fn has_leftovers(data_dir: &Path) -> bool {
    LEFTOVERS.iter().any(|name| data_dir.join(name).exists())
}

/// Delete the downloaded runtime (`data_dir/cef-runtime`) and the install leftovers beside it, and nothing
/// else. The `ready` marker goes first, so a delete that stops partway never reads as Installed. Refused,
/// with the reason, while an engine process is using it (`running`) or an install is writing next to it
/// (`installing`).
pub(crate) fn remove_runtime(data_dir: &Path, running: bool, installing: bool) -> Result<(), String> {
    if running {
        return Err("Close the tabs using it first".into());
    }
    if installing {
        return Err("It is installing; cancel that first".into());
    }
    let live = data_dir.join("cef-runtime");
    match std::fs::remove_file(live.join(MARKER)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    remove_dir(&live).map_err(|e| e.to_string())?;
    for name in LEFTOVERS {
        let path = data_dir.join(name);
        if path.is_dir() {
            remove_dir(&path).map_err(|e| e.to_string())?;
        } else if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Total size in bytes of the files under `dir` (0 when it is missing).
pub(crate) fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// A running (or just cancelled) install of one engine's runtime, on its own thread.
pub(crate) struct InstallJob {
    /// Bytes in the `.part` so far, written by the thread.
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    /// The pin's size, for `(received, total)`.
    total: u64,
    /// Set by Cancel: the UI no longer shows the job, but the thread may linger briefly (until it next checks the flag), and
    /// its result is discarded. The job stays in the map until the thread has finished, so a new install
    /// never races its `.part`.
    cancelled: bool,
    handle: JoinHandle<Result<PathBuf, InstallError>>,
}

impl crate::App {
    /// How `kind` is available now: the disk, then this app's running install (a cancelled one doesn't
    /// count) and its last failure.
    ///
    /// Cached for the frame (`engine_state_cache`): the disk is read once per engine per frame.
    pub(crate) fn engine_state(&self, kind: EngineKind) -> EngineState {
        if let Some(state) = self.engine_state_cache.borrow().get(&kind) {
            return state.clone();
        }
        let state = self.engine_state_at(kind, crate::process_manager::cef_on_disk());
        self.engine_state_cache.borrow_mut().insert(kind, state.clone());
        state
    }

    /// `engine_state` with the runtime's on-disk state given (tests use a temporary directory).
    pub(crate) fn engine_state_at(&self, kind: EngineKind, cef_on_disk: RuntimeOnDisk) -> EngineState {
        let installing = self
            .engine_installs
            .get(&kind)
            .filter(|job| !job.cancelled)
            .map(|job| (job.progress.load(Ordering::Relaxed), job.total));
        let failed = self.engine_failures.get(&kind).map(String::as_str);
        let custom = (kind == EngineKind::Cef).then(|| self.custom_cef_dir()).flatten();
        crate::process_manager::engine_availability(kind, cef_on_disk, installing, failed, custom.as_deref())
    }

    /// The configured bring-your-own CEF directory, only when it validates against the pin.
    pub(crate) fn custom_cef_dir(&self) -> Option<PathBuf> {
        self.cef_dir_verdict().and_then(|(dir, verdict)| verdict.ok().map(|()| dir))
    }

    /// The configured directory and whether it validates; `None` when none is set. The verdict is cached
    /// with the path (`cef_dir_check`): the disk is read when the path changes or every `CEF_DIR_CHECK_TTL`,
    /// not every frame, so a folder deleted or replaced behind the app's back is noticed.
    pub(crate) fn cef_dir_verdict(&self) -> Option<(PathBuf, Result<(), String>)> {
        let configured = self.settings.cef_directory.as_deref().filter(|s| !s.trim().is_empty())?;
        let mut cache = self.cef_dir_check.borrow_mut();
        if cache.as_ref().is_none_or(|(path, at, _)| path != configured || at.elapsed() > CEF_DIR_CHECK_TTL) {
            let verdict = match crate::cef_pin::pin_for_platform() {
                Some(pin) => validate_custom(Path::new(configured), pin),
                None => Err(NO_PIN_REASON.to_string()),
            };
            *cache = Some((configured.to_string(), std::time::Instant::now(), verdict));
        }
        let verdict = cache.as_ref().map(|(_, _, v)| v.clone())?;
        Some((PathBuf::from(configured), verdict))
    }

    /// Forget the cached verdict (and the states built on it) so the next read looks at the disk again.
    pub(crate) fn invalidate_cef_dir_check(&self) {
        *self.cef_dir_check.borrow_mut() = None;
        self.engine_state_cache.borrow_mut().clear();
    }

    /// Set (or clear, for an empty path) the bring-your-own CEF directory: save it, drop the cached
    /// verdict and states so the next read validates it.
    pub(crate) fn set_cef_directory(&mut self, path: &str) {
        let path = path.trim();
        self.settings.cef_directory = (!path.is_empty()).then(|| path.to_string());
        self.invalidate_cef_dir_check();
        self.save_settings();
    }

    /// Download and install `kind`'s runtime into the data directory. See `start_engine_install_at`.
    pub(crate) fn start_engine_install(&mut self, kind: EngineKind) -> bool {
        let Some(data_dir) = crate::process_manager::engine_hosts_dir() else {
            self.queue_toast("No data directory to install into".into());
            return false;
        };
        let Some(pin) = crate::cef_pin::pin_for_platform() else {
            self.queue_toast(crate::engine_info::PickOutcome::unavailable_toast(kind, NO_PIN_REASON));
            return false;
        };
        self.start_engine_install_at(kind, &data_dir, pin)
    }

    /// Start installing `kind` (only CEF downloads anything) under `data_dir`. Refused, with a toast
    /// saying why, while that engine's install is running or its cancelled thread has not yet stopped
    /// (it would share the `.part`), or while a CEF engine process is alive (it holds the runtime's
    /// DLLs, so the swap would fail). Returns whether a job started.
    pub(crate) fn start_engine_install_at(&mut self, kind: EngineKind, data_dir: &Path, pin: &Pin) -> bool {
        if kind != EngineKind::Cef {
            return false;
        }
        let label = crate::engine_info::engine_label(kind);
        if let Some(job) = self.engine_installs.get(&kind) {
            if !job.cancelled {
                self.queue_toast(format!("{label} is already installing"));
            } else if !job.handle.is_finished() {
                self.queue_toast(format!("The cancelled {label} download is still stopping; try again in a moment"));
            } else {
                // Finished: collect it first (its result is discarded), then start afresh.
                self.poll_engine_installs();
                return self.start_engine_install_at(kind, data_dir, pin);
            }
            return false;
        }
        if self.nodes.values().any(|n| n.as_tab().is_some_and(|t| t.tab.kind == kind)) {
            self.queue_toast(format!("Close {label} tabs before installing it"));
            return false;
        }
        if !self.spawning.is_empty() {
            // A tab still starting may be a CEF host about to load the runtime's DLLs.
            self.queue_toast(format!("A tab is still starting; try installing {label} again in a moment"));
            return false;
        }
        self.engine_failures.remove(&kind);
        self.engine_state_cache.borrow_mut().clear();
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (thread_progress, thread_cancel) = (progress.clone(), cancel.clone());
        let (data_dir, pin) = (data_dir.to_path_buf(), pin.clone());
        let total = pin.size;
        let handle = std::thread::spawn(move || {
            install(&pin, &data_dir, &mut |n| thread_progress.store(n, Ordering::Relaxed), &thread_cancel)
        });
        self.engine_installs.insert(kind, InstallJob { progress, cancel, total, cancelled: false, handle });
        self.egui_ctx.request_repaint();
        true
    }

    /// Delete `kind`'s downloaded runtime (Settings' Remove, after its confirm), toasting the outcome.
    pub(crate) fn remove_engine(&mut self, kind: EngineKind) {
        let label = crate::engine_info::engine_label(kind);
        let Some(dir) = crate::process_manager::engine_hosts_dir() else { return };
        // A spawn still in flight may be this engine's host (it isn't a node yet), so it counts too.
        let running =
            !self.spawning.is_empty() || self.nodes.values().any(|n| n.as_tab().is_some_and(|t| t.tab.kind == kind));
        let installing = self.engine_installs.contains_key(&kind);
        let removed = remove_runtime(&dir, running, installing);
        // Even a failed removal may have deleted some of it: the Settings page reads the disk again.
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
        match removed {
            Ok(()) => {
                // A recorded failure described the files just deleted.
                self.engine_failures.remove(&kind);
                self.engine_state_cache.borrow_mut().clear();
                self.queue_toast(format!("{label} removed"));
            }
            Err(why) => self.queue_toast(format!("Couldn't remove {label}: {why}")),
        }
    }

    /// Stop showing `kind`'s install at once and tell its thread to stop: it notices within about 100 ms
    /// while downloading (even on a stalled read), and between unpack entries and before the swap after
    /// that. The `.part` stays for resuming and no failure is recorded.
    pub(crate) fn cancel_engine_install(&mut self, kind: EngineKind) {
        if let Some(job) = self.engine_installs.get_mut(&kind) {
            job.cancel.store(true, Ordering::Relaxed);
            job.cancelled = true;
        }
        self.engine_state_cache.borrow_mut().clear();
        self.egui_ctx.request_repaint();
    }

    /// Collect finished jobs (once per frame): toast and record a failure, toast a success, drop a
    /// cancelled job's result. Keeps repainting while any job is running.
    pub(crate) fn poll_engine_installs(&mut self) {
        if self.engine_installs.is_empty() {
            return;
        }
        let finished: Vec<EngineKind> =
            self.engine_installs.iter().filter(|(_, job)| job.handle.is_finished()).map(|(kind, _)| *kind).collect();
        if !finished.is_empty() {
            self.engine_state_cache.borrow_mut().clear();
            // An install (or a cancelled one's leftovers) changed what the Settings page shows from disk.
            crate::settings_page::forget_disk_cache(&self.egui_ctx);
        }
        for kind in finished {
            let Some(job) = self.engine_installs.remove(&kind) else { continue };
            let result =
                job.handle.join().unwrap_or_else(|_| Err(InstallError::Io("the install thread panicked".into())));
            let label = crate::engine_info::engine_label(kind);
            match result {
                _ if job.cancelled => {}
                Err(InstallError::Cancelled) => {}
                Ok(_) => {
                    self.engine_failures.remove(&kind);
                    self.queue_toast(format!("{label} installed"));
                }
                Err(e) => {
                    self.queue_toast(format!("{label} install failed: {e}"));
                    self.engine_failures.insert(kind, e.to_string());
                }
            }
            self.egui_ctx.request_repaint();
        }
        if !self.engine_installs.is_empty() {
            self.egui_ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unpacked_files_keep_exec_bits_but_never_setuid_or_group_write() {
        assert_eq!(unpacked_mode(0o100755), 0o755);
        assert_eq!(unpacked_mode(0o644), 0o644);
        assert_eq!(unpacked_mode(0o4755), 0o755, "setuid dropped: CEF runs with --no-sandbox");
        assert_eq!(unpacked_mode(0o777), 0o755);
    }

    use super::*;
    use crate::store::testutil::temp_dir;
    use std::io::BufRead;
    use std::net::TcpListener;

    const PIN: Pin = Pin { version: "1.0", url: "http://x/a", size: 10, sha1: "aa", sha256: "bb" };

    #[test]
    fn engine_state_is_reused_within_a_frame_and_refreshed_when_an_install_changes() {
        let mut app = App::default();
        let kind = EngineKind::Cef;
        app.engine_state_cache.borrow_mut().insert(kind, EngineState::Failed("cached".into()));
        assert_eq!(app.engine_state(kind), EngineState::Failed("cached".into()), "reused");
        app.cancel_engine_install(kind);
        assert_ne!(app.engine_state(kind), EngineState::Failed("cached".into()), "refreshed");
        app.engine_state_cache.borrow_mut().insert(kind, EngineState::Failed("cached".into()));
        app.engine_state_cache.borrow_mut().clear(); // what each new frame does
        assert_ne!(app.engine_state(kind), EngineState::Failed("cached".into()));
    }

    #[test]
    fn remove_deletes_the_runtime_and_its_leftovers_only_when_idle() {
        let dir = temp_dir("remove_runtime");
        std::fs::create_dir_all(dir.join("cef-runtime")).unwrap();
        std::fs::write(dir.join("cef-runtime").join(lib()), vec![0u8; 5]).unwrap();
        std::fs::write(dir.join("cef-runtime.part"), b"keep").unwrap();
        assert_eq!(dir_size(&dir.join("cef-runtime")), 5);
        assert!(remove_runtime(&dir, true, false).is_err());
        assert!(remove_runtime(&dir, false, true).is_err());
        assert!(dir.join("cef-runtime").exists());
        remove_runtime(&dir, false, false).unwrap();
        assert!(!dir.join("cef-runtime").exists());
        assert!(!dir.join("cef-runtime.part").exists(), "Remove also clears the leftovers");
        remove_runtime(&dir, false, false).unwrap(); // already gone is fine
    }

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

    /// The pin of the platform the tests run on (Windows or Linux).
    fn cef_pin() -> &'static Pin {
        crate::cef_pin::pin_for_platform().expect("a pin on a platform with tests")
    }

    /// The runtime library's file name on this platform.
    fn lib() -> &'static str {
        crate::cef_platform::PLATFORM.runtime_lib
    }

    /// A bring-your-own directory as CEF's own archive lays it out: the runtime library and `archive.json`.
    fn custom_dir(tag: &str, archive_name: Option<&str>, dll: bool) -> PathBuf {
        let dir = temp_dir(tag);
        if dll {
            std::fs::write(dir.join(lib()), b"x").unwrap();
        }
        if let Some(name) = archive_name {
            std::fs::write(dir.join("archive.json"), format!(r#"{{"type":"minimal","name":"{name}","sha1":"ab"}}"#))
                .unwrap();
        }
        dir
    }

    #[test]
    fn a_custom_directory_of_the_pinned_version_validates() {
        let pin = cef_pin();
        let name = crate::cef_platform::PLATFORM.archive_name(pin.version);
        assert_eq!(validate_custom(&custom_dir("vc1", Some(&name), true), pin), Ok(()));
        // The pin's own sample: a name without the prefix or suffix still reads.
        assert_eq!(validate_custom(&custom_dir("vc1b", Some(pin.version), true), pin), Ok(()));
    }

    #[test]
    fn a_custom_directory_of_another_version_names_both() {
        let dir =
            custom_dir("vc2", Some(&crate::cef_platform::PLATFORM.archive_name("150.1.2+gabc+chromium-150.0.1")), true);
        let err = validate_custom(&dir, cef_pin()).unwrap_err();
        assert!(err.contains("150.1.2+gabc+chromium-150.0.1"), "{err}");
        assert!(err.contains(cef_pin().version), "{err}");
    }

    #[test]
    fn a_custom_directory_without_archive_json_or_libcef_is_refused() {
        let pin = cef_pin();
        assert!(validate_custom(&custom_dir("vc3", None, true), pin).unwrap_err().contains("archive.json"));
        let name = crate::cef_platform::PLATFORM.archive_name(pin.version);
        assert!(validate_custom(&custom_dir("vc4", Some(&name), false), pin).unwrap_err().contains(lib()));
        assert!(validate_custom(&temp_dir("vc5").join("nope"), pin).is_err());
        let bad = custom_dir("vc6", None, true);
        std::fs::write(bad.join("archive.json"), "{}").unwrap();
        assert!(validate_custom(&bad, pin).is_err());
    }

    #[test]
    fn a_runtime_we_installed_validates_by_its_marker() {
        // Our install leaves the runtime library and `MARKER` (the pin's version), no `archive.json`.
        let pin = cef_pin();
        let ours = custom_dir("vm1", None, true);
        std::fs::write(ours.join(MARKER), pin.version).unwrap();
        assert_eq!(validate_custom(&ours, pin), Ok(()));

        let stale = custom_dir("vm2", None, true);
        std::fs::write(stale.join(MARKER), "150.1.2+gabc+chromium-150.0.1\n").unwrap();
        let err = validate_custom(&stale, pin).unwrap_err();
        assert!(err.contains("150.1.2+gabc+chromium-150.0.1") && err.contains(pin.version), "{err}");

        // No marker and no archive.json: both sources are named.
        let none = validate_custom(&custom_dir("vm3", None, true), pin).unwrap_err();
        assert!(none.contains("archive.json") && none.contains(MARKER), "{none}");
    }

    #[test]
    fn archive_json_outranks_the_marker() {
        let pin = cef_pin();
        let other = crate::cef_platform::PLATFORM.archive_name("150.1.2+gabc+chromium-150.0.1");
        let dir = custom_dir("vm4", Some(&other), true);
        std::fs::write(dir.join(MARKER), pin.version).unwrap();
        assert!(validate_custom(&dir, pin).unwrap_err().contains("150.1.2"));
        // An invalid archive.json is an error even with a good marker beside it.
        std::fs::write(dir.join("archive.json"), "{}").unwrap();
        assert!(validate_custom(&dir, pin).unwrap_err().contains("archive.json"));
    }

    #[test]
    fn a_custom_directory_deleted_later_stops_being_custom() {
        let pin = cef_pin();
        let dir = custom_dir("vc9", Some(&crate::cef_platform::PLATFORM.archive_name(pin.version)), true);
        let mut app = App::default();
        app.set_cef_directory(dir.to_str().unwrap());
        assert!(app.cef_dir_verdict().unwrap().1.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(app.cef_dir_verdict().unwrap().1.is_ok(), "still inside the TTL: the cached verdict stands");
        // Age the cached verdict past the TTL, as waiting would.
        let aged = std::time::Instant::now().checked_sub(CEF_DIR_CHECK_TTL * 2).unwrap();
        app.cef_dir_check.borrow_mut().as_mut().unwrap().1 = aged;
        assert!(app.cef_dir_verdict().unwrap().1.is_err());
        assert_eq!(app.custom_cef_dir(), None);
    }

    #[test]
    fn a_valid_custom_directory_makes_cef_custom_and_an_invalid_one_does_not() {
        let pin = cef_pin();
        let good = custom_dir("vc7", Some(&crate::cef_platform::PLATFORM.archive_name(pin.version)), true);
        let mut app = App::default();
        app.set_cef_directory(good.to_str().unwrap());
        assert_eq!(app.engine_state_at(EngineKind::Cef, RuntimeOnDisk::Missing), EngineState::Custom(good.clone()));
        assert_eq!(app.engine_state_at(EngineKind::Servo, RuntimeOnDisk::Missing), EngineState::Installed);

        let bad = custom_dir("vc8", Some(&crate::cef_platform::PLATFORM.archive_name("1.0")), true);
        app.set_cef_directory(bad.to_str().unwrap());
        assert_eq!(app.custom_cef_dir(), None);
        assert!(app.cef_dir_verdict().unwrap().1.is_err(), "the page shows why");
        // No silent fallback and no Custom: the normal on-disk state.
        assert_eq!(
            app.engine_state_at(EngineKind::Cef, RuntimeOnDisk::Missing),
            EngineState::NotInstalled { size: pin.size }
        );
        assert_eq!(app.engine_state_at(EngineKind::Cef, RuntimeOnDisk::Ready), EngineState::Installed);

        app.set_cef_directory("  ");
        assert_eq!(app.settings.cef_directory, None);
        assert!(app.cef_dir_verdict().is_none());
    }

    #[test]
    fn a_directory_without_a_marker_is_missing_not_ready() {
        let dir = temp_dir("ei3");
        std::fs::write(dir.join(lib()), b"x").unwrap();
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

    /// The installer fetches on the shared agent (`net::agent`), so it sends manifold's User-Agent.
    #[test]
    fn download_sends_the_user_agent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase());
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK
Content-Length: 3
Connection: close

abc",
            );
        });
        let part = temp_dir("ei-ua").join("f.part");
        download(&url, &part, 3, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        let request = rx.recv().unwrap();
        assert!(request.contains("user-agent: manifold/"), "{request}");
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

    /// A `tar.bz2` holding `entries` (path, bytes); a path is written raw, so `..` survives.
    fn archive(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append(&h, *data).unwrap();
        }
        bz(dir, "a.tar.bz2", &tar.into_inner().unwrap())
    }

    fn bz(dir: &Path, name: &str, raw: &[u8]) -> PathBuf {
        let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        enc.write_all(raw).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, enc.finish().unwrap()).unwrap();
        path
    }

    const GOOD: [(&str, &[u8]); 4] = [
        ("top/Release/runtime.bin", b"dll"),
        ("top/Resources/resources.pak", b"pak"),
        ("top/include/x.h", b"h"),
        ("top/Release/libcef.lib", b"lib"),
    ];

    /// A pin for the archive at `path`, served at `url`.
    fn pin_for(path: &Path, url: String) -> Pin {
        let bytes = std::fs::read(path).unwrap();
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        Pin {
            version: "9.9",
            url: leak(url),
            size: bytes.len() as u64,
            sha1: leak(hex(&Sha1::digest(&bytes))),
            sha256: leak(hex(&Sha256::digest(&bytes))),
        }
    }

    #[test]
    fn unpack_keeps_runtime_files_and_drops_leftovers() {
        let dir = temp_dir("ei20");
        let a = archive(&dir, &GOOD);
        let out = dir.join("out");
        unpack(&a, &out, &pin_for(&a, String::new()), &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(out.join("runtime.bin")).unwrap(), b"dll");
        assert_eq!(std::fs::read(out.join("resources.pak")).unwrap(), b"pak");
        assert!(!out.join("x.h").exists() && !out.join("include").exists());
        // An import library is a Windows build leftover; elsewhere `.lib` is not special.
        assert_eq!(!out.join("libcef.lib").exists(), !crate::cef_platform::PLATFORM.skip_suffixes.is_empty());
        assert_eq!(std::fs::read_to_string(out.join(MARKER)).unwrap(), "9.9");
    }

    #[test]
    fn a_corrupt_tar_header_is_a_bad_archive_not_a_file_error() {
        let dir = temp_dir("ei22");
        // 512 bytes of junk: not a valid header, and its checksum cannot match.
        let a = bz(&dir, "junk.tar.bz2", &[b'x'; 1024]);
        let err = unpack(&a, &dir.join("out"), &pin_for(&a, String::new()), &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::Unpack(_)), "{err:?}");
    }

    #[test]
    fn unpack_refuses_a_path_that_escapes() {
        let dir = temp_dir("ei21");
        let a = archive(&dir, &[("top/Release/runtime.bin", b"x"), ("top/Release/../../evil", b"x")]);
        // `into` is nested so that a naive join lands at a place inside `dir` that we can check.
        let nested = dir.join("nested");
        let out = nested.join("out");
        let err = unpack(&a, &out, &pin_for(&a, String::new()), &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::Unpack(_)), "{err:?}");
        assert!(!dir.join("evil").exists() && !out.join(MARKER).exists());
        for (name, lands) in [
            ("top/Release/../evil", nested.join("evil")),
            ("top/Release/..\\evil", nested.join("evil")),
            ("top\\Release\\..\\..\\evil", dir.join("evil")),
            ("../evil", dir.join("evil")),
            ("/top/Release/evil", PathBuf::from("/top/Release/evil")),
        ] {
            let a = archive(&dir, &[(name, b"x")]);
            let r = unpack(&a, &out, &pin_for(&a, String::new()), &AtomicBool::new(false));
            assert!(matches!(r, Err(InstallError::Unpack(_))), "{name}: {r:?}");
            assert!(!lands.exists(), "{name} wrote {lands:?}");
        }
    }

    #[test]
    fn unpack_refuses_names_with_a_colon() {
        // `C:evil.dll` is drive-relative: `join` would let it replace the destination; `a:b` is a stream.
        let dir = temp_dir("ei40");
        let out = dir.join("out");
        for name in ["top/Release/C:evil.dll", "top/Release/a:b", "top/Release/sub/D:x", "top/Resources/c:"] {
            let a = archive(&dir, &[(name, b"x")]);
            let r = unpack(&a, &out, &pin_for(&a, String::new()), &AtomicBool::new(false));
            assert!(matches!(r, Err(InstallError::Unpack(_))), "{name}: {r:?}");
            assert!(!out.join(MARKER).exists(), "{name}");
        }
        assert!(target(Path::new("top/Release/C:evil.dll"), &out).is_err());
        assert!(target(Path::new("top/Release/runtime.bin"), &out).unwrap().is_some());
    }

    #[test]
    fn unpack_stops_when_cancelled() {
        let dir = temp_dir("ei41");
        let a = archive(&dir, &GOOD);
        let r = unpack(&a, &dir.join("out"), &pin_for(&a, String::new()), &AtomicBool::new(true));
        assert_eq!(r, Err(InstallError::Cancelled));
        assert!(!dir.join("out").join(MARKER).exists());
    }

    #[test]
    fn a_cancel_before_unpack_keeps_the_part_and_makes_no_runtime() {
        let dir = temp_dir("ei42");
        let a = archive(&dir, &GOOD);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        // The part is already complete, so the download returns at once; the first unpack entry sees the flag.
        std::fs::copy(&a, data.join("cef-runtime.part")).unwrap();
        let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(err, InstallError::Cancelled);
        assert!(data.join("cef-runtime.part").exists(), "the part is kept");
        assert!(!data.join("cef-runtime").exists() && !data.join("cef-runtime.tmp").exists());
    }

    #[test]
    fn a_cancel_once_the_download_has_started_changes_nothing() {
        let dir = temp_dir("ei43a");
        let a = archive(&dir, &GOOD);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
        std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
        // Cancelled from the progress callback: the download loop itself sees it (not the checks after it).
        let cancel = AtomicBool::new(false);
        let err = install(&pin, &data, &mut |_| cancel.store(true, Ordering::Relaxed), &cancel).unwrap_err();
        assert_eq!(err, InstallError::Cancelled);
        assert!(data.join("cef-runtime.part").exists());
        assert!(!data.join("cef-runtime.tmp").exists());
        assert!(data.join("cef-runtime").join("old").exists(), "no swap happened");
    }

    /// An install whose cancel flag flips at the `n`th unpack entry (counting from 1); `n` past the last
    /// entry never flips it. Returns the result and the data dir, which holds a previous install.
    fn install_cancelled_at_entry(name: &str, n: usize) -> (Result<PathBuf, InstallError>, PathBuf, usize) {
        let dir = temp_dir(name);
        let a = archive(&dir, &GOOD);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
        std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
        let cancel = AtomicBool::new(false);
        let mut seen = 0;
        let r = install_with(&pin, &data, &mut |_| {}, &cancel, &mut || {
            seen += 1;
            if seen == n {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        (r, data, seen)
    }

    #[test]
    fn a_cancel_during_unpack_removes_tmp_and_keeps_the_part() {
        let (r, data, seen) = install_cancelled_at_entry("ei43", 2);
        assert_eq!(r, Err(InstallError::Cancelled));
        assert_eq!(seen, 2, "the unpack stopped at the next entry");
        assert!(data.join("cef-runtime.part").exists());
        assert!(!data.join("cef-runtime.tmp").exists(), "install removed the half-filled tmp");
        assert!(data.join("cef-runtime").join("old").exists());
    }

    #[test]
    fn a_cancel_after_the_last_entry_stops_before_the_swap() {
        // A dry run counts the entries; flipping at the last one lets the whole unpack finish.
        let (r, _, entries) = install_cancelled_at_entry("ei43b", usize::MAX);
        assert!(r.is_ok());
        let (r, data, _) = install_cancelled_at_entry("ei43c", entries);
        assert_eq!(r, Err(InstallError::Cancelled));
        assert!(data.join("cef-runtime.part").exists());
        assert!(!data.join("cef-runtime.tmp").exists());
        assert!(!data.join("cef-runtime.old").exists());
        assert!(data.join("cef-runtime").join("old").exists(), "the previous install is untouched");
        assert!(!data.join("cef-runtime").join(MARKER).exists(), "nothing new was swapped in");
    }

    #[test]
    fn a_stalled_download_times_out_keeps_the_part_and_resumes() {
        let b = body(300_000);
        let (url, release) = serve_held(b.clone());
        let part = temp_dir("ei44").join("f.part");
        let started = std::time::Instant::now();
        let err = download_with_idle(
            &url,
            &part,
            b.len() as u64,
            &mut |_| {},
            &AtomicBool::new(false),
            Duration::from_millis(400),
        )
        .unwrap_err();
        assert!(matches!(err, InstallError::Network(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        let kept = std::fs::metadata(&part).unwrap().len();
        assert!(kept > 0 && kept < b.len() as u64, "kept {kept}");
        release.send(()).unwrap();
        // A resume from the kept part finishes the file.
        let url = serve(b.clone(), Mode::Honour, None);
        download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), *b);
    }

    #[test]
    fn a_stalled_download_is_cancelled_promptly() {
        let b = body(300_000);
        let (url, release) = serve_held(b.clone());
        let part = temp_dir("ei45").join("f.part");
        let cancel = AtomicBool::new(false);
        let err = download_with_idle(
            &url,
            &part,
            b.len() as u64,
            &mut |_| cancel.store(true, Ordering::Relaxed),
            &cancel,
            Duration::from_secs(60),
        )
        .unwrap_err();
        assert_eq!(err, InstallError::Cancelled);
        release.send(()).unwrap();
    }

    #[test]
    fn a_failed_swap_removes_tmp_and_keeps_the_part() {
        let dir = temp_dir("ei46");
        let a = archive(&dir, &GOOD);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        // A file where `cef-runtime.old` should be a directory: the swap cannot clear it.
        std::fs::write(data.join("cef-runtime.old"), b"x").unwrap();
        std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
        std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
        let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::Io(_)), "{err:?}");
        assert!(!data.join("cef-runtime.tmp").exists(), "tmp removed");
        assert!(data.join("cef-runtime.part").exists(), "a file error keeps the part");
        assert!(data.join("cef-runtime").join("old").exists());
    }

    #[test]
    fn remove_deletes_the_marker_first_and_the_leftovers() {
        let dir = temp_dir("ei47");
        let live = dir.join("cef-runtime");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(MARKER), "v").unwrap();
        std::fs::write(live.join(lib()), b"x").unwrap();
        std::fs::write(dir.join("cef-runtime.part"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("cef-runtime.tmp")).unwrap();
        std::fs::create_dir_all(dir.join("cef-runtime.old")).unwrap();
        assert!(has_leftovers(&dir));
        remove_runtime(&dir, false, false).unwrap();
        assert!(!live.exists() && !has_leftovers(&dir));
        // Leftovers with no runtime: still deletable.
        std::fs::write(dir.join("cef-runtime.part"), b"x").unwrap();
        remove_runtime(&dir, false, false).unwrap();
        assert!(!has_leftovers(&dir));
    }

    #[test]
    fn unpack_refuses_a_symlink() {
        let dir = temp_dir("ei22");
        let mut tar = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_path("top/Release/link").unwrap();
        h.set_link_name("/etc/passwd").unwrap();
        h.set_size(0);
        h.set_cksum();
        tar.append(&h, std::io::empty()).unwrap();
        let a = bz(&dir, "s.tar.bz2", &tar.into_inner().unwrap());
        let err = unpack(&a, &dir.join("out"), &pin_for(&a, String::new()), &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::Unpack(_)), "{err:?}");
    }

    /// Serve the archive at `a`.
    fn serve_file(a: &Path) -> String {
        serve(Arc::new(std::fs::read(a).unwrap()), Mode::Honour, None)
    }

    #[test]
    fn install_end_to_end_against_the_local_server() {
        let dir = temp_dir("ei23");
        let a = archive(&dir, &GOOD);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        let mut last = 0;
        let out = install(&pin, &data, &mut |n| last = n, &AtomicBool::new(false)).unwrap();
        assert_eq!(out, data.join("cef-runtime"));
        assert_eq!(runtime_state(&out, &pin), RuntimeOnDisk::Ready);
        assert_eq!(last, pin.size);
        assert!(!data.join("cef-runtime.part").exists() && !data.join("cef-runtime.tmp").exists());
        // A second install replaces the first.
        std::fs::write(out.join("stale.txt"), b"x").unwrap();
        install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert!(!out.join("stale.txt").exists() && out.join("runtime.bin").exists());
    }

    #[test]
    fn install_clears_a_stale_tmp_and_old() {
        let dir = temp_dir("ei27");
        let a = archive(&dir, &GOOD);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("cef-runtime.tmp")).unwrap();
        std::fs::write(data.join("cef-runtime.tmp").join("junk"), b"x").unwrap();
        std::fs::create_dir_all(data.join("cef-runtime.old")).unwrap();
        std::fs::write(data.join("cef-runtime.old").join("junk"), b"x").unwrap();
        let out = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(runtime_state(&out, &pin), RuntimeOnDisk::Ready);
        assert!(!out.join("junk").exists());
        assert!(!data.join("cef-runtime.tmp").exists() && !data.join("cef-runtime.old").exists());
    }

    #[test]
    fn a_failed_swap_restores_the_previous_install() {
        let dir = temp_dir("ei28");
        let live = dir.join("cef-runtime");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(MARKER), "old").unwrap();
        // The new directory does not exist, so the second rename fails.
        let err = swap(&dir.join("cef-runtime.tmp"), &live).unwrap_err();
        assert!(matches!(err, InstallError::Io(_)), "{err:?}");
        assert_eq!(std::fs::read_to_string(live.join(MARKER)).unwrap(), "old");
        assert!(!dir.join("cef-runtime.old").exists());
    }

    #[test]
    fn a_bad_hash_leaves_no_runtime_directory_and_no_part_file() {
        let dir = temp_dir("ei24");
        let a = archive(&dir, &GOOD);
        let pin = Pin { sha256: "00", ..pin_for(&a, serve_file(&a)) };
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("cef-runtime.tmp")).unwrap();
        let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::BadHash { .. }), "{err:?}");
        assert!(!data.join("cef-runtime").exists() && !data.join("cef-runtime.tmp").exists());
        assert!(!data.join("cef-runtime.part").exists());
    }

    #[test]
    fn a_failed_unpack_deletes_tmp_and_part_and_keeps_the_previous_install() {
        let dir = temp_dir("ei25");
        let a = archive(&dir, &[("../evil", b"x")]);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
        std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
        let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::Unpack(_)), "{err:?}");
        assert!(data.join("cef-runtime").join("old").exists());
        assert!(!data.join("cef-runtime.tmp").exists() && !data.join("cef-runtime.part").exists());
    }

    #[test]
    fn a_network_failure_keeps_the_part_file_and_the_previous_install() {
        let dir = temp_dir("ei26");
        let a = archive(&dir, &GOOD);
        let bytes = std::fs::read(&a).unwrap();
        let pin = pin_for(&a, serve(Arc::new(bytes), Mode::Honour, Some(50)));
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
        std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
        let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, InstallError::Network(_)), "{err:?}");
        assert_eq!(std::fs::metadata(data.join("cef-runtime.part")).unwrap().len(), 50);
        assert!(data.join("cef-runtime").join("old").exists());
    }

    // ---- the install job on the App ----

    use crate::App;

    /// The state the engine list would show for CEF with `data` as the data directory.
    fn cef_state(app: &App, data: &Path, pin: &Pin) -> EngineState {
        app.engine_state_at(EngineKind::Cef, runtime_state(&data.join("cef-runtime"), pin))
    }

    /// Poll until no job is left (the thread has finished and been collected).
    fn settle(app: &mut App) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !app.engine_installs.is_empty() {
            app.poll_engine_installs();
            assert!(std::time::Instant::now() < deadline, "the job never finished");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn toasts(app: &App) -> Vec<String> {
        app.save_toast_queue.iter().cloned().collect()
    }

    #[test]
    fn a_job_installs_and_toasts() {
        let dir = temp_dir("ei30");
        let a = archive(&dir, &GOOD);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        let mut app = App::default();
        assert!(matches!(cef_state(&app, &data, &pin), EngineState::NotInstalled { .. }));
        assert!(app.start_engine_install_at(EngineKind::Cef, &data, &pin));
        assert!(matches!(cef_state(&app, &data, &pin), EngineState::Installing { total, .. } if total == pin.size));
        settle(&mut app);
        assert_eq!(cef_state(&app, &data, &pin), EngineState::Installed);
        assert_eq!(toasts(&app), ["Blink (CEF) installed"]);
        assert!(app.engine_failures.is_empty());
    }

    #[test]
    fn an_install_waits_while_a_tab_is_still_starting() {
        let dir = temp_dir("ei30b");
        let a = archive(&dir, &GOOD);
        let pin = pin_for(&a, serve_file(&a));
        let data = dir.join("data");
        let mut app = App::default();
        let pending = crate::session_sync::Shared {
            kind: crate::store::model::Kind::Tab,
            group: None,
            fields: Default::default(),
        };
        app.spawning.insert("t1".into(), pending);
        assert!(!app.start_engine_install_at(EngineKind::Cef, &data, &pin));
        assert!(app.engine_installs.is_empty());
        assert_eq!(toasts(&app), ["A tab is still starting; try installing Blink (CEF) again in a moment"]);
        app.spawning.clear();
        assert!(app.start_engine_install_at(EngineKind::Cef, &data, &pin));
        settle(&mut app);
    }

    #[test]
    fn a_failed_job_records_the_reason_and_a_retry_clears_it() {
        let dir = temp_dir("ei31");
        let a = archive(&dir, &GOOD);
        let good = pin_for(&a, serve_file(&a));
        let bad = Pin { sha256: "0000", ..good.clone() };
        let data = dir.join("data");
        let mut app = App::default();
        assert!(app.start_engine_install_at(EngineKind::Cef, &data, &bad));
        settle(&mut app);
        let EngineState::Failed(message) = cef_state(&app, &data, &bad) else { panic!("not failed") };
        assert!(message.contains("checksum"), "{message}");
        assert_eq!(toasts(&app), [format!("Blink (CEF) install failed: {message}")]);
        assert!(app.start_engine_install_at(EngineKind::Cef, &data, &good));
        assert!(matches!(cef_state(&app, &data, &good), EngineState::Installing { .. }), "failure cleared");
        settle(&mut app);
        assert_eq!(cef_state(&app, &data, &good), EngineState::Installed);
    }

    /// A server that sends half of `body`, then holds the connection until `release` is sent.
    fn serve_held(body: Arc<Vec<u8>>) -> (String, std::sync::mpsc::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f", listener.local_addr().unwrap());
        let (release, wait) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(&body[..body.len() / 2]);
            let _ = stream.flush();
            let _ = wait.recv();
        });
        (url, release)
    }

    #[test]
    fn cancel_is_immediate_keeps_the_part_and_records_no_failure() {
        let dir = temp_dir("ei32");
        let a = archive(&dir, &GOOD);
        let b = Arc::new(std::fs::read(&a).unwrap());
        let (url, release) = serve_held(b.clone());
        let pin = pin_for(&a, url);
        let data = dir.join("data");
        let mut app = App::default();
        assert!(app.start_engine_install_at(EngineKind::Cef, &data, &pin));
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !matches!(cef_state(&app, &data, &pin), EngineState::Installing { received, .. } if received > 0) {
            assert!(std::time::Instant::now() < deadline, "no progress");
            std::thread::sleep(Duration::from_millis(10));
        }
        app.cancel_engine_install(EngineKind::Cef);
        assert!(matches!(cef_state(&app, &data, &pin), EngineState::NotInstalled { .. }), "at once");
        // The helper thread is still blocked on its read, but the job sees the cancel within ~100 ms.
        release.send(()).unwrap();
        settle(&mut app);
        assert!(matches!(cef_state(&app, &data, &pin), EngineState::NotInstalled { .. }));
        assert!(app.engine_failures.is_empty() && toasts(&app).is_empty(), "a cancel is not a failure");
        assert!(std::fs::metadata(data.join("cef-runtime.part")).unwrap().len() > 0, "the part is kept");
        // Once the thread is gone a new install starts (and resumes the part).
        assert!(app.start_engine_install_at(EngineKind::Cef, &data, &Pin { url: pin.url, ..pin.clone() }));
        app.cancel_engine_install(EngineKind::Cef);
    }

    #[test]
    fn one_install_per_engine_and_only_cef_downloads() {
        let dir = temp_dir("ei33");
        let a = archive(&dir, &GOOD);
        let (url, release) = serve_held(Arc::new(std::fs::read(&a).unwrap()));
        let pin = pin_for(&a, url);
        let data = dir.join("data");
        let mut app = App::default();
        assert!(app.start_engine_install_at(EngineKind::Cef, &data, &pin));
        assert!(!app.start_engine_install_at(EngineKind::Cef, &data, &pin), "already installing");
        assert_eq!(app.engine_installs.len(), 1);
        app.cancel_engine_install(EngineKind::Cef);
        release.send(()).unwrap();
        settle(&mut app);
        assert!(!app.start_engine_install_at(EngineKind::Webkit, &data, &pin), "only CEF downloads anything");
    }

    #[test]
    fn the_pin_matches_the_cef_crates_version() {
        let lock = include_str!("../../../Cargo.lock");
        let after = lock.split("name = \"cef\"").nth(1).expect("Cargo.lock has a cef package");
        let line = after.lines().nth(1).expect("a version line follows the name");
        let version = line.strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')).expect("a version line");
        // `152.1.0+152.0.6`: the crate's own version, then the CEF build it wraps.
        let build = version.split_once('+').expect("the cef crate version names its CEF build").1;
        let pinned = cef_pin().version;
        assert_eq!(pinned.split('+').next(), Some(build), "the pin is not the CEF build the crate wraps: {pinned}");
    }

    /// Verifies and unpacks a real downloaded archive for this platform's pin, timing both:
    /// `MANIFOLD_CEF_ARCHIVE=<archive> MANIFOLD_CEF_UNPACK_TO=<new dir> cargo test -p manifold
    /// unpack_a_real_archive -- --ignored --nocapture`. Ignored: it needs the 172 MB (Windows) or 321 MB
    /// (Linux) download, which no test makes.
    #[test]
    #[ignore]
    fn unpack_a_real_archive() {
        let archive = PathBuf::from(std::env::var_os("MANIFOLD_CEF_ARCHIVE").expect("MANIFOLD_CEF_ARCHIVE"));
        let into = PathBuf::from(std::env::var_os("MANIFOLD_CEF_UNPACK_TO").expect("MANIFOLD_CEF_UNPACK_TO"));
        let pin = cef_pin();
        let start = std::time::Instant::now();
        verify(&archive, pin).unwrap();
        println!("verify: {:.1} s", start.elapsed().as_secs_f64());
        let start = std::time::Instant::now();
        unpack(&archive, &into, pin, &AtomicBool::new(false)).unwrap();
        println!("unpack: {:.1} s", start.elapsed().as_secs_f64());
        assert_eq!(runtime_state(&into, pin), RuntimeOnDisk::Ready);
    }
}
