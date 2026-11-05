---
status: done
---

# Trail and debug clusters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline; the tasks share files, so no per-task subagents). Steps use checkbox syntax.

**Goal:** `nav_history`, `trail_nav`, `trail_strip` become `trail/{history,nav,strip}.rs`; `debug_control`, `debug_export`, `event_loop/debug_input` become `debug.rs` + `debug/{verbs,reader,export,input}.rs`; their 5 big test blocks move to `tests.rs`. No behavior change.

**Spec:** `docs/superpowers/specs/2026-11-04-trail-debug-clusters-design.md` (approved 2026-11-04; p1, q3, r1). Parent plan items LAY2, LAY6: `docs/superpowers/plans/2026-11-04-restructuring-cleanup.md`.

## Global Constraints

- Work on `main`; small commits; never push, tag or bump. `cargo fmt --all` before each commit; the pre-commit hook runs fmt and `cargo lint`. Never `--no-verify`.
- Moves are `git mv` first and committed alone (renames at 100%), edits after.
- No re-exports, no shims: callers are edited to the new paths.
- The `App.nav_history` field and `nav_history` locals are data, not the module: leave them.
- Python runs as `python -P file.py < /dev/null`, never on stdin. Cargo uses `-j 8`.
- Test-id check: `node <scratchpad>/testlist.js OUT` writes the sorted list of all manifold and gate tests; map the prefixes below (anchored at line start with `^`, since ids also contain `session_sync::tests::trail::`), then `diff` must be empty. Take the BEFORE list at the start of each task from the previous commit's tree (rebuild if the tree changed).

Prefix map LAY2: `nav_history::` -> `trail::history::`, `trail_nav::` -> `trail::nav::`, `trail_strip::` -> `trail::strip::`. LAY6: `debug_control::` -> `debug::`, `debug_export::` -> `debug::export::`, `event_loop::debug_input::` -> `debug::input::`.

## Review Focus

- A missed `use super::*` after a move: compile error, not silent. Covered by `cargo check --all-targets`.
- A moved doc cite that still names the old file (silent): the old-name grep in each task's last step.
- A lost `VERBS` row (LAY6): `every_verb_has_a_parsing_sample` plus the `gate two` smoke run on request.
- The scanner `main/tests.rs:111` reads `debug_control.rs` by path: LAY6 is gated on ARC1 (r1).

---

### Task 1: LAY2 trail cluster

**Files:** move `crates/manifold/src/{nav_history,trail_nav,trail_strip}.rs` to `trail/{history,nav,strip}.rs`; create `trail.rs`; edit the sites in the spec census; tests to `trail/{history,nav,strip}/tests.rs`.

- [x] **Step 1: Baseline.** `git status --short` shows only `docs/runs.md`; `node testlist.js BEFORE` (2029+ tests).
- [x] **Step 2: Move.** `mkdir crates/manifold/src/trail`; `git mv` the three files; create `trail.rs` with `mod history; mod nav; mod strip;` (visibility `pub(crate)` on each, since callers outside use `crate::trail::history::X`). In `main.rs` replace `mod nav_history;` with `mod trail;` and delete the `mod trail_nav;` and `mod trail_strip;` lines. The tree does not compile until Step 3, so Steps 2 and 3 are one commit (the hook checks it); `git diff -M --stat` must still show the renames at 100%-ish before the edits are staged (check it before Step 3, not after).
- [x] **Step 3: Paths.** `perl -pi -e` over the files from `git grep -l "nav_history\|trail_strip\|trail_nav" -- crates`, excluding docs: `s/crate::nav_history::/crate::trail::history::/g; s/crate::trail_strip::/crate::trail::strip::/g; s/crate::trail_nav::/crate::trail::nav::/g`. In `main.rs` and `main/tests.rs` (no `crate::` prefix) replace `(?<![.\w])nav_history::` with `trail::history::` and `(?<![.\w])trail_strip::` with `trail::strip::`; `main.rs:481` `nav_history::load()` is covered by the same pattern. In `trail/nav.rs` replace `use super::*;` (line 7, crate root) with `use crate::*;`; the test modules' `use super::*;` stay.
- [x] **Step 4: Cites.** Edit by hand: `store/model.rs:146`, `popups.rs:16`, `trail/history.rs` doc lines naming `trail_nav.rs`/`trail_strip.rs`, `gate/src/sleep.rs:11`, README and `PARKED.md` mentions (grep each old name in the two docs), and the plan/spec text is left as is (frozen).
- [x] **Step 5: Tests split.** For each of `trail/history`, `trail/nav`, `trail/strip` run the split script (below) so the block moves to `<stem>/tests.rs` and the parent ends `#[cfg(test)] mod tests;`; then `cargo fmt --all`.
- [x] **Step 6: Verify.** `cargo check -j 8 -p manifold --all-targets` (Expected: finishes, no warnings); `node testlist.js AFTER`; apply the LAY2 prefix map to BEFORE; `diff` (Expected: empty); `git diff --stat -M HEAD` shows the three renames; `git grep -n "nav_history::\|trail_nav\b\|trail_strip\b" -- . ':!docs' ':!*.md'` returns only `App.nav_history` data uses and nothing naming the module (inspect each hit).
- [x] **Step 7: Lint and commit.** `cargo lint -j 8 -- -D warnings`; `cargo fmt --all --check`; commit `LAY2: trail cluster moves to trail/ (history, nav, strip)`; record the outcome on the LAY2 item of the parent plan in the same commit (what changed, actual size, the checks).

Split script (`split.py`, run `python -P split.py trail/history trail/nav trail/strip < /dev/null` from `crates/manifold/src`):

