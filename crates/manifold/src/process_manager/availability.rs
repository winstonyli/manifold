//! Where each engine host and runtime lives on disk, and whether an engine can run here: the sibling-binary
//! lookup, the cached on-disk check, and the availability shown in the engine lists.

use super::*;

/// The engine-host binaries are sibling executables in the same
/// target/{debug,release} directory as manifold itself — standard
/// Cargo workspace layout, no separate install/discovery step needed.
pub(super) fn sibling_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.set_file_name(crate::cef_platform::PLATFORM.host_exe(name));
    path
}

/// Where to find each engine-host's executable.
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
/// Falls back to the host file `engine_install::install_host` puts flat in
/// `engine_hosts_dir()` (under `profile::engine_data_dir()`) when the sibling
/// binary isn't there — for an engine fetched on demand after install rather
/// than bundled next to `manifold.exe`. (A downloaded CEF host is never run
/// from there: `spawn_tab` copies it beside the runtime's `libcef.dll`.)
pub(super) fn engine_binary_path(kind: EngineKind) -> PathBuf {
    let file_name = crate::cef_platform::PLATFORM.host_exe(kind.desc().binary);
    let sibling = match kind {
        EngineKind::Servo => sibling_binary(kind.desc().binary),
        EngineKind::Cef => cef_sibling_exe(),
        // WebView2Loader is a single small loader (statically linked by
        // the `webview2-com` crate, or a lone DLL at worst) — no ANGLE-style
        // filename collision with Servo/CEF's runtime files, so no isolation
        // directory needed like CEF's `cef-runtime/` above.
        EngineKind::Webview2 => sibling_binary(kind.desc().binary),
        EngineKind::Webkit => unreachable!("Webkit is spawned via wsl.exe, not a local path"),
    };
    if sibling.exists() {
        return sibling;
    }
    let Some(mut fallback) = engine_hosts_dir() else { return sibling };
    if kind == EngineKind::Webview2 {
        // Where `spawn_tab` extracts the embedded host (`embedded_host::ensure`), in a folder of its own.
        return crate::embedded_host::host_path(&fallback);
    }
    fallback.push(file_name);
    fallback
}

pub(crate) fn engine_hosts_dir() -> Option<PathBuf> {
    crate::profile::engine_data_dir().map(|mut dir| {
        dir.push("engine-hosts");
        dir
    })
}

/// `cef-runtime/<exe>` next to `manifold.exe`: where `scripts/sync-runtime-deps.sh` puts it for dev builds.
pub(super) fn cef_sibling_exe() -> PathBuf {
    let mut path = sibling_binary(EngineKind::Cef.desc().binary);
    let file_name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
    path.pop();
    path.push("cef-runtime");
    path.push(file_name);
    path
}

