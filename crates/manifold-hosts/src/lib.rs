//! The host table: one row per engine host, saying what its exe is called, where a dev tree builds it, how a user
//! gets it and what must sit beside it, with the file names and paths derived from that. It is the one copy of a
//! host's layout for the app, its build script and the gate, so they cannot drift apart.
//!
//! `std` only, no dependencies (`tests/layout.rs` checks it) and no `cfg`: every answer that depends on the OS takes
//! the OS manifold runs on (`running`) as an argument, so one test binary checks every OS's answers. A build script
//! uses only `id`, `delivery` and `embed_var()`, never a method taking an OS: there `Os::here()` would be the build
//! machine, not the target. Rows are compared by `id` (`Host` holds a `fn`, so it has no `PartialEq`).

use std::path::{Path, PathBuf};

/// The folder under the profile's view data directory that holds downloaded and extracted hosts.
pub const HOSTS_DIR: &str = "engine-hosts";
/// CEF's runtime folder: beside its host in a dev tree, under `HOSTS_DIR` when downloaded.
pub const CEF_RUNTIME_DIR: &str = "cef-runtime";

/// An operating system a host is built for, or manifold runs on. `Other` is any not named here (FreeBSD and the like):
/// only the engines built for whichever OS manifold runs on (Servo, CEF) can run there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Windows,
    Linux,
    Mac,
    Other,
}

impl Os {
    /// In the order a refusal lists them ("needs Windows or Linux").
    pub const ALL: [Os; 4] = [Os::Windows, Os::Linux, Os::Mac, Os::Other];

    /// The OS of a `std::env::consts::OS` value.
    pub fn from_consts(os: &str) -> Os {
        match os {
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            "macos" => Os::Mac,
            _ => Os::Other,
        }
    }

    /// The OS manifold is running on.
    pub fn here() -> Os {
        Os::from_consts(std::env::consts::OS)
    }

    /// Its name in a refusal ("needs Windows or Linux").
    pub fn name(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Linux => "Linux",
            Os::Mac => "macOS",
            Os::Other => "another OS",
        }
    }

    /// The manifest's name for the platform a host built for this OS is pinned under.
    pub fn pin_platform(self) -> &'static str {
        match self {
            Os::Windows => "windows-x64",
            Os::Linux => "linux-x64",
            Os::Mac => "macos-x64",
            // No host is pinned for it, so a lookup finds nothing.
            Os::Other => "other-x64",
        }
    }

    /// The suffix of an exe built for this OS.
    pub fn exe_suffix(self) -> &'static str {
        match self {
            Os::Windows => ".exe",
            Os::Linux | Os::Mac | Os::Other => "",
        }
    }
}

/// How a user gets a host.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery {
    /// Pinned in the release's manifest and downloaded into `HOSTS_DIR` on demand.
    Download,
    /// Carried inside manifold.exe and extracted to `HOSTS_DIR/<id>/<file>` at first use.
    Embedded,
}

/// Where cargo leaves a file or folder a host needs besides its exe: `<build>/<krate>-*/out/<tail>` or
/// `<build>/<krate>/*/out/<tail>` (older cargo nightlies use the first layout, newer ones the second).
#[derive(Debug)]
pub struct BuildOutput {
    pub krate: &'static str,
    pub tail: &'static str,
}

/// A file that sits beside the host exe when it runs on `os`.
#[derive(Debug)]
pub struct Sidecar {
    pub file: &'static str,
    pub os: Os,
    pub from: BuildOutput,
}

/// A runtime folder beside the host, holding the runtime and a copy of the host exe.
#[derive(Debug)]
pub struct Runtime {
    pub dir: &'static str,
    /// The file whose presence marks a runtime folder, per OS the host is built for.
    pub lib: fn(Os) -> &'static str,
    /// Where a dev build's runtime comes from (Windows: cef-dll-sys's `cef_windows_x86_64`).
    pub from: BuildOutput,
}

