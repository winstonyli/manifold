//! Where an engine's host is built to run, and how that maps onto the machine manifold runs on.
//!
//! `Os` (the host table's, `manifold_hosts::Os`) is the operating systems an engine host can target, and `Other` for
//! every one without a host of its own; a host's row says which it is built for (`Host::os`). `HostRuntime` says how
//! a host built for one OS runs on another: natively on its own, or a Linux host through WSL on Windows. `HostLink` is
//! how a host reaches manifold, a property of the host, not of the runtime. A fourth compatibility layer (Wine) would
//! be one more `HostRuntime` and one more arm of `HostRuntime::between`.

use super::ViewKind;
use manifold_hosts::Os;
use std::path::Path;

/// The WSL distro the WebKit host runs in (`webkit.rs`), and whose share Explorer opens for its other paths.
pub(crate) const WSL_DISTRO: &str = "Ubuntu";

/// How a host built for one OS runs on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostRuntime {
    /// A program on this machine.
    Native,
    /// A Linux program run through `wsl.exe` on Windows: a force-killed relay would orphan its Linux process tree, so
    /// it exits on its own (`reap_host`).
    Wsl,
}

impl HostRuntime {
    /// How a host built for `host` runs on `here`, or `None` when it cannot.
    pub(crate) fn between(host: Os, here: Os) -> Option<HostRuntime> {
        match (host, here) {
            (host, here) if host == here => Some(HostRuntime::Native),
            (Os::Linux, Os::Windows) => Some(HostRuntime::Wsl),
            _ => None,
        }
    }

    /// How `kind`'s host runs on this machine, or `None` when it cannot.
    pub(crate) fn of(kind: ViewKind) -> Option<HostRuntime> {
        HostRuntime::for_os(kind, Os::here())
    }

    fn for_os(kind: ViewKind, here: Os) -> Option<HostRuntime> {
        match kind.desc().host.os {
            None => Some(HostRuntime::Native),
            Some(host) => HostRuntime::between(host, here),
        }
    }

    /// `path` (a Windows path in the app) as the host sees it: unchanged natively; through WSL a drive path as
    /// `/mnt/<drive>/...` (drvfs), a `\\?\` prefix dropped. `None` when the host cannot see it: through WSL a path not
    /// on a drive (a UNC share, a drive-relative `C:x`), and anywhere one that is not UTF-8.
    pub(crate) fn host_path(self, path: &Path) -> Option<String> {
        let text = path.to_str()?;
        match self {
            HostRuntime::Native => Some(text.to_string()),
            HostRuntime::Wsl => {
                let text = text.strip_prefix(r"\\?\").unwrap_or(text);
                let mut chars = text.chars();
                let drive = chars.next().filter(char::is_ascii_alphabetic)?;
                let rest = chars.as_str().strip_prefix(':').filter(|rest| rest.starts_with(['\\', '/']))?;
                Some(format!("/mnt/{}{}", drive.to_ascii_lowercase(), rest.replace('\\', "/")))
            }
        }
    }

    /// A path the host reported, in the app's terms (`host_path` the other way): unchanged natively; through WSL
    /// `/mnt/<drive>/...` as `X:\...`, and any other absolute path as the `\\wsl.localhost\<distro>\...` share
    /// Explorer opens.
    pub(crate) fn app_path(self, path: String) -> String {
        match self {
            HostRuntime::Native => path,
            HostRuntime::Wsl => {
                if let Some(rest) = path.strip_prefix("/mnt/") {
                    let mut chars = rest.chars();
                    if let Some(drive) = chars.next().filter(char::is_ascii_alphabetic) {
                        let tail = chars.as_str();
                        if tail.is_empty() || tail.starts_with('/') {
                            let tail = if tail.is_empty() { "/" } else { tail };
                            return format!("{}:{}", drive.to_ascii_uppercase(), tail.replace('/', "\\"));
                        }
                    }
                }
                match path.strip_prefix('/') {
                    Some(posix) => format!("\\\\wsl.localhost\\{WSL_DISTRO}\\{}", posix.replace('/', "\\")),
                    None => path,
                }
            }
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
    if HostRuntime::for_os(kind, here).is_some() {
        return None;
    }
    let names: Vec<&str> =
        Os::ALL.into_iter().filter(|os| HostRuntime::for_os(kind, *os).is_some()).map(Os::name).collect();
    Some(format!("needs {}", names.join(" or ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review Focus 2 (WSL): a folder with a space and a non-ASCII letter reaches the host as its drvfs path, and the
    /// host's paths come back in the form Explorer opens.
    #[test]
    fn a_wsl_host_sees_a_windows_folder_as_mnt_and_its_paths_come_back_as_windows_ones() {
        let wsl = HostRuntime::Wsl;
        let host = |p: &str| wsl.host_path(Path::new(p));
        assert_eq!(host(r"C:\Users\Zoë Q\Downloads").as_deref(), Some("/mnt/c/Users/Zoë Q/Downloads"));
        assert_eq!(host(r"\\?\D:\dl").as_deref(), Some("/mnt/d/dl"));
        assert_eq!(host(r"D:\").as_deref(), Some("/mnt/d/"));
        assert_eq!(host(r"\\nas\dl"), None, "a share WSL can't see");
        assert_eq!(host("C:dl"), None, "drive-relative");
        let app = |p: &str| wsl.app_path(p.to_string());
        assert_eq!(app("/mnt/c/Users/Zoë Q/Downloads/a.zip"), r"C:\Users\Zoë Q\Downloads\a.zip");
        assert_eq!(app("/mnt/d/"), r"D:\");
        assert_eq!(app("/mnt/d"), r"D:\");
        assert_eq!(app("/home/u/a.zip"), r"\\wsl.localhost\Ubuntu\home\u\a.zip");
        assert_eq!(app("/mnt/data/a.zip"), r"\\wsl.localhost\Ubuntu\mnt\data\a.zip", "not a drive letter");
        assert_eq!(app("b.zip"), "b.zip");
        assert_eq!(HostRuntime::Native.host_path(Path::new(r"C:\a b")).as_deref(), Some(r"C:\a b"));
        assert_eq!(HostRuntime::Native.app_path("/home/u/a".into()), "/home/u/a");
    }

    #[test]
    fn a_host_runs_natively_on_its_own_os_and_in_wsl_for_linux_on_windows() {
        use Os::{Linux, Mac, Windows};
        assert_eq!(HostRuntime::between(Windows, Windows), Some(HostRuntime::Native));
        assert_eq!(HostRuntime::between(Linux, Linux), Some(HostRuntime::Native));
        assert_eq!(HostRuntime::between(Linux, Windows), Some(HostRuntime::Wsl));
        assert_eq!(HostRuntime::between(Linux, Mac), None);
        assert_eq!(HostRuntime::between(Windows, Linux), None);
        assert_eq!(HostRuntime::between(Windows, Mac), None);
        assert_eq!(HostRuntime::between(Mac, Windows), None);
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
                let Some(host) = kind.desc().host.os else { continue };
                if HostRuntime::between(host, here) == Some(HostRuntime::Wsl) {
                    assert_eq!(kind.desc().link, HostLink::Tcp, "{kind:?} on {here:?}");
                }
            }
        }
    }
}
