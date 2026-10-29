//! Where each engine host and runtime lives on disk, and whether an engine can run here: the sibling-binary
//! lookup, the cached on-disk check, and the availability shown in the engine lists.

use super::*;

/// A file beside `manifold.exe` named as this platform names an exe: the PDF renderer (`pdf_view_exe`), and CEF's
/// flat host (`sibling_host`).
pub(super) fn sibling_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.set_file_name(crate::cef_platform::PLATFORM.host_exe(name));
    path
}

/// The file name of `kind`'s host: as this platform names an exe (`PLATFORM.host_exe`), except a host built for another
/// OS than this one, which keeps its bare name (WebKit's ELF, run in WSL from Windows).
pub(super) fn host_file_name(kind: ViewKind) -> String {
    let desc = kind.desc();
    if desc.host_os.is_some_and(|os| os != Os::here()) {
        desc.binary.to_owned()
    } else {
        crate::cef_platform::PLATFORM.host_exe(desc.binary)
    }
}

/// Where `kind`'s host goes beside `manifold.exe` in `exe_dir`, whether or not it is there: flat, except CEF's, which
/// lives in its runtime folder (`cef-runtime/`). CEF ships its own `libEGL.dll`/`libGLESv2.dll` (ANGLE), and Windows
/// resolves an exe's implicit DLL dependencies from the exe's own folder first, so a CEF host in one folder with
/// another ANGLE user would load whichever copy was put there last. The other hosts are a lone exe beside their own
/// files (WebView2's loader is statically linked by the `webview2-com` crate), so they need no folder of their own.
fn beside_exe(kind: ViewKind, exe_dir: &Path) -> PathBuf {
    let file = host_file_name(kind);
    match kind.desc().runtime_dir {
        Some(runtime) => exe_dir.join(runtime).join(file),
        None => exe_dir.join(file),
    }
}

/// Every place `kind`'s host can run in place from, given `manifold.exe`'s folder `exe_dir`, in the order they are
/// tried: its dev build folder (`ViewDesc::dev_dir`, then `debug/`), found by walking up at most as far as the repo
/// root of a worktree under `.claude/`, then beside `manifold.exe` (`beside_exe`). The dev folder comes first: it is
/// where a dev tree builds the host, and a stray copy left beside the exe must not shadow a fresh build.
fn in_place_paths(kind: ViewKind, exe_dir: &Path) -> Vec<PathBuf> {
    let file = host_file_name(kind);
    let mut paths: Vec<PathBuf> = match kind.desc().dev_dir {
        Some(dev) => exe_dir.ancestors().take(5).map(|dir| dir.join(dev).join("debug").join(&file)).collect(),
        None => Vec::new(),
    };
    paths.push(beside_exe(kind, exe_dir));
    paths
}

/// The host of `kind` that a spawn runs as it stands, when one is there (`in_place_paths`). It is never anything under
/// `view_hosts_dir()` (a download, hash-checked by its install, or the embedded WebView2 host), and for CEF only its
/// `cef-runtime/` copy, the one a tab runs. This is the exe whose protocol number `read_on_disk` checks. `None` under
/// `cfg(test)`, so no unit test reads a real host from disk (a dev tree's hosts are reachable from the test exe).
fn run_in_place(kind: ViewKind) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    run_in_place_from(kind, &current_exe_dir())
}

/// `run_in_place` for a `manifold.exe` in `exe_dir`.
pub(super) fn run_in_place_from(kind: ViewKind, exe_dir: &Path) -> Option<PathBuf> {
    in_place_paths(kind, exe_dir).into_iter().find(|path| path.is_file())
}

/// Where to find each view-host's executable: the host that runs in place (`run_in_place`), else the copy
/// `view_install` puts in `view_hosts_dir()` (under `profile::view_data_dir()`), whether or not it is there yet: flat
/// for a downloaded host, in a folder of its own for the embedded one. (A downloaded CEF host is never run from there:
/// `prepare_cef` copies it beside the runtime's `libcef.dll`.) Without a data directory, the place beside
/// `manifold.exe` (`beside_exe`).
///
/// TODO: a dev tree's runtime files are put beside its hosts by `gate sync-runtime-deps` (README "Runtime-file
/// placement"), not by cargo; a real install or packaging step should do that.
pub(super) fn view_binary_path(kind: ViewKind) -> PathBuf {
    let desc = kind.desc();
    assert!(desc.link == HostLink::PipesShm, "{} is reached over TCP, not by a local path", desc.label);
    if let Some(exe) = run_in_place(kind) {
        return exe;
    }
    let Some(mut fallback) = view_hosts_dir() else { return beside_exe(kind, &current_exe_dir()) };
    if let HostSource::Embedded(host) = desc.source {
        // Where `spawn_tab` extracts the embedded host (`Embedded::ensure`), in a folder of its own.
        return host.host_path(&fallback);
    }
    fallback.push(host_file_name(kind));
    fallback
}

