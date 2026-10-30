//! Where each engine host and runtime lives on disk, and whether an engine can run here: the in-place host
//! lookup, the cached on-disk check, and the availability shown in the engine lists.

use super::*;
use manifold_hosts::{HOSTS_DIR, WEBKIT};

/// The copy of `host` that a spawn runs as it stands, when one is there: the first of its in-place paths
/// (`Host::in_place`) that is a file. It is never anything under `view_hosts_dir()` (a download, hash-checked by its
/// install, or an extracted embedded host), and for CEF only its `cef-runtime/` copy, the one a tab runs. This is the
/// exe whose protocol number `read_on_disk` checks, and the PDF view `pdf_view_exe` runs. `None` under `cfg(test)`, so
/// no unit test reads a real host from disk (a dev tree's hosts are reachable from the test exe).
pub(super) fn run_in_place(host: &Host) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    run_in_place_from(host, &current_exe_dir(), walks_dev_folders())
}

/// `run_in_place` for a `manifold.exe` in `exe_dir`; `dev_folders`: whether to look in dev build folders
/// (`Host::dev_dir`: Servo's `target-servo`, README "Building Servo on its own"; WebKit's `target-linux`, built inside
/// WSL). The dev folder comes before the place beside the exe: it is where a dev tree builds the host, and a stray copy
/// left beside the exe must not shadow a fresh build.
pub(super) fn run_in_place_from(host: &Host, exe_dir: &Path, dev_folders: bool) -> Option<PathBuf> {
    host.in_place(Os::here(), exe_dir, dev_folders).into_iter().find(|path| path.is_file())
}

/// Whether the in-place lookup looks in dev build folders: only in a build that pins no hosts (a dev build). A release
/// (`host_pin::embedded`) runs a host from beside `manifold.exe` or from its hash-pinned download, never from a folder
/// found by walking up: any user can create a folder in a drive root, so the walk could reach a host nobody pinned, and
/// a host in place skips the hash check.
fn walks_dev_folders() -> bool {
    !crate::host_pin::embedded()
}

/// Where to find each view-host's executable: the host that runs in place (`run_in_place`), else where it is installed
/// under `view_hosts_dir()` (under `profile::view_data_dir()`; `Host::installed_in`), whether or not it is there yet:
/// flat for a downloaded host, in a folder of its own for an embedded one (where `embedded_host` extracts it). (A
/// downloaded CEF host is never run from there: `prepare_cef` copies it beside the runtime's `libcef.dll`.) Without a
/// data directory, the place beside `manifold.exe` (`Host::beside`).
///
/// TODO: a dev tree's runtime files are put beside its hosts by `gate sync-runtime-deps` (README "Runtime-file
/// placement"), not by cargo; a real install or packaging step should do that.
pub(super) fn view_binary_path(kind: ViewKind) -> PathBuf {
    let desc = kind.desc();
    assert!(desc.link == HostLink::PipesShm, "{} is reached over TCP, not by a local path", desc.label);
    if let Some(exe) = run_in_place(desc.host) {
        return exe;
    }
    match view_hosts_dir() {
        Some(dir) => desc.host.installed_in(Os::here(), &dir),
        None => desc.host.beside(Os::here(), &current_exe_dir()),
    }
}

pub(crate) fn view_hosts_dir() -> Option<PathBuf> {
    crate::profile::view_data_dir().map(|mut dir| {
        dir.push(HOSTS_DIR);
        dir
    })
}

/// `cef-runtime/<exe>` next to `manifold.exe` (`Host::beside`): where `gate sync-runtime-deps` puts it for dev builds.
/// CEF's host lives in its runtime folder rather than flat: CEF ships its own `libEGL.dll`/`libGLESv2.dll` (ANGLE),
/// and Windows resolves an exe's implicit DLL dependencies from the exe's own folder first, so a CEF host in one folder
/// with another ANGLE user would load whichever copy was put there last. The other hosts are a lone exe beside their
/// own files (WebView2's loader is statically linked by the `webview2-com` crate), so they need no folder of their own.
pub(super) fn cef_sibling_exe() -> PathBuf {
    CEF.beside(Os::here(), &current_exe_dir())
}

