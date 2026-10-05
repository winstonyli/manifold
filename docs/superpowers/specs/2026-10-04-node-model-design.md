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

- Common data fields, on every node: `type`, `pos`, `color`, `name`, `pinned`, `closed`. (The per-kind carve-outs
  today, `pinned` only on tabs and `name` only on groups and jars, go away.) Lifecycle is live, closed, deleted: `closed` is a
  reversible boolean (section 5); `deleted` is the store's final tombstone (`store/ops.rs`: fields cleared, later Sets
  and re-Creates ignored). Only `closed` is writable by a verb; history eviction turns closed into deleted. The single
  per-node liveness gate (`Node::is_live`, `store/model.rs`) also checks `!closed`; hiding a closed node's descendants
  is `effective_group`'s job (section 5). "Not yet created" (a placeholder made by an op that arrives before its Create)
  means "has no `type`".
- Edge fields are stored per node but defined by the edge table (section 3): `group` (the `contains` edge), `parent`
  (the `opened-from` edge; today's `PARENT`), and later `jar`. A node carries one if the edge row's source pattern
  accepts it.
- One name precedence rule, the existing `names.rs` rules. A stored `color` layers over the row's default color.
- Unknown types resolve to the `*/*` row; their fields are kept raw. Unlike today (where `is_live` hides unknown kinds),
  they are shown as a generic card.

## 2. Capability table

Rows are plain data registered through `register(row)` (compiled-in now, so plugins can register later).

- **Match**: a row is `pattern` plus optional `requires` tags. Patterns, ranked low to high: `*/*`, type wildcard
  (`image/*`), suffix wildcard (`*/*+json`), subtype prefix, exact. A row's rank is the pair (pattern rank, number of
  `requires` tags), compared lexicographically, so tags break ties within a pattern rank and never outrank a more
  specific pattern. One shared
  `matches(pattern, type)` matcher serves capability and edge rows.
- **Layering**: all matching rows apply in rank order; a higher row overrides a lower row per field. List fields (such
  as `views`) append. Equal rank: the later registration wins; registering an identical pattern and tags twice is an
  error. Edge rows (section 3) are not layered: each is found by its edge-type id and uses `matches` only to check the
  endpoints.
- **Tags** (`Tag`, a closed plain enum in a bitset): backing `inline | file | web`; state
  `failed | loaded | engine-missing`; `file-missing`; content `blank` (a note with no text). Tags are runtime facts, not stored.
- **`caps(node)`** is read at use time, never cached on the node. No per-(type, tags) merged-row cache is built until
  profiling shows a need (about nine rows, so merging per call is cheap); amend this section then.
- **Flags after dedup**: `renamable`, `pinnable`, `portable` (duplicate and export), `restorable` (close is
  undoable; independent of `portable`), `activate` (default open action), `presentation` (icon, card style, `draw_layer` (replaces `GRAPH_DRAW_ORDER`), and a `runtime` field naming the
  runtime payload family), and `fields` (a list field: the extra stored fields this type carries beyond the common
  ones, such as `url`, `path`, `text`; section 6). Derived, not flags: containment/groupable and movable (from the `contains` edge row), copy
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

