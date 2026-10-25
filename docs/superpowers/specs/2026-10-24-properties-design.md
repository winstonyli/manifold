---
status: active
---

# Card properties, labels and inline Json cards (design, 2026-10-24)

The decision and its rulings are in `docs/decisions/2026-10-24-properties-tags-json.md` (with the size measurement); this
spec turns them into a design. It replaces the label-node-plus-`Link` design presented on 2026-10-24 (membership is now a
register on the card, not a `Link` node).

## Goal

1. **Labels** on cards, for find, glance and bulk action; they replace the per-card `color` field.
2. **Properties**: any card can carry a nested tree of values that merges per key, is searchable and is editable.
3. **Inline Json cards**: a Json card whose content is such a tree, so it is mutable and syncs, beside today's
   file-backed read-only Json card.

All three are one mechanism: a **tree value** stored as path-keyed last-writer-wins registers on the node, with `Ref`
leaves. Not in scope: the compute graph, group membership as a list (both in PARKED.md), collaborative text inside a
string, write-back to files.

## 1. Store

### 1.1 Register keys

`Node.fields` changes from `BTreeMap<u32, Reg>` to `BTreeMap<RegKey, Reg>`, where
`RegKey { field: u32, path: TreePath }` (`store/tree.rs`; the bytes of 1.3). Every existing field has an empty `path`, so each is a one-segment path and
behaves as before (`Node::get(no)` reads the empty path). Two new fields hold trees: `PROPS = 25` (annotations on any
card) and `BODY = 26` (an inline Json card's document). A non-empty path on any other *known* field is ignored by
`apply`; on a field number this build does not know it is kept raw, as unknown fields are now.

A `path` is a sequence of segments in an order-preserving tuple encoding (a terminator byte, with the terminator
escaped inside text), so a subtree is one key range and a sorted scan visits parents before children:

- `Key(text)`: an object key.
- `Id(NodeId)`: an element of an id-keyed map (a set, or an array). The writer mints the id like a node id.

### 1.2 Values

A register's `enc` holds one value, or is empty (a tombstone, as now):

`Null | Bool | Int(i64) | Float(f64) | Num(text) | Str | Ref(NodeId) | EmptyObj | EmptyArr` (the Rust type is `Leaf`)

`Num` keeps a number that is neither an `i64` nor a finite `f64` as its source text, so a JSON number round-trips.
`EmptyObj` and `EmptyArr` are leaf values: they are how `{}` and `[]` are stored, and any child written below them
replaces them (below).

### 1.3 Shape rules

- **Object**: a path with visible `Key` children. **Id-keyed map**: a path with visible `Id` children.
- **Array**: an id-keyed map whose elements are objects `{o, v}`: `o` is a fractional-index string that orders the
  element, `v` is its value. Concurrent reorders converge; two inserts at one slot order by id. A **set** (labels) is an
  id-keyed map whose elements are plain `Int(added_at_ms)` leaves (ruling L2, plan 3) and have no `o`; it sorts by `(added_at_ms, id)`.
- Mixed `Key` and `Id` children under one path show as an object with the ids as text. It is not an error.

### 1.4 Merge rule

Per path, last writer wins (`Reg.stamp`), as now. Across paths, one rule:

> A register at path `A` with stamp `s` hides every register at a proper descendant of `A` whose stamp is lower than `s`.

A register is **visible** when no register at a proper prefix of its path hides it, and it is not a tombstone. The value
at a path is: the object or id-keyed map formed by its visible children when there are any, else its own visible
register, else absent. So writing a child below a scalar turns the scalar into an object, and a delete is a tombstone at
the subtree root.

Properties this gives, each pinned by a test (section 6):

- The stored set is LWW-per-path of the ops, and visibility is a pure function of it, so any order and any repetition of
  the same ops give the same tree.
- A register at a path only ever gains stamp, so what it hides only grows (no container markers, which would break
  this: replacing a scalar by a marker would un-hide children that arrived earlier).
- Compaction (`Doc::prune`) drops the registers a prefix register hides (they can never become visible again), so equal documents still
  encode to equal bytes. Tombstones are kept (an older op that arrives late must still be hidden), one per deleted
  subtree root, bounded by deletes.
- In `BODY`, removing the last child of an object or array writes `EmptyObj` or `EmptyArr` at its path, so the emptied
  container keeps its place in its parent. A concurrent add under it that carries a lower stamp is hidden by that
  write: documented, not fixed. In `PROPS` an emptied map is simply absent, which keeps concurrent adds.
- Known quirk, documented not fixed: a scalar at `/a` whose replacement children were all tombstoned shows its old scalar
  again (a hidden register never reappears, since stamps only rise). `Node::tree` returns the ancestor scalar beside
  the visible children, so the reader (plan 2's resolver) applies "children win".

A node-level `Delete` still clears every register of the node, trees included, and later sets are ignored.

### 1.5 Operations and the file

- `OpBody::Set { node, field, path, enc }` gains `path: TreePath` (root for all current uses). The Set body
  (`{1: node, 2: field, 3: enc}`) gains an optional field 4 (the path), and a snapshot register
  (`{1: field, 2: stamp, 3: enc}`) likewise; both are free today. `Create`'s embedded field map stays path-less: a
  card made with properties (duplicate, import) is a `Create` followed by `Set`s.
- Journal `FORMAT_VERSION` 3 to 4, a **Breaking** line in the changelog. Older builds open a version 4 file read-only
  (the existing rule). A version 3 file is refused by a version 4 build and set aside (`read_readable_header`, the existing
  no-migration rule); see decision D1.
- No change to `PROTOCOL_VERSION`, `RENDER_PROTOCOL_VERSION` or the local store `FORMAT_VERSION`.
- Cost, measured (decision record): a register is about 140 B in memory (about 170 B with a path), a node about 930 B.

## 2. The app's side

### 2.1 The mirror

The mirror keeps the invariant it has: it equals the document except for edits still queued in `App::writes`. For
trees it holds each tree field's **live leaves** per `(node, field)` (`Mirror::tree`: `Tree`, a sorted map from `TreePath`
to `Leaf`, the registers that count in the document; `Trees` is the per-field map a `Snapshot` and `Carried` carry).
Readers call `tree::resolved`, which lets a child win over a scalar above it (plan 2 refined this from "the mirror holds
the resolved tree"; label order reads the membership leaf `added_at_ms`, L2). A remote change replaces the
tree with the document's and puts the edits still queued in `App::writes` back on top; a local edit applies to the tree
directly (a local write is the newest, so this equals what the document will compute; `tree::edit`).

Writers (the only way to change a tree): `App::edit_prop(node, field, path, Option<Leaf>)` queues the register and
updates the mirror, as `edit_field` does; `leaf == None` writes the tombstone, so a subtree delete is the same call.
Readers go through `Mirror` accessors (`tree`, `trees_of`), never the document.

Each tree has a **version** (`Mirror::tree_version(id, field)`): a tick of one monotonic clock, set by any change and never
reused (not even after a forget), 0 when the tree is empty, so fingerprints and the JSON level's assembled `Value` are
cached by version and never hash or rebuild from leaves.

### 2.2 Undo

One new `UndoKind::Props(Vec<PropEdit>)`, `PropEdit { node, field, path, before, after }`: the leaves at and under `path`
before, and the leaf after. The caller of `edit_prop` records it. Replay goes through `edit_prop` (a tombstone at `path`
and a write per arriving leaf) and is skipped when the mirror no longer shows exactly what is being left (the rule of
`replay_revert`). It serves labels, properties and Json edits alike: one arm for the whole feature. `UndoKind::Revert`
stays as it is (parked: unify it into register edits).

### 2.3 Copies, kind changes, export

- A duplicated card copies its trees: `Snapshot.trees` and `Carried.trees` carry them (a tab duplicate sets them on its
  spawning snapshot, which lands them in the mirror and creates them with the tab).
- `store/json.rs`: a tree is exported as a nested JSON object (`"props"`, `"body"`). A `Ref` is `{"$ref": "<id>"}`; an
  id-keyed map's keys are the element ids' text. Import gives every node a fresh id, so ids inside a tree are remapped
  through the file's id table like `parent` is. PROPS only (plan 2): `Id` segments are keys `@<id>` (a key starting with
  `@` gets one more, and so does `$ref` or `$$ref`), deeper leaves are not exported, empty containers are dropped and arrays refused; limits `MAX_DEPTH` 64 and `MAX_LEAVES` 10,000
  (`store/tree_json.rs`).

## 3. Labels

- **Label node**: a new `Kind::Label` (type string `application/vnd.manifold.label`, fields `NAME` and `COLOR`), a stored
  node that is never a graph card, like `Link`. `COLOR` stays on jars and labels, and is no longer read or written on
  other cards; old values are ignored. The mirror keeps a label registry (name, color) like `jar_def`.
- **Stored-only kinds (D7)**: about eight sites ask "is this a card?" by testing `Kind::Link` (`session_sync.rs:346-349`,
  `session_sync/remote.rs:50,282,315`, `store/json.rs:113,127,288`, `Doc::is_shown`). Label would be a second copy of each
  test, so first add `Kind::card()` (false for `Link` and `Label`) and use it at those sites; the tests that mean a
  link's own ends (`link_ends_among`, `links.rs`) stay link-specific. A registry node holding definitions in a tree was
  rejected: it needs a singleton every document must create and agree on.
- **Membership**: `PROPS / labels / Id(label) = Int(added_at_ms)` on the card (L2). Adding is a set, removing a tombstone, a rename
  is one edit of the label's `name`, and a delete of the label node leaves memberships that are hidden by the derived
  rule "a label that is not live is not shown".
- **Order**: by the leaf `added_at_ms` (a monotonic ms source), then the label id, so a card's first label is the
  one added first (decision D3). Re-adding a removed label puts it last.
- **Show**: the card border takes the first label's color; at normal zoom the bottom edge shows up to two chips, right-aligned and clipped to the card, and "+N";
  chips drop in dense views (above `LOD_SIMPLIFY_COUNT`), as the pin mark does. The per-card color cycle
  (`next_tag_color`, `carried.color()`) is removed.
- **Find**: the label names join `card_filter_text` and the index text (so the filter box and the omnibar match them),
  and the omnibar gains a `#label` token. The index fingerprint (`search/sync.rs` `index_print`) hashes a card's resolved
  label names (a handful) and the version counter of its `PROPS` text, so renaming a label reindexes its cards.
- **Act**: a "Label" button on the selection bar opens a menu to apply, remove, create, rename or delete a label across
  the selection; one undo step (`UndoKind::Props`).
- **Suggested groups**: `accept_suggestion` gets a second arm that applies a label.
- **Names**: a label's stored name must reach the screen through the name rule (`names::tests`' scan).
- **Scope** (D5): one global list; every stored card except jars; labels are not per jar.