/// The host exe of `kind` that ships in place (a dev tree's, or a packaged one's), when it is there: the one that runs
/// in place (`run_in_place`), except for a host with a runtime folder (CEF), which is its flat exe beside
/// `manifold.exe` first, then its copy in the runtime folder (`Host::beside`; `shipped_cef_host` copies the flat one
/// into a runtime that has no host). A host in place makes the host part Ready whatever the pin table says.
pub(super) fn sibling_host(kind: ViewKind) -> Option<PathBuf> {
    let host = kind.desc().host;
    if host.runtime.is_none() {
        return run_in_place(host);
    }
    let exe_dir = current_exe_dir();
    let flat = Some(exe_dir.join(host.file_name(Os::here()))).filter(|path| path.exists());
    flat.or_else(|| Some(host.beside(Os::here(), &exe_dir)).filter(|path| path.exists()))
}

fn current_exe_dir() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current_exe");
    dir.pop();
    dir
}

/// Where WebKit's host is: the one that runs in place (`run_in_place_from`), else where `view_install` installs it
/// (`Host::installed_in`) in `hosts_dir` (`engine-hosts/`), whether or not it is there yet. `None` without a data
/// directory.
pub(super) fn pick_webkit_host(exe_dir: &Path, hosts_dir: Option<&Path>, dev_folders: bool) -> Option<PathBuf> {
    run_in_place_from(&WEBKIT, exe_dir, dev_folders)
        .or_else(|| hosts_dir.map(|dir| WEBKIT.installed_in(Os::here(), dir)))
}

/// `pick_webkit_host` for the running exe.
pub(super) fn webkit_host_path() -> Option<PathBuf> {
    pick_webkit_host(&current_exe_dir(), view_hosts_dir().as_deref(), walks_dev_folders())
}

/// The directory a downloaded runtime unpacks into (and whose `ready` marker
/// `runtime_state` reads); `None` for engines with no runtime archive, or
/// without a data directory.
pub(crate) fn view_runtime_dir(kind: ViewKind) -> Option<PathBuf> {
    kind.desc().host.runtime_in(&view_hosts_dir()?)
}

/// The host part of `kind` on disk: Ready when a host is in place (`sibling_host`), else what the `<kind>.host`
/// marker says about this build's pinned files (`host_state`; Missing when it pins none).
fn host_on_disk(kind: ViewKind) -> RuntimeOnDisk {
    if sibling_host(kind).is_some() {
        return RuntimeOnDisk::Ready;
    }
    match view_hosts_dir() {
        Some(dir) => host_state(&dir, kind, host_components(kind)),
        None => RuntimeOnDisk::Missing,
    }
}

/// The runtime part of `kind` on disk: Ready for an engine with none; for CEF, a bundled sibling
/// `cef-runtime/` (dev builds) is Ready, otherwise the data-dir runtime is judged by its marker.
fn runtime_on_disk(kind: ViewKind) -> RuntimeOnDisk {
    if !has_runtime(kind) || cef_sibling_exe().exists() {
        return RuntimeOnDisk::Ready;
    }
    match (view_runtime_dir(kind), crate::cef_pin::pin_for_platform()) {
        (Some(dir), Some(pin)) => runtime_state(&dir, pin),
        _ => RuntimeOnDisk::Missing,
    }
}

/// How long `view_on_disk` reuses an answer: the engine lists read it every frame they draw.
pub(super) const ON_DISK_CHECK_TTL: Duration = Duration::from_secs(5);

/// The last disk answer per engine, with when it was read.
pub(super) type OnDiskCache = std::sync::Mutex<Vec<(ViewKind, std::time::Instant, OnDisk)>>;
static ON_DISK: OnDiskCache = std::sync::Mutex::new(Vec::new());

