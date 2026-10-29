//! On-demand installs of what an engine needs besides `manifold.exe`: an engine is a list of components, a pinned
//! host (one or more single files, `host_pin`) and, for CEF, the runtime archive (`Pin`). Also the states the UI shows.

use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::host_pin::Component as HostFile;
use crate::process_manager::runtime::Os;
use crate::process_manager::{HostSource, ViewKind, CEF_RUNTIME_DIR};

use manifold_view_protocol::transport::to_hex;
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
pub(crate) enum ViewStatus {
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
    /// It can't run here: this OS can't run it (`platform_unavailable_reason`), the host that would run speaks another
    /// protocol than this build (`OnDisk::skew`), or there is nothing to download and nothing there (no CEF build for
    /// this platform, `NO_PIN_REASON`; no host pinned for the engine, `no_download_reason`). No offer made. Holds the
    /// reason to show.
    Unavailable(String),
}

/// Why CEF's runtime cannot be installed where `cef_pin::pin_for_platform` is `None`.
pub(crate) const NO_PIN_REASON: &str = "no CEF build for this platform yet";

/// Why `kind` cannot be installed in a build whose `host_pin` table has no entry for it (the repo's table is empty;
/// a release writes it): there is nothing to fetch and no sibling host.
pub(crate) fn no_download_reason(kind: ViewKind) -> String {
    format!("this build has no {} download", kind.desc().label)
}

/// What the install directory holds, judged by its marker alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeOnDisk {
    Missing,
    Ready,
    Stale,
}

/// What the disk holds of an engine's two downloadable parts, each judged by a marker (never by hashing), and whether
/// the host that would run speaks this build's protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OnDisk {
    /// The host exe and the files that travel with it: `host_state`, or Ready when a sibling host is there.
    pub(crate) host: RuntimeOnDisk,
    /// CEF's runtime archive (`runtime_state`); Ready for an engine with none.
    pub(crate) runtime: RuntimeOnDisk,
    /// The host that would run in place speaks another protocol than this build (`availability::read_on_disk_at`);
    /// `None` when it speaks this one or none runs in place.
    pub(crate) skew: Option<manifold_view_protocol::Skew>,
}

impl OnDisk {
    pub(crate) const fn both(state: RuntimeOnDisk) -> Self {
        Self { host: state, runtime: state, skew: None }
    }

    /// The one answer for the parts that matter: Ready when all are, Missing when any is missing, else Stale.
    /// A bring-your-own runtime (`custom_runtime`) stands in for the downloaded one, so only the host counts.
    pub(crate) fn needed(self, custom_runtime: bool) -> RuntimeOnDisk {
        let runtime = if custom_runtime { RuntimeOnDisk::Ready } else { self.runtime };
        match (self.host, runtime) {
            (RuntimeOnDisk::Ready, RuntimeOnDisk::Ready) => RuntimeOnDisk::Ready,
            (RuntimeOnDisk::Missing, _) | (_, RuntimeOnDisk::Missing) => RuntimeOnDisk::Missing,
            _ => RuntimeOnDisk::Stale,
        }
    }
}

/// What installing an engine fetches.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    /// The pinned host files, in install order (`host_pin::Entry::components`).
    pub(crate) host: &'static [HostFile],
    /// CEF's runtime archive.
    pub(crate) runtime: Option<Pin>,
}

impl Plan {
    fn host_size(&self) -> u64 {
        self.host.iter().map(|f| f.size).sum()
    }

    /// Every byte of every part: the total a running install reports progress against.
    pub(crate) fn size(&self) -> u64 {
        self.host_size() + self.runtime.as_ref().map_or(0, |pin| pin.size)
    }

