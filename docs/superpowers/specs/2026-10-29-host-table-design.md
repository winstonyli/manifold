---
status: draft
---

# One table per host — Design

## Question

Each host's layout facts are spelled separately by every program that needs them. The app's `ViewDesc` holds the exe
name, the OS it is built for, its dev build folder, how it is delivered and its runtime folder
(`crates/manifold/src/process_manager.rs:55`-`102`). `embedded_host.rs:15`-`56` and `build.rs:12`-`15` each spell the two
embedded hosts again. The gate copies Servo's and CEF's layout in `sync_deps.rs` (`SERVO_DLLS`, `SERVO_EXE` at `:25`-`26`;
`sync_servo` and `sync_cef`, two functions for one job) and about twenty more exe-name and `cef-runtime` literals across
its stages. `release.yml` has its own host table (`:84`-`88`), its own mozangle search (`:79`-`81`) and its own embedded
copy and hash records (`:108`-`123`). The exe file name alone is derived four ways (`availability.rs:8`, `:16`,
`view_install.rs:846`, `process_manager.rs:644`), and two disagree: `view_install.rs:846` gives WebKit a `.exe` candidate
on Windows, where its host is a Linux ELF.

This design gives every host one row, read by the app, its build script, the gate and (through the gate) the release.
It closes PARKED "One table per host", GATE-CEF-DIR and "Servo's DLLs come from the newest mozangle output in either
target folder", and it comes before the CEF and WebKit download cases in `gate page-kind` (PARKED "CEF and WebKit
download paths unexercised at runtime"), which would otherwise spell two more hosts' layouts in the gate.

Decided conversationally on 2026-10-29 from a three-design judge panel (approaches below). Decisions carry ids H1-H14 so
the plan and the review can cite them.

## Decisions

- **H1. A new leaf crate, `manifold-hosts`.** Zero dependencies, `std` only, no `cfg`. `manifold` depends on it (normal
  and build dependency), and so does `gate`. Nothing else does: no host crate, not `manifold-view-protocol`, not
  `manifold-pages`, so a table edit rebuilds the app and the gate and no host (Servo takes about 33 minutes).
- **H2. `ViewDesc` gets `host: &'static Host`**, replacing `name`, `binary`, `source`, `host_os`, `runtime_dir` and
  `dev_dir`. `HostSource` is deleted. This amends `docs/decisions/2026-10-23-host-runtimes.md:68`-`69`, which says those
  fields "stay plain host data": they still are plain data, in a table of their own rather than in `ViewDesc`.
- **H3. The row key is `id`, not `name`.** It is the engine's store name (`settings.json`, the `view_kind` field via
  `store::model::VIEW_KINDS`, session exports) and the manifest's first key (`host_pin.rs:13`). `desc().name` becomes
  `desc().host.id`. Calling it `name` would make every `desc().host.name` read an unruled node-name read for the names
  guard, which exempts only `desc().name` (`names/tests.rs:470`-`495`).
- **H4. PDFium is a fifth row with no `ViewKind`.** It is embedded exactly as WebView2 is, so the embedding code, the
  freshness check and `release_verify` handle both in one loop.
- **H5. Two new gate subcommands replace `release.yml`'s PowerShell host logic.** `gate release-pins` replaces the source
  map and manifest loop (`release.yml:74`-`101`); `gate release-embedded` replaces the embedded copy and hash records
  (`:108`-`112`, `:117`-`123`). The `cargo build` lines and their CRT flags stay in the YAML.
- **H6. The embedded hash records are renamed `<stem>.embedded-<id>.sha256`** (`embedded-webview2`, `embedded-pdfium`;
  were `embedded-host`, `embedded-pdf-view`). `release.yml:144` and `release_verify.rs:74`, `:81` change in the same
  commit.
- **H7. `build.rs` generates the embedded-host code from the rows.** The input variable and the blob file are derived
  from the id (`MANIFOLD_WEBVIEW2_HOST`, `webview2-host.bin`; `MANIFOLD_PDFIUM_HOST`, `pdfium-host.bin`: today's names).
  Refinement found while writing: the generated file carries each hash as a literal, so the `MANIFOLD_<ID>_SHA256`
  rustc-env values, whose only reader was `embedded_host.rs:46`-`47`, are deleted rather than derived. This deletes
  `embedded_host::Embedded` and `build.rs`'s `HOSTS` tuple.
- **H8. CEF's runtime marker moves into the row** (`Runtime.lib`: `libcef.dll` on Windows, `libcef.so` elsewhere,
  as `cef_platform.rs:77`-`96` has it). `CefPlatform.runtime_lib` is deleted.
