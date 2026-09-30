# One entry point for placing a node

Status: draft, policy B decided 2026-09-30; awaiting go-ahead to implement. Nothing here is implemented.
Origin: Part C of `2026-09-30-main-split-and-engine-table-design.md`, after
the main.rs split. Survey line numbers are at `1b82e41`; grep the names.

## Problem

A new node gets its group, id, position, `node_order` entry and save from
whichever site creates it. About 20 sites do this, and the JSON-level rule
("a new node goes in the Json node's own group", `insertion_group`) is
applied three different ways:

| Policy | Sites |
|---|---|
| Ask `insertion_group()` | `insert_group_at`, `insert_note_at`; `GraphAction::NewTab`, `GroupSelected`; reopen a closed entry (`closed.rs`); import (`session_sync.rs`) |
| Leave the JSON level first, then ask | `add_file_at`, `drop_file` (graph branch); reopen; import |
| Refuse | `new_group_shortcut`, `new_note_shortcut` |

Sites that build a `GraphNode { .. }` literal themselves: `insert_group_at`,
`insert_note_at`, `GroupSelected`, `insert_download_node`,
`insert_placeholder_node`, `insert_{image,pdf,json}_node_with_id`, and four
in the first-launch demo (`main.rs`). The id plus
`GRAPH_WORLD_CENTER + spawn_jitter(&id)` pair is repeated in four places.

Checked while surveying, and not problems:
- The empty-canvas card (`pending_new_node`) and the toolbar buttons are
  already hidden on a JSON level (`graph_ui.rs`, `on_json_level`). Only
  shortcuts, headless commands and file drops reach creation there.
- Every `apply_graph_action` ends in `save_session()`, so the `NewGroup`
  card does save. `insert_note_at`'s own save is a redundant second save on
  that path, and `insert_group_at` has none because its callers save.

## Goals

- One place decides where a node goes, so the JSON-level rule has one
  implementation and one test.
- The literal `GraphNode { .. }` construction for a plain new node exists
  once.

Non-goals: tabs (they become nodes when the engine answers, via
`spawn_tab_async`), imports (`materialize_imported`), `GroupSelected`'s
centroid placement beyond calling the same helper, folding the node-kind
enums.

## Policy (decided: policy B for all)

Creating any node from a JSON level leaves the level first
(`leave_json_level()`), then places the node in `current_group`. This
covers notes, groups, tabs, downloads and file adds, so the user always
sees what they made, and the old refusals in `new_group_shortcut` and
`new_note_shortcut` go. Reopen and import already work this way.

Exception, unchanged: a drop onto Tab view beside the focused tab keeps
the focused node's own group (it is not the level being viewed).

Consequence: `placement()` needs `&mut self` (it leaves the level), and
after it runs `insertion_group()` equals `current_group`, so creation
sites stop calling `insertion_group()`. It stays for
`switch_group_level` in `json_level.rs`, which still needs "the Json
node's own group" to leave a level.

## Design

In `fetched.rs` (next to the other insert functions):

```rust
/// Where a new node lands.
pub(crate) struct Placement { pub group: Option<String>, pub pos: egui::Vec2 }

impl App {
    /// Leaves a JSON level, then returns `current_group` with `pos`, or a
    /// jittered world-center spawn for `id` when `pos` is `None`.
    pub(crate) fn placement(&mut self, id: &str, pos: Option<egui::Vec2>) -> Placement;

    /// Inserts a plain node at `at` under `id` (allocated when `None`),
    /// appends to `node_order`, returns the id. Does not save: the caller
    /// does, as `apply_graph_action` already does once per action.
    pub(crate) fn place_node(&mut self, id: Option<String>, payload: NodePayload, at: Placement) -> String;
}
```

- `insert_group_at`, `insert_note_at`, `insert_placeholder_node`, the three
  `insert_*_node_with_id` bodies and the demo literals become
  `place_node` calls. The `_with_id` functions keep their `id` parameter,
  since the id is chosen before the file is read.
- `GroupSelected` calls `place_node` with its centroid position.
- `insert_note_at`'s `save_session()` moves to its callers
  (`new_note_shortcut`, which then saves like `new_group_shortcut` does).
- `add_file_at` and `drop_file` drop their own `leave_json_level()`
  calls; `placement()` does it.
- Tabs (`spawn_new_tab`, `spawn_tab_at` callers), reopen and import call
  `placement()` for the group only.

## Steps

One commit each, gate after each (fmt, clippy `-D warnings`, tests, smoke):

1. `placement` and `place_node`, with tests: on a JSON level it leaves the
   level and gives the outer group; elsewhere the current group; an explicit `pos`
   wins; jitter is deterministic per id.
2. Convert `insert_group_at`, `insert_note_at`, `GroupSelected`; drop the
   two early returns; move the note save. Tests: `NEWNOTE`
   and `NEWGROUP` on a JSON level leave it and land in the outer group.
3. Convert the `insert_*_node_with_id`, `insert_placeholder_node` and
   download literals (pure refactor, existing tests cover them).
4. Convert the first-launch demo literals.
5. Tabs: `spawn_new_tab`, `spawn_tab_at` and their callers take their group
   from `placement()`, so a new tab from a JSON level leaves it (test:
   `NEWTAB` on a JSON level).

## Risks

- Steps 2 and 5 change behaviour; steps 1, 3 and 4 are refactors.
- `node_order` position: every literal site pushes to the end, which
  `place_node` keeps.
- A new tab made on a JSON level now leaves it, as a file add already does.

## Testing

Unit tests as listed; `scripts/headless-smoke-test.sh` after each step;
one headless run that opens a JSON node, sends `NEWNOTE`, leaves the level
and checks the note is in the outer group.
