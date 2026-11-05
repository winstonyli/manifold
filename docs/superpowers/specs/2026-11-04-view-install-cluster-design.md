---
status: draft
---

# View-host cluster: `view_install/` (LAY4)

Plan item LAY4 of `plans/2026-11-04-restructuring-cleanup.md` (open in `PARKED.md` "Cluster moves still open"). Same recipe as LAY2, LAY3 and LAY6 (`specs/2026-11-04-trail-debug-clusters-design.md`, `specs/2026-11-04-json-cluster-design.md`): a real `parent.rs` + `parent/`, no `#[path]` (d1); no re-exports, callers are edited (p1); tests stay put or move to `foo/tests.rs` (d2). Behavior is unchanged. `cef_pin` and `cef_platform` are not part of this (they go to `manifold-hosts`, CRT5, decided d3e).

## Goal

`view_install.rs` (1204 production lines, over the 1,200 limit and held there only by its `limits.rs` ceiling) becomes a parent plus one file per responsibility, and the two view-host files that exist for installing and running hosts (`host_pin.rs`, `embedded_host.rs`) join the cluster, so the install code, its manifest and the embedded-host extraction sit under one folder.

## Decisions (user, 2026-11-04)

- **o1** Full split of `view_install.rs` into five submodules (below), not a UI-only split.
- **p1** `host_pin.rs` and `embedded_host.rs` move in with their stems: `view_install::host_pin`, `view_install::embedded_host`. The rename to `pin`/`embedded` is parked (`PARKED.md` "view_install vocabulary").
- **q1** `view_install/tests.rs` (1402 lines) stays one file; its imports are edited. Splitting it per submodule is parked in the same entry.

## Target tree

```
view_install.rs             parent: Pin, ViewStatus, NO_PIN_REASON, no_download_reason, Plan (+ impl), downloads,
                            has_runtime, host_platform, host_components, plan_for, InstallError (+ From, Display),
                            remove_path (used by download, unpack and remove), and the one test-only `thread_local!`
                            block holding HASHED and ON_ENTRY, unchanged;
                            pub(crate) mod host_pin; embedded_host; state; download; unpack; remove; ui;
view_install/host_pin.rs    from host_pin.rs        (225 lines)
view_install/embedded_host.rs  from embedded_host.rs (325)
view_install/state.rs       SERVO_VC_RUNTIME, missing_files, missing_system_libs, missing_vc_runtime, RuntimeOnDisk,
                            OnDisk (+ impl), MARKER, Compat, cef_major, runtime_compat, runtime_state, host_marker_path,
                            marker_text, host_state, DirCheck, CEF_DIR_CHECK_TTL, custom_version, validate_custom,
                            newer_than_tested                                                   (~190)
view_install/download.rs    CHUNK, download, IDLE_TIMEOUT, download_with_idle, Fetched, SLOW_BYTES_PER_SEC, fetch, verify,
                            sha256_file, verify_sha256, verify_installed_host                  (~250)
view_install/unpack.rs      target, unpacked_mode, unpack, install, swap, install_host, install_plan  (~230)
view_install/remove.rs      leftovers, host_file_names, host_leftovers, has_leftovers, installed_paths_in, installed_paths,
                            kind_has_leftovers, remove_view_files, dir_size                    (~120)
view_install/ui.rs          ViewInstallUi, InstallJob, `impl crate::App` (view_status and the install verbs)  (~250)
view_install/tests.rs       same file; its `use` lines are rewritten (d2/q1, see "Tests")
```

The groups are item lists, not contiguous line ranges: the parent's and `state`'s items interleave at lines 145-196, `verify_installed_host` (835) sits between unpack and remove items, and `impl From<io::Error> for InstallError` (349) precedes its enum. The cut works per top-level item (Task 2). `remove_path` stays in the parent because three siblings call it (a private parent item is visible to its children, so no visibility change). `host_marker_path` and `marker_text` stay in `state` and get `pub(super)` (used by `unpack` and `remove`). Sizes are from the current line numbers; the executor measures. Every file ends under the 1,200-line production limit, so `view_install.rs` leaves `PROD_ALLOWED` in `crates/gate/src/limits.rs` (the ratchet fails if the entry stays).

## Reach (census 2026-11-04, `git grep`)