- **H9. One sync rule for every host.**
  - A host's own build folder is searched before `target/`'s (`find_first`), which is PARKED "Servo's DLLs ..."'s fix.
  - A host whose build folder is missing is skipped, not a failure of the run; the run fails only if no host's build
    folder exists.
  - A missing part is an error if and only if the host exe is built in that folder. No `required` flag.
  - Consequence, verified 2026-10-29 in `cef-dll-sys-152.1.0+152.0.6/build.rs:74`-`103`: the crate writes
    `out/cef_windows_x86_64` only when neither `FLATPAK` nor `CEF_PATH` is set (`:103`, `resolve_cef_dir(&out_dir)`);
    with `CEF_PATH` it resolves the runtime outside `OUT_DIR` (`:80`-`101`). So a `CEF_PATH` dev build with a built
    `manifold-view-cef.exe` and no older `out/` now fails `gate sync-runtime-deps`, where today it warns and copies the
    exe into a `cef-runtime/` that may have no runtime. The error says to copy the runtime from `CEF_PATH` by hand or
    build without it. (`CEF_PATH` appears in no repo code or doc; two `.superpowers/` experiment reports used it for
    release builds, which this sync does not touch.)
- **H10. The Linux `webkit-host` job keeps two literals** (`release.yml:25` `-p manifold-view-webkitgtk`, `:29`
  `target/release/manifold-view-webkitgtk`), tied to the WebKit row by a layout test. The gate cannot run there: it does
  not compile on Linux (`crates/gate/src/profile.rs:35` imports `std::os::windows` unconditionally, `main.rs:45` declares
  `mod profile;` without a `cfg`).
- **H11. The exe suffix is decided once, at run time: `Os::exe_suffix`.** `Os` moves from `process_manager/runtime.rs:14`-
  `62` into the leaf (moved, not copied; callers import it from there). `CefPlatform.exe_suffix` and `host_exe` are
  deleted. A host built for another OS keeps that OS's suffix, which fixes `view_install.rs:846`.
- **H12. Checks** (detailed under "Tests"): layout tests in the leaf; a test in the app that `ViewKind` maps onto the
  rows; exact-set checks of a manifest against the table in `release_verify` and `host_pin::parse`; a `tier.rs` Full rule
  for `crates/manifold-hosts`; and a golden test (below), so the table is not its own only witness.
- **H13. Parked with this design, not done in it:** the app's CEF-only runtime code, the repeated `-p` crate lists, and
  per-host environment variable names spelled in both app and host. Entries in PARKED.md, "Host table (2026-10-29)".
- **H14. Release proof needs a push.** The `release.yml` change cannot run locally; its first real run is the workflow's
  manual dry run, which needs the branch pushed. Pushing is the user's call.

## Approaches considered

Three designs were drafted independently and scored by two judges (end state; coverage and correctness), each out of 10.

| Design | End state | Coverage | Why not chosen |
|---|---|---|---|
| D1: a const table in `manifold-view-protocol` | 7 | 5 | Every host crate depends on the protocol crate, so a row edit rebuilds Servo and CEF, and the gate would gain interprocess, memmap2, postcard and serde. Its claimed fix for the Servo DLL source kept `find_newest` unchanged, so did not deliver it. |
| D2: a checked-in `hosts.json` plus a shared `model.rs` | 5 | 6 | No types and no exhaustive matching; each reader re-implements the path helpers. Its one advantage, that PowerShell reads JSON directly, is covered here by the gate subcommands. |
| D3: a zero-dependency leaf crate | 8 | 7 | Chosen, amended by H3, H7, H9 and the judges' points below. |

Keeping the gate independent of the app's layout (PARKED GATE-CEF-DIR's 2026-10-20 ruling) was the fourth option. Its
trigger fired on 2026-10-29: `sync_deps.rs:25`-`26` and `:182` now copy a second host's layout from the app. Independence
is kept where it matters by the golden test and by `release_verify`'s re-hash of `dist/` against the manifest, which
reads files, not the table.

Judges' points taken: derive the gate's WebKit `SITE_HOST` case from the row (below); one function for the embedded
record names; `package.rs`'s engine lists derived, with WebKit's absence explained; fix `sleep.rs:87`'s host name;
`Cargo.lock` in the same commit as the new crate (`release.yml` builds `--locked`); a golden check. Not taken: a
`RELEASE_OS` constant (see "Weighed and not taken").

## The crate

