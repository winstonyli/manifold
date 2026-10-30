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
  guard, whose one exemption is `desc().name` (`names/tests.rs:470`, `:477`). That exemption has no use left and is
  deleted with its test line (`:495`).
- **H4. PDFium is a fifth row with no `ViewKind`.** It is embedded exactly as WebView2 is, so the embedding code, the
  freshness check and `release_verify` handle both in one loop.
- **H5. Two new gate subcommands replace `release.yml`'s PowerShell host logic.** `gate release-pins` replaces the source
  map and manifest loop (`release.yml:74`-`101`); `gate release-embedded` replaces the embedded copy and hash records
  (`:108`-`112`, `:117`-`123`). The `cargo build` lines and their CRT flags stay in the YAML.
- **H6. The embedded hash records are renamed `<stem>.embedded-<id>.sha256`** (`embedded-webview2`, `embedded-pdfium`;
  were `embedded-host`, `embedded-pdf-view`). Their writer (`release.yml:122`-`123`, replaced by `gate
  release-embedded`), the draft-release upload (`:144`) and the reader (`release_verify.rs:74`, `:81`) change in one
  commit, the last (see "Order of work").
- **H7. `build.rs` generates the embedded-host code from the rows.** The input variable and the blob file are derived
  from the id (`MANIFOLD_WEBVIEW2_HOST`, `webview2-host.bin`; `MANIFOLD_PDFIUM_HOST`, `pdfium-host.bin`: today's names).
  Refinement found while writing: the generated file carries each hash as a literal, so the `MANIFOLD_<ID>_SHA256`
  rustc-env values, whose only reader was `embedded_host.rs:46`-`47`, are deleted rather than derived. This deletes
  `embedded_host::Embedded` and `build.rs`'s `HOSTS` tuple.
- **H8. CEF's runtime marker moves into the row** (`Runtime.lib`: `libcef.dll` on Windows, `libcef.so` elsewhere,
  as `cef_platform.rs:77`-`96` has it). `CefPlatform.runtime_lib` is deleted.
- **H9. One sync rule for every host.**
  - A host's own build folder is searched before `target/`'s (`find_first`), which is PARKED "Servo's DLLs ..."'s fix.
  - A host whose build folder is missing is skipped, not a failure of the run; the run fails only if no host with
    something to sync has its build folder.
  - A part (a sidecar, a runtime) found in no build output is kept if it is already in place (the sidecar beside the
    exe; the runtime folder holding `Runtime.lib`), an error if it is not and the host exe is built, and skipped
    otherwise. No `required` flag.
  - Why "kept in place" matters, verified 2026-10-29 in `cef-dll-sys-152.1.0+152.0.6/build.rs:74`-`104`: the crate
    writes `out/cef_windows_x86_64` only when neither `FLATPAK` nor `CEF_PATH` is set (`:104`,
    `resolve_cef_dir(&out_dir)`); with `CEF_PATH` it resolves the runtime outside `OUT_DIR` (`:80`-`101`). A Windows
    `CEF_PATH` dev build whose `cef-runtime/` already holds the runtime keeps syncing its exe, as today. One whose
    `cef-runtime/` has no runtime gets an error naming `CEF_PATH` and the folder to copy the runtime into; today it warns
    and then fails the exe copy with a bare path-not-found error (`sync_deps.rs:141`, `sync_file`'s `fs::copy` at `:79`
    into a folder only `copy_runtime` creates, `:153`). The repo's own uses of `CEF_PATH` (README `:20`, `:712`) are
    Linux builds, which this sync does not cover.
- **H10. The Linux `webkit-host` job keeps two literals** (`release.yml:25` `-p manifold-view-webkitgtk`, `:29`
  `target/release/manifold-view-webkitgtk`), tied to the WebKit row by a layout test. The gate cannot run there: it does
  not compile on Linux (`crates/gate/src/profile.rs:35` imports `std::os::windows` unconditionally, `main.rs:45` declares
  `mod profile;` without a `cfg`).
- **H11. The exe suffix is decided at run time from the OS a host is built for: `Os::exe_suffix`.** `Os` moves from
  `process_manager/runtime.rs:14`-`62` into the leaf (moved, not copied; callers import it from there). Every `Host`
  method whose answer depends on the OS takes the OS manifold runs on as an argument; none reads `cfg` or calls
  `Os::here()` itself. `CefPlatform.exe_suffix` and `host_exe` are deleted. A host built for another OS keeps that OS's
  suffix, which fixes `view_install.rs:846`.
- **H12. Checks** (detailed under "Tests"): layout tests in the leaf; a test in the app that `ViewKind` maps onto the
  rows; exact-set checks of a manifest against the table in `release_verify` and `host_pin::parse`, through one
  predicate in the leaf; a `tier.rs` Full rule for `crates/manifold-hosts`; and a golden test, so the table is not its
  own only witness.
- **H13. Parked with this design, not done in it:** the app's CEF-only runtime code, the repeated `-p` crate lists, and
  per-host environment variable names spelled in both app and host. They are in PARKED.md, "Host table (2026-10-29)".
