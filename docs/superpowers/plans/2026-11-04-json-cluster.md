---
status: draft
---

# JSON cluster Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline; one task). Steps use checkbox syntax.

**Goal:** `json_edit`, `json_level`, `json_level_ui`, `json_view` become `json.rs` + `json/{edit,level,ui,view}.rs`; `tier.rs` matches UI files by path prefix.

**Spec:** `docs/superpowers/specs/2026-11-04-json-cluster-design.md` (s1, s2). Parent item LAY3.

## Global Constraints

- Work on `main`; small commits; never push, tag or bump. `cargo fmt --all` before each commit (twice); the pre-commit hook runs fmt and `cargo lint`. Never `--no-verify`.
- Moves are `git mv` first and committed alone (renames at 100%), edits after.
- No re-exports, no shims: callers are edited. `App.json_level`, `json_level_state`, `json_ui`, `JsonLevelUi` and the `"json_level"` export key are data names: leave them.
- Python runs as `python -P file.py < /dev/null`, never on stdin. Cargo uses `-j 8`. No bare `cat` or `python` that reads stdin.
- Test-id check: `node <scratchpad>/testlist.js OUT` (scratchpad `C:/Users/WINSTO~1/AppData/Local/Temp/claude/C--Users-Winston-Li-Documents-GitHub-manifold/a2c355a1-5493-43d2-a2ce-bd6f49ba9245/scratchpad`); ids print without a module path, so the diff checks names and count; the new tier test adds one.

## Review Focus

- A `json_level` rename that hits the data field or the export key (silent for the key, which the gate reads): the old-name grep lists every remaining hit, inspect each.
- The tier rule dropping a file's tier: the new `tier` test covers `json/ui.rs`, `json/ui/tests.rs`, `json/level.rs` and a flat stem (`theme/tests.rs`).
- A missed `use super::*` after a move: compile error, not silent.

---

### Task 1: LAY3 json cluster

**Files:** move `crates/manifold/src/json_{edit,level,level_ui,view}.rs` and the `json_{edit,level,view}/` folders; create `json.rs`; edit the sites in the spec census; `crates/gate/src/tier.rs`.

- [ ] **Step 1: Baseline.** `git status --short` is clean (or only `docs/runs.md`); `node testlist.js BEFORE`.
- [ ] **Step 2: Move.** `mkdir crates/manifold/src/json`; `git mv json_edit.rs json/edit.rs`, `json_edit json/edit`, likewise `json_level` -> `json/level`, `json_view` -> `json/view`, `json_level_ui.rs` -> `json/ui.rs`; create `json.rs` with `pub(crate) mod edit; pub(crate) mod level; pub(crate) mod ui; pub(crate) mod view;` and a one-line module doc; in `main.rs` replace the four `mod json_*;` lines with `mod json;` and change `use json_level::{..}` to `use json::level::{..}`.
- [ ] **Step 3: Paths.** `perl -pi -e` over the files from `git grep -l "json_view\|json_level\|json_level_ui\|json_edit" -- crates`, excluding docs and the gate crate: `s/\bjson_level_ui::/json::ui::/g; s/\bjson_level::/json::level::/g; s/\bjson_edit::/json::edit::/g; s/\bjson_view::/json::view::/g`, in that order; bare `use json_view::..` lines inside the moved files become `crate::json::view::..`. Do not touch `.json_level`, `json_level_state`, `"json_level"`. Let `cargo check --all-targets` find the rest.
- [ ] **Step 4: Cites.** By hand: `card_id.rs:12`, `level.rs:14`, `names/tests.rs` (`"json_level.rs"` -> `"json/level.rs"`, `"json_level_ui.rs"` -> `"json/ui.rs"`, keep the lists sorted), the four `PARKED.md` cites, any README mention.
- [ ] **Step 5: Tier rule.** In `tier.rs` make `UI_FILES` entries paths under `src/` without `.rs`: replace `"json_level_ui"` and `"json_view"` with `"json/ui"` and `"json/view"`; in `classify` replace the first-component `b` lookup with a match on any entry `e` where `stem == e` or `stem.starts_with("{e}/")`, `stem` being `rel` without `.rs`; keep the message naming the matched entry. Add a test with the four cases above.
- [ ] **Step 6: Verify.** `cargo fmt --all` (twice); `cargo check -j 8 -p manifold -p gate --all-targets`; `node testlist.js AFTER`; apply the prefix map to BEFORE and `diff` (Expected: only the added tier test); `git diff --stat -M HEAD` shows the renames; `git grep -nE "json_(view|level_ui|edit)\b|json_level::" -- . ':!docs' ':!*.md'` returns only data names (inspect each hit); `cargo test -j 8 -p gate tier`.
- [ ] **Step 7: Lint, unit, commit.** `cargo lint -j 8 -- -D warnings`; `cargo fmt --all --check`; `cargo unit -j 8` through `run.ps1` (Expected: all ok); commit `LAY3: json cluster moves to json/ (edit, level, ui, view); tier matches by path prefix`; record the outcome on LAY3 in the parent plan and remove the LAY3 part of the PARKED "Cluster moves still open" entry in the same commit.