/// `kind`'s answer from `cache` if read within `ON_DISK_CHECK_TTL` of `now`, else `read()`'s, remembered.
pub(super) fn on_disk_cached(
    cache: &OnDiskCache,
    kind: ViewKind,
    now: std::time::Instant,
    read: impl FnOnce() -> OnDisk,
) -> OnDisk {
    let mut entries = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(&(_, at, on_disk)) = entries.iter().find(|e| e.0 == kind) {
        if now.saturating_duration_since(at) <= ON_DISK_CHECK_TTL {
            return on_disk;
        }
    }
    let on_disk = read();
    entries.retain(|e| e.0 != kind);
    entries.push((kind, now, on_disk));
    on_disk
}

/// Drops the remembered answers, so the next `view_on_disk` reads the disk: an install, removal or cancel changed it.
pub(crate) fn forget_on_disk() {
    ON_DISK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
}

/// The stamp last read per host path, keyed by the file's length and modified time, so a host rebuilt in place is
/// read again.
pub(super) type StampCache = std::sync::Mutex<Vec<(PathBuf, u64, Option<std::time::SystemTime>, Option<u32>)>>;
static STAMPS: StampCache = std::sync::Mutex::new(Vec::new());

/// The protocol number `read` finds in `path`, remembered in `cache` until the file's length or modified time changes.
/// A path that can't be stat'ed is read every time and never remembered.
pub(super) fn stamp_cached(cache: &StampCache, path: &Path, read: impl FnOnce(&Path) -> Option<u32>) -> Option<u32> {
    let Ok(meta) = std::fs::metadata(path) else { return read(path) };
    let (len, modified) = (meta.len(), meta.modified().ok());
    let mut entries = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(&(_, _, _, stamp)) = entries.iter().find(|e| e.0 == path && e.1 == len && e.2 == modified) {
        return stamp;
    }
    let stamp = read(path);
    entries.retain(|e| e.0 != path);
    entries.push((path.to_path_buf(), len, modified, stamp));
    stamp
}

/// The disk check `kind` needs before it can run, read now, for every engine: `read_on_disk_at` with the host that
/// would run in place (`run_in_place`, never a host under `view_hosts_dir()`) and this process's stamp cache.
fn read_on_disk(kind: ViewKind) -> OnDisk {
    read_on_disk_at(kind, run_in_place(kind.desc().host).as_deref(), &STAMPS)
}

/// `read_on_disk` given the host `in_place` that would run in place, its number read through `cache`
/// (`host_protocol`): the skew when it speaks another protocol than this build or carries none (built before hosts
/// were stamped, or not a host at all), none when it speaks this one or no host runs in place; and for a downloaded
/// engine its host (in place, or the marker of the pinned files) and CEF's runtime. Only markers, file sizes and a
/// host's section table are read (the table cached by the file's length and time), never whole files. An engine that
/// is not downloaded (an embedded host) has no parts to check, which read Ready.
pub(super) fn read_on_disk_at(kind: ViewKind, in_place: Option<&Path>, cache: &StampCache) -> OnDisk {
    use manifold_view_protocol::{host_protocol, Skew, PROTOCOL_VERSION};
    let parts = if downloads(kind) {
        OnDisk { host: host_on_disk(kind), runtime: runtime_on_disk(kind), skew: None }
    } else {
        OnDisk::both(RuntimeOnDisk::Ready)
    };
    let skew = in_place.and_then(|exe| Skew::between(stamp_cached(cache, exe, host_protocol), PROTOCOL_VERSION));
    OnDisk { skew, ..parts }
}

/// The `on_disk` that `view_availability` takes for `kind`, for the engine lists: `read_on_disk`'s answer, reread
/// at most every `ON_DISK_CHECK_TTL`.
pub(crate) fn view_on_disk(kind: ViewKind) -> OnDisk {
    on_disk_cached(&ON_DISK, kind, std::time::Instant::now(), || read_on_disk(kind))
}

/// `view_on_disk` read now, not through the cache: a host built or a CEF runtime copied a moment ago
/// should launch.
pub(super) fn view_on_disk_fresh(kind: ViewKind) -> OnDisk {
    read_on_disk(kind)
}

