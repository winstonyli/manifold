---
status: draft
---

# View-host cluster Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline; two tasks). Steps use checkbox syntax.

**Goal:** `host_pin.rs` and `embedded_host.rs` move under `view_install/`, and `view_install.rs` splits into a parent plus `state`, `download`, `unpack`, `remove`, `ui`.

**Spec:** `docs/superpowers/specs/2026-11-04-view-install-cluster-design.md` (decisions o1, p1, q1). Parent item LAY4.

## Global Constraints

- Work on `main`; small commits; never push, tag or bump. `cargo fmt --all` before each commit (twice); the pre-commit hook runs fmt and `cargo lint`. Never `--no-verify`.
- No re-exports, no shims: callers are edited. Item text does not change except visibility (`pub(super)` where the compiler asks) and `use` lines.
- Python runs as `python -P file.py < /dev/null`, never on stdin. Cargo uses `-j 8`. No bare `cat` or `python` that reads stdin.
- Test-id check: `node <scratchpad>/testlist.js OUT` (scratchpad `C:/Users/WINSTO~1/AppData/Local/Temp/claude/C--Users-Winston-Li-Documents-GitHub-manifold/a2c355a1-5493-43d2-a2ce-bd6f49ba9245/scratchpad`). Ids carry the module path; the map is `^host_pin::` -> `view_install::host_pin::`, `^embedded_host::` -> `view_install::embedded_host::`. The test count must not change.
- `cargo unit` and any run over a minute go through `~/.claude/runs/run.ps1` (RUNS.md); set the headline in `run.json` and the `docs/runs.md` row.

## End-state fit

See the spec's "End-state fit": one `view_install/` folder with a model parent and five responsibility files, plus the host manifest and embedded-host extraction as siblings; deleted are the 1204-line body, its `limits.rs` ceiling and the two root module names. The parent also keeps the shared types (`Pin`, `ViewStatus`, `Plan`, `InstallError`), so no sibling depends on another sibling's private items beyond `pub(super)` helpers.

## Review Focus

