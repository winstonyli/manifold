//! On-disk state of an install: what the marker says is there, CEF runtime compatibility, bring-your-own CEF directories.

use crate::process_manager::ViewKind;
use crate::view_install::host_pin::Component as HostFile;
use manifold_hosts::{Os, CEF};
use std::path::{Path, PathBuf};

use super::*;

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
    /// The host exe and the files that travel with it: `host_state`, or Ready when a host is in place.
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

/// The Visual C++ runtime DLLs Servo's Windows host imports (its mozjs and ANGLE are built against the dynamic CRT, so
/// it cannot carry them; the Microsoft Visual C++ Redistributable installs them in System32).
const SERVO_VC_RUNTIME: &[&str] = &["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"];

/// The files of `names` that `dir` lacks.
pub(super) fn missing_files(dir: &Path, names: &[&'static str]) -> Vec<&'static str> {
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
pub(super) fn host_marker_path(dir: &Path, kind: ViewKind) -> PathBuf {
    dir.join(format!("{}.host", kind.desc().host.id))
}

pub(super) fn marker_text(files: &[HostFile]) -> String {
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

/// A bring-your-own CEF directory's last validation: the configured path, when it was checked, the verdict.
pub(crate) type DirCheck = (String, std::time::Instant, Result<(), String>);

/// How long a bring-your-own CEF directory's verdict is trusted before the disk is read again.
pub(super) const CEF_DIR_CHECK_TTL: std::time::Duration = std::time::Duration::from_secs(5);

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

/// Whether `dir` is a CEF runtime this host runs on: it holds the runtime library of the OS CEF's host is built for
/// (`Host::runtime_lib`; nothing else is required, e.g. CEF 154's archive has no `libEGL.dll`)
/// and its version (`custom_version`: `archive.json` or our `MARKER`) is the pin's CEF major or a later one
/// (`runtime_compat`, while that major keeps our API version in its supported range); a version with no
/// readable major is not usable. An `Err` says what is wrong, naming both versions on a mismatch.
pub(crate) fn validate_custom(dir: &Path, pin: &Pin) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let lib = CEF.runtime_lib(Os::here()).expect("CEF has a runtime");
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