/// How `kind` is available, in one place. `plan` is what installing it would fetch in this build (`plan_for`), and
/// `on_disk` what the engine has on disk (`view_on_disk`). A host that would run speaking another protocol is
/// Unavailable, naming which side to rebuild. Otherwise an engine that is not downloaded (embedded, or in place only)
/// is Installed, and a downloaded one follows its host and, for CEF, its runtime: a custom path (and a running
/// install) override; a failed install overrides Missing and Stale but not parts that are current; with nothing to
/// download and nothing there it is Unavailable with the plan's reason.
pub(crate) fn view_availability(
    kind: ViewKind,
    plan: &Result<Plan, String>,
    on_disk: OnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> ViewStatus {
    availability_with(Os::here(), plan, kind, on_disk, installing, failed, custom)
}

/// Why `kind` cannot run on `os` at all, or `None` when it can (`runtime::unavailable_reason`). An engine whose
/// host is built for whichever OS manifold runs on (Servo; CEF, whose platforms are its pin's, `plan_for`) always
/// can. (WSL is not probed: on Windows it is only found missing when a tab spawns.)
pub(super) fn platform_unavailable_reason(os: Os, kind: ViewKind) -> Option<String> {
    runtime::unavailable_reason(kind, os)
}

/// The engine a fresh profile starts new tabs in on `os`: the first in picker order that may start one
/// (`ViewDesc::starting_default`) and that the platform can run.
pub(crate) fn platform_default_view_kind(os: Os) -> ViewKind {
    ViewKind::picker_order()
        .into_iter()
        .find(|kind| kind.desc().starting_default && platform_unavailable_reason(os, *kind).is_none())
        .unwrap_or(ViewKind::Servo)
}

/// `view_availability` for a given OS. After the platform's reason, a host speaking another protocol (`OnDisk::skew`)
/// is `Unavailable` with the `Skew`'s reason. A plan that is an `Err` means there is nothing to download:
/// `Unavailable` with its reason, unless the parts are already there (a dev tree's hosts) or, for CEF, a custom
/// runtime is set and the host is there.
pub(super) fn availability_with(
    os: Os,
    plan: &Result<Plan, String>,
    kind: ViewKind,
    on_disk: OnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> ViewStatus {
    if let Some(reason) = platform_unavailable_reason(os, kind) {
        return ViewStatus::Unavailable(reason);
    }
    // A host that would start speaking another protocol can't run whatever else holds: a custom runtime, a running
    // install or a recorded failure doesn't change which exe starts. (CEF with a custom runtime starts a copy of
    // `shipped_cef_host()`, which can differ from the `cef-runtime/` copy read here: PARKED "Host protocol stamp".)
    if let Some(skew) = on_disk.skew {
        return ViewStatus::Unavailable(skew.to_string());
    }
    if !downloads(kind) {
        return ViewStatus::Installed;
    }
    // A bring-your-own runtime replaces CEF's runtime archive, never its host.
    let custom = custom.filter(|_| has_runtime(kind));
    if let Some(path) = custom.filter(|_| on_disk.host == RuntimeOnDisk::Ready) {
        return ViewStatus::Custom(path.to_path_buf());
    }
    if let Some((received, total)) = installing {
        return ViewStatus::Installing { received, total };
    }
    // Parts that are there and current outrank a recorded failure (the failure was an update or a retry that did
    // not matter in the end); a failure outranks Missing and Stale.
    let needed = on_disk.needed(custom.is_some());
    if needed == RuntimeOnDisk::Ready {
        return ViewStatus::Installed;
    }
    let plan = match plan {
        Ok(plan) => plan,
        Err(reason) => return ViewStatus::Unavailable(reason.clone()),
    };
    if let Some(message) = failed {
        return ViewStatus::Failed(message.to_string());
    }
    let size = plan.fetch_size(on_disk, custom.is_some());
    match needed {
        RuntimeOnDisk::Ready => ViewStatus::Installed,
        RuntimeOnDisk::Missing => ViewStatus::NotInstalled { size },
        RuntimeOnDisk::Stale => ViewStatus::UpdateNeeded { size },
    }
}