    /// The bytes an install would fetch given `disk`: each part that is not Ready, whole. A bring-your-own runtime
    /// (`custom_runtime`) is never fetched.
    pub(crate) fn fetch_size(&self, disk: OnDisk, custom_runtime: bool) -> u64 {
        let host = if disk.host == RuntimeOnDisk::Ready { 0 } else { self.host_size() };
        let runtime = match &self.runtime {
            Some(pin) if disk.runtime != RuntimeOnDisk::Ready && !custom_runtime => pin.size,
            _ => 0,
        };
        host + runtime
    }
}

/// Whether `kind` is installed by downloading (`HostSource::Download`), rather than found beside `manifold.exe`
/// or carried inside it.
pub(crate) fn downloads(kind: ViewKind) -> bool {
    kind.desc().source == HostSource::Download
}

/// Whether `kind` also needs a runtime archive besides its host exe (`ViewDesc::runtime`).
pub(crate) fn has_runtime(kind: ViewKind) -> bool {
    kind.desc().runtime_dir.is_some()
}

/// The Visual C++ runtime DLLs Servo's Windows host imports (its mozjs and ANGLE are built against the dynamic CRT, so
/// it cannot carry them; the Microsoft Visual C++ Redistributable installs them in System32).
const SERVO_VC_RUNTIME: &[&str] = &["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"];

/// The files of `names` that `dir` lacks.
fn missing_files(dir: &Path, names: &[&'static str]) -> Vec<&'static str> {
    names.iter().copied().filter(|name| !dir.join(name).is_file()).collect()
}

/// System libraries `kind`'s host needs that the system lacks (`ViewDesc::missing_system_libs`): CEF's NSS/NSPR on
/// Linux, the Visual C++ runtime for Servo on Windows.
pub(crate) fn missing_system_libs(kind: ViewKind) -> Vec<&'static str> {
    (kind.desc().missing_system_libs)()
}

/// Servo's: the Visual C++ runtime DLLs its Windows host imports that System32 lacks. Nothing elsewhere.
pub(crate) fn missing_vc_runtime() -> Vec<&'static str> {
    if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map_or_else(Vec::new, |root| missing_files(&Path::new(&root).join("System32"), SERVO_VC_RUNTIME))
    } else {
        Vec::new()
    }
}

/// `host_pin`'s name for the platform `kind`'s host is built for: this build's own OS, except an engine built for
/// another (WebKit's is a Linux ELF even when manifold runs on Windows, through WSL).
pub(crate) fn host_platform(kind: ViewKind) -> &'static str {
    kind.desc().host_os.unwrap_or_else(Os::here).pin_platform()
}

/// The host files this build pins for `kind` on this platform; empty for an engine that is not downloaded.
pub(crate) fn host_components(kind: ViewKind) -> &'static [HostFile] {
    if downloads(kind) {
        crate::host_pin::components_for(kind, host_platform(kind))
    } else {
        &[]
    }
}

/// What installing `kind` fetches in this build, or why nothing can be: it is not downloaded or pins no host
/// (`no_download_reason`), or CEF has no runtime build for this platform (`NO_PIN_REASON`).
pub(crate) fn plan_for(kind: ViewKind) -> Result<Plan, String> {
    let host = host_components(kind);
    if host.is_empty() {
        return Err(no_download_reason(kind));
    }
    let runtime =
        if has_runtime(kind) { Some(crate::cef_pin::pin_for_platform().ok_or(NO_PIN_REASON)?.clone()) } else { None };
    Ok(Plan { host, runtime })
}

/// The file written last by a CEF runtime install; holds the pinned version it installed.
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

/// `kind`'s host marker in the hosts directory `dir`: one `<sha256> <file>` line per installed file, written last
/// by `install_host`.
fn host_marker_path(dir: &Path, kind: ViewKind) -> PathBuf {
    dir.join(format!("{}.host", kind.desc().name))
}

fn marker_text(files: &[HostFile]) -> String {
    files.iter().map(|f| format!("{} {}\n", f.sha256, f.file)).collect()
}

