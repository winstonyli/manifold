# Node model (sub-project A): MIME identity, capability table, edge-type table

Status: design, awaiting owner review (2026-10-04). Sub-project A of the homogeneity vision in
`2026-10-04-homogeneity-decisions.md`: anything (tabs, groups, cookie jars, files, JSON values, notes) can be
manipulated and connected homogeneously.

## Goal

Replace the five app-layer kind enums (`Kind`, `NodePayload`, `ItemNodeKind`, `ContentKind`, `LevelItemKind`) and the
per-kind `match`es with three data tables keyed by MIME type: identity, capabilities, edges. Selection, pinning, the
menu, duplicate, close and undo become generic verbs over those tables.

## Non-goals and decisions

- No migration, no compatibility, no stability promise. There is no session that must survive the change; the rename is
  as intrusive as needed (owner, 2026-10-04).
- Out of scope, each its own spec: B links (connect gesture UI), C jars as nodes, D views registry (in-process view
  trait, IPC hosts as proxies), D2 external plugins, E broader undo. Order: A, then B and E, then C, then D, then D2.
- Jars are left alone by A: A reserves their type string and nothing else; the `jar` edge row, `JAR_SET` removal and
  anything else about jars belong to C. (The store already has `Kind::Jar`; the plan checks what C actually changes.)
- Bookmarks are not nodes yet.

## 1. Identity

A node's identity is its MIME type, a common field replacing `kind`. Manifold's own kinds use vendor types:

| Node | Type |
|---|---|
| tab (one merged type; "loaded" is runtime state) | `application/vnd.manifold.tab` |
| group | `application/vnd.manifold.group` |
| cookie jar | `application/vnd.manifold.jar` |
| link | `application/vnd.manifold.link` |
| note | `text/markdown` |
| JSON value | `application/json` |
| PDF | `application/pdf` |
| images | real `image/*` |
| other files | sniffed MIME (existing `content.rs` table), else `application/octet-stream` |

- Common fields: `type`, `name`, `color`, `pinned`, `closed`. Lifecycle is live, closed, deleted: `closed` is a
  reversible boolean (section 5); `deleted` is the store's final tombstone (`store/ops.rs`: fields cleared, later Sets
  and re-Creates ignored). Only `closed` is writable by a verb; history eviction turns closed into deleted. The single
  per-node liveness gate (`Node::is_live`, `store/model.rs`) also checks `!closed`; hiding a closed node's descendants
  is `effective_group`'s job (section 5). Edge fields (parent, opened-from, jar) are not common fields; they belong to
  the edge table (section 3).
- One name precedence rule, the existing `names.rs` rules. A stored `color` layers over the row's default color.
- Unknown types resolve to the `*/*` row; their fields are kept raw, as unknown kinds are today.

## 2. Capability table

Rows are plain data registered through `register(row)` (compiled-in now, so plugins can register later).

- **Match**: a row is `pattern` plus optional `requires` tags. Patterns, ranked low to high: `*/*`, type wildcard
  (`image/*`), suffix wildcard (`*/*+json`), subtype prefix, exact. Each `requires` tag raises rank. One shared
  `matches(pattern, type)` matcher serves capability and edge rows.
- **Layering**: all matching rows apply in rank order; a higher row overrides a lower row per field. List fields (such
  as `views`) append. Equal rank: the later registration wins; registering the same exact pattern and tags twice is an
  error.
- **Tags** (`Tag`, a closed `#[non_exhaustive]` enum in a bitset): backing `inline | file | web`; state
  `failed | loaded | engine-missing`; `file-missing`. Tags are runtime facts, not stored.
- **`caps(node)`** is read at use time, never cached on the node; a per-(type, tags) merged-row cache backs it.
- **Flags after dedup**: `renamable`, `pinnable`, `portable` (duplicate and export), `restorable` (close is
  undoable; independent of `portable`), `activate` (default open action), `presentation` (icon, card style, and a `runtime` field naming the
  runtime payload family). Derived, not flags: containment/groupable and movable (from the `contains` edge row), copy
  forms (from backing). There is no `selectable` flag until a kind needs to refuse selection.
- The `*/*` row sets `renamable` and `pinnable` true and `portable`, `restorable` false: an unknown node's raw fields
  are not validated, so it can be closed for good but not duplicated or undone.

## 3. Edge-type table

An edge type is a row: endpoints (patterns plus tags), cardinality (`at-most-one` is a field on the source; `many` is
a Link node), `acyclic`, `on-target-removal` (`Clear | Cascade | Adopt`, named variants a verb may select),
`drawing`, `user-creatable`. Contains and opened-from share a row template.

Built-in rows:

