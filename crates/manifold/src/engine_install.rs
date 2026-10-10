//! On-demand install of the CEF runtime: the pin, what is on disk, and the states the UI shows.

use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::process_manager::EngineKind;

use ipc_protocol::transport::to_hex;
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

/// How a CEF runtime's version relates to the pin's, by CEF major version: the host is built against one
/// CEF API version (CEF_API_VERSION 15200 for CEF 152), which libcef of that major and later majors serve
/// and earlier ones refuse (the 2026-10-04 spike: a host built for 152 ran on CEF 154, and CEF 151 refused it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Compat {
    /// The pin's major.
    Current,
    /// A later major: usable, though not what the app was tested with.
    Newer,
    /// An earlier major: the host can't start on it.
    Older,
    /// Either version has no leading major number.
    Unknown,
}

/// The CEF major of a version string: its leading number, in every form we read ("152.0.6",
/// "152.0.6+g708dc14+chromium-152.0.7977.83", the cef crate's "152.1.0+152.0.6").
fn cef_major(version: &str) -> Option<u32> {
    let version = version.trim();
    let end = version.find(|c: char| !c.is_ascii_digit()).unwrap_or(version.len());
    let rest = &version[end..];
    if end == 0 || !(rest.is_empty() || rest.starts_with(['.', '+'])) {
        return None;
    }
    version[..end].parse().ok()
}

/// `runtime_version` against `pin_version`, by CEF major (`Compat`).
pub(crate) fn runtime_compat(pin_version: &str, runtime_version: &str) -> Compat {
    match (cef_major(pin_version), cef_major(runtime_version)) {
        (Some(pin), Some(runtime)) if runtime == pin => Compat::Current,
        (Some(pin), Some(runtime)) if runtime > pin => Compat::Newer,
        (Some(_), Some(_)) => Compat::Older,
        _ => Compat::Unknown,
    }
}