/// What `dir` holds of the host `files`, judged without hashing (the 5 s on-disk poll reads this): Ready when the
/// marker lists exactly these files and hashes and every file has its pinned size, Stale when the marker lists
/// others (another release), Missing without a marker or when a listed file is gone or the wrong size. The hashes
/// are checked at install and again when a tab spawns (`verify_installed_host`).
pub(crate) fn host_state(dir: &Path, kind: ViewKind, files: &[HostFile]) -> RuntimeOnDisk {
    if files.is_empty() {
        return RuntimeOnDisk::Missing;
    }
    let Ok(marker) = std::fs::read_to_string(host_marker_path(dir, kind)) else { return RuntimeOnDisk::Missing };
    if marker != marker_text(files) {
        return RuntimeOnDisk::Stale;
    }
    if files.iter().all(|f| std::fs::metadata(dir.join(f.file)).is_ok_and(|m| m.len() == f.size)) {
        RuntimeOnDisk::Ready
    } else {
        RuntimeOnDisk::Missing
    }
}

#[cfg(test)]
thread_local! {
    /// How many files this thread has hashed (`sha256_file`), for the test that the on-disk poll hashes none.
    static HASHED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Called by `unpack` at each entry after the cancel check, so a test can flip `cancel` at an exact entry.
    static ON_ENTRY: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
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

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
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

/// The permission bits an unpacked file gets: the archive's own, minus
/// setuid/setgid/sticky and group/other write. `chrome-sandbox` needs setuid
/// root only for the setuid sandbox, which manifold doesn't use (the CEF host
/// passes `--no-sandbox`, see `manifold-view-cef/src/app.rs`).
#[cfg_attr(not(unix), allow(dead_code, reason = "only applied on Unix; tested everywhere"))]
fn unpacked_mode(header_mode: u32) -> u32 {
    header_mode & 0o755
}

/// Extract the runtime from the CEF `tar.bz2` at `archive` into `into`: the files under `<top>/Release/`
/// and `<top>/Resources/` (`CefPlatform::runtime_dirs`), flattened into `into`, without the build leftovers
/// (`CefPlatform::skip_*`; on Windows `*.lib`, `include`, `cmake`, `libcef_dll`, `CMakeLists.txt`,
/// `debug.log`). Any entry that is a link, or whose path has a `..`,
/// root or prefix component, fails the whole unpack. `MARKER` is written last, holding the pin's version.
/// `cancel` is checked between entries (`Cancelled`). On error `into` may be half-filled; the caller
/// discards it.
pub(crate) fn unpack(archive: &Path, into: &Path, pin: &Pin, cancel: &AtomicBool) -> Result<(), InstallError> {
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
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(std::io::BufReader::new(file)));
    std::fs::create_dir_all(into)?;
    let mut files = 0usize;
    // Reading the tar itself failing (a bad header checksum is `ErrorKind::Other`) is a bad archive whatever the kind.
    let corrupt = |e: std::io::Error| InstallError::Unpack(e.to_string());
    for entry in tar.entries().map_err(corrupt)? {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        #[cfg(test)]
        ON_ENTRY.with(|hook| hook.borrow_mut().as_mut().map(|f| f()));
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
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut out).map_err(bad)?;
        // Without this every file lands 0644 and the CEF host can't exec its helpers.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.header().mode().map_err(corrupt)?;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(unpacked_mode(mode)))?;
        }
        files += 1;
    }
    if files == 0 {
        return Err(InstallError::Unpack("the archive held no runtime files".into()));
    }
    Ok(std::fs::write(into.join(MARKER), pin.version)?)
}

