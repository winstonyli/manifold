//! On-demand installs of what an engine needs besides `manifold.exe`: an engine is a list of components, a pinned
//! host (one or more single files, `host_pin`) and, for CEF, the runtime archive (`Pin`). Also the states the UI shows.

use crate::process_manager::ViewKind;
use crate::view_install::host_pin::Component as HostFile;
use manifold_hosts::{Delivery, Os};
use std::fmt;
use std::path::{Path, PathBuf};

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
/// a release writes it): there is nothing to fetch and no host in place.
pub(crate) fn no_download_reason(kind: ViewKind) -> String {
    format!("this build has no {} download", kind.desc().label)
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

/// Whether `kind` is installed by downloading (its host row's `Delivery::Download`), rather than run in place (from its
/// dev folder or beside `manifold.exe`) or carried inside it.
pub(crate) fn downloads(kind: ViewKind) -> bool {
    kind.desc().host.delivery == Delivery::Download
}

/// Whether `kind` also needs a runtime archive besides its host exe (`Host::runtime`), which also takes a
/// bring-your-own runtime directory: only CEF's, the one component that is an archive (`Pin`, `unpack`) rather than
/// single pinned files.
pub(crate) fn has_runtime(kind: ViewKind) -> bool {
    kind.desc().host.runtime.is_some()
}

/// `host_pin`'s name for the platform `kind`'s host is built for (`Host::pin_platform`): this build's own OS, except an
/// engine built for another (WebKit's is a Linux ELF even when manifold runs on Windows, through WSL).
pub(crate) fn host_platform(kind: ViewKind) -> &'static str {
    kind.desc().host.pin_platform(Os::here())
}

/// The host files this build pins for `kind` on this platform; empty for an engine that is not downloaded.
pub(crate) fn host_components(kind: ViewKind) -> &'static [HostFile] {
    if downloads(kind) {
        host_pin::components_for(kind, host_platform(kind))
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

#[cfg(test)]
thread_local! {
    /// How many files this thread has hashed (`sha256_file`), for the test that the on-disk poll hashes none.
    static HASHED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Called by `unpack` at each entry after the cancel check, so a test can flip `cancel` at an exact entry.
    static ON_ENTRY: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
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

/// Delete the file or directory at `path`; one that is already gone is fine.
fn remove_path(path: &Path) -> std::io::Result<()> {
    let removed = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

pub(crate) mod download;
pub(crate) mod embedded_host;
pub(crate) mod host_pin;
pub(crate) mod remove;
pub(crate) mod state;
use state::{OnDisk, RuntimeOnDisk};
pub(crate) mod ui;
mod unpack;

#[cfg(test)]
mod tests;