## 4. Inline Json cards

- A Json card is **inline** when it has a `BODY` tree and **file-backed** when it has `PATH` (as today). The kind, the
  JSON level and its `JsonView` overrides serve both; the level reads its `Value` from `Mirror::body` for an inline card
  and from the file for the other.
- **Creating**: "New JSON" makes an inline card. "Add File..." is unchanged (file-backed). A new menu action **Detach to
  inline** copies a file-backed card's value into `BODY` and drops `PATH` (one way; there is no write-back to the file).
  An **Export** action writes a file. Detach refuses a document over `INLINE_MAX_LEAVES` (10,000, decision D4) with a
  message.
- **Editing** (built in 4b): edit a scalar in place, add a key, delete a key or element, insert
  and reorder array elements, change a value's type. Each edit is one undo step (one or more `edit_prop` calls).
- **Search**: the level's own worker search runs over the assembled `Value` as now. The global index gets the `PROPS`
  scalar text (capped at 4 KiB a card) but not `BODY` (parked).
- A tree's `Ref` leaf shows as a card chip that opens its target; a dangling `Ref` is shown as "missing" and kept.

## 5. Phases

Each ends green and could be tagged on its own.

1. **Store**: `RegKey`, paths, values, the merge rule, the file format, the tests in section 6. No UI.
2. **App** (plan 2, done; plan 1 is this section 1 only, and `Kind::card()` moved to plan 3): mirror trees and version counter, `edit_prop`, `UndoKind::Props`, duplicate and `Carried`, export and import.
3. **Labels**: `Kind::card()` first (D7), then `Kind::Label`, the menu and bar, chips and border, find, suggest arm,
   color removal, the `labels` gate stage.
