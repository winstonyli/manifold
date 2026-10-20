//! Where each engine host and runtime lives on disk, and whether an engine can run here: the sibling-binary
//! lookup, the cached on-disk check, and the availability shown in the engine lists.

use super::*;

/// The view-host binaries are sibling executables in the same
/// target/{debug,release} directory as manifold itself — standard
/// Cargo workspace layout, no separate install/discovery step needed.
pub(super) fn sibling_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.set_file_name(crate::cef_platform::PLATFORM.host_exe(name));
    path
}

/// Where to find each view-host's executable.
///
/// CEF is the one exception to the plain-sibling rule: it ships its own
/// `libEGL.dll`/`libGLESv2.dll` (ANGLE), which collide with Servo's
/// differently-built copies of the same-named DLLs if both engine-hosts
/// live in the same directory — Windows resolves an implicit DLL
/// dependency from the loading EXE's own directory first, so whichever
/// engine's ANGLE build was copied there last wins for *both* engines.
/// manifold-engine-cef.exe and its full CEF runtime (libcef.dll, resources,
/// locales, its own ANGLE DLLs, ...) are isolated in `cef-runtime/` next to
/// the other binaries.
///
/// TODO: this isolation + the runtime file copying itself is currently a
/// manual post-build step (see project memory), not automated — a real
/// install/packaging step should do this, not a person running `cp`.
///
/// Falls back to the host file `view_install::install_host` puts flat in
/// `view_hosts_dir()` (under `profile::view_data_dir()`) when the sibling
/// binary isn't there — for an engine fetched on demand after install rather
/// than bundled next to `manifold.exe`. (A downloaded CEF host is never run
/// from there: `spawn_tab` copies it beside the runtime's `libcef.dll`.)
pub(super) fn view_binary_path(kind: ViewKind) -> PathBuf {
    let desc = kind.desc();
    assert!(desc.spawn_mode == SpawnMode::Local, "{} is spawned via wsl.exe, not a local path", desc.label);
    let file_name = crate::cef_platform::PLATFORM.host_exe(desc.binary);
    // An engine with a runtime keeps its host in that runtime's `cef-runtime/`. The others are a lone exe (WebView2's
    // loader is statically linked by the `webview2-com` crate, or a lone DLL at worst): no ANGLE-style filename
    // collision with a runtime's files, so no isolation directory.
    let sibling = if desc.runtime_dir.is_some() { cef_sibling_exe() } else { sibling_binary(desc.binary) };
    if sibling.exists() {
        return sibling;
    }
    let Some(mut fallback) = view_hosts_dir() else { return sibling };
    if let HostSource::Embedded(host) = desc.source {
        // Where `spawn_tab` extracts the embedded host (`Embedded::ensure`), in a folder of its own.
        return host.host_path(&fallback);
    }
    fallback.push(file_name);
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
    let mut path = sibling_binary(ViewKind::Cef.desc().binary);
    let file_name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
    path.pop();
    path.push(CEF_RUNTIME_DIR);
    path.push(file_name);
    path
}

/// The host exe of `kind` that ships beside `manifold.exe` (a dev tree's, or a packaged one's), when it is there.
/// CEF also has the `cef-runtime/` copy a dev build keeps. A sibling host makes the host part Ready whatever the
/// pin table says.
pub(super) fn sibling_host(kind: ViewKind) -> Option<PathBuf> {
    let desc = kind.desc();
    if desc.spawn_mode == SpawnMode::Wsl {
        return built_webkit_host(&current_exe_dir());
    }
    let flat = Some(sibling_binary(desc.binary)).filter(|path| path.exists());
    if desc.runtime_dir.is_some() {
        flat.or_else(|| Some(cef_sibling_exe()).filter(|path| path.exists()))
    } else {
        flat
    }
}

fn current_exe_dir() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current_exe");
    dir.pop();
    dir
}

/// The WebKit host ELF that is already built where `exe_dir` (the folder of `manifold.exe`) can see it: beside it
/// (a Linux package), else a dev tree's `target-linux/debug/` (WebKit is built inside WSL into its own target dir,
/// README "Building"), found by walking up at most as far as the repo root of a worktree under `.claude/`.
pub(super) fn built_webkit_host(exe_dir: &Path) -> Option<PathBuf> {
    const NAME: &str = "manifold-engine-webkit";
    let flat = exe_dir.join(NAME);
    let dev = |dir: &Path| dir.join("target-linux").join("debug").join(NAME);
    std::iter::once(flat).chain(exe_dir.ancestors().take(5).map(dev)).find(|path| path.is_file())
}

/// Where WebKit's host is: a built one (`built_webkit_host`), else the copy `view_install` puts in `hosts_dir`
/// (`engine-hosts/`), whether or not it is there yet. `None` without a data directory.
pub(super) fn pick_webkit_host(exe_dir: &Path, hosts_dir: Option<&Path>) -> Option<PathBuf> {
    built_webkit_host(exe_dir).or_else(|| hosts_dir.map(|dir| dir.join(ViewKind::Webkit.desc().binary)))
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
    availability_with(std::env::consts::OS, plan, kind, on_disk, installing, failed, custom)
}

/// Why `kind` cannot run on `os` (a `std::env::consts::OS` value) at all, or `None` when it can. The one
/// place that knows: availability (every engine list) and the spawn refusal both read it.
/// WebView2 is Windows-only. WebKit runs through WSL on Windows and is hosted natively on Linux; macOS has
/// no WebKit host wired yet. (WSL is not probed: on Windows it is only found missing when a tab spawns.)
pub(super) fn platform_unavailable_reason(os: &str, kind: ViewKind) -> Option<&'static str> {
    // An engine with no `only_on` runs wherever manifold does (Servo's host builds there; CEF's platforms are its
    // pin's, `plan_for`).
    let desc = kind.desc();
    (!desc.only_on.is_empty() && !desc.only_on.contains(&os)).then_some(desc.only_on_reason)
}

/// The engine a fresh profile starts new tabs in on `os`: the first in picker order that may start one
/// (`ViewDesc::starting_default`) and that the platform can run.
pub(crate) fn platform_default_view_kind(os: &str) -> ViewKind {
    ViewKind::picker_order()
        .into_iter()
        .find(|kind| kind.desc().starting_default && platform_unavailable_reason(os, *kind).is_none())
        .unwrap_or(ViewKind::Servo)
}

/// `view_availability` for a given OS. A plan that is an `Err` means there is nothing to download: `Unavailable`
/// with its reason, unless the parts are already there (a dev tree's siblings) or, for CEF, a custom runtime is
/// set and the host is there.
pub(super) fn availability_with(
    os: &str,
    plan: &Result<Plan, String>,
    kind: ViewKind,
    on_disk: OnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> ViewStatus {
    if let Some(reason) = platform_unavailable_reason(os, kind) {
        return ViewStatus::Unavailable(reason.to_string());
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
