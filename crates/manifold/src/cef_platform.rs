//! What differs per OS about installing and launching CEF, in one place (spec "Platforms",
//! `docs/superpowers/specs/2026-10-02-engine-install-design.md`). Windows has a pin and a tested
//! install; Linux has a pin and a layout checked against the real archive, but its install is unrun.
//!
//! CEF CDN platform ids: `windows64`, `linux64`, `macosx64`, `macosarm64` (see `cef_pin::pin_for`).

/// The per-platform facts the install and the host launch read.
pub(crate) struct CefPlatform {
    /// The CEF CDN's platform id: the `_<id>_` in `cef_binary_<version>_<id>_minimal.tar.bz2`.
    pub(crate) id: &'static str,
    /// The runtime library whose presence marks a CEF directory.
    pub(crate) runtime_lib: &'static str,
    /// Appended to an engine host's binary name (`.exe` on Windows).
    pub(crate) exe_suffix: &'static str,
    /// The archive's subfolders (under its one top folder) that hold the runtime; flattened on unpack.
    pub(crate) runtime_dirs: [&'static str; 2],
    /// Build leftovers inside the runtime folders: any path with one of these directory names...
    pub(crate) skip_dirs: [&'static str; 3],
    /// ...or ending in one of these file names (compared ignoring case)...
    pub(crate) skip_files: [&'static str; 2],
    /// ...or ending in one of these suffixes (Windows import libraries).
    pub(crate) skip_suffixes: &'static [&'static str],
}

impl CefPlatform {
    /// `binary`'s file name on this platform.
    pub(crate) fn host_exe(&self, binary: &str) -> String {
        format!("{binary}{}", self.exe_suffix)
    }

    /// The archive file name for `version`, as CEF's own `archive.json` names it.
    #[cfg(test)]
    pub(crate) fn archive_name(&self, version: &str) -> String {
        format!("cef_binary_{version}_{}_minimal.tar.bz2", self.id)
    }
}

#[cfg(windows)]
pub(crate) const PLATFORM: CefPlatform = CefPlatform {
    id: "windows64",
    runtime_lib: "libcef.dll",
    exe_suffix: ".exe",
    runtime_dirs: ["Release", "Resources"],
    skip_dirs: ["include", "cmake", "libcef_dll"],
    skip_files: ["CMakeLists.txt", "debug.log"],
    skip_suffixes: &[".lib"],
};

/// Every other OS reads as Linux for now (layout checked against the linux64 archive, install never run on Linux); macOS (`macosx64`/`macosarm64`) needs its own entry (an app
/// bundle layout, see the spec) and has no pin, so nothing is offered there either.
#[cfg(not(windows))]
pub(crate) const PLATFORM: CefPlatform = CefPlatform {
    id: "linux64",
    runtime_lib: "libcef.so",
    exe_suffix: "",
    runtime_dirs: ["Release", "Resources"],
    skip_dirs: ["include", "cmake", "libcef_dll"],
    skip_files: ["CMakeLists.txt", "debug.log"],
    skip_suffixes: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_exe_adds_the_platforms_suffix() {
        assert_eq!(PLATFORM.host_exe("manifold-engine-cef"), format!("manifold-engine-cef{}", PLATFORM.exe_suffix));
        assert_eq!(PLATFORM.host_exe("x"), if cfg!(windows) { "x.exe" } else { "x" });
    }

    #[test]
    fn the_archive_name_carries_the_platform_id() {
        assert_eq!(PLATFORM.archive_name("1.2+g3"), format!("cef_binary_1.2+g3_{}_minimal.tar.bz2", PLATFORM.id));
    }

    #[test]
    fn this_platforms_pin_is_for_this_platform() {
        // Windows has a pin and its URL names the platform; a platform without one has no lookup hit.
        match crate::cef_pin::pin_for_platform() {
            Some(pin) => assert!(pin.url.contains(&format!("_{}_minimal", PLATFORM.id)), "{}", pin.url),
            None => assert_ne!(PLATFORM.id, "windows64", "Windows must have a pin"),
        }
    }

    #[test]
    fn pin_lookup_is_none_for_a_platform_without_a_pin() {
        assert!(crate::cef_pin::pin_for("windows64").is_some());
        assert!(crate::cef_pin::pin_for("linux64").is_some());
        for id in ["macosx64", "macosarm64", "", "nonsense"] {
            assert!(crate::cef_pin::pin_for(id).is_none(), "{id}");
        }
    }
}
