# The `closed` field and a history derived from the document: design, 2026-10-14

Status: design, decisions confirmed by the user on 2026-10-14 (live model B, history derived from the document, the
Default jar forbidden inside a closeable group, Dissolve undo parked). Revised after an adversarial review (12
findings, all addressed below). Plan 3 of `2026-10-04-node-model-design.md`; item 1 of
`2026-10-04-homogeneity-decisions.md`. Supersedes that design's section 5 (History), the "Correction (B, 2026-10-07)"
paragraph of section 3, and the `closed` bullet of section 1, where they differ from this document. Surveys behind it:
2026-10-14 (plan-3 gap, live model, history ownership, jars and closing, destructive verbs).

## Goal

A close is a snapshot today: `closed.rs` serialises the subtree, the nodes are Deleted (final), and a reopen creates
copies with fresh ids. That loses links to nodes outside the subtree, the node's identity, and anything the snapshot
does not carry (an unknown type's raw fields), and it keeps a second, per-replica copy of facts the document could
hold. After this change a close sets one flag, `closed`, on the closed root; nothing is serialised, and undo clears
the flag.

## Decisions

1. **A `closed` field.** Field 23, a Bool register on every node (add it to the `FIELDS` table and to every kind's
   field list in `model.rs`), default false. Only the root of a close is flagged; its descendants keep their own
   fields and are hidden by their position (decision 2). Only the close and reopen verbs write it.
2. **Two predicates, not one.** `Node::is_live` keeps its meaning (typed and not deleted): it is the commit gate
   (`session_sync.rs:1276` queued Deletes, `:1285` `editable`, `:143/:152` `resolve_id` and `create_ops`), and a closed
   node must pass it or the reopen `Set`, the eviction Delete and the edges that name a closed node would be dropped
   or nulled. A new `Doc::placement(id)` walks the raw `group` chain and answers `Shown(Option<group>)` or `Hidden`:
   a group that is closed and not deleted hides everything below it (checked first); a deleted or typeless group is
   skipped and the member passes up to the nearest shown ancestor, as `effective_group` does now. A node is *shown*
   when it is live, not itself closed, and not `Hidden`. Display consumers use shown: `Doc::shown()` (the old
   `Doc::live()` call sites split by purpose: `restore_session`, `remote_changes`, `store::shown`,
   `check_graph_matches_document`, `store/json.rs:121,129` export, `:749`, `:1446`, `:1871`; the plan lists each; the
   reload-diff change detection `journal.rs:1087` stays `is_live`). `effective_group` itself is unchanged: a shown
   node never has a closed ancestor, so `session_sync.rs:1298,1359` keep their meaning. A remote move into a closed group is covered because the remote change set already
   includes descendants (`journal.rs:1095`). Cost is O(n · depth) per call, fine.
3. **Live model B, with a hide path separate from the delete path.** A closed or hidden node is not in `App.nodes`, the
   mirror's node entries or `node_order`. `remove_remotely_deleted` (`session_sync.rs:802-846`) is a delete path: it
   re-parents children (`follow_remote_parent`), dissolves a group's members up (`dissolve_group_quietly`), deletes
   the node's links (`links_naming`) and releases backing files. The hide path does none of that; it only drops the
   node and its hidden descendants from `App.nodes`, `node_order` and the mirror (a new `Mirror::forget_node`),
   unloads engines and runs `forget_id`, after one shared helper `leave_hidden(id)` (also used by a local close) has
   cleaned view state: if the current level is the hidden node or inside it, the level falls to the nearest shown
   ancestor (as `dissolve_members` does with `switch_group_level`, `groups.rs:63`); it clears selection, hover, drag,
   a link in progress, `focused`, `viewed_image`, `split_tab` and `wakes_in_flight` that name the id, and a
   `remote_in_flight` spawn needs its own third state. Persisted `LocalState.current_group`, `focused`,
   `viewed_image`, `split_tab` and the camera level id that name a hidden id fall back the same way on restore. The
   hide path keeps the id's `LocalNode` (zoom, title, live url), so a close by another replica loses nothing.
   `RemoteChange` becomes three-state (shown, hidden, deleted) instead of an `Option`. A later clear re-materialises
   with `materialize`, groups first, as `apply_remote` orders them. A Delete of a node that is not in `App.nodes` (an
   evicted hidden subtree, local or remote) still runs `release_backing`, drops the `LocalNode` and runs `forget_id`:
   `remove_remotely_deleted` returns early for an id missing from `App.nodes` (`:813`), so that cleanup is keyed by
   the document's ids, and `teardown_running_session`'s per-id loop (`:2107`) walks the document, not `App.nodes`.
4. **Close writes a flush, a flag, and the Deletes of what cannot come back.** `close_node` first writes the state the
   snapshot used to capture (`live_fields`, `closed.rs:104-123`) into the document: a note's typed text, the position as
   drawn, a file's resolved path. A node's live url stays personal (decision 8). Then it sets `closed` on the root. A
   `restorable` kind (caps row plus `worth_reopening`'s value test) is closed; a root that is not restorable is
   Deleted for real with no entry, as today. Inside a closing group, members that are not restorable (a blank note,
   a completed download) are Deleted at the same time, as the snapshot dropped them; an in-progress download has no
   document node and is cancelled and removed. A group closes with its contents hidden in place (Cascade), nothing
   moved. The Default jar is never closed (the `close_node` guard stays). Tabs unload their engine and come back
   asleep.
5. **History is derived.** The history is the nodes with `closed == true` that are not `Hidden` (a closed node under
   another closed node is part of that node's entry, not its own), newest first by the `closed` register's own stamp
   (`Reg.stamp`, `model.rs:412`: `Doc::closed_roots() -> Vec<(NodeId, Stamp)>`, excluding Deleted nodes); the order
   is total and identical on every replica. `LocalState.closed`, `ClosedEntry`, `record_closed`, `closed_changed_at`, `restore_closed` and the
   load-time repair pass go away. A nested closed node becomes a history entry when the node above it is reopened.
6. **Eviction deletes the whole subtree.** Whenever the history holds more than `MAX_CLOSED` (20) roots, after a local
   close, after applying remote ops, and at load, every root past 20 has its entire subtree Deleted: the root and
   every descendant, nested closed nodes included (they are not in the history while hidden, so none is retained). A
   deleted root alone would pass its children up and resurface them, hence the whole subtree. A Delete is idempotent,
   so replicas that evict the same root converge, and every replica evicts without waiting for a local close. A
   reopen concurrent with its eviction loses (Delete wins). Eviction is the only place `closed` becomes `deleted`.
   It also re-parents nothing: children outside the subtree whose `opened-from` edge names a Deleted node read the
   edge as none (the grandparent link is lost, as when a closed parent was Deleted before).
7. **Reopen is un-close.** The shortcut and the Recently Closed popup set `closed` to false on the chosen root: same
   ids, group, position, links, fields. If its group was Deleted it shows at the nearest shown ancestor; if its group
   is still closed it is restored but stays hidden with it, and a toast says so. Reopening a node G also re-stamps
   every closed root nested directly under it (`Set closed = true` again), so they enter the history newest, in
   their old relative order; otherwise their old stamps would sort them oldest and a reopen at 20 roots would evict
   them at once with their whole subtree. "Undo" in this document means Reopen. It lands where it was, so the app
   reveals it (`reveal.rs`) when it is not at the current level. `Ctrl+Shift+T` reopens the newest root this replica
   closed (the stamp's `actor`); this is a behaviour change from `shortcuts.rs:375`, which pops any entry, and the
   popup lists every closed root, any replica's.
8. **Per-node local state.** The zoom, title, muted flag and live url that live in `doc.local` of a snapshot today
   (`LocalNode`, `local_state.rs`) are kept in the replica's local state keyed by node id, for closed roots and their
   hidden descendants, and dropped when the node is Deleted. Another replica reopening the node gets defaults.
   Scroll, navigation history and a loaded engine are lost on close, as now; typed note text and drawn position are
   not (decision 4).
9. **Links and edges: read rules, one accessor.**
   - A Link node is not changed by a close. The mirror keeps Link nodes whose ends are hidden (so `links_naming`,
     which reads the mirror, still lets eviction delete them); drawing, selection and EXPORTSTATE show a link only
     when both ends are in `App.nodes` (`level_edges`, `links.rs`, `graph_ui`, `debug_control`). This replaces the
     "Correction (B, 2026-10-07)".
   - The `opened-from` edge: closing a tab or note no longer re-parents its children (`drop_tab`/`drop_node`'s
     `edit_parent`); the children keep the edge. Because the hidden node leaves the mirror, one accessor
     (`App::opened_from(id)`, `None` unless the parent is in `App.nodes`) is the only reader of the edge; the plan
     greps and converts every reader (graph edges, layout, quick switch, `children_of`, `reveal`).
   - A `jar` edge to a closed jar dangles and resolves to the Default (`resolve_jar`'s `jar_exists` reads `App.nodes`,
     so a closed jar is already absent) and reverts on reopen; `sweep_jar_switches` runs after a close and a reopen
     to sleep and re-wake engines on the changed jar.
   - A real Delete still removes the Link nodes naming the deleted node.
10. **The Default jar.** It may not be inside a group that can close. `edit_group` already refuses moving it into a
    group; `ensure_default_jar` gains the invariant "a Default with a group sets its group to none", checked over
    every document jar, hidden ones included (a Default that arrives inside an already-closed group is repaired, not
    shadowed by a second Default). That deletes `destroy_group_members`' eject branch and `destroy_node`'s second
    guard. `IS_DEFAULT` uniqueness ignores a jar that is closed or hidden, with the repair rule as the backstop.
11. **Backing files.** The id survives a reopen, so `touch_backing` and the close-path call of `rebind_jar_files` go
    away (`rebind_jar_files` stays for Duplicate and session import, `session_sync.rs:2045`). `backing_keep` is every
    non-deleted node in the document, closed ones included; files are released only on Delete, and the release runs
    after the Delete has committed (`release_backing` reads `backing_keep`, `fetched.rs:205`, so it must not run
    before the commit). A replica that never receives an eviction keeps the file until the launch-time prune.
12. **Destructive verbs other than close stay as they are.** Dissolve, merge and Remove from Group record nothing and
    have no undo today; they remain plain edits. Parked: undoing a Dissolve (needs a recorded transaction, an
    `adopted_from` field, or a txn id on ops; revisit with general undo). The group card's X stays
    `NodeAction::Dissolve` (`card.rs:549`).
13. **Duplicate keeps a snapshot path.** `duplicate_nodes` copies a subtree with fresh ids through `subtree_entry`,
    `live_fields` and `rebind_jar_files` (so a copy never aliases a jar file) and the import half of `reopen_closed`.
    That survives as a pure export of a shown subtree (`duplicate.rs`, moved out of `closed.rs`), independent of
    history: shell copies for unknown types (the unknown-type spec's decision 4 holds for it), `IS_DEFAULT` stripped,
    the Default jar skipped, links copied when both ends are inside, tabs asleep.
14. **Sessions and mixed versions.** Replacing a session (Open or Import Session, `teardown_running_session`,
    `session_sync.rs:2083`, whose `live()` call is at `:2092`) Deletes every non-deleted node of the old document, closed ones included: a closed node is
    history, and history belongs to its document. That changes `:2057-2063`, which kept the history across a
    replacement. Export Session and the JSON export write shown nodes only. No migration: an existing profile's
    `LocalState.closed` is dropped (those entries are lost; the nodes were already Deleted), and a session with no
    `closed` fields reads as nothing closed. An older build ignores field 23 and shows a closed node as live;
    accepted, no compatibility is kept.

## Model

- `field::CLOSED = 23`; `Node::closed()`; `Doc::placement`, `Doc::shown`, `Doc::closed_roots`; `Mirror::forget_node`.
- Unknown-type spec, decision 4 ("Reopen makes a shell"): superseded for Close and Reopen. A closed unknown node
  keeps its raw fields and reopens whole; only Duplicate and import make shells.

## Behaviour

- **Close** (all callers of `close_node`): flush, set `closed`, delete the non-restorable, hide, unload,
  `sweep_jar_switches`, evict past 20. **Reopen**: clear `closed`, materialise the node and every shown descendant,
  `sweep_jar_switches`, reveal.
- **Sync**: close and reopen are ordinary `Set`s; concurrent close and edit converge per field (last writer wins); a
  closed node costs journal space until Deleted (the prune at `ops.rs:162-179` runs only on a never-shared document
  and keeps closed nodes; an evicted subtree is deleted whole, so its tombstones are freed there).
- **Failed-twin / spawn-resolution duplicates** (`tabs.rs:691`, `spawn_result.rs:266,323`) call `record_closed` today
  for a system-made duplicate, which can be an arranged tab's card (`spawn_result.rs:304-307`). They stop recording
  and Delete it: a deliberate behaviour change, since the user can no longer reopen such a card from history.
- **Recently Closed rows** read the document: the label from the node's fields (`name`, text, `url`, `path`,
  `view_kind`; `popups.rs:695-745`), the title from the node's `LocalNode`, the member count is the number of
  non-deleted descendants (nested closed nodes included), and the toolbar count is `closed_roots().len()`
  (`graph_ui.rs:1144`).
- **Export and named sessions.** `session.rs` `backing_names` (`:39-61`) builds the kept set for named sessions from
  the shown-only JSON export plus `LocalState.closed`; it must read every non-deleted node of the document instead,
  or a closed node's files in another session would be pruned.

## Touch points (the plan verifies each by reading the code)

- `store/model.rs` (field table and per-kind lists, `placement`, `shown`, `closed_roots`, `effective_group`),
  `store/ops.rs`, `store/mod.rs` (`shown`), `store/json.rs` (`is_live_card`, export), `store/journal.rs` (`:1087`,
  `:1095`).
- `session.rs` (`backing_names`), `session_sync.rs`: `remote_changes`/`RemoteChange` (three-state), `leave_hidden`, `apply_remote`, `remove_remotely_deleted` (split),
  `restore_session`, `check_graph_matches_document`, `resolve_id`, `create_ops`, `editable`, `links_naming`,
  `queue_delete`, `materialize`, `teardown_running_session`.
- `closed.rs` (shrinks to history queries, eviction, reopen), the new `duplicate.rs`, `local_state.rs`,
  `node_ops.rs`, `groups.rs`, `tabs.rs`, `node_store.rs`, `shortcuts.rs`, `popups.rs`, `graph_ui.rs`, `level.rs`,
  `links.rs`, `jar_defs.rs`, `mirror.rs`, `fetched.rs`, `spawn_result.rs`, `debug_control.rs`.
- Docs: `2026-10-04-node-model-design.md` (superseded notes on sections 1, 3, 5 and the stale claims: `JAR_SET` and
  `dissolve_group`'s re-snapshot are gone, `diff` is gone, "18 fields to 16" is wrong), the unknown-type spec's
  decision 4, the jars-as-cards spec where it mentions reopen copying jar files, README, the homogeneity list.
- Tests that assert the current behaviour and must change (from the review; the plan re-greps): `closed.rs` tests,
  `fetched/tests.rs:248,273`, `local_state.rs:238,251`, `jar_defs/tests.rs:56,650`, `session_sync/tests.rs` (about
  fourteen close/reopen cases from line 1940 to 6026), `popups.rs:1795-1827`, `main/tests.rs:126`.
- Gate: `smoke.rs` (`683-732`, `872-968`; the check at `956-959` that the reopened tab names a *new* jar flips to the
  same id), `unknown_type.rs:136-139`, `jar.rs` use close and reopen; `two`, `sleep` and `shots` have no such verbs. A
  new stage `closed` (close, reopen, group close with contents, links to outside nodes surviving, two instances
  converging on the history order and on eviction).

## Testing

- Store: a `closed` Set hides a node; a closed group hides its descendants, a deleted group passes them up; a closed
  node nested in a closed group is not in `closed_roots` until the outer one is cleared; order by stamp; a reopen
  Set and an eviction Delete are accepted for a closed node (the commit gate).
- Sync: a remote `closed` Set hides without releasing backing, re-parenting or deleting links; a clear
  re-materialises with hidden descendants, groups first; two replicas close different nodes and agree on the order;
  a remote move into a closed group is hidden.
- History: 21 closes evict the oldest root's whole subtree, nested closed nodes included; two replicas evicting the
  same root agree; a replica that only receives remote closes also evicts; reopen versus eviction loses;
  `Ctrl+Shift+T` skips another replica's close.
- Close fidelity: a note's typed text and a card's drawn position survive close and reopen; a blank note in a closed
  group is deleted; an in-progress download in a closed group is cancelled.
- Links and edges: a link to an outside node survives close and reopen; an `opened-from` child keeps its edge to a
  closed parent and reads it as none; a tab on a closed jar uses the Default and returns; the Default cannot end up in
  a group, including one already closed.
- Duplicate of a group with an unknown member, a jar and an outside link; session replacement deletes the old
  closed nodes; export skips them.
- Gate stage `closed`, then `gate all LOG full`.

## Decided and open

- Decided: the 14 decisions above.
- Parked: undo of Dissolve and merge (needs a recorded transaction mechanism); a shared-history UI; a redo stack;
  keeping scroll and navigation history across close.
- Open: whether the group card's X should become Close (children hidden) instead of Dissolve; whether a reopened
  node that lands at another level should navigate there (this spec reveals it without leaving the level).