/// Ready when the marker names the pin's version or a later CEF major (`Compat::Newer`: an install by a
/// newer app, which this host still runs on), Stale when it names any other (an older major, or another
/// build of the pin's major, so a re-pin within a major still offers its update), Missing without one.
/// Deliberately, a later build of the pin's major is Stale too: its "Update needed" downloads the pin,
/// an older build of that major; it is the pin the app was tested with.
pub(crate) fn runtime_state(dir: &Path, pin: &Pin) -> RuntimeOnDisk {
    match std::fs::read_to_string(dir.join(MARKER)) {
        Ok(v) if v.trim() == pin.version || runtime_compat(pin.version, &v) == Compat::Newer => RuntimeOnDisk::Ready,
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

/// Whether `dir` is a CEF runtime this host runs on: it holds the platform's runtime library
/// (`CefPlatform::runtime_lib`; nothing else is required, e.g. CEF 154's archive has no `libEGL.dll`)
/// and its version (`custom_version`: `archive.json` or our `MARKER`) is the pin's CEF major or a later one
/// (`runtime_compat`, while that major keeps our API version in its supported range); a version with no
/// readable major is not usable. An `Err` says what is wrong, naming both versions on a mismatch.
pub(crate) fn validate_custom(dir: &Path, pin: &Pin) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let lib = crate::cef_platform::PLATFORM.runtime_lib;
    if !dir.join(lib).is_file() {
        return Err(format!("no {lib} in that directory"));
    }
    let found = custom_version(dir)?;
    if !matches!(runtime_compat(pin.version, &found), Compat::Current | Compat::Newer) {
        return Err(format!("found CEF {found}, expected {} or a later CEF version", pin.version));
    }
    Ok(())
}

/// A note for a usable bring-your-own runtime of a later CEF major than the pin's, which runs but was
/// not what this build was tested with; `None` for any other.
pub(crate) fn newer_than_tested(dir: &Path, pin: &Pin) -> Option<String> {
    let found = custom_version(dir).ok()?;
    (runtime_compat(pin.version, &found) == Compat::Newer).then(|| {
        let tested = pin.version.split('+').next().unwrap_or(pin.version);
        format!("CEF {} is newer than the tested {tested}", found.split('+').next().unwrap_or(&found))
    })
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
                if have + chunk.len() as u64 > total {
                    // The pin says how big the file is. More than that is a wrong or hostile stream: stop
                    // before it fills the disk, and drop what arrived (a resume would ask the same server).
                    drop(file);
                    let _ = std::fs::remove_file(part);
                    return Err(InstallError::Network(format!("the server sent more than the {total} bytes expected")));
                }
                file.write_all(&chunk).map_err(io)?;
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
    for (want, got) in [(pin.sha256, to_hex(&sha256.finalize())), (pin.sha1, to_hex(&sha1.finalize()))] {
        if !want.eq_ignore_ascii_case(&got) {
            return Err(InstallError::BadHash { want: want.to_string(), got });
        }
    }
    Ok(())
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
#[cfg_attr(not(unix), allow(dead_code, reason = "only applied on Unix; tested everywhere"))]
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

/// The engine runtime installs and their Settings UI (`App::engine_ui`).
#[derive(Default)]
pub(crate) struct EngineInstallUi {
    /// Engine runtime installs, at most one per engine (see `InstallJob`).
    pub(crate) engine_installs: HashMap<EngineKind, InstallJob>,
    /// The last install's failure message per engine, until the next attempt starts.
    pub(crate) engine_failures: HashMap<EngineKind, String>,
    /// An engine download or removal waiting for its confirm dialog (`build_engine_confirm_popup`).
    pub(crate) pending_engine_confirm: Option<crate::engine_info::EngineConfirm>,
    /// `engine_state` per engine for the current frame: it stats the disk, and cards, pickers, menus
    /// and Settings all ask. Cleared at the start of each frame and whenever an install changes.
    pub(crate) engine_state_cache: std::cell::RefCell<HashMap<EngineKind, EngineState>>,
    /// The bring-your-own CEF directory's last validation, with the path it was for (`custom_cef_dir`).
    pub(crate) cef_dir_check: std::cell::RefCell<Option<DirCheck>>,
    /// Settings' text field for that directory while it is being edited; committed on Enter or focus loss.
    pub(crate) cef_dir_draft: Option<String>,
}

/// A running (or just cancelled) install of one engine's runtime, on its own thread.
pub(crate) struct InstallJob {
    /// Bytes in the `.part` so far, written by the thread.
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    /// The pin's size, for `(received, total)`.
    total: u64,
    /// Set by Cancel: the UI no longer shows the job, but the thread may linger briefly (until it next
    /// checks the flag), and its result is discarded. The job stays in the map until the thread has
    /// finished, so a new install never races its `.part`.
    cancelled: bool,
    handle: JoinHandle<Result<PathBuf, InstallError>>,
}

impl crate::App {
    /// How `kind` is available now: the disk, then this app's running install (a cancelled one doesn't
    /// count) and its last failure.
    ///
    /// Cached for the frame (`engine_state_cache`): the disk is read once per engine per frame.
    pub(crate) fn engine_state(&self, kind: EngineKind) -> EngineState {
        if let Some(state) = self.engine_ui.engine_state_cache.borrow().get(&kind) {
            return state.clone();
        }
        let state = self.engine_state_at(kind, crate::process_manager::engine_on_disk(kind));
        self.engine_ui.engine_state_cache.borrow_mut().insert(kind, state.clone());
        state
    }

    /// `engine_state` with the engine's on-disk state (`engine_on_disk`) given (tests use a temporary directory).
    pub(crate) fn engine_state_at(&self, kind: EngineKind, on_disk: RuntimeOnDisk) -> EngineState {
        let installing = self
            .engine_ui
            .engine_installs
            .get(&kind)
            .filter(|job| !job.cancelled)
            .map(|job| (job.progress.load(Ordering::Relaxed), job.total));
        let failed = self.engine_ui.engine_failures.get(&kind).map(String::as_str);
        let custom = (kind == EngineKind::Cef).then(|| self.custom_cef_dir()).flatten();
        crate::process_manager::engine_availability(kind, on_disk, installing, failed, custom.as_deref())
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
        let mut cache = self.engine_ui.cef_dir_check.borrow_mut();
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

    /// Forget the cached verdict (and the states built on it, and the Settings page's disk reads) so the next
    /// read looks at the disk again.
    pub(crate) fn invalidate_cef_dir_check(&self) {
        *self.engine_ui.cef_dir_check.borrow_mut() = None;
        self.engine_ui.engine_state_cache.borrow_mut().clear();
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
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
        if let Some(job) = self.engine_ui.engine_installs.get(&kind) {
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
        if !self.spawns.spawning.is_empty() {
            // A tab still starting may be a CEF host about to load the runtime's DLLs.
            self.queue_toast(format!("A tab is still starting; try installing {label} again in a moment"));
            return false;
        }
        self.engine_ui.engine_failures.remove(&kind);
        self.engine_ui.engine_state_cache.borrow_mut().clear();
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (thread_progress, thread_cancel) = (progress.clone(), cancel.clone());
        let (data_dir, pin) = (data_dir.to_path_buf(), pin.clone());
        let total = pin.size;
        let handle = std::thread::spawn(move || {
            install(&pin, &data_dir, &mut |n| thread_progress.store(n, Ordering::Relaxed), &thread_cancel)
        });
        self.engine_ui.engine_installs.insert(kind, InstallJob { progress, cancel, total, cancelled: false, handle });
        self.egui_ctx.request_repaint();
        true
    }

    /// Delete `kind`'s downloaded runtime (Settings' Remove, after its confirm), toasting the outcome.
    pub(crate) fn remove_engine(&mut self, kind: EngineKind) {
        let label = crate::engine_info::engine_label(kind);
        let Some(dir) = crate::process_manager::engine_hosts_dir() else { return };
        // A spawn still in flight may be this engine's host (it isn't a node yet), so it counts too.
        let running = !self.spawns.spawning.is_empty()
            || self.nodes.values().any(|n| n.as_tab().is_some_and(|t| t.tab.kind == kind));
        let installing = self.engine_ui.engine_installs.contains_key(&kind);
        let removed = remove_runtime(&dir, running, installing);
        // Even a failed removal may have deleted some of it: the Settings page reads the disk again.
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
        match removed {
            Ok(()) => {
                // A recorded failure described the files just deleted.
                self.engine_ui.engine_failures.remove(&kind);
                self.engine_ui.engine_state_cache.borrow_mut().clear();
                self.queue_toast(format!("{label} removed"));
            }
            Err(why) => self.queue_toast(format!("Couldn't remove {label}: {why}")),
        }
    }

    /// Stop showing `kind`'s install at once and tell its thread to stop: it notices within about 100 ms
    /// while downloading (even on a stalled read), and between unpack entries and before the swap after
    /// that. The `.part` stays for resuming and no failure is recorded.
    pub(crate) fn cancel_engine_install(&mut self, kind: EngineKind) {
        if let Some(job) = self.engine_ui.engine_installs.get_mut(&kind) {
            job.cancel.store(true, Ordering::Relaxed);
            job.cancelled = true;
        }
        self.engine_ui.engine_state_cache.borrow_mut().clear();
        self.egui_ctx.request_repaint();
    }

    /// Collect finished jobs (once per frame): toast and record a failure, toast a success, drop a
    /// cancelled job's result. Keeps repainting while any job is running.
    pub(crate) fn poll_engine_installs(&mut self) {
        if self.engine_ui.engine_installs.is_empty() {
            return;
        }
        let finished: Vec<EngineKind> = self
            .engine_ui
            .engine_installs
            .iter()
            .filter(|(_, job)| job.handle.is_finished())
            .map(|(kind, _)| *kind)
            .collect();
        if !finished.is_empty() {
            self.engine_ui.engine_state_cache.borrow_mut().clear();
            // An install (or a cancelled one's leftovers) changed what the Settings page shows from disk.
            crate::settings_page::forget_disk_cache(&self.egui_ctx);
        }
        for kind in finished {
            let Some(job) = self.engine_ui.engine_installs.remove(&kind) else { continue };
            let result =
                job.handle.join().unwrap_or_else(|_| Err(InstallError::Io("the install thread panicked".into())));
            let label = crate::engine_info::engine_label(kind);
            match result {
                _ if job.cancelled => {}
                Err(InstallError::Cancelled) => {}
                Ok(_) => {
                    self.engine_ui.engine_failures.remove(&kind);
                    self.queue_toast(format!("{label} installed"));
                }
                Err(e) => {
                    self.queue_toast(format!("{label} install failed: {e}"));
                    self.engine_ui.engine_failures.insert(kind, e.to_string());
                }
            }
            self.egui_ctx.request_repaint();
        }
        if !self.engine_ui.engine_installs.is_empty() {
            self.egui_ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests;