/// Delete the file or directory at `path`; one that is already gone is fine.
fn remove_path(path: &Path) -> std::io::Result<()> {
    let removed = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
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
    std::fs::create_dir_all(data_dir)?;
    let part = data_dir.join(format!("{CEF_RUNTIME_DIR}.part"));
    let tmp = data_dir.join(format!("{CEF_RUNTIME_DIR}.tmp"));
    let live = data_dir.join(CEF_RUNTIME_DIR);
    download(pin.url, &part, pin.size, progress, cancel)?;
    if let Err(e) = verify(&part, pin) {
        if matches!(e, InstallError::BadHash { .. }) {
            let _ = remove_path(&part);
            let _ = remove_path(&tmp);
        }
        return Err(e);
    }
    let cancelled = || cancel.load(Ordering::Relaxed);
    // A cancel that arrives after the download is seen by the first unpack entry, so no separate check here.
    remove_path(&tmp)?;
    if let Err(e) = unpack(&part, &tmp, pin, cancel) {
        let _ = remove_path(&tmp);
        // The archive itself is bad: a retry must download afresh. Cancel and disk errors keep it.
        if matches!(e, InstallError::Unpack(_)) {
            let _ = remove_path(&part);
        }
        return Err(e);
    }
    if cancelled() {
        let _ = remove_path(&tmp);
        return Err(InstallError::Cancelled);
    }
    if let Err(e) = swap(&tmp, &live) {
        let _ = remove_path(&tmp);
        return Err(e);
    }
    let _ = remove_path(&part);
    Ok(live)
}

/// Put `new` at `live`, keeping the previous `live` aside as `live.old` until the new one is in place.
/// If the second rename fails, the previous install is moved back, so `live` is never half-deleted.
fn swap(new: &Path, live: &Path) -> Result<(), InstallError> {
    let old = live.with_extension("old");
    remove_path(&old)?;
    let had_old = match std::fs::rename(live, &old) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    if let Err(e) = std::fs::rename(new, live) {
        if had_old {
            let _ = std::fs::rename(&old, live);
        }
        return Err(e.into());
    }
    let _ = remove_path(&old);
    Ok(())
}

/// Install the host `files` of `kind` into the hosts directory `dir`: each file that is not already there with its
/// pinned hash is downloaded to `<file>.<hash prefix>.part` (a partial of another release is never resumed),
/// verified, and renamed into place; `<kind>.host`, the marker `host_state` reads, is written last, through a
/// `.tmp`, only once every file is in. A bad hash deletes the `.part`; a network failure, a cancel or a file error
/// keeps it. Files already finished stay, so a retry fetches only what is left. `progress` gets the bytes done
/// across all the files (finished ones included). Blocking: run it on a worker thread.
pub(crate) fn install_host(
    dir: &Path,
    kind: ViewKind,
    files: &[HostFile],
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), InstallError> {
    std::fs::create_dir_all(dir)?;
    let mut done = 0u64;
    for file in files {
        let dest = dir.join(file.file);
        let current =
            std::fs::metadata(&dest).is_ok_and(|m| m.len() == file.size) && verify_sha256(&dest, file.sha256).is_ok();
        if !current {
            let part = dir.join(format!("{}.{}.part", file.file, file.sha256.get(..12).unwrap_or(file.sha256)));
            download(file.url, &part, file.size, &mut |n| progress(done + n), cancel)?;
            if let Err(e) = verify_sha256(&part, file.sha256) {
                if matches!(e, InstallError::BadHash { .. }) {
                    let _ = remove_path(&part);
                }
                return Err(e);
            }
            std::fs::rename(&part, &dest)?;
        }
        done += file.size;
        progress(done);
    }
    let marker = host_marker_path(dir, kind);
    let tmp = marker.with_extension("host.tmp");
    std::fs::write(&tmp, marker_text(files))?;
    Ok(std::fs::rename(&tmp, &marker)?)
}