`crates/manifold-hosts`: package `manifold-hosts`, `version.workspace`, `edition.workspace`, `[lints] workspace = true`,
no `[dependencies]`. Added to the root `members` (`Cargo.toml:3`-`13`) and to `[workspace.dependencies]` beside the other
path crates (`Cargo.toml:23`-`24`). Files: `src/lib.rs` (the table and helpers) and `tests/layout.rs` (checks against the
repo's files).

```rust
/// The folder under the profile's view data directory that holds downloaded and extracted hosts.
pub const HOSTS_DIR: &str = "engine-hosts";
/// CEF's runtime folder: beside its host in a dev tree, under `HOSTS_DIR` when downloaded.
pub const CEF_RUNTIME_DIR: &str = "cef-runtime";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os { Windows, Linux, Mac, Other }

impl Os {
    pub const ALL: [Os; 4];
    pub fn from_consts(os: &str) -> Os;              // std::env::consts::OS values
    pub fn here() -> Os;
    pub fn name(self) -> &'static str;               // "Windows" | "Linux" | "macOS" | "another OS"
    pub fn pin_platform(self) -> &'static str;       // "windows-x64" | "linux-x64" | "macos-x64" | "other-x64"
    pub fn exe_suffix(self) -> &'static str;         // ".exe" on Windows, "" elsewhere
}

pub enum Delivery {
    /// Pinned in the release's manifest and downloaded into `HOSTS_DIR` on demand.
    Download,
    /// Carried inside manifold.exe and extracted to `HOSTS_DIR/<id>/<file>` at first use.
    Embedded,
}

/// Where cargo leaves a file or folder a host needs besides its exe: `<build>/<krate>-*/out/<tail>` or
/// `<build>/<krate>/*/out/<tail>` (both layouts, sync_deps.rs:14-16).
pub struct BuildOutput { pub krate: &'static str, pub tail: &'static str }

/// A file that sits beside the host exe when it runs on `os`.
pub struct Sidecar { pub file: &'static str, pub os: Os, pub from: BuildOutput }

/// A runtime folder beside the host, holding the runtime and a copy of the host exe.
pub struct Runtime {
    pub dir: &'static str,
    /// The file whose presence marks a runtime folder, per OS the host runs on.
    pub lib: fn(Os) -> &'static str,
    /// Where a dev build's runtime comes from (Windows: cef-dll-sys's `cef_windows_x86_64`).
    pub from: BuildOutput,
}

pub struct Host {
    /// The engine's store name and the manifest key (H3).
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

pub static SERVO: Host;     // servo,    manifold-view-servo,     os None,        dev_dir target-servo, Download,
                            //           sidecars libEGL.dll, libGLESv2.dll (Windows, from mozangle)
pub static CEF: Host;       // cef,      manifold-view-cef,       os None,        Download,
                            //           runtime cef-runtime (libcef.dll | libcef.so, from cef-dll-sys cef_windows_x86_64)
pub static WEBKIT: Host;    // webkit,   manifold-view-webkitgtk, os Some(Linux), dev_dir target-linux, Download
pub static WEBVIEW2: Host;  // webview2, manifold-view-webview2,  os Some(Windows), Embedded
pub static PDFIUM: Host;    // pdfium,   manifold-view-pdfium,    os None,        Embedded
pub static ALL: [&Host; 5]; // ViewKind::ALL's order, then PDFium
pub fn by_id(id: &str) -> Option<&'static Host>;   // ignoring ASCII case, as ViewKind::from_name does

impl Host {
    /// The OS this host is built for when manifold runs on `running`.
    pub fn built_for(&self, running: Os) -> Os;                    // os.unwrap_or(running)
    /// The one file-name derivation: `exe` plus the suffix of the OS it is built for.
    pub fn file_name_on(&self, running: Os) -> String;
    pub fn file_name(&self) -> String;                             // file_name_on(Os::here())
    pub fn pin_platform(&self, running: Os) -> &'static str;       // built_for(running).pin_platform()
    /// Beside manifold.exe in `exe_dir`: flat, or inside the runtime folder for a host with one.
    pub fn beside(&self, exe_dir: &Path) -> PathBuf;
    /// Where it runs in place from, in order: with `dev_folders`, `<dev_dir>/debug/<file>` in `exe_dir` and its four
    /// parents, nearest first; then `beside(exe_dir)`.
    pub fn in_place(&self, exe_dir: &Path, dev_folders: bool) -> Vec<PathBuf>;
    /// Where it is installed under `hosts_dir`: flat for Download, `<id>/<file>` for Embedded.
    pub fn installed_in(&self, hosts_dir: &Path) -> PathBuf;
    pub fn runtime_in(&self, hosts_dir: &Path) -> Option<PathBuf>;  // hosts_dir/<runtime.dir>
    /// The files a manifest pins for it: its exe, then the sidecars for the OS it is built for.
    pub fn pinned_files(&self, running: Os) -> Vec<String>;
    /// `pinned_files`, plus `<runtime.dir>/<lib>` for a host with a runtime: what an install leaves on disk.
    pub fn installed_files(&self, running: Os) -> Vec<String>;
    /// The build variable that names this host's file for embedding: `MANIFOLD_<ID>_HOST`.
    pub fn embed_var(&self) -> String;
}
```

Every method takes the OS manifold runs on, never a compile-time `cfg`, so the gate (Windows) can ask about WebKit
(Linux) and a test can ask about every OS. `build.rs` must not call `file_name()`: there `Os::here()` is the build
machine, not the target.

Rows are compared by `id` (no `PartialEq` on `Host`, which holds a `fn`).

## The app

- **`ViewDesc`** (`process_manager.rs:55`-`102`): `host: &'static Host` replaces the six fields (H2). Each row
  (`:121`-`147`, `:148`-`171`, `:172`-`203`, `:204`-`232`) names `host: &manifold_hosts::SERVO` and so on. The kept
  fields are `code`, `label`, `tags`, `link`, `starting_default`, `missing_system_libs`, `download_note`, `prepare`,
  `color`, `tier`, `pros`, `cons`, `picker_rank`. `HostSource` (`:36`-`46`) and `CEF_RUNTIME_DIR` (`:48`-`49`) are deleted.
- **Identity:** `ViewKind::from_name` (`:237`) and the serializer (`:255`) read `desc().host.id`, as do
  `debug_control.rs:452`, `debug_export.rs:420`, `view_install.rs:251`, `:860` and `session_sync/tests/misc.rs:201`-`204`.
- **In-place lookup** (`process_manager/availability.rs`): `run_in_place` and `run_in_place_from` take `&Host`, and read
  `host.in_place(exe_dir, dev_folders)`. `sibling_binary` (`:8`), `host_file_name` (`:16`), `beside_exe` (`:30`) and
  `in_place_paths` (`:42`) are deleted; their callers use `file_name()`, `beside()` and `in_place()`.
  `view_binary_path` (`:84`) ends in `host.installed_in(dir)`; `view_hosts_dir` (`:99`) joins `HOSTS_DIR`;
  `cef_sibling_exe` (`:107`) is `CEF.beside(exe_dir)`; `sibling_host` (`:115`) tries `exe_dir.join(file_name())`, then
  `beside(exe_dir)`, for a host with a runtime; `pick_webkit_host` (`:131`) falls back to `WEBKIT.installed_in(dir)`;
  `view_runtime_dir` (`:144`) is `host.runtime_in(dir)`. `walks_dev_folders` and its rule (a release never looks in a
  dev build folder) and `run_in_place`'s `None` under `cfg(test)` are unchanged.
- **`shipped_cef_host`** (`process_manager.rs:641`-`648`) falls back to `CEF.installed_in(dir)` in place of formatting
  the file name (`:644`).
- **The PDF view** (`pdf_view_exe`, `process_manager.rs:1212`-`1221`) uses the same in-place lookup and embedded
  fallback as WebView2's `prepare_webview2` (`:1011`): `run_in_place(&PDFIUM)`, else extract. PDFium has no dev folder,
  so its lookup is "beside `manifold.exe`", as today.
- **Install and pins** (`view_install.rs`): `downloads` (`:134`) reads `host.delivery`; `has_runtime` (`:139`) reads
  `host.runtime.is_some()`; `host_platform` (`:170`) is `host.pin_platform(Os::here())`; the `CEF_RUNTIME_DIR` uses
  (`:719`-`721`, `:839`, `:886`, `:921`) import the leaf's constant; `host_file_names` (`:846`) uses `file_name()`;
  `validate_custom` (`:326`), `settings_page.rs:472` and `view_install/tests.rs:155` read `(CEF.runtime.lib)(Os::here())`
  through one helper.
- **`Os`** moves to the leaf (H11). `runtime.rs` keeps `HostRuntime`, `HostLink` and `WSL_DISTRO`; `HostRuntime::for_os`
  (`:90`) and the WSL-speaks-TCP test (`:233`) read `desc().host.os`.
- **`cef_platform.rs`** loses `runtime_lib` (`:12`), `exe_suffix` (`:14`), `host_exe` (`:28`-`31`) and their test
  (`:119`-`122`). It keeps the CEF archive facts: `id`, `runtime_dirs`, `skip_*`, `system_libs`.
- **`host_pin::parse`** (`host_pin.rs:88`) checks each entry against the table: the id names a `Download` host, the
  platform is one `Os::pin_platform` names, and the file list equals `host.pinned_files(os)` as a set. Anything else
  refuses the whole manifest, the existing policy for a bad manifest (`:86`-`87`). Its tests' file names come from the
  table.

### Embedded hosts

`build.rs` loops `manifold_hosts::ALL` for `Delivery::Embedded` rows. For each, it reads `host.embed_var()`, writes
`OUT_DIR/<id>-host.bin` (the file's bytes, or empty), and appends one entry to a generated `OUT_DIR/embedded.rs`:

```rust
// generated by build.rs
static EMBEDDED: &[(&str, &[u8], &str)] = &[
    ("webview2", include_bytes!(concat!(env!("OUT_DIR"), "/webview2-host.bin")), "<sha256 or empty>"),
    ("pdfium", include_bytes!(concat!(env!("OUT_DIR"), "/pdfium-host.bin")), "<sha256 or empty>"),
];
```

A host is embedded only when its variable names a file and the target is Windows, as today (`build.rs:20`-`22`): the
extraction sets no execute bit, so the embed is a Windows mechanism, not a property of any one host.

`embedded_host.rs` `include!`s that file. `Embedded` and its `folder`, `file`, `label`, `bytes`, `host_path` (`:15`-`56`)
and the two `include_bytes!` statics (`:21`-`22`) are deleted. `ensure(host: &'static Host, hosts_dir)` looks the bytes
up by id and extracts to `host.installed_in(hosts_dir)`; `ensure_with` takes `&Host` in place of `Embedded`. Error
messages name the host's file instead of the deleted labels ("could not write manifold-view-pdfium.exe for ..." in place
of "the PDF view"). The tests assert file names only (`embedded_host.rs:194` onward), so they change only in how they
name the host. `process_manager/tests.rs:1091` uses `WEBVIEW2.installed_in(hosts)`.

## The gate

### `gate sync-runtime-deps` (`sync_deps.rs`)

`sync_servo` (`:111`-`130`) and `sync_cef` (`:131`-`148`) become one function:

```rust
fn sync_host(root: &Path, host: &Host, outcome: &mut Outcome) {
    // the host's build folder: <dev_dir or "target">/debug
    // skip quietly (one line) when that folder is missing
    // exe_there = <folder>/<host.file_name()> is a file
    // roots = [<folder>/build, target/debug/build] (one root when dev_dir is None)
    // each sidecar for Os::here(): find_first(roots, &sidecar.from) -> copy beside the exe
    //     none found: an error when exe_there, else one skip line
    // runtime: find_first(roots, &runtime.from) -> copy_runtime into <folder>/<runtime.dir>/
    //     none found: an error when exe_there (naming CEF_PATH as the likely cause), else one skip line
    //     then the exe into <folder>/<runtime.dir>/, always compared (it is rebuilt with its crate)
}
```

- `run` (`:176`-`187`) loops `manifold_hosts::ALL`. A host with no sidecars and no runtime (WebKit, WebView2, PDFium) does
  nothing and prints nothing. The run fails only if no host's build folder exists ("build first"), replacing today's
  hard failure when `target/debug` alone is missing (`:179`-`182`).
- `find_first(roots, from)` replaces `find_newest` (`:36`-`53`): the newest match in the first root that has one. Within
  a root both cargo layouts are searched, as today.
- `SERVO_DLLS`, `SERVO_EXE`, `sync_servo`, `sync_cef` and `run`'s literal paths are deleted. `copy_runtime`, `copy_dir`,
  `sync_file`, `Outcome`, `step_error` and `finish` stay.
- The tests (`:241`-`327`) are rewritten against `sync_host` on temp trees: own folder before `target/`; a missing folder
  skipped; a missing sidecar or runtime an error only with the exe built; the exe copied into the runtime folder only for
  a host with a runtime.

What changes for this machine's tree: Servo's DLLs come from `target-servo/debug/build/mozangle/` instead of whichever of
the two trees built last (both hold the same files on 2026-10-29); nothing else.

### `gate release-pins vX.Y.Z DIST FOREIGN` and `gate release-embedded vX.Y.Z DIST`

New stages in `STAGES` (`main.rs:138`-`189`), in a new `cfg(windows)` module `release_hosts.rs` beside
`release_verify.rs`. The release's own platform is `Os::here()`: the gate runs on the release runner.

- **`release-pins`** loops the `Download` rows. A host built for `Os::here()` comes from `target/release/<file>` and its
  sidecars from `find_first([target/release/build], ...)` (the sync's own search; `release.yml:79`-`81` searched
  recursively for a path containing `mozangle`, which on today's layout finds the same `build/mozangle/<hash>/out/`
  files). A host built for another OS comes from `FOREIGN/<file>` (the YAML's download of the Linux job's artifact,
  `webkit-host/`). Each file is copied to `DIST/<file>-<platform>` and hashed with the gate's `sha2`, and the manifest
  `DIST/manifold-<TAG>-<platform>.host-pins.json` is written with `serde_json` (`preserve_order`,
  `crates/gate/Cargo.toml:11`). A missing file is an error naming it and where it was looked for. On success it prints one
  line, `MANIFOLD_HOST_PINS=<absolute manifest path>`, on stdout; progress goes to stderr.
- **`release-embedded`** loops the `Embedded` rows: copies `target/release/<file>` to `DIST/embedded/<file>`, writes
  `DIST/manifold-<TAG>-<platform>.embedded-<id>.sha256` (`<hash>  <file>`, the format `release.yml:117`-`120` writes),
  and prints `<embed_var>=<absolute path>` per host.
- One function, `embedded_record(stem, host)`, names the record; `release-embedded` and `release_verify` both call it.

`release.yml` after the change (the Windows job; the Linux job is untouched, H10):

```yaml
      - name: Downloadable hosts and their pin manifest
        run: |
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold-view-cef
          if ($LASTEXITCODE -ne 0) { exit 1 }
          & target/debug/gate.exe release-pins $env:TAG dist webkit-host | Add-Content $env:GITHUB_ENV
          if ($LASTEXITCODE -ne 0) { exit 1 }
      - name: Embedded hosts
        run: |
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold-view-webview2 -p manifold-view-pdfium
          if ($LASTEXITCODE -ne 0) { exit 1 }
          & target/debug/gate.exe release-embedded $env:TAG dist | Add-Content $env:GITHUB_ENV
          if ($LASTEXITCODE -ne 0) { exit 1 }
      - name: manifold.exe
        run: |
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold
          if ($LASTEXITCODE -ne 0) { exit 1 }
          $stem = "manifold-$env:TAG-windows-x64"
          Copy-Item target/release/manifold.exe "dist/$stem.exe"
          $h = (Get-FileHash "dist/$stem.exe" -Algorithm SHA256).Hash.ToLower()
          "$h  $stem.exe" | Set-Content -Encoding ascii "dist/$stem.exe.sha256"
```

`$GITHUB_ENV` lines take effect from the next step, so `cargo build -p manifold` moves into a step of its own; the YAML
names no embedded host and no environment variable of the app's. `Add-Content` is how the job already writes
`$GITHUB_ENV` (`:48`). The draft-release step (`:144`) lists the records by pattern,
`Get-ChildItem dist -File -Filter "$stem.embedded-*.sha256"`, instead of by name. The Servo step (`:63`-`67`) and the
package and verify steps (`:124`-`130`) are unchanged.

### Other gate readers

- **`release_verify.rs`**: the manifest path, the exe name and the record names come from `Os::here().pin_platform()`
  and `embedded_record` (`:57`, `:65`, `:70`-`83`); the two embedded checks become one loop over the `Embedded` rows. New:
  the manifest's `(id, platform, file)` set must equal the table's for every `Download` host (`pinned_files` under
  `pin_platform(Os::here())`), so a host or sidecar left out of the release fails the verify. The re-hash of each file
  against the manifest (`:58`-`63`) is unchanged and reads only files.
- **`package.rs`**: `installed_files` (`:131`-`137`) is `host.installed_files(Os::here())` for a `Download` host built for
  `Os::here()`. The engines it checks (`:208`, `:219`) and `install-check`'s usage text (`main.rs:183`) come from the same
  filter, which yields CEF and Servo: WebKit is pinned but runs only through WSL, which the release runner and a bare
  `gate package` run do not have. `pins` (`:46`-`49`) and `newest_release` (`:31`-`43`) use `Os::here().pin_platform()`.
  The extracted-host checks (`:226`-`240`) use `installed_in(hosts)` for each `Embedded` row.
- **`fresh.rs`**: the CEF copy check (`:54`-`66`) becomes "for each host with a runtime: the built exe is not newer than
  its copy in the runtime folder". The dep-info list (`:69`) names `[&WEBVIEW2, &PDFIUM]` by row (the hosts the gate's
  build line builds; one list of those is PARKED "The `-p` crate lists are repeated"), and the hint at `:87` is built from
  the same list.
- **Fixtures and probes**: `jar.rs:481`, `:744`; `probes.rs:560`-`561`; `shots.rs:75`, `:1590`, `:1630`, `:1632`,
  `:1652`-`1653`; `stress.rs:40`; `package.rs:168`, `:226`; and the messages at `main.rs:8`, `:246` read `file_name()`,
  `CEF_RUNTIME_DIR`, `HOSTS_DIR` or `Runtime.lib`. Test literals that pin an old value on purpose stay
  (`fresh.rs:153`-`166`).
- **`sleep.rs:87`**: `kill_hosts` takes the host's `file_name()` from `by_id(view)` instead of formatting
  `manifold-view-{view}.exe`, which names no host for WebKit. WebKit's host still names no Windows process (manifold's
  child there is the `wsl.exe` relay), so the sleep stage's crash case kills nothing for WebKit; said plainly, not fixed
  here.
- **`all.rs:318`-`326`**: the `SITE_HOST` case is keyed on the step's host being built for another OS than the gate's
  (`by_id(engine).built_for(Os::here()) != Os::here()`), not on the string `"webkit"`: such a host runs in a VM, which
  reaches the page server at another address.

## Tests

- **Golden table** (leaf, `src/lib.rs` tests): for each row, the derived values written out as literals, on Windows and on
  Linux: `file_name_on`, `pin_platform`, `pinned_files`, `installed_files`, `installed_in` under a fixed folder,
  `in_place` for a fixed `exe_dir`, `embed_var`. An edit to a row then changes a test by hand, so the app and the gate no
  longer agree on a typo silently. It includes `WEBKIT.file_name_on(Os::Windows) == "manifold-view-webkitgtk"`.
- **Layout** (leaf, `tests/layout.rs`, reading repo files with `std::fs` only):
  - every row's `crates/<exe>/Cargo.toml` has `[package] name` equal to `exe`, and any `[[bin]] name` too;
  - every `dev_dir` is ignored in `.gitignore` (`/<dev_dir>`);
  - every row's `exe` follows a `-p ` in `release.yml`, and every row built for Linux has `target/release/<file>` there
    (H10);
  - the asset pattern at `release.yml:144` matches `-<platform>` for every platform a `Download` row is pinned under;
  - no crate under `crates/` other than `manifold` and `gate` names `manifold-hosts` in its `Cargo.toml`.
- **App**: `ViewKind::ALL` maps onto distinct rows, and the only row no `ViewKind` reaches is `PDFIUM`; the existing
  store-name test (`session_sync/tests/misc.rs:201`) keeps the ids equal to `store::model::VIEW_KINDS`.
- **Manifests**: `host_pin::parse` refuses a manifest whose file set for a host differs from the table, names a host that
  is not downloaded, or names an unknown platform; `release_verify` fails a manifest missing a pinned host or file.
- **Gate**: `sync_host` (above); `release-pins` and `release-embedded` on temp trees (files copied and named
  `<file>-<platform>`, manifest keys and sizes, a missing file is an error naming it, a foreign host taken from `FOREIGN`,
  records in the `<hash>  <file>` format, the printed `NAME=VALUE` lines).
- **Tier**: `tier.rs` classifies `crates/manifold-hosts/**` as Full (today `:151` would send it to Light as a "path on no
  list"), with a case beside `:281`.

## Tooling

- `-p manifold-hosts` joins `ci.yml:16` (clippy), `:17` and `:24` (tests; it builds on Linux), `.githooks/pre-commit:7`,
  the clippy command in CLAUDE.md "Formatting and lints", and the unit stage in `all.rs:289`-`301`.
- `Cargo.lock` gains the crate in the same commit that adds it: `release.yml` and `ci.yml` build `--locked`.
- `tier.rs`: the Full rule above.

## Not covered

Each stays outside the table; where it duplicates a row, a test ties it.

- Cargo package and bin names and the `crates/<exe>/` folders: TOML cannot read Rust. Tied by the layout test.
- `.gitignore`'s dev folders and the `CARGO_TARGET_DIR=target-servo|target-linux` convention (README "Building Servo on
  its own"): tied for `.gitignore`; nothing can make a build command use the folder.
- The CRT flags per build step and the Linux job's two literals (H10): build policy and a job the gate cannot run in.
- The `windows-x64` stem in the YAML (`release.yml:115`, `:126`, `:141`) and its asset pattern (`:144`): the release
  job's own platform. The gate derives the same stem from `Os::here()`, so a mismatch fails `release-verify` in the run.
- `ViewDesc`'s app facts (colours, tags, fn pointers, copy), `HostLink`, `ViewKind` identity and the default-engine
  policy: app facts, not layout.
- `cef_platform`'s archive facts, `SERVO_VC_RUNTIME` (`view_install.rs:145`) and `WSL_DISTRO` (`runtime.rs:12`): the
  download archive and the system, not a host's layout.
- The two manifest parsers (`host_pin.rs:88`, `release_verify.rs:34`): one wire format under two error policies; both
  gain the exact-set check.
- The profile prefix `manifold/data` in the gate (`package.rs:168`, `:226`, `shots.rs:1590`): the app's
  `profile::view_data_dir`; only the `engine-hosts` component comes from the table.
- Linux and macOS CEF dev syncing: `Runtime.from`'s tail is the Windows one, the only OS the gate runs on.
- About 45 `"webview2"` default-engine strings in gate scenarios: scenario input, not layout.
- No file or folder in a profile changes name, so no profile or session is affected.

## Deleted

- `HostSource`; `ViewDesc.name`, `.binary`, `.source`, `.host_os`, `.runtime_dir`, `.dev_dir`;
  `process_manager::CEF_RUNTIME_DIR`.
- `runtime::Os` (moved).
- `CefPlatform.runtime_lib`, `.exe_suffix`, `host_exe` and its test.
- `availability::{sibling_binary, host_file_name, beside_exe, in_place_paths}` and the `"engine-hosts"` literal.
- `embedded_host::Embedded` and its methods, the two `include_bytes!` statics, `build.rs`'s `HOSTS` tuple and the
  `MANIFOLD_<ID>_SHA256` rustc-env values.
- `sync_deps.rs`'s `SERVO_DLLS`, `SERVO_EXE`, `sync_servo`, `sync_cef`, `find_newest`.
- `release.yml:74`-`101` (source map, host table, manifest loop) and the embedded copy and records (`:108`-`112`,
  `:117`-`123`).
- `release_verify.rs`'s two hand-written embedded checks and the asset names `embedded-host` and `embedded-pdf-view`.
- PARKED entries GATE-CEF-DIR, "One table per host" and "Servo's DLLs come from the newest mozangle output in either
  target folder".

## Docs

- README "Runtime-file placement (required after every build)" (`:80`): the sync's new search order, the per-host skip,
  and the CEF error.
- README "Releases" (`:387`-`389`): the gate subcommands, the record names, and the stale `hosts.yml` in the Packaging
  bullet (`:388`; that workflow no longer exists).
- `docs/decisions/2026-10-23-host-runtimes.md:68`-`69`: amended per H2, with a pointer here.
- PARKED.md: the three entries above deleted; the H13 entries and "Weighed and not taken"'s freshness entry added.
- `CLAUDE.md` "Formatting and lints": the clippy command gains `-p manifold-hosts`.
- `CHANGELOG.md`: nothing now; the entry is written at the bump (Internal: the host table; Changed: the record asset names).

## Risks

- **R1. The gate is no longer an independent oracle of the layout.** A wrong row is agreed on by the app and the gate.
  Mitigation: the golden test, the layout tests, and `release_verify`'s file re-hash, which reads `dist/`, not the table.
- **R2. The `release.yml` change runs for the first time in the dry run** (H14). Mitigation: `release-pins` and
  `release-embedded` are unit-tested on temp trees, and `release-embedded` can also run locally against this machine's
  `target/release` WebView2 build into a scratch folder. A wrong manifest fails `release-verify`'s exact-set check and
  `gate package`'s engine states in the same run. The YAML is the last commit.
- **R3. Sync behaviour changes on purpose** (H9): a `CEF_PATH` dev build now fails the sync until its runtime is copied.
- **R4. Asset rename** (H6): published releases keep the old record names; anything outside the repo that fetched
  `embedded-host.sha256` breaks. Nothing in the repo does except `release.yml` and `release_verify.rs`, which change
  together (grep 2026-10-29: only old plans and specs name them otherwise).
- **R5. Manifest key order changes** (Servo, CEF, WebKit, from `ALL`, in place of CEF, Servo, WebKit at
  `release.yml:85`-`87`). Both parsers iterate the object, so order does not matter.
- **R6. The exact-set check in `host_pin::parse` makes a malformed release manifest pin nothing at run time.** A
  release cannot ship one: `release-verify` and `gate package` run the same check and the engine states first.
- **R7. A build script with a dependency**: `build.rs` gains its first workspace build dependency. The leaf has none, so
  the risk is a compile error in the leaf stopping every build, which `cargo test -p manifold-hosts` catches first.

## Weighed and not taken

- **A `RELEASE_OS` constant** (judges' graft): the gate already runs on the release machine, so `Os::here()` there is the
  release's OS; a constant would be a second statement of the same fact.
- **A small bin in the leaf for the Linux job** (removing H10's literals): a CLI in a table crate for two strings. Taken
  if the literals drift.
- **Running the old PowerShell beside the new subcommands in one dry run**: `release-verify`'s exact-set check and
  `gate package` already fail on any manifest difference that matters, so a side-by-side adds no check.
- **Freshness of the Servo and WebKit hosts** (`fresh.rs` checks neither): with the table, a loop over every host built
  in the tree is easy, but a gate run would then fail after any `manifold-pages` or protocol edit until Servo (about 33
  minutes) is rebuilt, though most runs never start a Servo tab. The check belongs per stage, for the engines the run
  uses (`WITH_ENGINES`); parked as "Fresh-host check for opt-in engines".

## Order of work

1. The leaf crate with `Os`, the table, the helpers, the golden and layout tests; `Cargo.lock`; `tier.rs`; the `-p`
   lists.
2. The app reads the table (`ViewDesc`, availability, install, pins with the exact-set check, `cef_platform`, `Os` moved).
3. Embedded codegen (`build.rs`, `embedded_host.rs`, `pdf_view_exe`).
4. `sync_host`.
5. The other gate readers (`fresh`, `package`, fixtures, `sleep`, `all`).
6. `release-pins`, `release-embedded`, the record rename in `release_verify` and `release.yml:144`, then the rest of the
   `release.yml` change, last.
7. Docs and PARKED.

Verification before merging: unit tests for every crate touched; `gate sync-runtime-deps` on this tree before and after,
with the outputs compared; the gate at tier `full` (engine and packaging code); and, when the user pushes, one
`release.yml` dry run.