| Edge | Source | Target | Cardinality | Removal | Notes |
|---|---|---|---|---|---|
| `contains` | tab, group, note, file, ... | group | at-most-one, acyclic | Adopt (Cascade for "Close Group and Contents") | nests inside the card |
| `opened-from` | any | any | at-most-one, acyclic | Clear | drawn as a line; persists for all kinds |
| `jar` | tab, group | jar | at-most-one | Clear | added by sub-project C, not A; effective jar = own edge, else nearest ancestor via `contains`; edge presence means explicit (replaces `JAR_SET`, which stays until then) |
| `link` | any | any | many | Clear | stored as a Link node |

- `Clear` on a many-cardinality edge closes the Link node itself (recorded in the transaction, so undo brings it back).
- A Link node (`application/vnd.manifold.link`) has `from`, `to`, `edge-type`, and the common `name`. Any later
  many-to-many type (such as `references`) uses the same storage with its own `edge-type` id.
- The connect gesture offers the edge types whose endpoints accept the pair. Cycle checks read `acyclic`.
- Removal policy lives only in the row. Verbs pick a variant by name: Dissolve is `Adopt`, Close Contents is `Cascade`,
  both defined on `contains`.
- Storage: `contains` is the store's tree edge (the group field, changed only by Move, cycle-checked by the store's
  move). Every other at-most-one edge is an Id field changed by Set, and its `acyclic` check runs in the app layer.
- Duplicate and import remap Id fields only for nodes inside the copied subtree (`store/json.rs`). Edge fields that
  point outside it (a `jar`, an `opened-from` target) keep pointing at the original; on export to a file they are
  cleared, since the target is not in the file. A Link node is copied only when both endpoints are inside the subtree.

## 4. Selection, pinning, verbs

- **Selection** is UI state, a set of node ids of any mix of kinds. It replaces the separate tab and group selection
  paths.
- **Pinning** is the common `pinned` boolean on any `pinnable` row. Effect, identical for all kinds: sorts first in its
  container and is skipped by "close others" and bulk close. If pinning today also does something else, that effect
  goes in the row's `pinnable` entry, not in the verb (check during planning).
- **Verbs**, one implementation each, gated by a flag and written only through store ops: rename (`renamable`), pin
  (`pinnable`), duplicate (`portable`), close, undo(entry), connect/disconnect (edge table; moving a node is
  `connect(contains)`, so endpoint patterns and the acyclic check apply to drags), activate (open with the row's
  default view; rows with no view keep an explicit action, such as group enter or PDF reveal until D), open with (empty
  until D).
- A menu entry appears only if every selected node allows the verb. Pin and unpin are one entry: "Pin" if any selected
  node is unpinned, else "Unpin". Per-kind behavior comes from rows and edges, never
  from a `match` on kind.

## 5. History: delete-only undo, built to widen

One history replaces both undo and Recently Closed.

- **Close is reversible by a flag, not by deletion.** The store's Delete is final (fields cleared, later Sets and
  re-Creates ignored, `store/ops.rs`), so a close cannot be undone by re-creating the node. Closing a `restorable`
  node sets its `closed` field (section 1); the liveness gate then hides the node and its whole subtree, which keep
  their Ids, fields and parent edge. Incoming edges therefore still resolve after undo, and no serializer is involved.
- An entry is `{label, inverse: Vec<Op>}`, a transaction. A close records the `closed` Set, the removal-variant ops of
  section 3 (cleared fields, adopted children, closed Link nodes), and the inverse of each. Undo applies the inverse
  ops as new ops with fresh timestamps (consistent with LWW); the Recently Closed menu applies any chosen entry through
  the same path. Undoing a node whose old parent is gone (deleted) shows it at the nearest live ancestor, by the
  store's existing effective-group rule; if the parent is still closed, the node is restored but stays hidden with it
  until the parent is undone.
- **Closed hides the subtree; deleted does not.** A deleted group passes its children up to the nearest live ancestor
  (`Doc::effective_group`). A closed group hides its descendants with it, and only the root is flagged. So
  "Close Group and Contents" (`Cascade`) is one Set. A plain close and "Dissolve" are `Adopt`, today's behavior (a
  deleted group's children survive; the card's close button on a group is `NodeAction::Dissolve`, `card.rs:501`): Move
  the children to the group's parent, then close the group; both undo by inverse ops. Until C removes `JAR_SET`,
  `Adopt` keeps `dissolve_group`'s side effect (unset tabs re-snapshot their jar from the new level), and the old jar
  fields it writes are part of the recorded inverse. `effective_group` gets a closed-aware branch for this.
- A kind with `restorable` false is deleted for real on close, with no entry; the close confirmation says so.
- **Eviction** keeps the history bounded: when an entry falls off (`MAX_CLOSED` in `closed.rs`, currently 20), the
  whole closed subtree is Deleted for real, not just its root (a deleted root would otherwise pass its children up and
  resurface them). That is the only place closed becomes deleted. Emptying the history does the same.
