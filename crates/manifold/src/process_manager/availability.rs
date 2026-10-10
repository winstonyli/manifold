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
/// Falls back to `engine_runtime_dir` (under `profile::engine_data_dir()`)
/// when the sibling binary isn't there — for an engine fetched on demand
/// after install rather than bundled next to `manifold.exe`.
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
    // Same isolation as the sibling-dir case above: CEF's own ANGLE DLLs
    // would collide with Servo's if both engines' fallback files ever
    // landed flat in the same `engine-hosts/` directory.
    if let Some(runtime) = engine_runtime_dir(kind) {
        fallback = runtime;
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

/// The directory a downloaded runtime unpacks into (and whose `ready` marker
/// `runtime_state` reads); `None` for engines that download nothing, or
/// without a data directory.
pub(crate) fn engine_runtime_dir(kind: EngineKind) -> Option<PathBuf> {
    match kind {
        EngineKind::Cef => engine_hosts_dir().map(|dir| dir.join("cef-runtime")),
        _ => None,
    }
}

/// What CEF's runtime looks like on disk right now: a bundled sibling
/// `cef-runtime/` (dev builds) is Ready; otherwise the data-dir runtime is
/// judged by its marker.
pub(crate) fn cef_on_disk() -> RuntimeOnDisk {
    if cef_sibling_exe().exists() {
        return RuntimeOnDisk::Ready;
    }
    match (engine_runtime_dir(EngineKind::Cef), crate::cef_pin::pin_for_platform()) {
        (Some(dir), Some(pin)) => runtime_state(&dir, pin),
        _ => RuntimeOnDisk::Missing,
    }
}

/// Why Servo is Unavailable when its host exe is not where `engine_binary_path` looks.
pub(super) const SERVO_HOST_MISSING: &str = "the Servo host wasn't found";

/// Whether Servo's host exe is where `engine_binary_path` finds it: Ready, or Missing.
pub(super) fn servo_on_disk() -> RuntimeOnDisk {
    if engine_binary_path(EngineKind::Servo).exists() {
        RuntimeOnDisk::Ready
    } else {
        RuntimeOnDisk::Missing
    }
}

/// How long `engine_on_disk` reuses a CEF or Servo answer: the engine lists read it every frame they draw.
pub(super) const ON_DISK_CHECK_TTL: Duration = Duration::from_secs(5);

/// The last disk answer per engine, with when it was read.
pub(super) type OnDiskCache = std::sync::Mutex<Vec<(EngineKind, std::time::Instant, RuntimeOnDisk)>>;
static ON_DISK: OnDiskCache = std::sync::Mutex::new(Vec::new());

/// `kind`'s answer from `cache` if read within `ON_DISK_CHECK_TTL` of `now`, else `read()`'s, remembered.
pub(super) fn on_disk_cached(
    cache: &OnDiskCache,
    kind: EngineKind,
    now: std::time::Instant,
    read: impl FnOnce() -> RuntimeOnDisk,
) -> RuntimeOnDisk {
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

/// The `on_disk` that `engine_availability` takes for `kind`, for the engine lists: CEF's runtime
/// (`cef_on_disk`) and Servo's host exe (`servo_on_disk`), each reread at most every `ON_DISK_CHECK_TTL`, Ready for
/// the rest.
pub(crate) fn engine_on_disk(kind: EngineKind) -> RuntimeOnDisk {
    let read: fn() -> RuntimeOnDisk = match kind {
        EngineKind::Cef => cef_on_disk,
        EngineKind::Servo => servo_on_disk,
        EngineKind::Webview2 | EngineKind::Webkit => return RuntimeOnDisk::Ready,
    };
    on_disk_cached(&ON_DISK, kind, std::time::Instant::now(), read)
}

/// How `kind` is available, in one place. `on_disk` is what the engine needs on disk (`engine_on_disk`):
/// Servo is Installed when its host exe is there, else Unavailable (`SERVO_HOST_MISSING`); WebView2 and
/// WebKit ignore it and are Installed; CEF follows its runtime. A custom path and a running install
/// override CEF's; a failed install overrides Missing and Stale but not a current runtime.
pub(crate) fn engine_availability(
    kind: EngineKind,
    on_disk: RuntimeOnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> EngineState {
    availability_with(
        std::env::consts::OS,
        crate::cef_pin::pin_for_platform(),
        kind,
        on_disk,
        installing,
        failed,
        custom,
    )
}

/// Why `kind` cannot run on `os` (a `std::env::consts::OS` value) at all, or `None` when it can. The one
/// place that knows: availability (every engine list) and the spawn refusal both read it.
/// WebView2 is Windows-only. WebKit runs through WSL on Windows and is hosted natively on Linux; macOS has
/// no WebKit host wired yet. (WSL is not probed: on Windows it is only found missing when a tab spawns.)
pub(super) fn platform_unavailable_reason(os: &str, kind: EngineKind) -> Option<&'static str> {
    match kind {
        EngineKind::Webview2 if os != "windows" => Some("needs Windows"),
        EngineKind::Webkit if !matches!(os, "windows" | "linux") => Some("needs Windows or Linux"),
        _ => None,
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

/// `engine_availability` for a given OS and pin. `None` means this platform has no CEF build, so there is
/// nothing to download: `Unavailable`, unless a runtime is already there (a dev tree's) or a custom one is set.
pub(super) fn availability_with(
    os: &str,
    pin: Option<&Pin>,
    kind: EngineKind,
    on_disk: RuntimeOnDisk,
    installing: Option<(u64, u64)>,
    failed: Option<&str>,
    custom: Option<&Path>,
) -> EngineState {
    if let Some(reason) = platform_unavailable_reason(os, kind) {
        return EngineState::Unavailable(reason.to_string());
    }
    match kind {
        EngineKind::Cef => {}
        EngineKind::Servo if on_disk != RuntimeOnDisk::Ready => {
            return EngineState::Unavailable(SERVO_HOST_MISSING.to_string());
        }
        EngineKind::Servo | EngineKind::Webview2 | EngineKind::Webkit => return EngineState::Installed,
    }
    if let Some(path) = custom {
        return EngineState::Custom(path.to_path_buf());
    }
    if let Some((received, total)) = installing {
        return EngineState::Installing { received, total };
    }
    // A runtime that is there and current outranks a recorded failure (the failure was an update or a
    // retry that did not matter in the end); a failure outranks Missing and Stale.
    if on_disk == RuntimeOnDisk::Ready {
        return EngineState::Installed;
    }
    let Some(pin) = pin else { return EngineState::Unavailable(NO_PIN_REASON.to_string()) };
    if let Some(message) = failed {
        return EngineState::Failed(message.to_string());
    }
    match on_disk {
        RuntimeOnDisk::Ready => EngineState::Installed,
        RuntimeOnDisk::Missing => EngineState::NotInstalled { size: pin.size },
        RuntimeOnDisk::Stale => EngineState::UpdateNeeded { size: pin.size },
    }
}