- **H14. Release proof needs a push.** The `release.yml` change cannot run locally; its first real run is the workflow's
  manual dry run, which needs the branch pushed. Pushing is the user's call. The dry run does not reach the
  draft-release step (`release.yml:136`, tag runs only), so the upload line (`:144`) first runs on a real tag.

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
`Cargo.lock` with the new crate (`release.yml` builds `--locked`); a golden check. Not taken: a `RELEASE_OS` constant
(see "Weighed and not taken").

## The crate

`crates/manifold-hosts`: package `manifold-hosts`, `version.workspace`, `edition.workspace`, `[lints] workspace = true`,
no `[dependencies]`. Added to the root `members` (`Cargo.toml:3`-`13`) and to `[workspace.dependencies]` beside the other
path crates (`Cargo.toml:23`-`24`). Files: `src/lib.rs` (the table, the helpers and their unit tests) and
`tests/layout.rs` (checks against the repo's files).

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery {
    /// Pinned in the release's manifest and downloaded into `HOSTS_DIR` on demand.
    Download,
    /// Carried inside manifold.exe and extracted to `HOSTS_DIR/<id>/<file>` at first use.
    Embedded,
}

/// Where cargo leaves a file or folder a host needs besides its exe: `<build>/<krate>-*/out/<tail>` or
/// `<build>/<krate>/*/out/<tail>` (both layouts, sync_deps.rs:14-16).
#[derive(Debug)]
pub struct BuildOutput { pub krate: &'static str, pub tail: &'static str }

/// A file that sits beside the host exe when it runs on `os`.
#[derive(Debug)]
pub struct Sidecar { pub file: &'static str, pub os: Os, pub from: BuildOutput }

/// A runtime folder beside the host, holding the runtime and a copy of the host exe.
#[derive(Debug)]
pub struct Runtime {
    pub dir: &'static str,
    /// The file whose presence marks a runtime folder, per OS the host is built for.
    pub lib: fn(Os) -> &'static str,
    /// Where a dev build's runtime comes from (Windows: cef-dll-sys's `cef_windows_x86_64`).
    pub from: BuildOutput,
}

#[derive(Debug)]
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

pub static SERVO: Host;     // servo,    manifold-view-servo,     os None,         dev_dir target-servo, Download,
                            //           sidecars libEGL.dll, libGLESv2.dll (Windows, from mozangle)
pub static CEF: Host;       // cef,      manifold-view-cef,       os None,         Download,
                            //           runtime CEF_RUNTIME_DIR (libcef.dll | libcef.so, from cef-dll-sys
                            //           cef_windows_x86_64)
pub static WEBKIT: Host;    // webkit,   manifold-view-webkitgtk, os Some(Linux),  dev_dir target-linux, Download
pub static WEBVIEW2: Host;  // webview2, manifold-view-webview2,  os Some(Windows), Embedded
pub static PDFIUM: Host;    // pdfium,   manifold-view-pdfium,    os None,         Embedded
pub static ALL: [&Host; 5]; // ViewKind::ALL's order, then PDFium
pub fn by_id(id: &str) -> Option<&'static Host>;   // ignoring ASCII case, as ViewKind::from_name does

impl Host {
    /// The OS this host is built for when manifold runs on `running`.
    pub fn built_for(&self, running: Os) -> Os;                    // os.unwrap_or(running)
    /// The one file-name derivation: `exe` plus the suffix of the OS it is built for.
    pub fn file_name(&self, running: Os) -> String;
    pub fn pin_platform(&self, running: Os) -> &'static str;       // built_for(running).pin_platform()
    /// Beside manifold.exe in `exe_dir`: flat, or inside the runtime folder for a host with one.
    pub fn beside(&self, running: Os, exe_dir: &Path) -> PathBuf;
    /// Where it runs in place from, in order: with `dev_folders`, `<dev_dir>/debug/<file>` in `exe_dir` and its four
    /// parents, nearest first; then `beside(running, exe_dir)`.
    pub fn in_place(&self, running: Os, exe_dir: &Path, dev_folders: bool) -> Vec<PathBuf>;
    /// Where it is installed under `hosts_dir`: flat for Download, `<id>/<file>` for Embedded.
    pub fn installed_in(&self, running: Os, hosts_dir: &Path) -> PathBuf;
    /// `hosts_dir/<runtime.dir>` for a host with a runtime.
    pub fn runtime_in(&self, hosts_dir: &Path) -> Option<PathBuf>;
    /// The runtime's marker file for the OS it is built for, for a host with a runtime.
    pub fn runtime_lib(&self, running: Os) -> Option<&'static str>;
    /// The files a manifest pins for it: its exe, then the sidecars for the OS it is built for.
    pub fn pinned_files(&self, running: Os) -> Vec<String>;
    /// `pinned_files`, plus `<runtime.dir>/<runtime_lib>` for a host with a runtime: what an install leaves on disk.
    pub fn installed_files(&self, running: Os) -> Vec<String>;
    /// The files a manifest must list for this host under the platform string `platform`, or None when it may not
    /// list it there: the host is not `Download`, or `platform` is not `pin_platform(os)` for any `os` in `Os::ALL`.
    /// The one predicate both manifest readers use (H12).
    pub fn manifest_files(&self, platform: &str) -> Option<Vec<String>>;
    /// The build variable that names this host's file for embedding: `MANIFOLD_<ID>_HOST`.
    pub fn embed_var(&self) -> String;
}
```

`manifest_files` finds the `os` in `Os::ALL` whose `pin_platform()` is `platform`, then requires
`self.pin_platform(os) == platform`, so a host built for one OS (WebKit) is accepted only under that OS's platform
(`webkit/windows-x64` is refused), and returns `pinned_files(os)`.

`build.rs` uses only `id`, `delivery` and `embed_var()`, never a method taking an OS: there `Os::here()` would be the
build machine, not the target. Rows are compared by `id` (`Host` holds a `fn`, so it has no `PartialEq`).

## The app

- **`ViewDesc`** (`process_manager.rs:55`-`102`): `host: &'static Host` replaces the six fields (H2). Each row
  (`:121`-`147`, `:148`-`171`, `:172`-`203`, `:204`-`231`) names `host: &manifold_hosts::SERVO` and so on. The kept
  fields are `code`, `label`, `tags`, `link`, `starting_default`, `missing_system_libs`, `download_note`, `prepare`,
  `color`, `tier`, `pros`, `cons`, `picker_rank`. `HostSource` (`:36`-`46`) and `CEF_RUNTIME_DIR` (`:48`-`49`) are
  deleted, and the field doc that names it (`:71`) goes with its field.
- **Identity:** `ViewKind::from_name` (`:238`) and the serializer (`:256`) read `desc().host.id`, as do
  `debug_control.rs:452`, `debug_export.rs:420`, `view_install.rs:251`, `:860` and `session_sync/tests/misc.rs:201`-`204`.
  `host_pin.rs:12`-`13`'s doc comment names the id in place of `ViewKind::desc().name`.
- **`Os`** moves to the leaf (H11). `runtime.rs` keeps `HostRuntime`, `HostLink`, `WSL_DISTRO` and
  `unavailable_reason`, importing `Os` from the leaf; `HostRuntime::for_os` (`:89`-`94`) and the WSL-speaks-TCP test
  (`:229`-`239`) read `desc().host.os`. The imports that name `runtime::Os` change to the leaf's: `process_manager.rs:1203`
  (`use runtime::{HostLink, HostRuntime, Os}`), `view_install.rs:14`, and the two full paths in `settings.rs:189` and
  `:392`. Its tests (`runtime.rs:198`-`208`) move with it (see "Tests").
- **In-place lookup** (`process_manager/availability.rs`): `run_in_place` and `run_in_place_from` take `&Host`, and read
  `host.in_place(Os::here(), exe_dir, dev_folders)`. `sibling_binary` (`:8`), `host_file_name` (`:16`), `beside_exe`
  (`:30`) and `in_place_paths` (`:42`) are deleted, with their names in the import lists at `process_manager.rs:1180`-
  `1186` (the `cfg(test)` list and the plain one). `view_binary_path` (`:84`) ends in `host.installed_in(Os::here(),
  dir)` (its `HostSource::Embedded` branch, `:91`-`94`, is that same call); `view_hosts_dir` (`:99`) joins `HOSTS_DIR`;
  `cef_sibling_exe` (`:107`) is `CEF.beside(Os::here(), exe_dir)`; `sibling_host` (`:115`) tries
  `exe_dir.join(file_name(Os::here()))`, then `beside`, for a host with a runtime; `pick_webkit_host` (`:131`) falls
  back to `WEBKIT.installed_in(Os::here(), dir)`; `view_runtime_dir` (`:144`) is `host.runtime_in(dir)`.
  `walks_dev_folders` and its rule (a release never looks in a dev build folder) and `run_in_place`'s `None` under
  `cfg(test)` are unchanged.
- **`spawn_tab`'s custom-folder filter** (`process_manager.rs:1054`) reads `desc.host.runtime.is_some()`.
- **`shipped_cef_host`** (`process_manager.rs:641`-`648`) falls back to `CEF.installed_in(Os::here(), dir)` in place of
  formatting the file name (`:644`).
- **Embedded hosts in place or extracted.** `prepare_webview2` (`:1011`-`1019`) matches `kind.desc().source` against
  `HostSource::Embedded(host)` today; it calls `embedded_host::ensure(kind.desc().host, &dir)` when `sibling_host` finds
  nothing. `pdf_view_exe` (`:1212`-`1221`) does the same for PDFium: `run_in_place(&PDFIUM)`, else
  `embedded_host::ensure(&PDFIUM, &dir)`. PDFium has no dev folder, so its lookup is "beside `manifold.exe`", as today.
  `pdf_render.rs:140`'s "build manifold-view-pdfium" hint reads `PDFIUM.exe`.
- **Install and pins** (`view_install.rs`): `downloads` (`:134`) is `host.delivery == Delivery::Download`;
  `has_runtime` (`:139`) is `host.runtime.is_some()` (their doc comments, `:132`, `:138`, name the row's fields);
  `host_platform` (`:170`) is `host.pin_platform(Os::here())`; the `CEF_RUNTIME_DIR` import (`:15`) and uses (`:719`-
  `721`, `:839`, `:886`, `:921`) take the leaf's constant; `host_file_names` (`:846`) uses `file_name(Os::here())`, so
  WebKit's candidates on Windows hold no `.exe` name; `validate_custom` (`:326`), `settings_page.rs:472` and
  `view_install/tests.rs:155` read `CEF.runtime_lib(Os::here())`.
- **`cef_platform.rs`** loses `runtime_lib` (`:12`), `exe_suffix` (`:14`), `host_exe` (`:28`-`31`) and their test
  (`:119`-`122`). It keeps the CEF archive facts: `id`, `runtime_dirs`, `skip_*`, `system_libs`.
- **`host_pin::parse`** (`host_pin.rs:88`) checks each `(id, platform)` entry: `by_id(id)` names a host and
  `host.manifest_files(platform)` is `Some` and equal, as a set, to the entry's files. Anything else refuses the whole
  manifest, the existing policy for a bad manifest (`:86`-`87`). Its tests' file names come from the table.

### Embedded hosts

`build.rs` loops `manifold_hosts::ALL` for `Delivery::Embedded` rows. For each, it reads `host.embed_var()` (emitting
`cargo:rerun-if-env-changed` for it, as it does for today's names), writes `OUT_DIR/<id>-host.bin` (the file's bytes,
or empty), and appends one entry to a generated `OUT_DIR/embedded.rs`:

```rust
// generated by build.rs: one entry per Embedded row, in ALL's order
static EMBEDDED: &[(&str, &[u8], &str)] = &[
    ("webview2", include_bytes!(concat!(env!("OUT_DIR"), "/webview2-host.bin")), "<sha256, or empty>"),
    ("pdfium", include_bytes!(concat!(env!("OUT_DIR"), "/pdfium-host.bin")), "<sha256, or empty>"),
];
```

A host is embedded only when its variable names a file and the target is Windows, as today (`build.rs:20`, `:23`): the
extraction sets no execute bit, so the embed is a Windows mechanism, not a property of any one host. `build.rs` keeps
its `sha2` build dependency to hash the blobs.

`embedded_host.rs` `include!`s that file and reads it through one function, `embedded(host: &Host) -> (&'static [u8],
&'static str)`: the bytes and hash of the row's entry, or `(&[], "")` for a host with no entry or an empty one.
`Embedded` and its `folder`, `file`, `label`, `bytes`, `host_path` (`:15`-`56`) and the two `include_bytes!` statics
(`:21`-`22`) are deleted. `ensure(host, hosts_dir)` returns `Ok(None)` when `embedded(host)` is empty, as an empty embed
does today, and otherwise extracts to `host.installed_in(Os::here(), hosts_dir)`; `ensure_with` takes `&Host` in place of
`Embedded`. Error messages name the host's file instead of the deleted labels ("could not write
manifold-view-pdfium.exe ..." in place of "the PDF view").

## The gate

### `gate sync-runtime-deps` (`sync_deps.rs`)

`sync_servo` (`:111`-`129`) and `sync_cef` (`:131`-`147`) become one function:

```rust
fn sync_host(root: &Path, host: &Host, outcome: &mut Outcome) {
    // nothing to sync (no sidecar for Os::here(), no runtime): return, printing nothing
    // folder = root/<dev_dir or "target">/debug; missing: one skip line, return
    // exe_there = folder/<host.file_name(Os::here())> is a file
    // roots = [folder/build, root/target/debug/build] (one root when dev_dir is None)
    // each sidecar for Os::here(), labelled "<dev_dir or target>/debug/<file>":
    //     find_first(roots, &sidecar.from) -> sync_file beside the exe
    //     else folder/<file> exists -> one line, kept in place
    //     else exe_there -> error naming the file and the build to run; else one skip line
    // runtime, labelled "<runtime.dir>/<name>":
    //     find_first(roots, &runtime.from) -> copy_runtime into folder/<runtime.dir>/
    //     else folder/<runtime.dir>/<runtime_lib(Os::here())> exists -> one line, kept in place
    //     else exe_there -> error naming CEF_PATH as the likely cause and the folder to fill; else one skip line
    //     then the exe into folder/<runtime.dir>/, always compared (it is rebuilt with its crate)
}
```

- `run` (`:176`-`187`) becomes `run_in(root)` over `manifold_hosts::ALL`, with `run` calling it on the repo root. A
  host with nothing to sync (WebKit, WebView2, PDFium) prints nothing. The run fails only if no host with something to
  sync has its build folder ("build first"), replacing today's hard failure when `target/debug` alone is missing
  (`:178`-`181`).
- `find_first(roots, from)` replaces `find_newest` (`:36`-`53`): the newest match in the first root that has one. Within
  a root both cargo layouts are searched, as today.
- `copy_runtime` takes its label prefix from `runtime.dir` (today `cef-runtime/{name}`, `:157`).
- `SERVO_DLLS`, `SERVO_EXE`, `sync_servo`, `sync_cef` and `run`'s literal paths are deleted. `copy_dir`, `sync_file`,
  `Outcome`, `step_error` and `finish` stay.

What changes for this machine's tree: Servo's DLLs come from `target-servo/debug/build/mozangle/` instead of whichever of
the two trees built last (both hold the same files on 2026-10-29); nothing else.

### `gate release-pins vX.Y.Z DIST FOREIGN` and `gate release-embedded vX.Y.Z DIST`

New stages in `STAGES` (`main.rs:138`-`189`), in a new `cfg(windows)` module `release_hosts.rs` beside
`release_verify.rs`. The release's own platform is `Os::here()`: the gate runs on the release runner.

- **`release-pins`** creates `DIST` if missing and loops the `Download` rows. A host built for `Os::here()` comes from
  `target/release/<file>` and its sidecars from `find_first([target/release/build], ...)` (the sync's own search;
  `release.yml:79`-`81` searched recursively for a path containing `mozangle`, which on today's layout finds the same
  `build/mozangle/<hash>/out/` files). A host built for another OS comes from `FOREIGN/<file>` (the YAML's download of
  the Linux job's artifact, `webkit-host/`). Each file is copied to `DIST/<file>-<platform>` and hashed with the gate's
  `sha2`, and the manifest `DIST/manifold-<TAG>-<platform>.host-pins.json` is written with `serde_json`
  (`preserve_order`, `crates/gate/Cargo.toml:11`). A missing file is an error naming it and where it was looked for. The
  manifest's text goes to stderr, as `release.yml:101`'s `Get-Content` put it in the log; on success stdout holds one
  line, `MANIFOLD_HOST_PINS=<path>`.
- **`release-embedded`** creates `DIST/embedded` if missing and loops the `Embedded` rows: copies
  `target/release/<file>` to `DIST/embedded/<file>`, writes `DIST/manifold-<TAG>-<platform>.embedded-<id>.sha256`
  (`<hash>  <file>`, the format `release.yml:117`-`120` writes), and prints `<embed_var>=<path>` per host.
- Printed paths are made absolute with `std::path::absolute`, not `canonicalize`, so they carry no `\\?\` prefix, like
  the `Resolve-Path` values they replace (`release.yml:107`, `:111`-`112`).
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
`$GITHUB_ENV` (`:48`), and a native exe piped into a cmdlet with `$LASTEXITCODE` checked after is how it already runs
`gate release-notes` (`:141`-`142`); the job's shell is `pwsh` (`:36`). `target/debug/gate.exe` is built at `:57`. The
draft-release step (`:144`) lists the records by pattern, `(Get-ChildItem dist -File -Filter
"$stem.embedded-*.sha256" | ForEach-Object FullName)`, instead of by name. The Servo step (`:63`-`67`) and the package
and verify steps (`:124`-`130`) are unchanged.

### Other gate readers

- **`release_verify.rs`**: the manifest path, the exe name and the record names come from `Os::here().pin_platform()`
  and `embedded_record` (`:57`, `:65`, `:70`-`83`); the two embedded checks become one loop over the `Embedded` rows.
  New: a pure `manifest_gaps(entries, running) -> Vec<String>` lists every `(id, platform, file)` the manifest has and
  the table does not, and the reverse, for the `Download` rows under `pin_platform(running)` (through
  `manifest_files`), and `verify` makes one `check` of it being empty. So a host or sidecar left out of the release, an
  extra file, or a host under the wrong platform fails the verify. The re-hash of each file against the manifest
  (`:58`-`63`) is unchanged and reads only files.
- **`package.rs`**: `installed_files` (`:131`-`137`) is `host.installed_files(Os::here())` for a `Download` host built for
  `Os::here()`. The engines it checks (`:208`, `:219`) and `install-check`'s messages (`:110`, `:114`) come from the same
  filter, which yields CEF and Servo: WebKit is pinned but runs only through WSL, which the release runner and a bare
  `gate package` run do not have. `install-check`'s usage string (`main.rs:185`) stays a literal, `cef|servo`, because
  `STAGES` is a `const` table of `&'static str`s (`main.rs:126`, `:138`); a gate test checks it equals the filter's ids
  joined by `|`. `pins` (`:46`-`49`) and `newest_release` (`:31`-`43`) use `Os::here().pin_platform()`. The
  extracted-host checks (`:226`-`240`) use `installed_in(Os::here(), hosts)` for each `Embedded` row.
- **`fresh.rs`**: the CEF copy check (`:54`-`66`) becomes "for each host with a runtime: the built exe is not newer than
  its copy in the runtime folder". The dep-info list (`:69`) names `[&WEBVIEW2, &PDFIUM]` by row (the hosts the gate's
  build line builds; one list of those is PARKED "The `-p` crate lists are repeated"), and the hint at `:87` is built
  from the same list. The test fixture's names (`:114`) come from the same rows.
- **Fixtures and probes**: `jar.rs:481`, `:744`; `probes.rs:560`-`561`; `shots.rs:75` (`FAKE_CEF_HOST`), `:1590`,
  `:1630`, `:1632`, `:1652`-`1653`, `:1704`; `stress.rs:40`; `package.rs:168`, `:226` read `file_name(Os::here())`,
  `CEF_RUNTIME_DIR`, `HOSTS_DIR` or `runtime_lib(Os::here())`. Test literals that pin an old value on purpose stay
  (`fresh.rs:153`-`166`).
- **`sleep.rs:87`**: `kill_hosts` takes the host's `file_name(Os::here())` from `by_id(view)` instead of formatting
  `manifold-view-{view}.exe`, which names no host for WebKit. WebKit's host still names no Windows process (manifold's
  child there is the `wsl.exe` relay), so the sleep stage's crash case kills nothing for WebKit; said plainly, not fixed
  here.
- **`all.rs:318`-`326`**: the `SITE_HOST` case is keyed on the step's host being built for another OS than the gate's
  (`by_id(engine).built_for(Os::here()) != Os::here()`), not on the string `"webkit"`: such a host runs in a VM, which
  reaches the page server at another address.

## Tests

- **Golden table** (leaf, unit tests in `src/lib.rs`): for each row, the derived values written out as literals for
  `Os::Windows` and for `Os::Linux`, in one test binary on either OS: `built_for`, `file_name`, `pin_platform`,
  `pinned_files`, `installed_files`, `runtime_lib`, `installed_in` under a fixed folder, `in_place` for a fixed
  `exe_dir`, `embed_var`. Expected paths are built from literal components with `Path::join`, so they compare equal on
  both OSes. An edit to a row then changes a test by hand, so the app and the gate no longer agree on a typo silently. It
  includes `WEBKIT.file_name(Os::Windows) == "manifold-view-webkitgtk"` (the H11 fix) and `built_for(Os::Windows)` per
  row (only WebKit is built for another OS than Windows, the `SITE_HOST` predicate). `manifest_files` cases:
  `cef`/`windows-x64` and `webkit`/`linux-x64` give their files; `webkit`/`windows-x64`, an unknown platform and an
  `Embedded` row give `None`.
- **`Os`** (leaf): `runtime.rs:198`-`208` (`from_consts` and `pin_platform`) moves into the leaf, extended with
  `exe_suffix`, `name` and every `pin_platform`. The golden `file_name` literals replace `cef_platform.rs:119`-`122`.
- **Layout** (leaf, `tests/layout.rs`, reading repo files from `CARGO_MANIFEST_DIR/../..` with `std::fs` and substring
  matching only; the leaf has no TOML, YAML or regex parser):
  - every row's `crates/<exe>/Cargo.toml` has `name = "<exe>"` under `[package]`, and under any `[[bin]]` too;
  - every `dev_dir` has a `/<dev_dir>` line in `.gitignore`;
  - every row's `exe` follows a `-p ` in `release.yml`, and every row built for Linux has `target/release/<file>` there
    (H10);
  - the `gh release create` line's `-Match '-(…)-x64$'` alternation names the OS part of `pin_platform(Os::Windows)`
    for every `Download` row (`windows`, `linux`): the platforms the release, built on Windows, pins;
  - no `Cargo.toml` under `crates/` other than the leaf's own, `manifold`'s and `gate`'s names `manifold-hosts` in a
    dependency table.
- **App**: `ViewKind::ALL` maps onto distinct rows, and the only row no `ViewKind` reaches is `PDFIUM`; the existing
  store-name test (`session_sync/tests/misc.rs:201`) keeps the ids equal to `store::model::VIEW_KINDS`.
  - The in-place tests (`process_manager/tests.rs:553`-`594`, `:599`-`613`, `:619`-`636`) are rewritten over
    `run_in_place_from(&Host, ..)`, `host.dev_dir`, `file_name(Os::here())`, `CEF.beside` and `HOSTS_DIR` (for the
    `engine-hosts` paths at `:562`, `:627`), with every assertion kept: dev folder before beside, CEF only from
    `cef-runtime/`, four parents and no further, a pinned build walks no dev folder, WebKit's fallback to
    `engine-hosts`. They gain a PDFium case: found beside the exe, not found without it.
  - `only_cef_has_a_downloaded_runtime_dir` (`:1033`) reads `host.runtime`; `:1091` uses
    `WEBVIEW2.installed_in(Os::here(), hosts)`.
  - `view_install`: `host_file_names` for WebKit holds no `.exe` name.
  - `embedded_host`: `an_empty_embed_returns_none_and_touches_nothing` (`:179`-`191`) and
    `the_embedded_hosts_extract_to_their_pinned_hashes` (`:308`-`323`) loop over the `Embedded` rows and read
    `embedded(host)` in place of `bytes()`, and `installed_in` in place of `host_path`; `files` (`:172`-`177`) likewise.
    `two_embedded_hosts_extract_side_by_side` (`:325`-`334`) compares with `PDFIUM.installed_in(Os::here(), &dir)` in
    place of the literal `pdfium/manifold-view-pdfium.exe`, which would fail on the Linux CI job. New: the generated
    table's ids equal the `Embedded` rows' ids, in order.
  - Names guard: `names/tests.rs:470`, `:477` and `:495` (the `desc().name` exemption) are deleted (H3).
- **Manifests**: `host_pin::parse` refuses a manifest whose file set for a host differs from the table (Servo without
  `libEGL.dll`), names a host that is not downloaded, names an unknown platform, or puts a host under another OS's
  platform (`webkit`/`windows-x64`), and accepts `cef`/`windows-x64` with `webkit`/`linux-x64`. `manifest_gaps` is tested
  directly: a missing host, a missing sidecar, an extra file, a host under the wrong platform.
- **Gate**:
  - `sync_host` and `run_in` on temp trees: both cargo output layouts in one root; the newest within a root; the host's
    own folder before `target/`; a missing build folder skipped with one line; a host with nothing to sync silent; a
    missing part an error only with the exe built and the part not in place; a part in place kept; the exe copied into
    the runtime folder only for a host with a runtime; the labels `<dev_dir or target>/debug/<file>` and
    `<runtime.dir>/<name>` (pinned today at `sync_deps.rs:278`, `:309`); `run_in` failing when no build folder exists.
    `a_held_file_says_to_close_manifold` (`:330`) stays.
  - `release-pins` and `release-embedded` on temp trees that start without `DIST`: files copied and named
    `<file>-<platform>`, manifest keys and sizes, a missing file an error naming it, a foreign host taken from
    `FOREIGN`, records in the `<hash>  <file>` format, the printed `NAME=VALUE` lines with absolute paths and no `\\?\`.
  - `fresh.rs`: a runtime copy older than the built exe is named with `gate sync-runtime-deps`; an absent copy is
    skipped.
  - The `install-check` usage literal equals the filter's ids.
- **Tier**: `tier.rs` classifies `crates/manifold-hosts/**` as Full (today `:151` would send `src/lib.rs` to Light as a
  "path on no list"); its case beside `:281` uses `crates/manifold-hosts/src/lib.rs` and `tests/layout.rs`, since
  `:107` already sends every `crates/*/Cargo.toml` to Full.

The leaf's own tests run in CI (`ci.yml:17`, `:24`) and with `cargo test -p manifold-hosts`; the gate's unit stage
(`all.rs:287`-`303`) runs only at tier `ui`, which a leaf change never is, so it does not list the leaf.

## Tooling

- `-p manifold-hosts` joins `ci.yml:16` (clippy), `:17` and `:24` (tests; it builds on Linux), `.githooks/pre-commit:7`
  and the clippy command in CLAUDE.md "Formatting and lints".
- `Cargo.lock` is committed with every commit that adds the crate or a dependency on it (`manifold`, `gate`):
  `release.yml` and `ci.yml` build `--locked`, and the pre-commit hook does not.
- `tier.rs`: the Full rule above.

## Not covered

Each stays outside the table; where it duplicates a row, a test ties it.

- Cargo package and bin names and the `crates/<exe>/` folders: TOML cannot read Rust. Tied by the layout test.
- `.gitignore`'s dev folders and the `CARGO_TARGET_DIR=target-servo|target-linux` convention (README "Building Servo on
  its own"): tied for `.gitignore`; nothing can make a build command use the folder.
- The CRT flags per build step and the Linux job's two literals (H10): build policy and a job the gate cannot run in.
- The `windows-x64` stem in the YAML (`release.yml:115`, `:126`, `:141`) and its asset pattern (`:144`): the release
  job's own platform. The gate derives the same stem from `Os::here()`, so a mismatch fails `release-verify` in the run;
  the pattern is tied by the layout test.
- `install-check`'s usage literal (`main.rs:185`): a `const` table entry, tied by a gate test.
- The protocol crate's render-mismatch hint, which names the `manifold-view-pdfium` package (`render.rs:51`, `:239`,
  and its tests `tests/render_child.rs:50`, `:65`): the protocol crate does not depend on the leaf (H1).
- `ViewDesc`'s app facts (colours, tags, fn pointers, copy), `HostLink`, `ViewKind` identity and the default-engine
  policy: app facts, not layout.
- `cef_platform`'s archive facts, `SERVO_VC_RUNTIME` (`view_install.rs:145`) and `WSL_DISTRO` (`runtime.rs:12`): the
  download archive and the system, not a host's layout.
- The two manifest parsers (`host_pin.rs:88`, `release_verify.rs:34`): one wire format under two error policies; both
  call `manifest_files`.
- The profile prefix `manifold/data` in the gate (`package.rs:168`, `:226`, `shots.rs:1590`): the app's
  `profile::view_data_dir`; only the `engine-hosts` component comes from the table.
- `package.rs:26`'s PDF fixture path (`crates/manifold-view-pdfium/tests/fixture.pdf`): a test input, not layout.
- Linux and macOS CEF dev syncing: `Runtime.from`'s tail is the Windows one, the only OS the gate runs on.
- About 45 `"webview2"` default-engine strings in gate scenarios: scenario input, not layout.
- No file or folder in a profile changes name, so no profile or session is affected.

## Deleted

- `HostSource`; `ViewDesc.name`, `.binary`, `.source`, `.host_os`, `.runtime_dir`, `.dev_dir`;
  `process_manager::CEF_RUNTIME_DIR`.
- `runtime::Os` (moved, with its tests).
- `CefPlatform.runtime_lib`, `.exe_suffix`, `host_exe` and its test.
- `availability::{sibling_binary, host_file_name, beside_exe, in_place_paths}` and the `"engine-hosts"` literal.
- `embedded_host::Embedded` and its methods, the two `include_bytes!` statics, `build.rs`'s `HOSTS` tuple and the
  `MANIFOLD_<ID>_SHA256` rustc-env values.
- `sync_deps.rs`'s `SERVO_DLLS`, `SERVO_EXE`, `sync_servo`, `sync_cef`, `find_newest`.
- `release.yml:74`-`101` (directory creation, source map, host table, manifest loop and its echo) and the embedded copy
  and records (`:108`-`112`, `:117`-`123`).
- `release_verify.rs`'s two hand-written embedded checks and the asset names `embedded-host` and `embedded-pdf-view`.
- The names guard's `desc().name` exemption (`names/tests.rs:470`, `:477`, `:495`).
- PARKED entries GATE-CEF-DIR, "One table per host" and "Servo's DLLs come from the newest mozangle output in either
  target folder".

## Docs

- README "Runtime-file placement (required after every build)" (`:80`): the sync's search order, the per-host skip, a
  part kept in place, and the error for a CEF build with no runtime.
- README "Releases" (`:387`-`390`): the gate subcommands, the record names (`:389`), the variables `gate
  release-embedded` prints and the workflow forwards (`:390`, which says `release.yml` sets them), and the stale
  `hosts.yml` in the Packaging bullet (`:388`; that workflow no longer exists).
- `docs/decisions/2026-10-23-host-runtimes.md:68`-`69`: amended per H2, with a pointer here.
- PARKED.md: the three entries above deleted. Its "Host table (2026-10-29)" section (H13 and "Weighed and not taken"'s
  freshness entry) is already in.
- `CLAUDE.md` "Formatting and lints": the clippy command gains `-p manifold-hosts`.
- Code comments that name a deleted item change with the code (`process_manager.rs:71`, `host_pin.rs:12`-`13`,
  `view_install.rs:132`, `:138`, `:318`).
- `CHANGELOG.md`: nothing now; the entry is written at the bump (Internal: the host table; Changed: the record asset
  names).

## Risks

- **R1. The gate is no longer an independent oracle of the layout.** A wrong row is agreed on by the app and the gate.
  Mitigation: the golden test, the layout tests, and `release_verify`'s file re-hash, which reads `dist/`, not the table.
- **R2. The `release.yml` change runs for the first time in the dry run** (H14), and the draft-release line (`:144`)
  only on a real tag. Mitigation: `release-pins` and `release-embedded` are unit-tested on temp trees, and
  `release-embedded` can also run locally against this machine's `target/release` WebView2 build into a scratch folder.
  A wrong manifest fails `release-verify`'s exact-set check and `gate package`'s engine states in the same run. The
  `:144` pattern is tied by the layout test. The YAML change and the `release_verify` rename are the last commit.
- **R3. Sync behaviour changes on purpose** (H9): a missing build folder skips that host only; a part found in no
  build output but in place is kept; a CEF build with neither gets a clearer error than today's path-not-found.
- **R4. Asset rename** (H6): published releases keep the old record names; anything outside the repo that fetched
  `embedded-host.sha256` breaks. In the repo, `release.yml`, `release_verify.rs` and README `:389` name them, and all
  three change together (grep 2026-10-29: otherwise only old plans and specs).
- **R5. Manifest key order changes** (Servo, CEF, WebKit, from `ALL`, in place of CEF, Servo, WebKit at
  `release.yml:85`-`87`). Both parsers iterate the object, so order does not matter.
- **R6. The exact-set check in `host_pin::parse` makes a malformed release manifest pin nothing at run time.** A
  release cannot ship one: `release-verify` runs the same predicate, and `gate package` the engine states, first.
- **R7. `build.rs` gains its first path-crate build dependency** (beside `sha2`). A compile error in the leaf then stops
  every build of the app, which `cargo test -p manifold-hosts` catches first.

## Weighed and not taken

- **A `RELEASE_OS` constant** (judges' graft): the gate already runs on the release machine, so `Os::here()` there is the
  release's OS; a constant would be a second statement of the same fact.
- **A small bin in the leaf for the Linux job** (removing H10's literals): a CLI in a table crate for two strings. Taken
  if the literals drift.
- **A `gate release-assets` listing the upload set** (removing `:144`'s pattern and stem): the pattern is one line tied
  by a test, and the upload set is otherwise `release_verify`'s; taken if the pattern drifts.
- **Running the old PowerShell beside the new subcommands in one dry run**: `release-verify`'s exact-set check and
  `gate package` already fail on any manifest difference that matters, so a side-by-side adds no check.
- **Freshness of the Servo and WebKit hosts** (`fresh.rs` checks neither): with the table, a loop over every host built
  in the tree is easy, but a gate run would then fail after any `manifold-pages` or protocol edit until Servo (about 33
  minutes) is rebuilt, though most runs never start a Servo tab. The check belongs per stage, for the engines the run
  uses (`WITH_ENGINES`); parked as "Fresh-host check for opt-in engines".

## Order of work

Each step leaves the tree building with its tests green, so each can be committed alone.

1. The leaf crate with `Os`, the table, the helpers, the golden, `Os` and layout tests; `Cargo.lock`; `tier.rs`; the
   `-p` lists.
2. The app reads the table, in one step because `HostSource`, `Embedded` and `sibling_binary` have users on both sides:
   `ViewDesc`, `Os` moved, availability, install, pins with the exact-set check, `cef_platform`, the embedded codegen
   (`build.rs`, `embedded_host.rs`), `prepare_webview2`, `pdf_view_exe`, the names guard; `Cargo.lock`.
3. `sync_host` and `run_in`; `Cargo.lock` (the gate's dependency).
4. The other gate readers (`fresh`, `package`, fixtures, `sleep`, `all`).
5. `release-pins` and `release-embedded` with `embedded_record` and their tests, called by nothing yet.
6. Last, one commit: `release_verify` (the record names, the one embedded loop, `manifest_gaps`) and the whole
   `release.yml` change, writer, upload and reader of the renamed records together (H6), with README "Releases".
7. The remaining docs and the PARKED deletions.

Verification before merging: `cargo test` for every crate touched, `-p manifold-hosts` included; `gate
sync-runtime-deps` on this tree before and after, with the outputs compared; the gate at tier `full` (engine and
packaging code); and, when the user pushes, one `release.yml` dry run.