- A cut that splits an item from its doc comment or attribute, or places an item in the wrong module: the per-item comparison in Task 2 Step 5 catches it (a chunk's text changes). `HASHED`/`ON_ENTRY` stay in one parent block so that check holds.
- `view_install/tests.rs` after the split: the parent no longer imports what it used; the plan's explicit `use` lines and `pub(super)` additions are found by the compiler, none by the grep.
- A `pub(super)` the compiler adds that should be `pub(crate)` because a non-sibling (e.g. `process_manager`) needs it: the compile finds it; keep the narrowest visibility that compiles.
- `build.rs` and its `include!` of `OUT_DIR` files after `embedded_host` moves: `cargo check` runs the build script.
- Tier: the two moved host files become full tier; the tier test list gains them.

---

### Task 1: host files move under `view_install/`

**Files:** `crates/manifold/src/{host_pin,embedded_host}.rs` -> `view_install/{host_pin,embedded_host}.rs`; `main.rs` (two `mod` lines); `view_install.rs` (declare the modules, fix paths); `process_manager.rs`, `process_manager/availability.rs`, `process_manager/tests.rs`, `pdf_render.rs`; `crates/gate/src/tier.rs` (test list); doc cites.

- [ ] **Step 1: Baseline.** `git status --short` clean; `node testlist.js lay4.before`; record the count.
- [ ] **Step 2: Move.** `git mv host_pin.rs view_install/host_pin.rs`, same for `embedded_host.rs`; delete `mod embedded_host;` and `mod host_pin;` from `main.rs`; add `pub(crate) mod embedded_host;` and `pub(crate) mod host_pin;` to `view_install.rs` (before `#[cfg(test)] mod tests;`).
- [ ] **Step 2b: Limits.** The two new `mod` lines make `view_install.rs` measure 1206 in `limits.rs` (its tests are `mod tests;`, so the whole file counts): change its `PROD_ALLOWED` entry from 1204 to 1206 in this commit (Task 2 deletes it).
- [ ] **Step 3: Paths.** `perl -pi -e 's/\bcrate::host_pin::/crate::view_install::host_pin::/g; s/\bcrate::embedded_host::/crate::view_install::embedded_host::/g'` over `git grep -l` hits; `use crate::embedded_host;` in `process_manager.rs` becomes `use crate::view_install::embedded_host;`; in `view_install.rs` `use crate::host_pin::Component as HostFile` becomes `use host_pin::Component as HostFile` (inside the parent), and its `crate::host_pin::components_for` becomes `host_pin::components_for`. The two moved files use `super::*` only inside their own inner `mod tests`, which stays correct; the compile settles anything else.
- [ ] **Step 4: Cites.** `build.rs:5,9`; `crates/gate/src/release_hosts.rs:316`, `release_verify.rs:64,188`; `view_install.rs` doc comments; README lines naming either file; `PARKED.md` LAY4 mentions are updated in Task 2 Step 8. Add `src/view_install/host_pin.rs` and `src/view_install/embedded_host.rs` to the path list in `tier.rs` test `core_modules_and_their_test_dirs_are_full`.
- [ ] **Step 5: Verify.** `cargo fmt --all` (twice); `cargo check -j 8 -p manifold -p gate --all-targets`; `node testlist.js lay4.t1`; apply the map to `lay4.before` and `diff` (Expected: empty); `git diff --stat -M HEAD` shows the two renames at 100%; `git grep -nE "\bcrate::(host_pin|embedded_host)\b|^mod (host_pin|embedded_host)"` returns nothing; `cargo test -j 8 -p gate tier` and `cargo test -j 8 -p gate limits`. `embedded_host.rs:67,75,300` call `view_install::sha256_file`, still valid in Task 1.
- [ ] **Step 6: Commit** (after `cargo lint -j 8 -- -D warnings`): `LAY4: host_pin and embedded_host move under view_install/`.

### Task 2: split `view_install.rs`

**Files:** `view_install.rs` -> parent + `view_install/{state,download,unpack,remove,ui}.rs`; `view_install/tests.rs` (imports); callers in 17 files; `crates/gate/src/limits.rs`; docs.

- [ ] **Step 1: Baseline.** `node testlist.js lay4.before2` (equals the Task 1 list); save `view_install.rs` as `$SP/vi.orig` for the check in Step 5.
- [ ] **Step 2: Cut by item.** The groups in the spec's table are item lists, not line ranges. Write a script file (`cut.py`, run as `python -P cut.py < /dev/null`, kept in the scratchpad) that splits `vi.orig` into top-level chunks (a chunk starts at a column-0 line after a blank line and runs to the next such line, so a doc comment, its attributes, the item and a whole `impl` block stay together), names each chunk by its item (`fn`/`struct`/`enum`/`const`/`type`/`impl X for Y`/`thread_local!`), and writes it to the file the spec's table names, in original order; an unassigned or doubly-assigned chunk is an error. The `use` block and `mod` lines are written by hand. Each new file starts with the parent's old `use` block; let `cargo check` and the unused-import warnings prune it. The parent declares `pub(crate) mod host_pin; embedded_host; state; download; remove; ui;` and a private `mod unpack;` (no outside caller uses it; the first two exist since Task 1). Widen a module only if the compiler asks. New files add no `//!` header.
- [ ] **Step 3: Visibility.** Build; for each `E0603`/`E0624` private-item error add the narrowest visibility (`pub(super)` for a sibling or the tests, `pub(crate)` only where a non-sibling needs it). Do not change any item's text otherwise. `HASHED` and `ON_ENTRY` stay in the parent block; `download.rs` and `unpack.rs` import them with `#[cfg(test)] use super::HASHED;` and `#[cfg(test)] use super::ON_ENTRY;`.
- [ ] **Step 4: Callers and tests.** Rewrite single-path `view_install::<item>` uses with a perl table from the spec (items by module: `state::{...}`, `download::{...}`, `unpack::{...}`, `remove::{...}`, `ui::{ViewInstallUi, InstallJob}`). Hand-edit the brace-group imports (`process_manager.rs:16-19`, `view_info.rs:416`) and the `App` field type naming `ViewInstallUi` (`main.rs`). `view_install/tests.rs` keeps `use super::*;` and gains explicit `use` lines for what the parent's old import block provided (for example `Arc`, `AtomicBool`, `Ordering`, `Duration`, `Sha1`, `Sha256`, `Digest`, `Read`, `Write`, `ViewKind`, `CEF`, `to_hex`, `HostFile`, `Os`: the compiler lists them) plus `use super::{state::*, download::*, unpack::*, remove::*, ui::*};`. `embedded_host.rs:67,75,300` (`sha256_file` -> `download::sha256_file`). Update the comment cites listed in the spec's Reach section (including `Cargo.toml:49`, `crates/gate/src/cef_pin.rs:8`, `PARKED.md:135` and `:164`) and `plans/2026-11-04-restructuring-cleanup.md:27` (the LAY4 line of the target tree: it still names `pin, embedded, status, download, unpack`).
- [ ] **Step 5: Prove nothing else changed.** A normalizer script (scratchpad): for `vi.orig` and for the parent plus the five new files, split into top-level chunks as in Step 2, drop chunks that are only `use` or `mod` lines (attribute lines such as `#[cfg(test)]` and `//!` headers count as ignorable when nothing else is in the chunk, and the parent's `#[cfg(test)] mod tests;` tail chunk is dropped from both sides), trim each line, strip `pub(super) `, `pub(crate) ` and `pub ` prefixes, and compare the multisets of chunk texts. Expected: identical, except that multi-line `use` statements are ignored. Run it before the final `cargo fmt` (a `pub(super)` can change line widths); if it differs only after fmt, inspect that chunk by eye. A doc comment moved to the wrong item changes its chunk and fails the check.
- [ ] **Step 6: Limits.** Delete `("manifold/src/view_install.rs", 1204)` from `PROD_ALLOWED` in `crates/gate/src/limits.rs`; every new file must be under 1,200 lines and no inline `mod tests {` block over 249.
- [ ] **Step 7: Verify.** `cargo fmt --all` (twice); `cargo check -j 8 -p manifold -p gate --all-targets` with no new warnings; `node testlist.js lay4.after`; `diff` against `lay4.before2` (Expected: empty, same count); the compile finds every stale path (single-path and brace-group alike); as a spot check, `git grep -nE "view_install::(verify|sha256_file|verify_sha256|install|install_host|install_plan|verify_installed_host|runtime_state|host_state|validate_custom|OnDisk|RuntimeOnDisk|ViewInstallUi|InstallJob)\b" -- crates` returns nothing (only `download` and `unpack` are also module names, so they are left out of the grep).
- [ ] **Step 8: Lint, unit, docs, commit.** `cargo lint -j 8 -- -D warnings`; `cargo fmt --all --check`; `cargo unit -j 8` through `run.ps1` (Expected: all ok, and `limits.rs` passes); record the outcome on LAY4 in the parent plan; in `PARKED.md` remove LAY4 from "Cluster moves still open" (and its `host_pin`/`embedded_host`/`view_install.rs` text), fix the LAY9 note listing `view_install.rs` among files over the limit; grep the docs for the old wording; commit `LAY4: view_install.rs splits into state, download, unpack, remove and ui`.