/// Install everything `plan` names under the data directory `dir`: the host files, then CEF's runtime archive.
/// One job, so `progress` runs from 0 to `plan.size()` across both. Returns `dir`.
pub(crate) fn install_plan(
    kind: ViewKind,
    plan: &Plan,
    dir: &Path,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, InstallError> {
    if !plan.host.is_empty() {
        install_host(dir, kind, plan.host, progress, cancel)?;
    }
    if let Some(pin) = &plan.runtime {
        let before = plan.host_size();
        install(pin, dir, &mut |n| progress(before + n), cancel)?;
    }
    Ok(dir.to_path_buf())
}

/// Hash every host file in `dir` against its pin. Run when a tab is about to spawn the host, so a file changed
/// after the install (the folder is user-writable) is refused instead of run. Not part of the on-disk poll.
pub(crate) fn verify_installed_host(dir: &Path, files: &[HostFile]) -> Result<(), InstallError> {
    files.iter().try_for_each(|file| verify_sha256(&dir.join(file.file), file.sha256))
}

/// The files an install leaves beside the CEF runtime while it works (and after some failures).
fn leftovers() -> [String; 3] {
    ["part", "tmp", "old"].map(|ext| format!("{CEF_RUNTIME_DIR}.{ext}"))
}

/// The file names in the hosts directory that are `kind`'s host: the pinned `files`, those its marker lists (an
/// install of another release) and the host exe's own name.
fn host_file_names(dir: &Path, kind: ViewKind, files: &[HostFile]) -> Vec<String> {
    let mut names: Vec<String> = files.iter().map(|f| f.file.to_string()).collect();
    names.push(crate::cef_platform::PLATFORM.host_exe(kind.desc().binary));
    if let Ok(marker) = std::fs::read_to_string(host_marker_path(dir, kind)) {
        names.extend(marker.lines().filter_map(|line| line.split_once(' ')).map(|(_, name)| name.to_string()));
    }
    // The marker is a file in a user-writable folder: only plain names, never a path out of it.
    names.retain(|n| Path::new(n).file_name().is_some_and(|f| f == n.as_str()));
    names.sort();
    names.dedup();
    names
}

/// `kind`'s partial downloads and temporaries in `dir`: `<file>.<hash prefix>.part`, `<file>.tmp`, the marker's `.tmp`.
fn host_leftovers(dir: &Path, kind: ViewKind, files: &[HostFile]) -> Vec<PathBuf> {
    let names = host_file_names(dir, kind, files);
    let marker_tmp = format!("{}.host.tmp", kind.desc().name);
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name == marker_tmp
                || names.iter().any(|n| {
                    name == format!("{n}.tmp") || (name.starts_with(&format!("{n}.")) && name.ends_with(".part"))
                })
        })
        .map(|entry| entry.path())
        .collect()
}

/// Whether `kind` has any install leftover (`.part`, `.tmp`, `.old`) in `dir`.
pub(crate) fn has_leftovers(dir: &Path, kind: ViewKind, files: &[HostFile]) -> bool {
    !host_leftovers(dir, kind, files).is_empty()
        || (has_runtime(kind) && leftovers().iter().any(|name| dir.join(name).exists()))
}

/// What `kind` has installed in `dir`: its host files and marker that exist, and CEF's runtime directory.
pub(crate) fn installed_paths_in(dir: &Path, kind: ViewKind, files: &[HostFile]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = host_file_names(dir, kind, files).iter().map(|n| dir.join(n)).collect();
    paths.push(host_marker_path(dir, kind));
    if has_runtime(kind) {
        paths.push(dir.join(CEF_RUNTIME_DIR));
    }
    paths.retain(|p| p.exists());
    paths
}

/// `installed_paths_in` for the real hosts directory and this build's pins; empty without a data directory.
pub(crate) fn installed_paths(kind: ViewKind) -> Vec<PathBuf> {
    crate::process_manager::view_hosts_dir()
        .map(|dir| installed_paths_in(&dir, kind, host_components(kind)))
        .unwrap_or_default()
}

/// `has_leftovers` for the real hosts directory and this build's pins.
pub(crate) fn kind_has_leftovers(kind: ViewKind) -> bool {
    crate::process_manager::view_hosts_dir().is_some_and(|dir| has_leftovers(&dir, kind, host_components(kind)))
}

