# Live-state to document sync: map of today's flow (2026-10-05)

Facts gathered before planning 2b/2c of the node model (`2026-10-04-node-model-design.md`, section 6 items 6 and 10). Paths are under `crates/manifold/src/`; line numbers are as of commit 9a0e48a and will drift.

## How it works

- **One production `diff` caller:** `save_session` (`session_sync.rs` ~970-1070) builds `live: BTreeMap<NodeId, Shared>` from `NodePayload` (`shared_nodes`, ~362-440; jars added as `Kind::Jar`), takes `keep` from `spawning`, calls `diff(doc, live, explicit, keep)` and commits. Up to 4 rounds: pending remote ops are applied first, then the diff is recomputed. 18 test calls in `session_sync/tests.rs`.
- **No dirty flag, no timer for local edits.** About 45 call sites call `save_session()` directly after a mutation (card, tabs, fetched, graph_ui, graph_nav, node_menu, popups, settings_page, shortcuts, json_level, closed, main, event_loop, view_modes, image_view). `poll_store` runs every frame but saves only when the file changed on disk or a retry is due. Edits with no save of their own (pin shortcut, Pin All, note keystrokes) ride on the next save.
- **`explicit`** (`App.explicit`, filled by `App::mark`, cleared after a successful commit): only POS, URL and PATH are `FieldDef.explicit`; diff `Set`s them only when marked. Marked at 3 sites: `drag_moved` (POS, card.rs ~1162), `navigate_tab` (URL, main.rs ~1849), engine URL reports that pass `shares_url_change` (event_loop.rs ~1013). PATH is never marked (never written after create). POS from layout physics is never written (personal state keeps it).
- **`keep`**: built each round from `self.spawning` so a tab whose engine is respawning (crash, swap, wake, new tab) is not Deleted for lack of a live node; a spawning tab the document lacks is Created from `spawn_shared`.
- **Reverse path:** `remote_changes`/`apply_remote` (jars, then groups, then the rest) call `materialize` -> `update_live_node` / `insert_materialized`; `last_shared` plus a field-level guard keep unsaved local fields over remote edits to other fields (last writer wins per field).

## What diff does implicitly (a direct-op design must replicate or drop)

- Creates groups before members; a group created later in the batch (cycle) becomes top level.
- Never recreates a tombstone; never touches a non-live existing node.
- Writes GROUP/PARENT as none when the target is neither live nor new (`valid`, PARENT only today).
- Compares groups with `effective_group`, so re-homing out of a deleted group is not a Move.
- Sees cascades only as end states: children's `parent` passes up on node removal, dissolve moves members to the group's parent then deletes the group, `delete_jar` clears references then the jar is absent from `live`.
- Batches many frames of edits into one commit; refreshes `last_shared` by re-reading the document.
- Computed (not user-driven) changes to non-explicit fields are written like edits: `refollow`/cascade jar changes, fallback engine on spawn.

## Mutation sites by synced field (about 60 across 20 files)

POS drag (card.rs) | COLOR 2 (node_menu) | GROUP 6 (graph_nav Join/GroupWith/merge, main remove_from_group, dissolve_group_quietly, GroupSelected) | PARENT 4 (graph_nav Reparent, main remove_node, tabs remove_tab, fetched conversion) | URL 2 | ENGINE 2 (view_modes swap, main wake_tab) | PINNED 3 (node_menu, shortcuts, set_all_pinned) | NAME 2 (group rename, jar rename) | TEXT 1 (note edit, every keystroke) | VIEW 1 (json_level edit_json_view) | JAR/JAR_SET ~6 entry points via jar_defs put_tab_jar | DEFAULT_JAR 3 | EPHEMERAL 1.
Node creation funnels: `place_node` (fetched.rs ~65; groups, notes, content, downloads), `spawn_tab_async` (tabs.rs ~127), `create_jar`, `materialize_imported` (import, replace session, `reopen_closed`). Deletion: `close_node` -> `destroy_node` -> `destroy_group`/`destroy_tab`/`remove_node`, plus dissolve, merge, `remove_tab`, teardown, `forget_jar`.

## Tests touching the mechanism

79 tests in `session_sync/tests.rs`: 8 pure `diff` tests (4-argument signature) plus `path_is_explicit` and `app_save_costs` call `diff`; four depend on `explicit`/`mark`; the rest cover remote apply, URL sharing, restore/personal state/wake, jars and import, content refetch, downloads and parent links.

## After 2b-1

- Mirrored fields (`color`, `pinned`) are written only by `App::edit_field`, which updates the mirror and queues the op in `App::pending`.
- `save_session` commits `App::pending` beside `diff`, dropping edits for nodes this commit creates or deletes or the document doesn't show live, and clears it only on a successful commit.
- `mirror_remote` (called from `materialize`) skips a field with a pending edit, and `Mirror::init` seeds a new tab at spawn landing only if the id has no entry.

## After 2b-2

- Links (`group`, `parent`) live in the mirror. Only `edit_group` and `edit_parent` queue ops for them (`App::pending_moves`, `App::pending`); every local link edit, including local cascades (dissolve, merge, `remove_node`, `remove_tab`), goes through them.
- The other mirror link writers change the mirror only:
  - `mirror_remote` (from `materialize`): another replica's effective group and parent, skipping a link with a pending edit.
  - `follow_remote_group` / `follow_remote_parent`: a remote removal's cascade (`remove_remotely_deleted`, `dissolve_group_quietly`). Mirror only, so a concurrent remote merge, move or re-parent stands; a local edit still pending for the link is rewritten instead, so it keeps winning.
  - `Mirror::seed`: a landing tab (`seed_landing`) or a new node (`place_node`), only for an id with no entry.
  - `seed_landing`'s safety net: a new spawn whose group was removed while it was in flight lands in the document's group (nearest one still here), through `follow_remote_group`, never as a queued `Move`.
  - `save_session`'s post-commit alignment: a pending link whose target the commit wrote as none (Ruling 5) is none in the mirror too.
- Cascades (`members_of`, `children_of`) also reach a tab mid-respawn: it is out of `nodes` but keeps its mirror entry. A new spawn has no entry until it lands.
- Commit order is `diff`, pending `Move`s, pending `Set`s, with targets resolved against the document and this commit; `diff` writes a created node's group or parent as none when the same commit deletes the target.