/// The host exe of `kind` that ships beside `manifold.exe` (a dev tree's, or a packaged one's), when it is there.
/// CEF also has the `cef-runtime/` copy a dev build keeps. A sibling host makes the host part Ready whatever the
/// pin table says.
pub(super) fn sibling_host(kind: EngineKind) -> Option<PathBuf> {
    let flat = Some(sibling_binary(kind.desc().binary)).filter(|path| path.exists());
    match kind {
        EngineKind::Cef => flat.or_else(|| Some(cef_sibling_exe()).filter(|path| path.exists())),
        EngineKind::Servo | EngineKind::Webview2 => flat,
        EngineKind::Webkit => built_webkit_host(&current_exe_dir()),
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

/// Where WebKit's host is: a built one (`built_webkit_host`), else the copy `engine_install` puts in `hosts_dir`
/// (`engine-hosts/`), whether or not it is there yet. `None` without a data directory.
pub(super) fn pick_webkit_host(exe_dir: &Path, hosts_dir: Option<&Path>) -> Option<PathBuf> {
    built_webkit_host(exe_dir).or_else(|| hosts_dir.map(|dir| dir.join(EngineKind::Webkit.desc().binary)))
}

/// `pick_webkit_host` for the running exe.
pub(super) fn webkit_host_path() -> Option<PathBuf> {
    pick_webkit_host(&current_exe_dir(), engine_hosts_dir().as_deref())
}

/// The directory a downloaded runtime unpacks into (and whose `ready` marker
/// `runtime_state` reads); `None` for engines with no runtime archive, or
/// without a data directory.
pub(crate) fn engine_runtime_dir(kind: EngineKind) -> Option<PathBuf> {
    if !has_runtime(kind) {
        return None;
    }
    engine_hosts_dir().map(|dir| dir.join("cef-runtime"))
}

/// The host part of `kind` on disk: Ready when a sibling host is there, else what the `<kind>.host` marker says
/// about this build's pinned files (`host_state`; Missing when it pins none).
fn host_on_disk(kind: EngineKind) -> RuntimeOnDisk {
    if sibling_host(kind).is_some() {
        return RuntimeOnDisk::Ready;
    }
    match engine_hosts_dir() {
        Some(dir) => host_state(&dir, kind, host_components(kind)),
        None => RuntimeOnDisk::Missing,
    }
}

/// The runtime part of `kind` on disk: Ready for an engine with none; for CEF, a bundled sibling
/// `cef-runtime/` (dev builds) is Ready, otherwise the data-dir runtime is judged by its marker.
fn runtime_on_disk(kind: EngineKind) -> RuntimeOnDisk {
    if !has_runtime(kind) || cef_sibling_exe().exists() {
        return RuntimeOnDisk::Ready;
    }
    match (engine_runtime_dir(kind), crate::cef_pin::pin_for_platform()) {
        (Some(dir), Some(pin)) => runtime_state(&dir, pin),
        _ => RuntimeOnDisk::Missing,
    }
}

/// How long `engine_on_disk` reuses an answer: the engine lists read it every frame they draw.
pub(super) const ON_DISK_CHECK_TTL: Duration = Duration::from_secs(5);

/// The last disk answer per engine, with when it was read.
pub(super) type OnDiskCache = std::sync::Mutex<Vec<(EngineKind, std::time::Instant, OnDisk)>>;
static ON_DISK: OnDiskCache = std::sync::Mutex::new(Vec::new());

/// `kind`'s answer from `cache` if read within `ON_DISK_CHECK_TTL` of `now`, else `read()`'s, remembered.
pub(super) fn on_disk_cached(
    cache: &OnDiskCache,
    kind: EngineKind,
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

/// Drops the remembered answers, so the next `engine_on_disk` reads the disk: an install, removal or cancel changed it.
pub(crate) fn forget_on_disk() {
    ON_DISK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
}

/// The disk check `kind` needs before it can run, read now: a downloaded engine's host (a sibling, or the marker
/// of the pinned files) and CEF's runtime. Only markers and file sizes are read, never file contents. Nothing to
/// check for the rest (an embedded host; WebKit's is found in WSL only when a tab spawns), which read Ready.
fn read_on_disk(kind: EngineKind) -> OnDisk {
    if downloads(kind) {
        OnDisk { host: host_on_disk(kind), runtime: runtime_on_disk(kind) }
    } else {
        OnDisk::both(RuntimeOnDisk::Ready)
    }
}

/// The `on_disk` that `engine_availability` takes for `kind`, for the engine lists: `read_on_disk`'s answer, reread
/// at most every `ON_DISK_CHECK_TTL`; Ready when there is nothing to check.
pub(crate) fn engine_on_disk(kind: EngineKind) -> OnDisk {
    if !downloads(kind) {
        return OnDisk::both(RuntimeOnDisk::Ready);
    }
    on_disk_cached(&ON_DISK, kind, std::time::Instant::now(), || read_on_disk(kind))
}

/// `engine_on_disk` read now, not through the cache: a host built or a CEF runtime copied a moment ago
/// should launch.
pub(super) fn engine_on_disk_fresh(kind: EngineKind) -> OnDisk {
    read_on_disk(kind)
}

/// How `kind` is available, in one place. `plan` is what installing it would fetch in this build (`plan_for`), and
/// `on_disk` what the engine has on disk (`engine_on_disk`). An engine that is not downloaded (embedded, or a sibling
/// only) is Installed. A downloaded one follows its host and, for CEF, its runtime: a custom path (and a running
/// install) override; a failed install overrides Missing and Stale but not parts that are current; with nothing to
/// download and nothing there it is Unavailable with the plan's reason.
pub(crate) fn engine_availability(
    kind: EngineKind,
    plan: &Result<Plan, String>,
    on_disk: OnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> EngineState {
    availability_with(std::env::consts::OS, plan, kind, on_disk, installing, failed, custom)
}

/// Why `kind` cannot run on `os` (a `std::env::consts::OS` value) at all, or `None` when it can. The one
/// place that knows: availability (every engine list) and the spawn refusal both read it.
/// WebView2 is Windows-only. WebKit runs through WSL on Windows and is hosted natively on Linux; macOS has
/// no WebKit host wired yet. (WSL is not probed: on Windows it is only found missing when a tab spawns.)
pub(super) fn platform_unavailable_reason(os: &str, kind: EngineKind) -> Option<&'static str> {
    match kind {
        EngineKind::Webview2 => (os != "windows").then_some("needs Windows"),
        EngineKind::Webkit => (!matches!(os, "windows" | "linux")).then_some("needs Windows or Linux"),
        // Servo's host builds wherever manifold does; CEF's platforms are its pin's (`plan_for`).
        EngineKind::Servo | EngineKind::Cef => None,
    }
}

/// The engine a fresh profile starts new tabs in on `os`: the first of WebView2, Servo that the platform
/// can run. CEF is never the default (an opt-in download) and WebKit needs setup.
pub(crate) fn platform_default_engine(os: &str) -> EngineKind {
    [EngineKind::Webview2, EngineKind::Servo]
        .into_iter()
        .find(|kind| platform_unavailable_reason(os, *kind).is_none())
        .unwrap_or(EngineKind::Servo)
}

/// `engine_availability` for a given OS. A plan that is an `Err` means there is nothing to download: `Unavailable`
/// with its reason, unless the parts are already there (a dev tree's siblings) or, for CEF, a custom runtime is
/// set and the host is there.
pub(super) fn availability_with(
    os: &str,
    plan: &Result<Plan, String>,
    kind: EngineKind,
    on_disk: OnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> EngineState {
    if let Some(reason) = platform_unavailable_reason(os, kind) {
        return EngineState::Unavailable(reason.to_string());
    }
    if !downloads(kind) {
        return EngineState::Installed;
    }
    // A bring-your-own runtime replaces CEF's runtime archive, never its host.
    let custom = custom.filter(|_| has_runtime(kind));
    if let Some(path) = custom.filter(|_| on_disk.host == RuntimeOnDisk::Ready) {
        return EngineState::Custom(path.to_path_buf());
    }
    if let Some((received, total)) = installing {
        return EngineState::Installing { received, total };
    }
    // Parts that are there and current outrank a recorded failure (the failure was an update or a retry that did
    // not matter in the end); a failure outranks Missing and Stale.
    let needed = on_disk.needed(custom.is_some());
    if needed == RuntimeOnDisk::Ready {
        return EngineState::Installed;
    }
    let plan = match plan {
        Ok(plan) => plan,
        Err(reason) => return EngineState::Unavailable(reason.clone()),
    };
    if let Some(message) = failed {
        return EngineState::Failed(message.to_string());
    }
    let size = plan.fetch_size(on_disk, custom.is_some());
    match needed {
        RuntimeOnDisk::Ready => EngineState::Installed,
        RuntimeOnDisk::Missing => EngineState::NotInstalled { size },
        RuntimeOnDisk::Stale => EngineState::UpdateNeeded { size },
    }
}
