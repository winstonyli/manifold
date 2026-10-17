---
status: done
---

# JSON "Expand as Graph" — Design

## Motivation

A JSON node today shows a compact card plus a popup collapsible-tree viewer
(`build_json_viewer`) for quick, read-only inspection. There's no way to treat
an individual value *inside* that JSON as a first-class graph citizen —
draggable, repositionable, color-taggable, or a jumping-off point for other
work (e.g. opening a tab/note anchored near a specific field) — the way any
other node already is. This feature lets a user explode a JSON node's
structure into real, nested graph nodes on demand.

## Scope decisions (confirmed during brainstorming)

- **Both** motivations apply: individual JSON values become real
  draggable/colorable graph nodes, *and* they act as anchors other work can be
  organized around.
- **Fully recursive**: every nested object/array becomes its own subgroup, all
  the way down — matches the graph's existing unlimited Group-nesting model.
- **Read-only**: exploded nodes are for navigation/organization only. No
  editing, no write-back to the source file. This eliminates an entire class
  of complexity (type/schema preservation, concurrent external edits, a bad
  write corrupting the user's file).
- **Opt-in, one-way**: the JSON node keeps its current card + popup viewer as
  the default. A new "Expand as Graph" context-menu action replaces it with
  the nested-subgroup form on demand. There is no "collapse back" — a user who
  wants the compact card back re-adds the file via "Add File...". This is a
  deliberate limitation, not an oversight.
- **No size cap**: a large/deep JSON can produce many nodes at once. Decided
  against a safety threshold — trusting the user to only expand JSON they
  expect to be reasonably sized, consistent with this being an opt-in action
  they explicitly requested.
- **Session-only, not persisted**: the exploded subgraph does not survive an
  app restart. Persisting it correctly would need either a JSON-pointer path
  back into the source file (re-navigate on load, silently wrong if the file
  changed shape) or an inline snapshot of every value in `session.json`
  (can drift from the real file) — real new session-format complexity for a
  read-only convenience view. Matches this app's own existing convention for
  display-scoped state (`pinned`, tag `color`, `zoom_factor` are all
  session-only for the same "smaller scope" reasoning).

## Data model: no new node kind

A container (`Object`/`Array`) becomes a `Group` node — reusing the existing
nesting model exactly as-is. A scalar leaf becomes a **plain `JsonPayload`**
node (the same `{ path, value }` struct the top-level JSON node already uses),
not a new dedicated type.

This was a deliberate choice over introducing a `JsonValuePayload` variant.
Adding a new `NodePayload` variant in this codebase is expensive: a new
struct, `as_json_value`/`as_json_value_mut` accessors, and a new match arm in
every one of the many exhaustive switches over node kind (rendering, drag,
filter, deletion, session export, …). Reusing `JsonPayload` costs nothing
there, and needs **no new rendering code at all** — `json_summary` and
`draw_json_value` (`main.rs`) are already generic over any `serde_json::Value`
shape, including a bare scalar, so a leaf node renders correctly today,
unmodified.

The one wrinkle: `path` on a leaf is inherited from the source file rather
than referring to something the leaf independently owns. This is documented
inline rather than hidden — "Reveal in Explorer" on a leaf still does the
correct, sensible thing (opens the source file's folder).

## Trigger

New "Expand as Graph" button in `build_json_context_menu`, alongside the
existing Delete/Reveal actions. Follows this file's established drain-then-
apply flag pattern: sets `expand_json_requested: Option<String>`, consumed
after the per-tab drain loop via `self.expand_json_as_graph(&id)`.

## Expansion algorithm

New method `expand_json_as_graph(&mut self, json_node_id: &str)`, modeled on
the existing `import_group_export` (`main.rs`), which already builds a Group
node plus a tree of children from external data:

1. Recursively walk the node's `JsonPayload.value`:
   - `Object` → a `Group` node named by its own key (an object's *entries*
     are each named by their key, recursively).
   - `Array` → a `Group` node named by its own key; its *elements* are each
     named by their index (`[0]`, `[1]`, …) instead, since an array has no
     keys of its own.
   - Scalar → a `JsonPayload` leaf, inheriting `path` from the source,
     `value` set to that scalar.
   - The root has no key of its own (nothing "contains" it) — named from the
     source file's own basename instead. If the root value is itself a bare
     scalar (a valid but unusual JSON document), "Expand as Graph" converts
     the node to a `JsonPayload` leaf with the same value it already had —
     a harmless no-op, not worth guarding against separately.
2. The **original** node is converted in place —
   `node.payload = NodePayload::Group(...)` — rather than removed and
   reinserted, so its existing `pos`/`group`/`color`/pin state (all live on
   the shared `GraphNode` wrapper, untouched by a payload swap) carry over
   for free.
3. Every new child gets a fresh id (`alloc_group_id`/`alloc_json_id`), a
   jittered position near its parent (`graph_layout::spawn_jitter`, offset
   from the parent's own position rather than world center), and
   `group: Some(parent_id)` — pushed onto `node_order`. The existing
   force-directed settle pass (already runs whenever the visible node set at
   a level changes size) spreads the new nodes out; no new layout code.

## Error handling

None needed. This operates purely on an already-successfully-parsed
in-memory `serde_json::Value` — no file I/O, no new fallible step. Combined
with the "no cap" decision, this method cannot fail.

## Lifecycle

One-way. Deleting the top Group cascades to the whole exploded subtree the
same way any other group-with-children deletion already does — no new
deletion logic required.

## Testing

- Build-verify with zero warnings.
- Live-verify headless: add a fixture JSON via `ADDFILE` (or equivalent),
  trigger the new action via a matching `DebugCommand` (same pattern as
  `DebugCommand::NewGroup`/`NewNote`), and confirm the resulting node
  count/nesting via `EXPORTSESSION`'s live dump — this reflects current
  in-memory state, not the persisted file, so it will show the synthetic
  subtree even though a restart won't.
