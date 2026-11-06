//! Unpacking the runtime archive and installing a plan's parts (swap, host files).

use crate::process_manager::ViewKind;
use crate::view_install::host_pin::Component as HostFile;
use manifold_hosts::CEF_RUNTIME_DIR;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use super::{download::*, state::*};

/// The archive's subfolders (under its one top folder) that hold the runtime; flattened on unpack.
const RUNTIME_DIRS: [&str; 2] = ["Release", "Resources"];
/// Build leftovers inside the runtime folders: any path with one of these directory names...
const SKIP_DIRS: [&str; 3] = ["include", "cmake", "libcef_dll"];
/// ...or ending in one of these file names (compared ignoring case)...
const SKIP_FILES: [&str; 2] = ["CMakeLists.txt", "debug.log"];
/// ...or ending in one of these suffixes (Windows import libraries).
const SKIP_SUFFIXES: [&str; 1] = [".lib"];

/// Where the runtime entry `path` goes under `into`, or None to skip it. The path is `<top>/Release/..`
/// or `<top>/Resources/..`; both folders flatten into `into`. Errors on any component that could escape:
/// `..`, a root or prefix, and any name containing `:` (on Windows `C:evil.dll` is a drive-relative name
/// that `join` would let replace the whole destination, and `a:b` names an NTFS stream) or `\`. A backslash
/// is a separator on Windows, so no component holds one there; on Linux it is an ordinary character, so
/// `Release/..\evil` is one odd name rather than an escape, refused anyway so an archive unpacks the same
/// (or fails the same) on every platform. CEF's archives use neither character.
pub(super) fn target(path: &Path, into: &Path) -> Result<Option<PathBuf>, InstallError> {
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
    if !RUNTIME_DIRS.iter().any(|d| *dir == *d) || rest.is_empty() {
        return Ok(None);
    }
    let last = rest[rest.len() - 1].to_string_lossy().to_ascii_lowercase();
    let skipped = rest.iter().any(|n| SKIP_DIRS.iter().any(|s| *n == *s))
        || SKIP_FILES.iter().any(|s| last == s.to_ascii_lowercase())
        || SKIP_SUFFIXES.iter().any(|s| last.ends_with(s));
    Ok((!skipped).then(|| rest.iter().fold(into.to_path_buf(), |p, n| p.join(n))))
}

/// The permission bits an unpacked file gets: the archive's own, minus
/// setuid/setgid/sticky and group/other write. `chrome-sandbox` needs setuid
/// root only for the setuid sandbox, which manifold doesn't use (the CEF host
/// passes `--no-sandbox`, see `manifold-view-cef/src/app.rs`).
#[cfg_attr(not(unix), allow(dead_code, reason = "only applied on Unix; tested everywhere"))]
pub(super) fn unpacked_mode(header_mode: u32) -> u32 {
    header_mode & 0o755
}

/// Extract the runtime from the CEF `tar.bz2` at `archive` into `into`: the files under `<top>/Release/`
/// and `<top>/Resources/` (`RUNTIME_DIRS`), flattened into `into`, without the build leftovers (`SKIP_DIRS`,
/// `SKIP_FILES`, `SKIP_SUFFIXES`). Any entry that is a link, or whose path has a `..`,
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
pub(super) fn swap(new: &Path, live: &Path) -> Result<(), InstallError> {
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