pub(crate) fn view_hosts_dir() -> Option<PathBuf> {
    crate::profile::view_data_dir().map(|mut dir| {
        dir.push("engine-hosts");
        dir
    })
}

/// `cef-runtime/<exe>` next to `manifold.exe`: where `gate sync-runtime-deps` puts it for dev builds.
pub(super) fn cef_sibling_exe() -> PathBuf {
    beside_exe(ViewKind::Cef, &current_exe_dir())
}

/// The host exe of `kind` that ships in place (a dev tree's, or a packaged one's), when it is there: the one that runs
/// in place (`run_in_place`), except CEF's, which is its flat exe beside `manifold.exe` first, then its `cef-runtime/`
/// copy (`shipped_cef_host` copies the flat one into a runtime that has no host). A host in place makes the host part
/// Ready whatever the pin table says.
pub(super) fn sibling_host(kind: ViewKind) -> Option<PathBuf> {
    if kind.desc().runtime_dir.is_none() {
        return run_in_place(kind);
    }
    let flat = Some(sibling_binary(kind.desc().binary)).filter(|path| path.exists());
    flat.or_else(|| Some(cef_sibling_exe()).filter(|path| path.exists()))
}

fn current_exe_dir() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current_exe");
    dir.pop();
    dir
}

/// Where WebKit's host is: the one that runs in place (`run_in_place_from`), else the copy `view_install` puts in
/// `hosts_dir` (`engine-hosts/`), whether or not it is there yet. `None` without a data directory.
pub(super) fn pick_webkit_host(exe_dir: &Path, hosts_dir: Option<&Path>) -> Option<PathBuf> {
    run_in_place_from(ViewKind::Webkit, exe_dir)
        .or_else(|| hosts_dir.map(|dir| dir.join(host_file_name(ViewKind::Webkit))))
}

/// `pick_webkit_host` for the running exe.
pub(super) fn webkit_host_path() -> Option<PathBuf> {
    pick_webkit_host(&current_exe_dir(), view_hosts_dir().as_deref())
}

/// The directory a downloaded runtime unpacks into (and whose `ready` marker
/// `runtime_state` reads); `None` for engines with no runtime archive, or
/// without a data directory.
pub(crate) fn view_runtime_dir(kind: ViewKind) -> Option<PathBuf> {
    let name = kind.desc().runtime_dir?;
    view_hosts_dir().map(|dir| dir.join(name))
}

/// The host part of `kind` on disk: Ready when a sibling host is there, else what the `<kind>.host` marker says
/// about this build's pinned files (`host_state`; Missing when it pins none).
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

/// The disk check `kind` needs before it can run, read now: a downloaded engine's host (a sibling, or the marker
/// of the pinned files) and CEF's runtime. Only markers and file sizes are read, never file contents. Nothing to
/// check for the rest (an embedded host; WebKit's is found in WSL only when a tab spawns), which read Ready.
fn read_on_disk(kind: ViewKind) -> OnDisk {
    if downloads(kind) {
        OnDisk { host: host_on_disk(kind), runtime: runtime_on_disk(kind) }
    } else {
        OnDisk::both(RuntimeOnDisk::Ready)
    }
}

/// The `on_disk` that `view_availability` takes for `kind`, for the engine lists: `read_on_disk`'s answer, reread
/// at most every `ON_DISK_CHECK_TTL`; Ready when there is nothing to check.
pub(crate) fn view_on_disk(kind: ViewKind) -> OnDisk {
    if !downloads(kind) {
        return OnDisk::both(RuntimeOnDisk::Ready);
    }
    on_disk_cached(&ON_DISK, kind, std::time::Instant::now(), || read_on_disk(kind))
}

/// `view_on_disk` read now, not through the cache: a host built or a CEF runtime copied a moment ago
/// should launch.
pub(super) fn view_on_disk_fresh(kind: ViewKind) -> OnDisk {
    read_on_disk(kind)
}

/// How `kind` is available, in one place. `plan` is what installing it would fetch in this build (`plan_for`), and
/// `on_disk` what the engine has on disk (`view_on_disk`). An engine that is not downloaded (embedded, or a sibling
/// only) is Installed. A downloaded one follows its host and, for CEF, its runtime: a custom path (and a running
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

/// `view_availability` for a given OS. A plan that is an `Err` means there is nothing to download: `Unavailable`
/// with its reason, unless the parts are already there (a dev tree's siblings) or, for CEF, a custom runtime is
/// set and the host is there.
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
