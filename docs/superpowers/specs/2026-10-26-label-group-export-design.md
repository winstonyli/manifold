---
status: draft
---

# Labels travel with a group export — Design

## Question

Export Group then Import Group (into another document, or the same one later) drops every card's labels: the export
leaves the label nodes out, so each membership names an id the target does not have and `Trust::Foreign` drops it
(ruling L6, `docs/superpowers/plans/2026-10-24-properties-3-labels.md`). This makes a group's labels part of its
export and, on import, reuses a label the target already has by name. It supersedes PARKED.md "Label export with a
subtree copy".

## Decisions (conversation 2026-10-26)

1. **The export carries the labels it uses (1a).** Ordinary `Label` nodes in the file, with name and colour. No new
   format section.
2. **Import reuses a same-named label (2a-i).** Matching is `names::name_key`, the rule L3 uses. The target's label
   and colour win; the memberships re-point to it. Done by a general reuse hook in `store::json::import`, not a label
   map and not a post-import rewrite: `import` is the one place a file id becomes a real id, so identity stays decided
   there. The parked "label name collisions" entry reuses the same hook.
3. **Scope: the group export and import routes only.** `export_group_json` and `import_group_json`. Duplicate
   (`paste_copy`) stays inside one document and already keeps labels. Whole-session import replaces the document and
   carries its own labels. No cross-document clipboard route exists.

## Design

- **Export (`store::json::subtree_to_json`).** After the subtree and its links, append every live `Label` node whose
  id appears as a `Seg::Id` in the `PROPS` tree of a node in the export (read from the `Doc`'s tree, the same
  `labels/@<id>` shape `Mirror::card_labels` reads; the plan names the accessor). Written like any node (`node_to_json(id,
  node, None)`: name and colour, no group). A deleted label is not live and is skipped (its memberships are hidden
  anyway, L4). A label no exported card uses is not exported. The root-first contract holds: labels come last.
- **Reuse hook (`store::json::import`).** The signature gains a `reuse: impl Fn(&JsonNode) -> Option<NodeId>`.
  The `fresh` map is built as `reuse(node).unwrap_or_else(next_id)`, so every later resolution (field ids, `PROPS`
  `@id` segments, link ends) follows it unchanged. `Imported` gains `reused: bool`, true for those slots, so the
  `imported[i]` is `json.nodes[i]` contract holds (`paste_copy` zips by index). A reused slot's `fields` and `trees`
  are still parsed and are ignored by the materializer; they are not written.
- **Existing callers.** `import(json, next_id, trust)` stays as `import_with(json, next_id, trust, |_| None)`'s
  thin wrapper, so the ~30 test callers and the other four production callers (`cli.rs`, `duplicate.rs`,
  `session_files.rs:436`, the gate) do not change. Only `import_group_json` calls `import_with` with a real hook.
- **The hook in `import_group_json`.** Before importing, build `live: HashMap<String, NodeId>` from `label_defs()` keyed
  by `name_key` (the closure cannot borrow `self` while `next_id` borrows it mutably). The hook returns
  `live.get(name_key(name))` for a `Label` node with a non-empty name, `None` otherwise.
- **`materialize_imported`.** Skips every `Imported` with `reused` set: nothing created, nothing written. Memberships on
  the imported cards already name the live label's id.
- **Docs.** Ruling L6 in the labels plan is history (status done); leave it. Delete PARKED.md "Label export with a
  subtree copy". README.md line 194 (the **Labels** paragraph) says "A subtree copy drops labels (parked: ...)": it
  becomes "A group export carries the labels its cards use, and an import reuses a same-named label". ROADMAP's v0.10
  label line loses "label export with a subtree copy".

## Edge cases

- **File label with no match:** created with a fresh id, name and colour from the file.
- **Two file labels, one live match each:** each reuses its own; source names are unique by key, so two never match
  one live label.
- **A file written by an older build (no label nodes):** the memberships still drop, as today.
- **A file label named only with characters `clean_name` removes:** `name_key` is empty, the hook returns `None`, the
  label imports under a fresh id as any label does.
- **Reopen Closed, Duplicate, the CLI, whole-session import:** no hook, so unchanged.
- **Format version:** additive (new nodes in a file type that already allows `Label` nodes); no version constant moves.

## Testing

- `store/json.rs`: a group export holds the labels its cards use and no others; a deleted label is not exported; the
  labels come after the subtree and links.
- `store/json.rs`: `import_with` whose hook maps a label to an existing id marks that slot `reused`, gives the cards'
  `PROPS` the existing id, creates no fresh id for it, and keeps `imported[i]` aligned with `json.nodes[i]`.
- `session_sync` tests (App level): export a labelled group from one app and import it into a second app with (a) no
  such label: the label exists afterwards with the file's name and colour and the card carries it; (b) a same-named
  label of another colour: no second label, the card carries the existing one, its colour is unchanged.
- The existing `import`/`subtree_to_json` tests pass unchanged.
- Gate: `light` (store, export and import paths). `gate/src/smoke.rs` and `gate/src/two/groups.rs` already drive
  Export Group and Import Group; they must still pass, and the plan adds a labelled card to one of them as the
  end-to-end check.

## Out of scope

Colliding names inside one document (PARKED "Label name collisions"), a jar-style fragment, label delete undo, any
cross-document clipboard route.
