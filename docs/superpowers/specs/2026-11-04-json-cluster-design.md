---
status: draft
---

# JSON cluster: `json_*` -> `json/` (LAY3)

Plan item LAY3 of `plans/2026-11-04-restructuring-cleanup.md` (parked in `PARKED.md` "Restructuring leftovers"). Same recipe as LAY2 and LAY6 (`specs/2026-11-04-trail-debug-clusters-design.md`): decisions d1 (a real `parent.rs` + `parent/`, no `#[path]`) and p1 (no re-exports, callers edited) carry over. Behavior is unchanged.

## Goal

The four JSON-level files become one cluster with a parent module, so the layout shows they are one responsibility, and the gate's tier rule classifies a nested file by its own path instead of its first component.

## Target tree

```
json.rs                     new parent: pub(crate) mod edit; level; ui; view;
json/edit.rs                from json_edit.rs        (633 lines)
json/edit/tests.rs          from json_edit/tests.rs  (627)
json/level.rs               from json_level.rs       (628)
json/level/tests.rs         from json_level/tests.rs (801)
json/ui.rs                  from json_level_ui.rs    (1331; ~200 inline test lines stay: under 250)
json/view.rs                from json_view.rs        (804)
json/view/tests.rs          from json_view/tests.rs  (518)
```

`json_file.rs` is not part of this (LAY10, parked). The RFC 6901 helpers in `json_view.rs` stay (CRT2, parked).

## Reach (census 2026-11-04, `git grep`)

- Module paths: `crate::json_view::`, `crate::json_level::`, `crate::json_level_ui::`, `crate::json_edit::` and `use json_*` lines: about 55 lines in 24 files (`main.rs` 4 `mod`/`use` lines, `card/json_body.rs` 11, `card.rs` 3, `card/tests.rs` 3, `level.rs` 3, plus the four moved files' own cross-references).
- Not module paths, left alone: the `App.json_level` field and `json_level_state()` (data), the exported-state key `"json_level"` read by `gate/src/{profile,shots,smoke}.rs`, `json_ui`, `JsonLevelUi`.
- File-name cites to fix by hand: `card_id.rs:12`, `level.rs:14`, `names/tests.rs` (`"json_level.rs"` in the allowed list, `"json_level_ui.rs"` in `OTHER_NAMES`), four `PARKED.md` lines that cite `json_edit.rs`, `json_view.rs`, `json_level_ui.rs` and `json_level.rs`. Frozen history (devlog, done plans and specs, `runs.md`) stays as written.
- `crates/gate/src/tier.rs`: `UI_FILES` holds `"json_level_ui"` and `"json_view"`.

## The one real change: tier rule by path prefix

`classify` names a source file's module by its first path component, so `json/ui.rs` and `json/view.rs` would both read as `json`, which is on no list (light). `json/level.rs` and `json/edit.rs` are light today and stay light. Generalizing instead of special-casing: `UI_FILES` entries become paths under `src/` without `.rs` (`"json/ui"`, `"json/view"`, and the existing stems unchanged), and a file matches an entry when its path without `.rs` equals it or starts with it plus `/`. This subsumes the first-component rule (`theme/tests.rs` still matches `theme`) and handles `json/ui/tests.rs`. About 6 lines plus one test. The import check ("on the ui list but imports a core module") is unchanged.

## Decisions to confirm

- **s1** Module names `json::{edit, level, ui, view}` (`json_level_ui` -> `ui`, the drawing half of the level). The crate root has no `json` module today (`store::json` is a different path); the compiler settles any glob clash.
- **s2** Tier rule as above (path-prefix match) rather than adding `"json"` to the UI list, which would wrongly make `json/level.rs` and `json/edit.rs` ui-tier.

## Checks

Per the LAY6 recipe: `cargo fmt --all`, `cargo lint -j 8 -- -D warnings`, `cargo check --all-targets`, sorted `-- --list` diff under the prefix map (`^json_level_ui::` -> `json::ui::`, `^json_level::` -> `json::level::`, `^json_edit::` -> `json::edit::`, `^json_view::` -> `json::view::`; in that order), `git diff --stat -M` showing renames at 100%, old-name grep (`json_view`, `json_level::`, `json_level_ui`, `json_edit` as module or file names) returning only frozen history and the data names above, `cargo unit` once through `run.ps1`. The gate tier of this change is `light` (UI code); run it on request.

## Out of scope

Splitting `ui.rs` further, the pointer helpers (CRT2), `json_file` (LAY10), any change to the JSON level's behavior.