/// Delete what `kind` installed in `dir` (its host files, `.part`/`.tmp` leftovers and marker, and for CEF the
/// runtime directory and its leftovers) and nothing else. The markers go first, so a delete that stops partway
/// never reads as Installed. Refused, with the reason, while an engine process is using it (`running`) or an
/// install is writing next to it (`installing`).
pub(crate) fn remove_view_files(
    dir: &Path,
    kind: ViewKind,
    files: &[HostFile],
    running: bool,
    installing: bool,
) -> Result<(), String> {
    if running {
        return Err("Close the tabs using it first".into());
    }
    if installing {
        return Err("It is installing; cancel that first".into());
    }
    let live = dir.join(CEF_RUNTIME_DIR);
    let remove = |path: &Path| remove_path(path).map_err(|e| e.to_string());
    remove(&host_marker_path(dir, kind))?;
    if has_runtime(kind) {
        remove(&live.join(MARKER))?;
    }
    for name in host_file_names(dir, kind, files) {
        remove(&dir.join(name))?;
    }
    for leftover in host_leftovers(dir, kind, files) {
        remove(&leftover)?;
    }
    if has_runtime(kind) {
        remove(&live)?;
        for name in leftovers() {
            remove(&dir.join(name))?;
        }
    }
    Ok(())
}

/// Total size in bytes of the file, or of the files under the directory, `path` (0 when it is missing).
pub(crate) fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.flatten().map(|e| dir_size(&e.path())).sum()
}

/// The engine runtime installs and their Settings UI (`App::view_ui`).
#[derive(Default)]
pub(crate) struct ViewInstallUi {
    /// Engine runtime installs, at most one per engine (see `InstallJob`).
    pub(crate) view_installs: HashMap<ViewKind, InstallJob>,
    /// The last install's failure message per engine, until the next attempt starts.
    pub(crate) view_failures: HashMap<ViewKind, String>,
    /// `view_status` per engine for the current frame: it stats the disk, and cards, pickers, menus
    /// and Settings all ask. Cleared at the start of each frame and whenever an install changes.
    pub(crate) view_status_cache: std::cell::RefCell<HashMap<ViewKind, ViewStatus>>,
    /// The bring-your-own CEF directory's last validation, with the path it was for (`custom_cef_dir`).
    pub(crate) cef_dir_check: std::cell::RefCell<Option<DirCheck>>,
    /// Settings' text field for that directory while it is being edited; committed on Enter or focus loss.
    pub(crate) cef_dir_draft: Option<String>,
}

/// A running (or just cancelled) install of one engine's components, on its own thread.
pub(crate) struct InstallJob {
    /// Bytes done across the plan's parts so far, written by the thread.
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    /// The plan's total size (host files plus runtime), for `(received, total)`.
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
    /// Cached for the frame (`view_status_cache`); the disk answers under it are reused for a few seconds (`view_on_disk`).
    pub(crate) fn view_status(&self, kind: ViewKind) -> ViewStatus {
        if let Some(state) = self.view_ui.view_status_cache.borrow().get(&kind) {
            return state.clone();
        }
        let state = self.view_status_at(kind, crate::process_manager::view_on_disk(kind), &plan_for(kind));
        self.view_ui.view_status_cache.borrow_mut().insert(kind, state.clone());
        state
    }

    /// `view_status` with the engine's on-disk state (`view_on_disk`) and install plan (`plan_for`) given (tests
    /// use a temporary directory and a plan of their own).
    pub(crate) fn view_status_at(&self, kind: ViewKind, on_disk: OnDisk, plan: &Result<Plan, String>) -> ViewStatus {
        let installing = self
            .view_ui
            .view_installs
            .get(&kind)
            .filter(|job| !job.cancelled)
            .map(|job| (job.progress.load(Ordering::Relaxed), job.total));
        let failed = self.view_ui.view_failures.get(&kind).map(String::as_str);
        let custom = has_runtime(kind).then(|| self.custom_cef_dir()).flatten();
        crate::process_manager::view_availability(kind, plan, on_disk, installing, failed, custom.as_deref())
    }

