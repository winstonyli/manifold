//! Where an engine's host is built to run, and how that maps onto the machine manifold runs on.
//!
//! `Os` is the operating systems an engine host can target, and `Other` for every one without a host of its own. `Runtime` says how a host built for one OS runs
//! on another: natively on its own, or a Linux host through WSL on Windows. `HostLink` is how a host reaches
//! manifold, a property of the host, not of the runtime. A fourth compatibility layer (Wine) would be one more
//! `Runtime` and one more arm of `Runtime::between`.

use super::ViewKind;

/// An operating system a host is built for, or manifold runs on. `Other` is any not named here (FreeBSD and the like):
/// only the engines built for whichever OS manifold runs on (Servo, CEF) can run there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Windows,
    Linux,
    Mac,
    Other,
}

impl Os {
    /// In the order a refusal lists them ("needs Windows or Linux").
    pub(crate) const ALL: [Os; 4] = [Os::Windows, Os::Linux, Os::Mac, Os::Other];

    /// The OS of a `std::env::consts::OS` value.
    pub(crate) fn from_consts(os: &str) -> Os {
        match os {
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            "macos" => Os::Mac,
            _ => Os::Other,
        }
    }

    /// The OS manifold is running on.
    pub(crate) fn here() -> Os {
        Os::from_consts(std::env::consts::OS)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Linux => "Linux",
            Os::Mac => "macOS",
            Os::Other => "another OS",
        }
    }

    /// `host_pin`'s name for the platform a host built for this OS is pinned under.
    pub(crate) fn pin_platform(self) -> &'static str {
        match self {
            Os::Windows => "windows-x64",
            Os::Linux => "linux-x64",
            Os::Mac => "macos-x64",
            // No host is pinned for it, so a lookup finds nothing.
            Os::Other => "other-x64",
        }
    }
}

/// How a host built for one OS runs on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Runtime {
    /// A program on this machine.
    Native,
    /// A Linux program run through `wsl.exe` on Windows: a force-killed relay would orphan its Linux process tree, so
    /// it exits on its own (`reap_host`).
    Wsl,
}

impl Runtime {
    /// How a host built for `host` runs on `here`, or `None` when it cannot.
    pub(crate) fn between(host: Os, here: Os) -> Option<Runtime> {
        match (host, here) {
            (host, here) if host == here => Some(Runtime::Native),
            (Os::Linux, Os::Windows) => Some(Runtime::Wsl),
            _ => None,
        }
    }

    /// How `kind`'s host runs on this machine, or `None` when it cannot.
    pub(crate) fn of(kind: ViewKind) -> Option<Runtime> {
        Runtime::for_os(kind, Os::here())
    }

    fn for_os(kind: ViewKind, here: Os) -> Option<Runtime> {
        match kind.desc().host_os {
            None => Some(Runtime::Native),
            Some(host) => Runtime::between(host, here),
        }
    }
}

/// How a host reaches manifold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostLink {
    /// A named-pipe pair and a file-backed shared-memory frame (`spawn_tab`): same machine, same kernel.
    PipesShm,
    /// One TCP connection carrying messages and frames (`spawn_tcp_host`): no frame file, no cache dir. The only
    /// link that crosses a VM boundary.
    Tcp,
}

/// Why `kind` cannot run on `here`, or `None` when it can: the OSes that can ("needs Windows or Linux"). The one
/// place that knows: availability (every engine list) and the spawn refusal both read it.
pub(crate) fn unavailable_reason(kind: ViewKind, here: Os) -> Option<String> {
    if Runtime::for_os(kind, here).is_some() {
        return None;
    }
    let names: Vec<&str> =
        Os::ALL.into_iter().filter(|os| Runtime::for_os(kind, *os).is_some()).map(Os::name).collect();
    Some(format!("needs {}", names.join(" or ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_runs_natively_on_its_own_os_and_in_wsl_for_linux_on_windows() {
        use Os::{Linux, Mac, Windows};
        assert_eq!(Runtime::between(Windows, Windows), Some(Runtime::Native));
        assert_eq!(Runtime::between(Linux, Linux), Some(Runtime::Native));
        assert_eq!(Runtime::between(Linux, Windows), Some(Runtime::Wsl));
        assert_eq!(Runtime::between(Linux, Mac), None);
        assert_eq!(Runtime::between(Windows, Linux), None);
        assert_eq!(Runtime::between(Windows, Mac), None);
        assert_eq!(Runtime::between(Mac, Windows), None);
    }

    #[test]
    fn an_os_name_from_the_standard_library_reads_as_one_of_four() {
        assert_eq!(Os::from_consts("windows"), Os::Windows);
        assert_eq!(Os::from_consts("linux"), Os::Linux);
        assert_eq!(Os::from_consts("macos"), Os::Mac);
        for other in ["freebsd", "openbsd"] {
            assert_eq!(Os::from_consts(other), Os::Other, "{other}");
        }
        assert_eq!(Os::Windows.pin_platform(), "windows-x64");
        assert_eq!(Os::Linux.pin_platform(), "linux-x64");
    }

    #[test]
    fn the_reason_an_engine_is_unavailable_names_the_oses_that_can_run_it() {
        for os in [Os::Linux, Os::Mac, Os::Other] {
            assert_eq!(unavailable_reason(ViewKind::Webview2, os).as_deref(), Some("needs Windows"), "{os:?}");
        }
        for os in [Os::Mac, Os::Other] {
            assert_eq!(unavailable_reason(ViewKind::Webkit, os).as_deref(), Some("needs Windows or Linux"), "{os:?}");
        }
        assert_eq!(unavailable_reason(ViewKind::Webkit, Os::Windows), None);
        assert_eq!(unavailable_reason(ViewKind::Webkit, Os::Linux), None);
        assert_eq!(unavailable_reason(ViewKind::Webview2, Os::Windows), None);
        for os in Os::ALL {
            // Built for whichever OS manifold runs on: nowhere to refuse.
            assert_eq!(unavailable_reason(ViewKind::Servo, os), None, "{os:?}");
            assert_eq!(unavailable_reason(ViewKind::Cef, os), None, "{os:?}");
        }
    }

    /// Nothing crosses a VM boundary by named pipe or shared memory, so a host that runs in WSL must speak TCP.
    #[test]
    fn a_wsl_host_speaks_tcp() {
        for kind in ViewKind::ALL {
            for here in Os::ALL {
                let Some(host) = kind.desc().host_os else { continue };
                if Runtime::between(host, here) == Some(Runtime::Wsl) {
                    assert_eq!(kind.desc().link, HostLink::Tcp, "{kind:?} on {here:?}");
                }
            }
        }
    }
}