- `view_install::<item>` and `use view_install` in 17 files, 38 path lines (one is a string literal in `gate/src/cef_pin.rs:43` that stays correct). Brace-group imports (`process_manager.rs:16-19` spans parent, `state` and `download`; `view_info.rs:416`) are edited by hand, not by the item table: each item's new module is in the table above (`view_install::download` the function becomes `view_install::download::download`; the module and the function share a name in different namespaces, which compiles).
- `host_pin`: `process_manager/availability.rs` (`host_pin::embedded()`), `process_manager/tests.rs` (`crate::host_pin::Component`), `view_install.rs`: 12 path lines in 4 files. `embedded_host`: `process_manager.rs`, `pdf_render.rs`: 6 code lines in 3 files plus 2 comment lines. Plus `main.rs`: the two `mod` lines.
- Comment cites to fix by hand (backtick text, nothing breaks, but they go stale): `process_manager.rs:596` (`install_host` -> `unpack::`), `:938`, `:1163`, `process_manager/availability.rs:28`, `:37`, `settings.rs:154`, `main.rs:359` (`view_install::ViewInstallUi`), README lines 108 and 111 (the `cargo test ... embedded_host` filter at 111 still matches by substring). Also: `build.rs:5` and `:9` (doc comments naming `embedded_host.rs`, `host_pin.rs`), `crates/gate/src/release_hosts.rs:316`, `release_verify.rs:64`, `:188` (comments naming `host_pin::parse`), `view_install.rs` doc comments, `README.md` lines naming either file, `PARKED.md` (the LAY4 entry). Frozen history (devlog, `runs.md`, done plans and specs, `docs/decisions/`) stays as written.
- `build.rs` includes `OUT_DIR/embedded.rs` and `OUT_DIR/host-pins.json` by `OUT_DIR` path: the Rust files' own location does not matter, so the build script is unaffected. Verified by the compile, not assumed: `cargo check` runs `build.rs`.
- Not touched: `view_status` and the other `App` methods keep their names (`ui.rs` holds the same `impl crate::App` block).

## Gate tier consequence (stated, not a change to `tier.rs`)

`view_install` is on `CORE_MODULES`, so every file under `view_install/` is full tier. `host_pin.rs` and `embedded_host.rs` are on no list today (light); after the move they are full. That fits their role (the hash trust anchor and extraction of executables), and `tier.rs` needs no edit (`view_install/tests.rs` is already asserted full by `core_modules_and_their_test_dirs_are_full`). The plan adds the two new paths to that test's list.

## Tests

`view_install/tests.rs` does `use super::*` and relies on the parent's `use` block (`Arc`, `AtomicBool`, `Ordering`, `Duration`, `Sha1`, `Sha256`, `ViewKind`, `CEF`, `to_hex`, `HostFile`, `Os`). The parent keeps only the imports it needs, so `tests.rs` gains explicit `use` lines for those, plus `use super::{state::*, download::*, unpack::*, remove::*, ui::*}` for the items. A glob from a sibling sees only items that are `pub(super)` or wider, so every private item the tests use gets `pub(super)` (for example `cef_major`, `fetch`, `Fetched`, `CHUNK`, `IDLE_TIMEOUT`, `target`, `swap`, `unpacked_mode`, `missing_files`, `marker_text`, `host_marker_path`, `custom_version`, `leftovers`, `host_leftovers`, `host_file_names`, `SERVO_VC_RUNTIME`, `CEF_DIR_CHECK_TTL`, `download_with_idle`, `SLOW_BYTES_PER_SEC`); the compiler lists them. `HASHED` and `ON_ENTRY` stay in the parent's one `thread_local!` block (`tests.rs` reaches them through `use super::*`; `download.rs` and `unpack.rs` import them with `#[cfg(test)] use super::HASHED` and `#[cfg(test)] use super::ON_ENTRY`). The file is edited, so it is not a 100% rename; the 100% claim holds for the two host files in Task 1 only.

## Two moves, two commits

1. Host files move in (`git mv`, paths, cites): compiles on its own.
2. The split of `view_install.rs`: items are grouped by the table; no item's text changes except visibility (`pub(super)` where the compiler asks, and `use` lines). A per-item comparison (Task 2 Step 5) proves nothing else changed.

## Checks

`cargo fmt --all` (twice), `cargo lint -j 8 -- -D warnings`, `cargo check --all-targets`, sorted `-- --list` diff under the prefix map (`^host_pin::` -> `view_install::host_pin::`, `^embedded_host::` -> `view_install::embedded_host::`; the `view_install::tests::` ids keep their path), `git diff --stat -M` showing renames at 100% for the two host files (Task 1), old-path grep, the `limits.rs` ratchet (via `cargo unit`), `cargo unit` through `run.ps1`. Gate tier of this change is `full` (core modules); the full gate runs on request, not by default.

## End-state fit

The install pipeline is one folder: model in the parent, then state, download, unpack, remove and UI as siblings; the host manifest and the embedded-host extraction sit beside it instead of two root stems. Deleted: `view_install.rs`'s 1204-line body, its `PROD_ALLOWED` entry, the root `host_pin` and `embedded_host` module names. No re-exports. The gate's tier rule needs no change.

## Out of scope

`cef_pin`/`cef_platform` (CRT5), the `impl App` bulk question (LAY9 is about `card.rs`/`gpu.rs`), renaming `host_pin`/`embedded_host` (parked p2), splitting `view_install/tests.rs` (parked q2), any behavior change.
