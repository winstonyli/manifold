---
status: approved 2026-11-04 (user)
---

# Compile-checked node-id forgetting (ARC1)

Plan item: `docs/superpowers/plans/2026-11-04-restructuring-cleanup.md`, ARC1, with its second survey and decision u1 (2026-11-04). Replaces PARKED `FORGET-ID-TYPED`. Behavior is unchanged except the corrections listed under "Deliberate changes".

## Problem

`App::forget_id` (`node_store.rs`) drops everything a removed node leaves behind. A source-text test, `every_id_keyed_field_is_forgotten_or_excused` (`main/tests.rs`, ~114 lines, with `GROUPS` of 17 `include_str!` paths and `NOT_FORGOTTEN` of 31 reasons), checks that every id-holding field is forgotten or excused. The survey found it checks less than it appears to:

- `ID_SHAPES` has no `CardId`, `BTreeMap<NodeId` or `BTreeSet<NodeId` shape: `jars`, `cookies.jar_dirty` and `GraphInteraction`'s `CardId` fields are never checked.
- 15 of the 31 excuses are dead (their fields never match a shape), and 6 give wrong or incomplete reasons.
- The 17 `include_str!` paths pin file locations, which blocks moves (the debug split, LAY6).

## Mechanism

**Exhaustive destructure.** Each struct that owns node-keyed state gets a `forget` that begins `let Self { a, b, c } = self;` with no `..`. A new field fails to compile until it is named, either used or written `field: _, // why it is left`. The reasons from `NOT_FORGOTTEN` move to those lines. This is the one mechanism; the newtype-wrapper and registry alternatives give no compile-time guarantee (survey, 2026-11-04) and are not used.

**Units that get a destructuring `forget`:**

| Struct | Today | Change |
|---|---|---|
| `SpawnTracker` (`tabs.rs`) | has `forget` | destructure all 12 fields; `spawning`, `remote_in_flight`, `wakes_in_flight`, `host_loads` excused (spawn lifecycle), `tab_spawns_in_flight`, `tabs_spawned_waiters`, `session_generation` `_` |
| `CookieState` (`jar.rs`) | `forget` returns the held answer | destructure all 8; `jar_dirty`, `jar_snapshots` excused with "forgotten by `App::forget_jar_memory`" (file I/O, stays an `App` method); `jar_last_pull` excused as a round timestamp, not per node |
| `GraphInteraction` (`graph_nav.rs`) | `forget(CardId)` | destructure all 11; `drag_moved`, `pending_new_node`, `drag_origin` (ids, taken by `apply_drop`), `rect_select_start`, `label_section` excused with their reasons |
| `NodeMenu`, `MenuPopups` (`popups/menus.rs`) | `NodeMenu::forget` | destructure `NodeMenu` (`fade` excused: its frozen copy fades out); `MenuPopups` has no id-keyed forget, `trail_menu` is a virtual card id |
| `PageTabGate` | has `forget` | destructure (one field) |
| `LinkGesture` (`links.rs`) | none; `App::forget_connect_source` | new `LinkGesture::forget(id)`: calls `self.end()` (which already clears `retarget` and `connect_drag`, links.rs:37-41) when the id is the source, replacing the `App::forget_connect_source` hop; `retarget_preview` (cleared per frame) and `line_press` excused with their reasons |
| `Doc` (`doc.rs`) | `App::forget_id` removes `hidden_local` | new `Doc::forget(id)`: `hidden_local` removed; every other field `_` with reason (`writes`, `held_text` pruned at save, ...) |
| `GpuState` (`gpu.rs`) | three free calls in `forget_id` | new `GpuState::forget(id)` wrapping `free_tab_thumbnail`, `free_tab_texture`, `free_image`, in that order (`free_tab_texture` reads `thumbs_dirty`, which the thumbnail free removes first, gpu.rs:767-790). A plain wrapper: `GpuState` holds device and queue, so no destructure and no compile guarantee here |
| **`NodeScoped`** (`node_store.rs`, new) | six loose `App` fields | `pending_sleeps`, `saved_scroll`, `nav_history`, `level_views`, `naming_jar`, `renaming_group` move into it as `App.scoped`; its `forget` destructures all six |

`App::forget_id` becomes a short sequence of these calls plus `forget_jar_memory`. The cookie hold-cancel (`clear_cookie_state` -> `cancel_held`) is redundant here because `pending_sleeps` is removed first (survey, `tabs/jar_pull.rs:158`; `pending_sleeps.remove` is at node_store.rs:188, `clear_cookie_state` at :189), so `forget_id` calls `CookieState::forget` and drops the returned `Held`; `note_spawned_jar` keeps `clear_cookie_state` unchanged.