4. **Inline Json**: **4a done 2026-10-25** (plan `docs/superpowers/plans/2026-10-25-properties-4a-inline-json.md`: `BODY`, the level reading it, New JSON, Detach to inline, the cap, export/import/duplicate). **4b done 2026-10-25** (plan `docs/superpowers/plans/2026-10-25-properties-4b-inline-json-editing.md`: the write path, undo for edits and Detach, the Export action, `Ref` chips, `PROPS` text in the global index).

## 6. Tests

- **Convergence** (phase 1): a seeded randomized test applies the same op multiset in shuffled orders, with duplicates,
  across 3 actors and mixed set, delete, scalar-over-subtree and subtree-over-scalar, and requires equal resolved trees
  and equal compacted bytes (compaction drops hidden registers, 1.4).
- Hidden-stays-hidden: a late older op under a tombstone stays hidden; compaction dropping dominated registers does not
  change the resolved tree; a tombstone survives a round trip.
- Path order: a subtree is one range; the tuple encoding sorts parents before children and never lets a key prefix
  another.
- Number round trip: `i64` extremes, `f64`, a 30-digit integer, `-0.0`.
- Journal: ops and snapshot round trip with paths; a version 3 file is refused; a version 4 file opens read-only in a
  build that reads 3 (the existing test pattern).
