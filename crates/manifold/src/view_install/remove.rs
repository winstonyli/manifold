//! What an install leaves behind and removing an engine's files.

use crate::process_manager::ViewKind;
use crate::view_install::host_pin::Component as HostFile;
use manifold_hosts::{Os, CEF_RUNTIME_DIR};
use std::path::{Path, PathBuf};

use super::state::*;
use super::*;

/// The files an install leaves beside the CEF runtime while it works (and after some failures).
fn leftovers() -> [String; 3] {
    ["part", "tmp", "old"].map(|ext| format!("{CEF_RUNTIME_DIR}.{ext}"))
}

/// The file names in the hosts directory that are `kind`'s host: the pinned `files`, those its marker lists (an
/// install of another release) and the host exe's own name (`Host::file_name`: as the OS it is built for names it, so
/// WebKit's has no `.exe` on Windows).
pub(super) fn host_file_names(dir: &Path, kind: ViewKind, files: &[HostFile]) -> Vec<String> {
    let mut names: Vec<String> = files.iter().map(|f| f.file.to_string()).collect();
    names.push(kind.desc().host.file_name(Os::here()));
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
pub(super) fn host_leftovers(dir: &Path, kind: ViewKind, files: &[HostFile]) -> Vec<PathBuf> {
    let names = host_file_names(dir, kind, files);
    let marker_tmp = format!("{}.host.tmp", kind.desc().host.id);
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