```python
import io, os, sys
for rel in sys.argv[1:]:
    L = io.open(rel + ".rs", encoding="utf-8").read().split("\n")
    m = [i for i, l in enumerate(L) if l == "mod tests {"]
    assert len(m) == 1 and L[m[0] - 1] == "#[cfg(test)]", rel
    m = m[0]
    end = max(i for i, l in enumerate(L) if l.strip())
    assert L[end] == "}", rel
    body = L[m + 1 : end]
    head = L[: m - 1]
    while head and not head[-1].strip(): head.pop()
    os.makedirs(rel, exist_ok=True)
    assert not os.path.exists(rel + "/tests.rs"), rel
    io.open(rel + "/tests.rs", "w", encoding="utf-8", newline="\n").write("\n".join(body) + "\n")
    io.open(rel + ".rs", "w", encoding="utf-8", newline="\n").write("\n".join(head + ["", "#[cfg(test)]", "mod tests;"]) + "\n")
```

### Task 2: LAY6 debug cluster (gated on ARC1)

**Files:** `debug_control.rs` -> `debug.rs` + `debug/{verbs,reader}.rs`; `debug_export.rs` -> `debug/export.rs`; `event_loop/debug_input.rs` -> `debug/input.rs`; tests to `debug/tests.rs`, `debug/export/tests.rs`; `main.rs`, `event_loop.rs`, `tabs.rs`, `graph_ui/keys/tests.rs`, `names/tests.rs`, `gate` doc cites.

- [x] **Step 0: Gate.** Proceed only after ARC1 has landed (the `main/tests.rs` scanner is gone). If the user says go earlier, change `main/tests.rs:111` `include_str!("../debug_control.rs")` to `../debug.rs` in Step 3 and to `../debug/reader.rs` in Step 4 (`HeadlessDebug` is still in `debug.rs` until then), and say so.
- [x] **Step 1: Baseline.** Clean tree; `node testlist.js BEFORE`.
- [x] **Step 2: Move.** `git mv debug_control.rs debug.rs`; `mkdir debug`; `git mv debug_export.rs debug/export.rs`; `git mv event_loop/debug_input.rs debug/input.rs`. `main.rs`: `mod debug_control;` -> `mod debug;`, delete `mod debug_export;`; `debug.rs` gets `mod export; mod input; mod reader; mod verbs;` (`pub(crate)` where an outside file names a path through them: `debug::reader::{Waiters, HeadlessDebug, spawn_stdin_reader, debug_key_event}`); `event_loop.rs` drops `mod debug_input;`. The tree does not compile until Step 3, so Steps 2 and 3 are one commit; check `git diff -M --stat` for the renames before staging the Step 3 edits.
- [x] **Step 3: Paths.** `debug_control::X` -> `debug::...` per item: `DebugJob`, `Job`, `ReaderOp`, `Line`, `parse_line`, `print_help` stay in `debug`; `Waiters`, `HeadlessDebug`, `spawn_stdin_reader`, `debug_key_event` -> `debug::reader::`. Edit `main.rs` (129, 372, 392, 393, 569, 668), `tabs.rs:52`, `event_loop.rs:6`, `graph_ui/keys/tests.rs:24,58`, `graph_nav.rs`, `cli.rs`, `names/tests.rs:428` (`"debug_export.rs"` -> `"debug/export.rs"`), `gate/src/{instance,state}.rs` cites, README. `use super::*;` in `debug.rs`, `debug/export.rs` -> `use crate::*;`; `debug/input.rs` gets explicit imports for what `event_loop`'s globs gave it (compiler-driven).
- [x] **Step 4: Split `debug.rs` by job** (a second commit). `debug.rs` keeps `Verb`, `Parsed`, `Job`, `DebugJob`, `ReaderOp`, `Line`, `row`, `pointer_row`, `parse_line`, `print_help`. `debug/verbs.rs` takes `MAX_WAIT_SECS`, `parse_secs` (used by `wait_timeout`, the SLEEP verb and the tests), the parsers (`parse_click`, `parse_move_node`, `parse_zoom_pct`, `parse_rename`, `parse_use_jar`, `wait_timeout`, `parse_mouse_button`, `parse_key_code`, `parse_add_file`, `split_id`, `id_word`, `on_off`, `coords`, `go`, `quit`, `cursor_to`, `button_at`, `mouse_button`, `on_focused_tab`, `job`/`with_*`/`key_job`), `VERBS`, `VERBS` needs `Verb`, `Parsed`, `row` and `pointer_row` from `debug.rs` (`pub(super)` or `use super::{..}`), and the four `debug_*` `impl App` helpers move there (the one `impl App` block at `debug_control.rs:1184-1236` is split: those four to `verbs.rs`, `run_debug_job` to `reader.rs`); `debug/reader.rs` takes `run_reader_op`, `spawn_stdin_reader`, `Waiters`, `HeadlessDebug`, the `impl App` with `run_debug_job`, `debug_key_event`. Add `pub(super)` only where the compiler asks. Move the module doc to `debug.rs` unchanged.
- [x] **Step 5: Tests.** Run the split script on `debug` and `debug/export`; the `debug::tests` block imports the `pub(super)` parsers it needs.
- [x] **Step 6: Verify.** As Task 1 Step 6 with the LAY6 prefix map; `cargo test -j 8 -p manifold debug` passes; the old-name grep for `debug_control`, `debug_export`, `debug_input` returns only frozen history (grep `debug_input`, since `debug_input_events` and `debug_input_pending` are live names). On request run `gate.exe all LOG light` through `run.ps1`.
- [x] **Step 7: Lint and commit** as Task 1 Step 7; record the outcome on LAY6.