- `Mirror`: a local edit and the same edit applied from the document give equal trees; undo of `Props` skips a register
  that changed.
- Labels (phase 3): apply, remove, rename, delete the label, filter, `#label`, close and reopen a labelled card,
  restart. The gate stage clicks the new controls by name.
- Names scan, and `profile::tests` (no `APPDATA` mention) stay green.

## 7. Docs to change when it lands

README (store, keys, selection bar, Json cards); the frame spec's "tag color"; the node-model spec's edge table (a
label is not a link); `CHANGELOG.md` at the bump (user's call): **Breaking: Journal format 3 -> 4**.

## 8. Decisions (settled 2026-10-24)

- **D1** A version 3 session is set aside by the new build, so export it first with the current build (`--import-json`
  brings it back). The new build does not read 3.
- **D2** The mirror holds the live leaves (an extra copy, as it does for every field; refined in plan 2: readers resolve); it keeps `App::default()` tests
  and the accessor rule.
- **D3** Label order by when the membership was added (its stamp, no extra register), not by label id (arbitrary: ids
  order by a random actor first). An explicit order register can come later without rework.
- **D4** `INLINE_MAX_LEAVES` = 10,000. The store would allow 100,000 (about 15 MiB); the limit is the viewer's cost, to be
  re-measured in phase 4.
- **D5** Global labels; all stored cards except jars; border from the first label; two chips and "+N". Jars keep their own
  `COLOR` and get no implicit chip.
- **D6** "Add File..." keeps making file-backed cards; inline comes from "New JSON" and Detach.
- **D7** Add `Kind::card()` and move the "not a card" tests onto it before adding `Label` (section 3): a second copy would
  be the third item of one shape.

Not in this spec: inherited context (a value climbing `group`); parked with the group rework (PARKED.md "Inherited
context helper").