    /// The bytes installing `kind` would fetch now, for the confirm dialog and the failed card's hint: what is not
    /// on disk yet (a bring-your-own CEF runtime is never fetched).
    pub(crate) fn view_download_size(&self, kind: ViewKind) -> u64 {
        plan_for(kind).map_or(0, |plan| {
            plan.fetch_size(crate::process_manager::view_on_disk(kind), self.custom_cef_dir().is_some())
        })
    }

    /// The configured bring-your-own CEF directory, only when it validates against the pin.
    pub(crate) fn custom_cef_dir(&self) -> Option<PathBuf> {
        self.cef_dir_verdict().and_then(|(dir, verdict)| verdict.ok().map(|()| dir))
    }

    /// The configured directory and whether it validates; `None` when none is set. The verdict is cached
    /// with the path (`cef_dir_check`): the disk is read when the path changes or every `CEF_DIR_CHECK_TTL`,
    /// not every frame, so a folder deleted or replaced behind the app's back is noticed.
    pub(crate) fn cef_dir_verdict(&self) -> Option<(PathBuf, Result<(), String>)> {
        let configured = self.settings.value.cef_directory.as_deref().filter(|s| !s.trim().is_empty())?;
        let mut cache = self.view_ui.cef_dir_check.borrow_mut();
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
        *self.view_ui.cef_dir_check.borrow_mut() = None;
        self.view_ui.view_status_cache.borrow_mut().clear();
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
    }

    /// Set (or clear, for an empty path) the bring-your-own CEF directory: save it, drop the cached
    /// verdict and states so the next read validates it.
    pub(crate) fn set_cef_directory(&mut self, path: &str) {
        let path = crate::settings::typed_folder(path);
        self.settings.value.cef_directory = (!path.is_empty()).then(|| path.to_string());
        self.invalidate_cef_dir_check();
        self.save_settings();
    }

    /// Download and install `kind`'s components into the data directory. See `start_view_install_at`.
    pub(crate) fn start_view_install(&mut self, kind: ViewKind) -> bool {
        let Some(data_dir) = crate::process_manager::view_hosts_dir() else {
            self.queue_toast("No data directory to install into".into());
            return false;
        };
        let mut plan = match plan_for(kind) {
            Ok(plan) => plan,
            Err(reason) => {
                self.queue_toast(crate::view_info::PickOutcome::unavailable_toast(kind, &reason));
                return false;
            }
        };
        if self.custom_cef_dir().is_some() {
            plan.runtime = None;
        }
        self.start_view_install_at(kind, &data_dir, &plan)
    }