/// One engine host's row.
#[derive(Debug)]
pub struct Host {
    /// The engine's store name and the manifest key.
    pub id: &'static str,
    /// Cargo package and bin name, without a suffix; also the `crates/<exe>/` folder.
    pub exe: &'static str,
    /// The OS it is built for; None: whichever OS manifold runs on.
    pub os: Option<Os>,
    /// A dev tree's build folder beside `target/`, when not `target/` itself (`<dev_dir>/debug/<file>`).
    pub dev_dir: Option<&'static str>,
    pub delivery: Delivery,
    pub sidecars: &'static [Sidecar],
    pub runtime: Option<Runtime>,
}

/// The file that marks CEF's runtime folder, per OS the host is built for: `libcef.dll` on Windows, `libcef.so`
/// elsewhere (a macOS CEF would need an app-bundle layout, not written yet).
fn cef_runtime_lib(os: Os) -> &'static str {
    match os {
        Os::Windows => "libcef.dll",
        Os::Linux | Os::Mac | Os::Other => "libcef.so",
    }
}

/// Servo: built for whichever OS manifold runs on, in its own build folder; on Windows ANGLE's two DLLs, from
/// mozangle's build output, sit beside it.
pub static SERVO: Host = Host {
    id: "servo",
    exe: "manifold-view-servo",
    os: None,
    dev_dir: Some("target-servo"),
    delivery: Delivery::Download,
    sidecars: &[
        Sidecar { file: "libEGL.dll", os: Os::Windows, from: BuildOutput { krate: "mozangle", tail: "libEGL.dll" } },
        Sidecar {
            file: "libGLESv2.dll",
            os: Os::Windows,
            from: BuildOutput { krate: "mozangle", tail: "libGLESv2.dll" },
        },
    ],
    runtime: None,
};

/// CEF: built for whichever OS manifold runs on, in `target/`. Its runtime folder holds the runtime and the copy of
/// the host a tab runs. CEF ships its own `libEGL.dll`/`libGLESv2.dll` (ANGLE), and Windows resolves an exe's implicit
/// DLL dependencies from the exe's own folder first, so a CEF host in one folder with another ANGLE user would load
/// whichever copy was put there last.
pub static CEF: Host = Host {
    id: "cef",
    exe: "manifold-view-cef",
    os: None,
    dev_dir: None,
    delivery: Delivery::Download,
    sidecars: &[],
    runtime: Some(Runtime {
        dir: CEF_RUNTIME_DIR,
        lib: cef_runtime_lib,
        from: BuildOutput { krate: "cef-dll-sys", tail: "cef_windows_x86_64" },
    }),
};

/// WebKitGTK: built for Linux only (run through WSL on Windows), in its own build folder.
pub static WEBKIT: Host = Host {
    id: "webkit",
    exe: "manifold-view-webkitgtk",
    os: Some(Os::Linux),
    dev_dir: Some("target-linux"),
    delivery: Delivery::Download,
    sidecars: &[],
    runtime: None,
};

/// WebView2: Windows only, carried inside manifold.exe. A lone exe: its loader is statically linked by the
/// `webview2-com` crate.
pub static WEBVIEW2: Host = Host {
    id: "webview2",
    exe: "manifold-view-webview2",
    os: Some(Os::Windows),
    dev_dir: None,
    delivery: Delivery::Embedded,
    sidecars: &[],
    runtime: None,
};

/// The PDF renderer: no engine of its own (no `ViewKind` reaches it), carried inside manifold.exe.
pub static PDFIUM: Host = Host {
    id: "pdfium",
    exe: "manifold-view-pdfium",
    os: None,
    dev_dir: None,
    delivery: Delivery::Embedded,
    sidecars: &[],
    runtime: None,
};

/// Every row: `ViewKind::ALL`'s order, then PDFium.
pub static ALL: [&Host; 5] = [&SERVO, &CEF, &WEBKIT, &WEBVIEW2, &PDFIUM];

