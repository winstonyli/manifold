# Properties, tags and mutable JSON: one mechanism (2026-10-24)

Status: decided in conversation, not built. A spec (`docs/superpowers/specs/`) and its measurement come next; this file is
the record of what was chosen and what was ruled out, so the spec starts from it.

## Decision

Per-card labels (find, glance, bulk action; they replace the card `color` field), arbitrarily nested card properties, mutable
Json cards and a possible future compute graph share one mechanism:

1. **A property tree per node, stored as path-keyed last-writer-wins registers.** Today's `Node.fields: BTreeMap<u32, Reg>`
   (`store/model.rs`) becomes keyed by a path (`field / key / key ...`); every existing field is a one-segment path. Objects
   are per-key registers. An array is a map of element id to value plus a fractional-index order register, so concurrent
   reorders converge. An empty `enc` still means absent.
2. **`Ref(node[, path])` is a value type**, beside `Id`, `Str` and the rest. Tags are `labels/<label-node-id>` entries on the
   card; a label node holds `name` and `color`, so a rename touches one node. A tab bar is a node whose `tabs` property is
   a list of `Ref(web card)`.
3. **A derived layer on top, never merged:** pure functions over the tree (search text, the label reverse index, compute
   outputs), invalidated by a change feed keyed by `(node, path)`. It replaces the once-a-second fingerprint poll in
   `search/sync.rs`. Derived values are not synced; an expensive result may be written once as a memo register keyed by an
   input hash.
4. **Order of work:** the path-keyed registers and change feed (journal `FORMAT_VERSION` 3 to 4, no migration code), then
   tags, then inline mutable Json cards. The compute graph is deferred; the base needs only refs and the change feed.

## Rulings

- **Refs do not own.** Deleting a list or an entry never deletes the card; a card's lifetime stays with `group` and
  `deleted`. One card can sit in many lists and carry many tags.
- **A dangling ref is hidden and kept.** An entry whose card is deleted is hidden by a derived rule and its register stays,
  so an undo restores it and replicas converge: the hide is computed from two independent registers, not from the order they
  arrived in. Cascading removal on delete was ruled out because replicas that see the add and the delete in opposite orders
  would disagree.
- **`group` stays.** A list of refs is an independent ordering, not group membership. See PARKED.md "Group membership as a
  list".
- **Size cap.** A Json card small enough to hold its leaves as registers is inline and mutable; a large imported file stays
  file-backed and read-only. The cap is set by the measurement.
- **Inline is a one-way detach.** No write-back to the file (the first JSON spec rejected it: type preservation, outside
  edits, a bad write corrupting the file).
- **Strings are one register.** Two edits to the same string lose one. Collaborative text inside a string is out of scope.

## Decided against

- **Everything as small nodes plus links** (a node per property or per JSON leaf). `Kind::JsonLeaf` was retired on
  2026-10-12 (`ceee913`) because every stored node entered the O(n^2) force layout.
- **Embedding Automerge or Yjs.** A second store beside ours with no refs and no derived layer, and a large dependency.
- **An owning list** (a card lives in one list; deleting the list deletes its cards). A card could not carry shared labels,
  and a concurrent move between lists would duplicate or lose it.
- **A per-feature subsystem** (a labels set, a JSON store and a compute store apart): the special-case growth the working
  rules forbid.

## What it cannot cover

Collaborative editing inside one string, true sequence editing with no interleaving of same-slot inserts, and nondeterministic
or side-effecting compute (that writes a memo register from one replica instead of recomputing everywhere).

## Measured (2026-10-24)

Run `2026-10-24-props-store-size3` (a throwaway test in `store/journal/`, not kept; the probe applied real `Op`s to a
real `Doc`, then took `snapshot_message`, `ops_message` and `decode_snapshot`). Memory counts requested bytes, not
the Windows heap's own overhead. The "+path" figures are computed, not measured: a register keyed by a path text instead
of a `u32` adds the key's bytes to the snapshot and to each op, and a boxed string to memory. Times were taken in the dev
profile at 29-39 % machine CPU, so only the ratios mean anything.

| Case | Heap | Snapshot | One edit (op frame) | Decode |
|---|---|---|---|---|
| 1,000 cards, no properties | 0.85 MiB | 132 KiB | 107 B (Create) | 5 ms |
| + 10 registers each | 2.2 MiB (+0.3 path) | 414 KiB (+163 path) | 56 B (Set) | 25 ms |
| + 10 property rows as nodes each | 9.7 MiB | 1.4 MiB | 104 B (Create) | 65 ms |
| 5,000-leaf document as registers | 0.6 MiB (+0.2 path) | 168 KiB (+126 path) | 60 B | 6 ms |
| 5,000-leaf document as nodes | 4.6 MiB | 705 KiB | 116 B | 31 ms |
| 100,000-leaf document as registers | 11.4 MiB (+3.8 path) | 3.5 MiB (+2.5 path) | 63 B | 154 ms |
| 100,000-leaf document as nodes | 88 MiB | 14 MiB | 120 B | 524 ms |

- A register costs about 140 B of memory (about 170 B with a path key); a stored node about 930 B. Registers are 5-8x
  smaller in memory (path keys included), 3-5x smaller in a snapshot, about 2x smaller per edit and decode 3x faster.
- A node also appends a `MoveRec` to `Doc.moves` (every `Create` applies a move), so 11,000 nodes hold 11,000 records.
- Not measured: the per-node cost in the session mirror and `App.nodes` (so the gap above is a lower bound), release-build
  timings, and the render cost of a large JSON level (it is virtual, so it does not depend on the store).
- Consequence for the cap: memory and decode would allow tens of thousands of inline leaves, so the cap is set by the
  JSON level (outline, search) and by journal compaction time, not by the store. 5,000 leaves is comfortable;
  100,000 is about 15 MiB. Pick the cap in the spec from the viewer's own cost.

## Open

The array order encoding, the path-key encoding on the wire (inline text per op against an interned path table in the
snapshot), and the cap value are for the spec.