- **`Clear` is a read rule, not an op.** An edge whose target is not live (closed, deleted or missing) reads as none,
  and a Link node whose endpoint is not live is not shown. Nothing is written when the target closes, so undoing the
  close brings every such edge back for free. (Deleted targets use the same rule, the existing "missing means deleted"
  contract; eviction also deletes Link nodes that name a node it deletes.) Only the structural variants, `Adopt` and
  `Cascade`, change the document.
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
- **Verbs**, one implementation each, gated by a flag and (end state, section 6 item 10) written only through store
  ops: rename (`renamable`), pin (`pinnable`), duplicate (`portable`; copies the subtree with
  `create_ops(subtree, remap = true)`, the existing import path in `store/json.rs`), close, undo(entry), connect/disconnect (edge table; moving a node is
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
  node sets its `closed` field (section 1); the liveness gate hides the node and `effective_group` hides its subtree,
  which keep their Ids, fields and parent edge. Edges into the node simply read as none while it is closed (section 3,
  `Clear`) and resolve again after undo, and no serializer is involved.
- An entry is `{label, inverse: Vec<Op>}`, a transaction. A close records the `closed` Set, the ops of the structural
  removal variant (`Adopt`: the children's Moves and the jar fix-up below; `Cascade`: none), and the inverse of each.
  `Clear` records nothing. Undo applies the inverse
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
  resurface them), except descendants that are themselves closed roots of an entry still in the history; those keep
  their closed flag and their own entry. That is the only place closed becomes deleted. Emptying the history does the
  same.
- **No orphans.** Closed nodes live in the shared document but the history is per replica, so a lost local state would
  leave closed roots no entry can reach. On load, a closed root with no entry in the history gets one (labelled from
  its name), so it ages out through eviction like any other.
- Closing a loaded tab unloads its engine; undo restores it unloaded (today's behavior).
- History does not serialize subtrees; `create_ops` (section 4, duplicate and export) is not used by it.
- Only close records entries now (owner decision). Widening later means recording more transactions (Set and Move
  inverses are the old value), coalescing (one drag, or typing, is one entry), a redo stack and a UI; the data model does
  not change.
- The history is persisted in the session in the generic entry form; `closed.rs`'s per-kind entry type goes away.

## 6. Session format simplifications

The journal and session code shrink as a result of sections 1-5. No compatibility is kept.

1. **One spelling of a node's type.** The `type` MIME string replaces the numeric kind code (`KINDS`, `from_code`,
   `name`, `from_name`, `Kind::Unknown(code)`) in the journal, and the JSON export spells it the same way. If size
   matters, intern it in a per-file type table like the actor table. Done in plan 2a: `type` is field 19 and the
   numeric table is gone; Image is stored as `image/png` and Download as `application/octet-stream` until 2c;
   `FORMAT_VERSION` 2 refuses version-1 files (set aside, not migrated).
2. **Field lists are row data.** `kind_fields` (`session_sync.rs`) gives nine kinds near-identical lists. The row `fields`
   list appends by layer: `*/*` carries the common fields, file-backed rows add `url` and `path`, `text/markdown` adds
   `text`, and so on. `diff`, `fields_of` and `imported_fields` read the merged list.
3. **Common fields on every node** (section 1: `type`, `pos`, `color`, `name`, `pinned`, `closed`), so the common part
   of every `fields` list is written once, on `*/*`.
4. **Edge validity is one rule.** `diff` hard-codes `PARENT` (`valid(...)` on the create and set paths). The rule it
   applies, "a target that is neither live nor new reads as none", is the same as section 3's `Clear` read rule, so it
   is stated once and applied to every Id-typed edge field from the edge table (`jar`, link endpoints, later edges)
   instead of to `PARENT` alone. Write side versus read side: a closed target keeps the stored edge (reads as none,
   undo restores it), so `diff`'s `valid(...)` must treat a closed node as a valid target and write `none` only for a
   deleted or unknown one. Testing `is_live` there would erase every edge into a node the moment it is closed.
5. **No separate closed-entry format.** `LocalState.closed: Vec<JsonDoc>` (a JSON serialization of each closed subtree)
   and `closed.rs`'s entry type are replaced by the `closed` flag plus a short list of `{label, inverse}` entries.
   Decision for the plan: the list stays in local state as now (history is per replica); closed nodes are in the shared
   document either way.
6. **Live state is the document node.** The app builds `Shared { kind, group, fields }` from `NodePayload` and back
   (`tab_fields`, `tab_common`, `str_of`, `url_of`, `bool_of`, `pos_of`, `id_of`, `engine_field` and the 33 kind sites in
   `session_sync.rs`). With a generic node (type, fields) the live model is the fields map plus a runtime side-table
   (the row's `presentation.runtime`: engine handle, texture, load state), and that translation layer disappears. This
   is the largest cut and the largest refactor; it is plan 2 (see "Rename plan and order").
7. **One "viewed" id in local state.** `viewed_image` (an image-specific field) becomes `viewed: Option<Id>` for any
   node; with D it names the node a view is open on.
8. After C: `JAR_SET` and `DEFAULT_JAR` retire into the single `jar` edge field (18 fields to 16).
9. **`Create` drops its `kind` slot.** `type` is an ordinary field written at create, so `Create{node, fields}` is
   uniform. "Not yet created" (today `Kind::Unknown(0)`, `Node::placeholder`, and its compaction special case at
   `ops.rs:158`) becomes "has no `type`". Behavior change: unknown types are no longer hidden (`is_live` today rejects
   `Kind::Unknown`); they show as the generic `*/*` card and keep their raw fields. The create-without-kind half is
   done (2a); showing unknown types is 2c.
10. **Sync by ops, not by diff (decision for the plan).** Today the app mutates live state and `diff` (with the
    `explicit` set and `keep`) derives ops after the fact. That conflicts with history: an undo transaction needs the
    ops a verb produced, with the verb's label, and a diff has lost both. Two ways to reconcile:
    - *Verbs emit ops directly* (spec section 4 already says verbs write only through store ops). `diff`, `explicit`
      and `keep` disappear, and `FieldDef.explicit` goes with them. This is the clean end state and the bigger change.
    - *Keep `diff`, capture around the verb*: snapshot the affected fields before and after, and take the inverse from
      the pair. Smaller, but it leaves the second sync mechanism in place, and once plan 3 adds `closed`, `diff` would need a
      "closing" set like `keep`: a restorable node removed from live state must become `Set closed`, not the `Delete` that `diff` emits for any
      document node missing from `live` (`doc.live()` must keep excluding closed nodes so this holds afterwards).
    Decided: direct ops, in plan 2 together with item 6. History is built only in plan 3, on top of that, so the
    capture approach is never built.

Unchanged: the typed `FIELDS` table, `explicit` fields, `ENGINE` and `ENGINES` (until D), and `JsonLeaf`'s
`key`/`value`/`path` (a JSON-path node type is a separate idea).

## Rename plan and order

Reordered 2026-10-04 after reading the live model: the app keeps per-kind live state (`NodePayload`; selection is
`selected_tabs` and covers tabs only; pin is `pinned_mut()` on tab payloads; rename touches groups only; close destroys
the live node and records a JSON snapshot). Generalizing selection, pin, rename and duplicate in that model would add
fields to every payload variant only for the live-state rewrite to delete them, and the `closed` flag needs sync by ops
to avoid a throwaway capture mechanism. So the generic verbs and history come after the live-state rewrite. Each plan
leaves the app working:

1. **Plan 1, tables and behavior-neutral consumers.** The row table, edge table and `caps()` with the old enums still in
   place. Rows describe today's behavior (for example `pinnable` is true only for tabs); plan 3 widens them. Migrate the
   decisions that are pure per-kind `match`es (group close is Dissolve, `worth_reopening`, draw order), each guarded by
   a golden test that behavior is unchanged.
2. **Plan 2, live state is the node.** Split three ways on 2026-10-04 (the scope is about 316 `NodePayload`, 178
   `ItemNodeKind` and 589 `Kind::` sites), each leaving the app working:
   - **2a, store and journal:** `type` string in the journal replaces the numeric kind code, `Create` drops its `kind`
     slot (section 6 items 1, 9), unknown types show as a generic card. The app's live model is unchanged; the sync
     layer maps enum to type string at its edge.
   - **2b, live state is the node, and sync by ops (2b and 2c merged 2026-10-05):** document node plus runtime
     side-table replaces `NodePayload`, `ItemNodeKind`, `ContentKind` and `LevelItemKind`; mutation sites write ops
     directly, and `diff`, `explicit`, `keep` and `FieldDef.explicit` go (items 6, 10); the caps bridge is deleted;
     fixtures (`seed.json`, smoke and drive scripts, README, first-launch demo) are rewritten; a grep-guard test, in
     the style of the `APPDATA` guard in `profile::tests`, fails if any source still names the old enums (items 2, 3,
     7). Split from the separate "sync by ops" plan because the sync map (`2026-10-05-sync-flow-map.md`: about 60
     mutation sites in 20 files) shows that emitting ops while `NodePayload` still exists would rewrite every site
     twice. Staged inside the plan by field group, each stage leaving the app working.
     **Read model (decided 2026-10-05): an app-side decoded mirror.** The app keeps one `Node { ty, fields:
     BTreeMap<u32, FieldValue>, runtime }` per id and one writer, `App::apply(ops)`, which commits the ops to the
     journal and updates the mirror; remote changes update the mirror through the same function. Runtime state sits
     in a side-table keyed by id. Every read goes through a `Node::get`-style accessor, so a later step (option C)
     can make `Doc` the app's own state with decoded registers and delete the mirror by changing only where the
     accessor reads. Rejected: reading the journal's `Doc` per read (register decoding per read, awkward with no
     journal in tests), and regenerating `NodePayload` from the type table (keeps the translation layer).
3. **Plan 3, generic verbs and history.** Selection, pin, rename, duplicate and connect over any node (edit the rows);
   the `closed` field (`is_live`, `Doc::shown`, the closed-aware `effective_group`, compaction and pruning keep closed
   nodes, content-file ownership and local per-node state keep closed ids); transaction history replacing
   `closed.rs`'s snapshots and `LocalState.closed`.

Plan 1 done: 7396c8f (capability table), 0ebad52 (edge table). Plans 2 and 3 are written after it.

Plan 2a done: 8454d59 (type string), 9f82dc5 (version-1 files set aside).

Known consequences of 2a:

- A version-1 session file is set aside as `.unreadable-<secs>` and the next session starts fresh. On a later launch
  the fresh session names none of that file's fetched content or jars, so `prune_unnamed` / `prune_orphans` may delete
  them, including the cookie stores of jars the old session named.
- A named session saved as version 1 makes `read_intact` return None, which turns off content pruning while it exists
  (safe, but disk use grows).
- `Node::kind()` re-parses the type register per call (39 non-test callers via `is_live` / `doc.live()`). Cache only
  if a per-frame path shows it.
- A `Set` of `type` with empty enc would leave a hidden node that prune keeps. Nothing writes it today; revisit in 2b.

Carry-overs for plan 2:

- Sites still hard-coding the container rule, to move onto the edge table: `card.rs` `card_view` members collection
  (~929), `graph_ui.rs` EnterGroup on activate (~1646) and the "Enter" hint (~713), `main.rs` `destroy_node` Group arm
  (~2620), `session_sync.rs` groups-before-members load order (~796, ~1105, ~1543-1549), `del_verb` (`card.rs` ~521).
- `Tag::Loaded` is attached by `bridge::item_node` but no row reads it yet; fine until plan 3 reads it, otherwise drop
  it.
- The `contains` source list names `application/octet-stream` exactly; with sniffed file types (`video/mp4`,
  `text/plain`) it must widen to `*/*` minus jar and link.
- `Pattern::parse` accepts nonsense (`*/json`, `*/*+`) and `matches` is false for a slash-less mime against any
  non-`*/*` pattern; add validation when plugin rows (D2) can register.
- The spec says duplicate registration of "the same exact pattern and tags" is an error; reword to "identical pattern
  and tags" (the code rejects any identical pattern).

Gaps found while planning, ruled here: a `draw_layer` in `presentation` (today's `GRAPH_DRAW_ORDER`); a `blank` tag
(a note with no text is not restorable today); until plan 3, `Kind::Download` maps to `application/octet-stream` and
`Kind::JsonLeaf` to `application/vnd.manifold.json-leaf` (the section 1 table lists neither); `Doc::live()` must mean
"live and not under a closed ancestor" (`Doc::shown`), or `diff` would Delete a closed group's descendants.

## Error handling

- Unknown type: the `*/*` row (closeable, generic card), fields kept raw.
- A Link node or edge field naming a node that is not live: not shown, nothing written (section 3, `Clear`).
- Undo that fails to apply: the reason goes in the status line and the history entry stays.
- A connect the edge table rejects (endpoints, cycle): no op is written and the gesture reports why.

## Testing

Table-driven tests for matcher rank, tag raising and per-field override; edge cardinality, `acyclic` and each removal
variant; a close/undo round trip for every `restorable` type, including incoming edges, a closed group's subtree hidden and
restored, Dissolve and Close Contents undone, and eviction deleting the closed subtree but not a descendant with its own
entry; an edge into a closed node reading as none and back after undo; a closed root with no history entry gaining one
on load; type-string journal round trip and `Create` without a kind (unknown type shown as a generic card); duplicate
keeping outside-pointing edges; `diff` leaving an edge into a closed node unwritten; the verb gate over mixed
selections; and the grep guard. Tests never touch the real `%APPDATA%\manifold` (project CLAUDE.md).

## Open checks for the plan

- What does pinning do today beyond sort-first and the bulk-close exemption?
- Which kinds, if any, must refuse selection (the dropped `selectable` flag)?
- `deleted` is read directly only inside `store/` (11 sites in `ops.rs`, `model.rs`, `journal.rs`); confirm in the plan
  that every one is right for `closed` too (the fold, `effective_group`, compaction at `ops.rs:158` must keep closed
  nodes, and journal pruning must not drop them).