/// The row whose `id` is `id`, ignoring ASCII case as `ViewKind::from_name` does.
pub fn by_id(id: &str) -> Option<&'static Host> {
    ALL.iter().copied().find(|host| host.id.eq_ignore_ascii_case(id))
}

impl Host {
    /// The OS this host is built for when manifold runs on `running`.
    pub fn built_for(&self, running: Os) -> Os {
        self.os.unwrap_or(running)
    }

    /// The one file-name derivation: `exe` plus the suffix of the OS it is built for.
    pub fn file_name(&self, running: Os) -> String {
        format!("{}{}", self.exe, self.built_for(running).exe_suffix())
    }

    /// The manifest platform it is pinned under: that of the OS it is built for.
    pub fn pin_platform(&self, running: Os) -> &'static str {
        self.built_for(running).pin_platform()
    }

    /// Beside manifold.exe in `exe_dir`: flat, or inside the runtime folder for a host with one.
    pub fn beside(&self, running: Os, exe_dir: &Path) -> PathBuf {
        let file = self.file_name(running);
        match &self.runtime {
            Some(runtime) => exe_dir.join(runtime.dir).join(file),
            None => exe_dir.join(file),
        }
    }

    /// Where it runs in place from, in order: with `dev_folders`, `<dev_dir>/debug/<file>` in `exe_dir` and its four
    /// parents, nearest first; then `beside(running, exe_dir)`. The dev folder comes first: it is where a dev tree
    /// builds the host, and a stray copy left beside the exe must not shadow a fresh build.
    pub fn in_place(&self, running: Os, exe_dir: &Path, dev_folders: bool) -> Vec<PathBuf> {
        let file = self.file_name(running);
        let mut paths: Vec<PathBuf> = match self.dev_dir.filter(|_| dev_folders) {
            Some(dev) => exe_dir.ancestors().take(5).map(|dir| dir.join(dev).join("debug").join(&file)).collect(),
            None => Vec::new(),
        };
        paths.push(self.beside(running, exe_dir));
        paths
    }

    /// Where it is installed under `hosts_dir`: flat for `Download`, `<id>/<file>` for `Embedded`.
    pub fn installed_in(&self, running: Os, hosts_dir: &Path) -> PathBuf {
        let file = self.file_name(running);
        match self.delivery {
            Delivery::Download => hosts_dir.join(file),
            Delivery::Embedded => hosts_dir.join(self.id).join(file),
        }
    }

    /// `hosts_dir/<runtime.dir>` for a host with a runtime.
    pub fn runtime_in(&self, hosts_dir: &Path) -> Option<PathBuf> {
        self.runtime.as_ref().map(|runtime| hosts_dir.join(runtime.dir))
    }

    /// The runtime's marker file for the OS it is built for, for a host with a runtime.
    pub fn runtime_lib(&self, running: Os) -> Option<&'static str> {
        self.runtime.as_ref().map(|runtime| (runtime.lib)(self.built_for(running)))
    }

    /// The sidecars that sit beside it: those whose `os` is the one it is built for. The one filter, used by
    /// `pinned_files` and by the gate's `sync_host`.
    pub fn sidecars_for(&self, running: Os) -> impl Iterator<Item = &Sidecar> {
        let os = self.built_for(running);
        self.sidecars.iter().filter(move |sidecar| sidecar.os == os)
    }

    /// The files a manifest pins for it: its exe, then the file names of `sidecars_for(running)`.
    pub fn pinned_files(&self, running: Os) -> Vec<String> {
        let mut files = vec![self.file_name(running)];
        files.extend(self.sidecars_for(running).map(|sidecar| sidecar.file.to_owned()));
        files
    }

    /// `pinned_files`, plus `<runtime.dir>/<runtime_lib>` for a host with a runtime: what an install leaves on disk.
    pub fn installed_files(&self, running: Os) -> Vec<String> {
        let mut files = self.pinned_files(running);
        if let Some(runtime) = &self.runtime {
            files.push(format!("{}/{}", runtime.dir, (runtime.lib)(self.built_for(running))));
        }
        files
    }

    /// The files a manifest must list for this host under the platform string `platform`, or None when it may not
    /// list it there: the host is not `Download`, no `os` in `Os::ALL` has `pin_platform()` equal to `platform`, that
    /// `os` is `Os::Other` (no host is pinned for it), or that `os` is not the one this host is built for
    /// (`self.pin_platform(os) != platform`). The predicate the app's manifest parser uses; the gate's check of a
    /// release's manifest is stricter.
    pub fn pinned_under(&self, platform: &str) -> Option<Vec<String>> {
        if self.delivery != Delivery::Download {
            return None;
        }
        let os = Os::ALL.into_iter().find(|os| os.pin_platform() == platform)?;
        if os == Os::Other || self.pin_platform(os) != platform {
            return None;
        }
        Some(self.pinned_files(os))
    }

    /// The build variable that names this host's file for embedding: `MANIFOLD_<ID>_HOST`.
    pub fn embed_var(&self) -> String {
        format!("MANIFOLD_{}_HOST", self.id.to_ascii_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_os_name_from_the_standard_library_reads_as_one_of_four() {
        assert_eq!(Os::from_consts("windows"), Os::Windows);
        assert_eq!(Os::from_consts("linux"), Os::Linux);
        assert_eq!(Os::from_consts("macos"), Os::Mac);
        for other in ["freebsd", "openbsd", ""] {
            assert_eq!(Os::from_consts(other), Os::Other, "{other:?}");
        }
        let facts: Vec<_> = Os::ALL.into_iter().map(|os| (os, os.name(), os.pin_platform(), os.exe_suffix())).collect();
        assert_eq!(
            facts,
            [
                (Os::Windows, "Windows", "windows-x64", ".exe"),
                (Os::Linux, "Linux", "linux-x64", ""),
                (Os::Mac, "macOS", "macos-x64", ""),
                (Os::Other, "another OS", "other-x64", ""),
            ]
        );
    }

    #[test]
    fn the_rows_in_their_order() {
        let rows: Vec<_> = ALL.iter().map(|host| (host.id, host.exe, host.os, host.dev_dir, host.delivery)).collect();
        assert_eq!(
            rows,
            [
                ("servo", "manifold-view-servo", None, Some("target-servo"), Delivery::Download),
                ("cef", "manifold-view-cef", None, None, Delivery::Download),
                ("webkit", "manifold-view-webkitgtk", Some(Os::Linux), Some("target-linux"), Delivery::Download),
                ("webview2", "manifold-view-webview2", Some(Os::Windows), None, Delivery::Embedded),
                ("pdfium", "manifold-view-pdfium", None, None, Delivery::Embedded),
            ]
        );
        assert_eq!((HOSTS_DIR, CEF_RUNTIME_DIR), ("engine-hosts", "cef-runtime"));
    }

    /// Where a dev build leaves each part a host needs besides its exe, and that exactly CEF has a runtime.
    #[test]
    fn where_a_dev_build_leaves_each_part() {
        let sidecars: Vec<_> = SERVO
            .sidecars
            .iter()
            .map(|sidecar| (sidecar.file, sidecar.os, sidecar.from.krate, sidecar.from.tail))
            .collect();
        assert_eq!(
            sidecars,
            [
                ("libEGL.dll", Os::Windows, "mozangle", "libEGL.dll"),
                ("libGLESv2.dll", Os::Windows, "mozangle", "libGLESv2.dll"),
            ]
        );
        for host in ALL {
            assert_eq!(host.sidecars.is_empty(), host.id != "servo", "only Servo has sidecars: {}", host.id);
            assert_eq!(host.runtime.is_some(), host.id == "cef", "only CEF has a runtime: {}", host.id);
        }
        let runtime = CEF.runtime.as_ref().expect("CEF has a runtime");
        assert_eq!(
            (runtime.dir, runtime.from.krate, runtime.from.tail),
            ("cef-runtime", "cef-dll-sys", "cef_windows_x86_64")
        );
        assert_eq!(Os::ALL.map(runtime.lib), ["libcef.dll", "libcef.so", "libcef.so", "libcef.so"]);
    }

    /// One row's derived values as manifold sees them running on `running`, written out by hand.
    struct Golden {
        host: &'static Host,
        running: Os,
        built_for: Os,
        file_name: &'static str,
        pin_platform: &'static str,
        /// The file names of `sidecars_for`.
        sidecars: &'static [&'static str],
        pinned_files: &'static [&'static str],
        installed_files: &'static [&'static str],
        runtime_lib: Option<&'static str>,
        /// Under the hosts folder.
        runtime_in: Option<&'static str>,
        /// Under the hosts folder, one component per entry.
        installed_in: &'static [&'static str],
        /// Under `manifold.exe`'s folder, one component per entry.
        beside: &'static [&'static str],
        /// The dev folder `in_place` walks with `dev_folders`, if any.
        walks: Option<&'static str>,
        embed_var: &'static str,
    }

    static GOLDEN: [Golden; 10] = [
        Golden {
            host: &SERVO,
            running: Os::Windows,
            built_for: Os::Windows,
            file_name: "manifold-view-servo.exe",
            pin_platform: "windows-x64",
            sidecars: &["libEGL.dll", "libGLESv2.dll"],
            pinned_files: &["manifold-view-servo.exe", "libEGL.dll", "libGLESv2.dll"],
            installed_files: &["manifold-view-servo.exe", "libEGL.dll", "libGLESv2.dll"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["manifold-view-servo.exe"],
            beside: &["manifold-view-servo.exe"],
            walks: Some("target-servo"),
            embed_var: "MANIFOLD_SERVO_HOST",
        },
        Golden {
            host: &SERVO,
            running: Os::Linux,
            built_for: Os::Linux,
            file_name: "manifold-view-servo",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-servo"],
            installed_files: &["manifold-view-servo"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["manifold-view-servo"],
            beside: &["manifold-view-servo"],
            walks: Some("target-servo"),
            embed_var: "MANIFOLD_SERVO_HOST",
        },
        Golden {
            host: &CEF,
            running: Os::Windows,
            built_for: Os::Windows,
            file_name: "manifold-view-cef.exe",
            pin_platform: "windows-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-cef.exe"],
            installed_files: &["manifold-view-cef.exe", "cef-runtime/libcef.dll"],
            runtime_lib: Some("libcef.dll"),
            runtime_in: Some("cef-runtime"),
            installed_in: &["manifold-view-cef.exe"],
            beside: &["cef-runtime", "manifold-view-cef.exe"],
            walks: None,
            embed_var: "MANIFOLD_CEF_HOST",
        },
        Golden {
            host: &CEF,
            running: Os::Linux,
            built_for: Os::Linux,
            file_name: "manifold-view-cef",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-cef"],
            installed_files: &["manifold-view-cef", "cef-runtime/libcef.so"],
            runtime_lib: Some("libcef.so"),
            runtime_in: Some("cef-runtime"),
            installed_in: &["manifold-view-cef"],
            beside: &["cef-runtime", "manifold-view-cef"],
            walks: None,
            embed_var: "MANIFOLD_CEF_HOST",
        },
        Golden {
            host: &WEBKIT,
            running: Os::Windows,
            built_for: Os::Linux,
            file_name: "manifold-view-webkitgtk",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-webkitgtk"],
            installed_files: &["manifold-view-webkitgtk"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["manifold-view-webkitgtk"],
            beside: &["manifold-view-webkitgtk"],
            walks: Some("target-linux"),
            embed_var: "MANIFOLD_WEBKIT_HOST",
        },
        Golden {
            host: &WEBKIT,
            running: Os::Linux,
            built_for: Os::Linux,
            file_name: "manifold-view-webkitgtk",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-webkitgtk"],
            installed_files: &["manifold-view-webkitgtk"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["manifold-view-webkitgtk"],
            beside: &["manifold-view-webkitgtk"],
            walks: Some("target-linux"),
            embed_var: "MANIFOLD_WEBKIT_HOST",
        },
        Golden {
            host: &WEBVIEW2,
            running: Os::Windows,
            built_for: Os::Windows,
            file_name: "manifold-view-webview2.exe",
            pin_platform: "windows-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-webview2.exe"],
            installed_files: &["manifold-view-webview2.exe"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["webview2", "manifold-view-webview2.exe"],
            beside: &["manifold-view-webview2.exe"],
            walks: None,
            embed_var: "MANIFOLD_WEBVIEW2_HOST",
        },
        Golden {
            host: &WEBVIEW2,
            running: Os::Linux,
            built_for: Os::Windows,
            file_name: "manifold-view-webview2.exe",
            pin_platform: "windows-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-webview2.exe"],
            installed_files: &["manifold-view-webview2.exe"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["webview2", "manifold-view-webview2.exe"],
            beside: &["manifold-view-webview2.exe"],
            walks: None,
            embed_var: "MANIFOLD_WEBVIEW2_HOST",
        },
        Golden {
            host: &PDFIUM,
            running: Os::Windows,
            built_for: Os::Windows,
            file_name: "manifold-view-pdfium.exe",
            pin_platform: "windows-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-pdfium.exe"],
            installed_files: &["manifold-view-pdfium.exe"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["pdfium", "manifold-view-pdfium.exe"],
            beside: &["manifold-view-pdfium.exe"],
            walks: None,
            embed_var: "MANIFOLD_PDFIUM_HOST",
        },
        Golden {
            host: &PDFIUM,
            running: Os::Linux,
            built_for: Os::Linux,
            file_name: "manifold-view-pdfium",
            pin_platform: "linux-x64",
            sidecars: &[],
            pinned_files: &["manifold-view-pdfium"],
            installed_files: &["manifold-view-pdfium"],
            runtime_lib: None,
            runtime_in: None,
            installed_in: &["pdfium", "manifold-view-pdfium"],
            beside: &["manifold-view-pdfium"],
            walks: None,
            embed_var: "MANIFOLD_PDFIUM_HOST",
        },
    ];

    /// `base` joined with each of `parts` in turn.
    fn under(base: &Path, parts: &[&str]) -> PathBuf {
        parts.iter().fold(base.to_path_buf(), |path, part| path.join(part))
    }

    /// `manifold.exe`'s folder in the golden cases (`d4/d3/d2/d1/exe`), then its four parents, nearest first.
    fn exe_dir_and_parents() -> Vec<PathBuf> {
        let parts = ["d4", "d3", "d2", "d1", "exe"];
        (1..=parts.len()).rev().map(|n| parts[..n].iter().collect::<PathBuf>()).collect()
    }

    /// Every row's derived values, for Windows and for Linux, against the literals above. Expected paths are built
    /// from literal components with `Path::join`, so they compare equal on either OS. An edit to a row changes a
    /// literal here by hand, so the app and the gate cannot agree on a typo silently.
    #[test]
    fn every_rows_derived_values_are_the_ones_written_here() {
        for host in ALL {
            for running in [Os::Windows, Os::Linux] {
                assert!(
                    GOLDEN.iter().any(|golden| golden.host.id == host.id && golden.running == running),
                    "no golden case for {} on {running:?}",
                    host.id
                );
            }
        }
        let hosts = Path::new("hosts");
        let exe_dirs = exe_dir_and_parents();
        let exe_dir = &exe_dirs[0];
        for golden in &GOLDEN {
            let (host, running) = (golden.host, golden.running);
            let case = format!("{} on {running:?}", host.id);
            assert_eq!(host.built_for(running), golden.built_for, "{case}");
            assert_eq!(host.file_name(running), golden.file_name, "{case}");
            assert_eq!(host.pin_platform(running), golden.pin_platform, "{case}");
            let sidecars: Vec<&str> = host.sidecars_for(running).map(|sidecar| sidecar.file).collect();
            assert_eq!(sidecars, golden.sidecars, "{case}");
            assert_eq!(host.pinned_files(running), golden.pinned_files, "{case}");
            assert_eq!(host.installed_files(running), golden.installed_files, "{case}");
            assert_eq!(host.runtime_lib(running), golden.runtime_lib, "{case}");
            assert_eq!(host.runtime_in(hosts), golden.runtime_in.map(|dir| hosts.join(dir)), "{case}");
            assert_eq!(host.installed_in(running, hosts), under(hosts, golden.installed_in), "{case}");
            let beside = under(exe_dir, golden.beside);
            assert_eq!(host.beside(running, exe_dir), beside, "{case}");
            assert_eq!(host.in_place(running, exe_dir, false), std::slice::from_ref(&beside), "{case}");
            let mut walked: Vec<PathBuf> = match golden.walks {
                Some(dev) => exe_dirs.iter().map(|dir| under(dir, &[dev, "debug", golden.file_name])).collect(),
                None => Vec::new(),
            };
            walked.push(beside);
            assert_eq!(host.in_place(running, exe_dir, true), walked, "{case}");
            assert_eq!(host.embed_var(), golden.embed_var, "{case}");
        }
    }

    /// The release is built on Windows, where a host built for another OS runs in a VM (the gate's `SITE_HOST`
    /// case): only WebKit, whose file keeps its Linux name there (H11).
    #[test]
    fn only_webkit_is_built_for_another_os_than_windows() {
        let foreign: Vec<&str> =
            ALL.iter().filter(|host| host.built_for(Os::Windows) != Os::Windows).map(|host| host.id).collect();
        assert_eq!(foreign, ["webkit"]);
        assert_eq!(WEBKIT.file_name(Os::Windows), "manifold-view-webkitgtk");
    }

    fn files(names: &[&str]) -> Option<Vec<String>> {
        Some(names.iter().map(|name| name.to_string()).collect())
    }

    #[test]
    fn a_manifest_may_list_a_host_only_under_the_platform_it_is_built_for() {
        assert_eq!(CEF.pinned_under("windows-x64"), files(&["manifold-view-cef.exe"]));
        assert_eq!(
            SERVO.pinned_under("windows-x64"),
            files(&["manifold-view-servo.exe", "libEGL.dll", "libGLESv2.dll"])
        );
        assert_eq!(WEBKIT.pinned_under("linux-x64"), files(&["manifold-view-webkitgtk"]));
        assert_eq!(WEBKIT.pinned_under("windows-x64"), None, "built for Linux");
        for unknown in ["windows-arm64", ""] {
            assert_eq!(CEF.pinned_under(unknown), None, "{unknown:?}");
        }
        for embedded in [&WEBVIEW2, &PDFIUM] {
            assert_eq!(embedded.pinned_under("windows-x64"), None, "{} is embedded", embedded.id);
        }
        // The table is OS-agnostic: the app looks up only `Os::here()`'s platform.
        assert_eq!(CEF.pinned_under("macos-x64"), files(&["manifold-view-cef"]));
        assert_eq!(CEF.pinned_under("linux-x64"), files(&["manifold-view-cef"]));
        assert_eq!(SERVO.pinned_under("linux-x64"), files(&["manifold-view-servo"]));
        assert_eq!(CEF.pinned_under("other-x64"), None, "no host is pinned for Os::Other");
    }

    #[test]
    fn a_row_is_found_by_its_id_ignoring_case() {
        for host in ALL {
            assert_eq!(by_id(host.id).map(|found| found.id), Some(host.id));
            assert_eq!(by_id(&host.id.to_ascii_uppercase()).map(|found| found.id), Some(host.id));
        }
        assert!(by_id("wkwebview").is_none());
        assert!(by_id("").is_none());
    }
}
