# Splitting main.rs and one engine table

Status: draft for approval, 2026-09-30. Nothing here is implemented.
Origin: the full-codebase review (findings 4 and 5), after the JSON node
rework.

## Goals

- `main.rs` (6,424 lines; `impl App` spans lines 2052 to 5592, about 200
  methods, and about 830 lines of tests) becomes a module-per-concern set
  of files, the way `level.rs` and `json_level.rs` already are. No
  behaviour change.
- One table says what each engine is, so adding a fifth engine is one
  edit that the compiler checks.

Non-goals: changing the `App` struct, folding the node-kind enums (parked
until the next node kind), any UI change.

## A. Engine table

Today an engine's facts are spread over `engine_kind_serde` (name),
`session_sync::engine_code`/`engine_of` (store code), `binary_name`,
`debug_control` (parse), `engine_info` (label, colour, tier, pros, cons),
`event_loop` (texture format) and `store::model::ENGINES`. Two decoders
fall back to Servo on an unknown value. Those fallbacks are deliberate
(an older build reading a newer file) and stay.

Plan: `EngineDesc { kind, name, code, binary, label, texture_format }`
and `const ENGINES: [EngineDesc; 4]` in `process_manager.rs`, built so
that `EngineKind::desc(self)` is an exhaustive `match`. Every other
mapping calls it or searches the table. `engine_info`'s prose (pros and
cons) stays where it is, keyed by `desc()`. The existing round-trip test
(`engine_codes_follow_the_store_table`) becomes a test over the table.

Risk: small. The store's `ENGINES` names are persisted, so the table's
`name` values must equal them exactly (the round-trip test enforces it).

## B. Splitting main.rs

Each step is a pure move (`use super::*; impl App { ... }`, as `level.rs`
does), one module per commit. The bar for each commit is the same:
`cargo fmt`, clippy `-D warnings`, all tests, and one smoke-test run. A
method keeps its name and visibility (`pub(crate)`); tests move with the
code they test.

| Module | Holds (current line ranges) | About |
|---|---|---|
| `node_menu.rs` | `NodeAction`, `NodeMenu*`, `ZoomChipAction` (136-760); `build_context_menu`, `build_node_menu`, `node_menu_view`, `apply_node_action` (5062-5348) | 900 |
| `closed.rs` | `ClosedEntry`, `record_closed`, `reopen_closed` (1044-1217, 2245-2312) | 250 |
| `view_modes.rs` | `ViewMode`, `Transition*`, the camera tween and momentum types, transitions and swaps (911-1040, 3665-3795, 3868-3908) | 450 |
| `tabs.rs` | spawn, respawn, focus, hide, split, destroy and remove for tabs (2476-2856, 3493-3660, 3795-3868, 3908-3926) | 900 |
| `fetched.rs` | content keep and prune, refresh, `reload_fetched_node`, `insert_content`, the `insert_*_node_with_id` family, `add_file*`, `drop_file` (4209-4796) | 600 |
| `graph_nav.rs` | fit and bounds, `subtree_ids`, settle key and settle, focus movement, `group_of`, drop targets, `apply_drop`, `merge_groups` (3043-3415) | 400 |
| `shortcuts.rs` | `handle_global_shortcuts` and every `*_shortcut` (3926-4170, 4796-5062) | 450 |

`main.rs` keeps `App`, its `Default`, `build_ui`, toasts, the window and
event-loop glue, and the small types they use: about 1,500 lines.

Order: `closed`, `node_menu`, `view_modes`, `graph_nav`, `shortcuts`,
`fetched`, `tabs`. Start with the self-contained ones so the method
boundaries show up before the tangled ones.

Risks:
- `use super::*` hides which names a module needs; the compiler flags a
  missing one, so this shows up as a build error, not a bug.
- `git blame` on moved code: add each move commit to
  `.git-blame-ignore-revs`, as the fmt reformat did.
- Merge conflicts with other branches touching `main.rs`: none known on
  `dev` (the `claude/*` branches are old), but check before starting.

## C. After the split (separate decision)

Node creation has no single entry point. The final JSON review found the
creation sites spread over `main.rs` and `session_sync.rs`
(`new_group_shortcut`, `new_note_shortcut`, `add_file_at`, `drop_file`,
`import_group_json`, `reopen_closed`, the toolbar and the empty-canvas
menu). Once `fetched.rs` and `shortcuts.rs` exist, one
`fn place_node(&mut self, ...)` that applies the JSON-level rule in one
place is natural. That changes behaviour, so it gets its own spec.

## Testing

- Existing suites: 251 tests in `manifold`, the `ipc-protocol` tests,
  clippy and fmt.
- `scripts/headless-smoke-test.sh` after each move commit (the
  `claude/*` worktree is left alone).
- For A: the round-trip test over the table plus one test that every
  `EngineKind` appears in it exactly once.