    /// Start installing `plan` for `kind` (only a downloaded engine installs) under `data_dir`. Refused, with a
    /// toast saying why, while that engine's install is running or its cancelled thread has not yet stopped (it
    /// would share the `.part`), or while an engine process of that kind is alive (it holds the files, so the
    /// replace would fail). Returns whether a job started.
    pub(crate) fn start_view_install_at(&mut self, kind: ViewKind, data_dir: &Path, plan: &Plan) -> bool {
        if !downloads(kind) {
            return false;
        }
        let label = crate::view_info::view_label(kind);
        if let Some(job) = self.view_ui.view_installs.get(&kind) {
            if !job.cancelled {
                self.queue_toast(format!("{label} is already installing"));
            } else if !job.handle.is_finished() {
                self.queue_toast(format!("The cancelled {label} download is still stopping; try again in a moment"));
            } else {
                // Finished: collect it first (its result is discarded), then start afresh.
                self.poll_view_installs();
                return self.start_view_install_at(kind, data_dir, plan);
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
        self.view_ui.view_failures.remove(&kind);
        self.view_ui.view_status_cache.borrow_mut().clear();
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (thread_progress, thread_cancel) = (progress.clone(), cancel.clone());
        let (data_dir, plan) = (data_dir.to_path_buf(), plan.clone());
        let total = plan.size();
        let handle = std::thread::spawn(move || {
            install_plan(kind, &plan, &data_dir, &mut |n| thread_progress.store(n, Ordering::Relaxed), &thread_cancel)
        });
        self.view_ui.view_installs.insert(kind, InstallJob { progress, cancel, total, cancelled: false, handle });
        self.egui_ctx.request_repaint();
        true
    }

    /// Delete what `kind` downloaded (Settings' Remove, after its confirm), toasting the outcome.
    pub(crate) fn remove_view(&mut self, kind: ViewKind) {
        let label = crate::view_info::view_label(kind);
        let Some(dir) = crate::process_manager::view_hosts_dir() else { return };
        // A spawn still in flight may be this engine's host (it isn't a node yet), so it counts too.
        let running = !self.spawns.spawning.is_empty()
            || self.nodes.values().any(|n| n.as_tab().is_some_and(|t| t.tab.kind == kind));
        let installing = self.view_ui.view_installs.contains_key(&kind);
        let removed = remove_view_files(&dir, kind, host_components(kind), running, installing);
        // Even a failed removal may have deleted some of it: the Settings page reads the disk again.
        crate::process_manager::forget_on_disk();
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
        match removed {
            Ok(()) => {
                // A recorded failure described the files just deleted.
                self.view_ui.view_failures.remove(&kind);
                self.view_ui.view_status_cache.borrow_mut().clear();
                self.queue_toast(format!("{label} removed"));
            }
            Err(why) => self.queue_toast(format!("Couldn't remove {label}: {why}")),
        }
    }

    /// Stop showing `kind`'s install at once and tell its thread to stop: it notices within about 100 ms
    /// while downloading (even on a stalled read), and between unpack entries and before the swap after
    /// that. The `.part` stays for resuming and no failure is recorded.
    pub(crate) fn cancel_view_install(&mut self, kind: ViewKind) {
        if let Some(job) = self.view_ui.view_installs.get_mut(&kind) {
            job.cancel.store(true, Ordering::Relaxed);
            job.cancelled = true;
        }
        self.view_ui.view_status_cache.borrow_mut().clear();
        self.egui_ctx.request_repaint();
    }

    /// Collect finished jobs (once per frame): toast and record a failure, toast a success, drop a
    /// cancelled job's result. Keeps repainting while any job is running.
    pub(crate) fn poll_view_installs(&mut self) {
        if self.view_ui.view_installs.is_empty() {
            return;
        }
        let finished: Vec<ViewKind> = self
            .view_ui
            .view_installs
            .iter()
            .filter(|(_, job)| job.handle.is_finished())
            .map(|(kind, _)| *kind)
            .collect();
        if !finished.is_empty() {
            self.view_ui.view_status_cache.borrow_mut().clear();
            // An install (or a cancelled one's leftovers) changed what the Settings page shows from disk.
            crate::settings_page::forget_disk_cache(&self.egui_ctx);
            crate::process_manager::forget_on_disk();
        }
        for kind in finished {
            let Some(job) = self.view_ui.view_installs.remove(&kind) else { continue };
            let result =
                job.handle.join().unwrap_or_else(|_| Err(InstallError::Io("the install thread panicked".into())));
            let label = crate::view_info::view_label(kind);
            match result {
                _ if job.cancelled => {}
                Err(InstallError::Cancelled) => {}
                Ok(_) => {
                    self.view_ui.view_failures.remove(&kind);
                    self.queue_toast(format!("{label} installed"));
                }
                Err(e) => {
                    self.queue_toast(format!("{label} install failed: {e}"));
                    self.view_ui.view_failures.insert(kind, e.to_string());
                }
            }
            self.egui_ctx.request_repaint();
        }
        if !self.view_ui.view_installs.is_empty() {
            self.egui_ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests;