- Closing a loaded tab unloads its engine; undo restores it unloaded (today's behavior).
- `create_ops(subtree, remap)` is not used by history. Duplicate and export use it with `remap = true` (the existing
  import path in `store/json.rs`).
- Only close records entries now (owner decision). Widening later means recording more transactions (Set and Move
  inverses are the old value), coalescing (one drag, or typing, is one entry), a redo stack and a UI; the data model does
  not change.
- The history is persisted in the session in the generic entry form; `closed.rs`'s per-kind entry type goes away.

## 6. Session format simplifications

The journal and session code shrink as a result of sections 1-5. No compatibility is kept.

1. **One spelling of a node's type.** The `type` MIME string replaces the numeric kind code (`KINDS`, `from_code`,
   `name`, `from_name`, `Kind::Unknown(code)`) in the journal, and the JSON export spells it the same way. If size
   matters, intern it in a per-file type table like the actor table.
2. **Field lists are row data.** `kind_fields` (`session_sync.rs`) gives nine kinds near-identical lists. The row `fields`
   list appends by layer: `*/*` carries the common fields, file-backed rows add `url` and `path`, `text/markdown` adds
   `text`, and so on. `diff`, `fields_of` and `imported_fields` read the merged list.
3. **Common fields on every node:** `pos`, `color`, `name`, `pinned`, `closed`, `parent`. The per-kind carve-outs
   (`pinned` only on tabs, `name` only on groups and jars) go away.
4. **Edge validity is generic.** `diff` hard-codes `PARENT` (`valid(...)` on the create and set paths); it iterates every
   Id-typed edge field from the edge table instead, so `jar`, `link` endpoints and later edges get "a target that is
   neither live nor new is written as none" for free.
5. **No separate closed-entry format.** `LocalState.closed: Vec<JsonDoc>` (a JSON serialization of each closed subtree)
   and `closed.rs`'s entry type are replaced by the `closed` flag plus a short list of `{label, inverse}` entries.
   Decision for the plan: the list stays in local state as now (history is per replica); closed nodes are in the shared
   document either way.
6. **Live state is the document node.** The app builds `Shared { kind, group, fields }` from `NodePayload` and back
   (`tab_fields`, `tab_common`, `str_of`, `url_of`, `bool_of`, `pos_of`, `id_of`, `engine_field` and the 33 kind sites in
   `session_sync.rs`). With a generic node (type, fields) the live model is the fields map plus a runtime side-table
   (the row's `presentation.runtime`: engine handle, texture, load state), and that translation layer disappears. This
   is the largest cut and the largest refactor; it lands in step 5 of the plan, once consumers read `caps()`.
7. **One "viewed" id in local state.** `viewed_image` (an image-specific field) becomes `viewed: Option<Id>` for any
   node; with D it names the node a view is open on.
8. After C: `JAR_SET` and `DEFAULT_JAR` retire into the single `jar` edge field (18 fields to 16).

Unchanged: the typed `FIELDS` table, `explicit` fields, `ENGINE` and `ENGINES` (until D), and `JsonLeaf`'s
`key`/`value`/`path` (a JSON-path node type is a separate idea).

## Rename plan and order

Each step leaves the app working:

1. Add the row table, edge table and `caps()` with the old enums still in place.
2. Switch consumers to `caps()` kind by kind, starting with the menu (`node_menu.rs`; heaviest sites: `node.rs` 58
   `NodePayload` matches, `session_sync.rs` 33, `fetched.rs` 30).
3. Add generic selection and the verbs.
4. Add the `closed` field (`is_live`, the closed-aware `effective_group`, compaction and pruning keep closed nodes),
   then move undo and Recently Closed to transactions.
5. Remove the five enums, rewrite fixtures (`seed.json`, smoke and drive scripts, README, first-launch demo) and add a
   grep-guard test, in the style of the `APPDATA` guard in `profile::tests`, that fails if any source still names the
   old enums.

## Error handling

- Unknown type: the `*/*` row (closeable, generic card), fields kept raw.
- Undo that fails to apply: the reason goes in the status line and the history entry stays.
- A connect the edge table rejects (endpoints, cycle): no op is written and the gesture reports why.

## Testing

Table-driven tests for matcher rank, tag raising and per-field override; edge cardinality, `acyclic` and each removal
variant; a close/undo round trip for every `restorable` type, including incoming edges, a closed group's subtree hidden and
restored, Dissolve and Close Contents undone, and eviction deleting the whole closed subtree; duplicate keeping
outside-pointing edges; the verb gate
over mixed selections; and the grep guard. Tests never touch the real `%APPDATA%\manifold` (project CLAUDE.md).

## Open checks for the plan

- What does pinning do today beyond sort-first and the bulk-close exemption?
- Which kinds, if any, must refuse selection (the dropped `selectable` flag)?
- `deleted` is read directly only inside `store/` (11 sites in `ops.rs`, `model.rs`, `journal.rs`); confirm in the plan
  that every one is right for `closed` too (the fold, `effective_group`, compaction at `ops.rs:158` must keep closed
  nodes, and journal pruning must not drop them).