**The slim check (u1).** The scanner shrinks to a ~15-line test over `struct App { .. }`'s direct fields only (no `GROUPS`, no `include_str!` other than `main.rs` and `node_store.rs`): each field whose type mentions `NodeId` or `CardId` after any path prefix (a regex such as `\b(\w+::)*(NodeId|CardId)\b`, so `store::model::NodeId` matches) must appear in `forget_id`'s body or in a short `EXCUSED` list. It stays until ARC7 groups the loose item-view fields.

What it sees after the move (reviewer-checked against `struct App`): `node_order`, `layout`, `current_group`, `focused`, `split_tab`, `viewed_image` (all need an `EXCUSED` reason) and `jars` (`BTreeMap<store::model::NodeId, Jar>`; `forget_id` never names it because `forget_jar_memory`, jar_defs/lifecycle.rs:177, does the removal, so it is excused as "via `forget_jar_memory`"). It does not see `cookies`, `link`, `doc`, `nodes` (a `NodeMap` alias), `confirm` or the moved `scoped` (their types hide the ids), and `App` has no direct `CardId` field. So `jar_dirty` and the `CardId` fields are covered by the destructures alone, not by the scan.

**Known hole:** a brand-new group struct added to `App` whose type does not mention an id is not seen by the scan; the review checklist is "a struct that holds node ids gets a destructuring `forget`".

## Deliberate changes

1. Dead excuses are deleted outright (15). Wrong reasons are rewritten: `toasts.undo` (no ids) leaves the list; `node_order` and `viewed_image` cite `unshow`; `held_text` says pruned for non-editing notes only, and teardown clears it.
2. `forget_id`'s doc header is corrected (callers: `unshow`, `teardown_running_session`, `forget_deleted`; `hide_node` runs it and re-inserts). `merge_groups`' path is not claimed until traced (survey flag).
3. `undo` (`UndoHistory`) stays outside: its entries keep ids so Reopen works; it is documented as "kept" where `forget_id` is documented, not destructured.
4. No other behavior change: the same removals, in the same order except `pending_sleeps` stays before the cookie forget.

## Cost (counted 2026-11-04)

- Moving six fields into `App.scoped` edits `.pending_sleeps` (23 lines, 7 files), `.saved_scroll` (11, 6), `.nav_history` (68, 15), `.level_views` (24, 7), `.naming_jar` (20, 4), `.renaming_group` (20, 11): about 166 lines in roughly 35 files, mechanical and compiler-driven (the field name is unique to `App`; tests use it most). Plus `main.rs` declarations and initialisers (not `.field` uses): 6 declarations (lines 218, 275, 278, 310, 321, 323) and 6 initialisers (523, 527, 528, 536, 541, 542; the `nav_history` one comes from the `Loaded` destructure at 497-502 and becomes `scoped: NodeScoped { nav_history, .. }`), about 10 further lines. `Loaded.nav_history` (main.rs:459) stays. There are no other `App { .. }` literals.
- New code: `NodeScoped` + `forget` (~30 lines), `Doc::forget`, `LinkGesture::forget`, `GpuState::forget` (~10 each); 8 existing `forget`s rewritten as destructures (~+30 lines of `_` reasons).
- Deleted: ~114 test lines, 17 `include_str!` paths, 31 `NOT_FORGOTTEN` entries (about 16 reappear as `_ // why` comments next to their fields).

## Verification

- `cargo fmt --all --check`, `cargo lint -j 8 -- -D warnings`, `cargo check --all-targets`.
- Sorted `-- --list` diff: `every_id_keyed_field_is_forgotten_or_excused` is replaced by the slim test (one rename or deletion, stated), all others identical; `forget_id_clears_every_id_keyed_table` (`main/tests.rs:9`) is kept and extended to assert `scoped.*`, `link` and `doc.hidden_local` are cleared.
- A seeded compile check: temporarily add a field to `SpawnTracker` and to `NodeScoped`; both must fail to compile (recorded in the plan outcome, then reverted).
- Old-name grep: `NOT_FORGOTTEN`, `GROUPS`, `FORGET-ID-TYPED` return only frozen history; PARKED `FORGET-ID-TYPED` is deleted.
- One `cargo unit` through `run.ps1`; gate tier `full` (it touches session removal paths), run on request at the plan's cadence.

## Order and collisions

Independent of LAY2 (landed). Enables LAY6 (its move no longer hits `include_str!("../debug_control.rs")`) and ARC5, ARC7, ARC9. Collides with `main.rs` (about 12 declaration and initialiser lines), `node_store.rs`, `tabs.rs`, `jar.rs`, `graph_nav.rs`, `links.rs`, `doc.rs`, and the parked branch `parked/idle-repaint-hook` only on `session_sync.rs`/`main.rs` (weak evidence: it is 625 commits behind).

## Out of scope

Grouping `focused`/`split_tab`/`viewed_image`/`view_mode`/`transition` (ARC7), egui-memory keyed by node ids (inference only), any change to which state is forgotten.
