# Links and menu-driven edges (sub-project B, design)

Status: Implemented, 2026-10-07 (plan `docs/superpowers/plans/2026-10-06-links.md`, whose Outcome lists the deviations
and parked items): f84961a and d1f4fe5 (Link node), ae0463d (edge table rows), 42ea6c9 and 3faf717 (connect,
disconnect, edges_of), cf6ea35 (drawing), 0121399 and fbcf9e9 (close, import, export, EXPORTSTATE), 5ce9988 (menu,
pick-target mode, debug commands), and the probe/docs commit "Links: probe, screenshots, docs". Designed 2026-10-06.
Builds on `2026-10-04-node-model-design.md` (sections 3 and 4) and
`2026-10-04-homogeneity-decisions.md`. Architectural: a new node kind, new edge-table columns, and a connect/disconnect
path that existing code (`edit_parent`, drag reparent) moves onto.

## Problem

1. A user cannot make or remove an edge between two nodes except by one drag gesture (`DropEffect::Reparent`,
   `graph_nav.rs`), which sets `parent` ("opened from") only. There is no any-to-any relation.
2. The edge table (`caps/edges.rs`) is a pattern matcher with one row, `contains`. The node-model spec's columns
   (cardinality, `acyclic`, drawing) and its `opened-from` and `link` rows are not built.
3. Vision (homogeneity): any edge type should be as easy to make and remove as any other.

## Decisions (agreed 2026-10-06)

1. **Scope: `link` plus `opened-from`.** The new `link` edge, and the existing `parent` field moved onto the same
   connect and disconnect path. `contains` (group membership), `jar` (sub-project C) and the drag gestures' own
   behavior stay as they are; they become table rows later without changing this design.
2. **Menus only in v1.** A node's context menu gets "Link to…", "Opened from…" and one "Disconnect from X" per live
   edge of the node. Drag-to-connect and click-on-a-line-to-disconnect are a later revisit (see Parked).
3. **`link` is undirected, plain, many.** Any node to any node, any number per node, no arrowhead, no meaning. It is
   stored with `from` and `to` (the node that started it is `from`), so a direction can be added later without a
   migration. Naming a link is out of v1.
4. **Links do not move nodes.** `affects layout` is a table column: true for `opened-from` (springs, as today), false
   for `link` (drawn only).
5. **Same level only.** "Link to…" and "Opened from…" offer targets at the current level (same group). A link between
   levels is not offered in v1, and the read rule below hides one that arrives by sync from a replica that has it.
6. **No type chooser.** The menu has one entry per user-creatable edge type, so there is nothing to ask when more than
   one type fits a pair.

## Design

### Edge table

`EdgeRow` gains the columns B needs, and only those: `storage` (`Field(field_no)` for an at-most-one edge stored on the
source, `Node(type)` for a many edge stored as its own node), `acyclic`, `affects_layout`, `user_creatable` and a
`verb` (the menu text). Rows:

| Edge | Source | Target | Storage | Acyclic | Layout | User-creatable |
|---|---|---|---|---|---|---|
| `contains` | as now | group | the group link (store tree edge) | yes | no | no (own gestures) |
| `opened-from` | every card type | every card type | `Field(PARENT)` | yes | yes | yes, "Opened from…" |
| `link` | every card type | every card type | `Node(TYPE_LINK)` | no | no | yes, "Link to…" |

Cardinality follows from `storage`. The other spec columns (removal variants beyond the read rule, `drawing`) are not
built until something needs them.

### The Link node

A stored node of type `application/vnd.manifold.link` (`TYPE_LINK`, `Kind::Link`) with fields `from` and `to` (node
ids) and the common `name` (unused in v1). Like a jar, it is store and mirror state and never a graph card
(`materialize` takes an early path as it does for `Kind::Jar`; the plan confirms every `match` on `Kind` handles it).
Create, delete, sync and the mirror's per-field rules are the 2b machinery unchanged. Unknown-to-older-builds nodes of
this type stay raw (`Kind::Unknown` handling).

### Connect and disconnect

One pair of functions on `App`, the only way the menus and the drag reparent change an edge:

- `connect(edge, from, to) -> Result<(), ConnectError>`: checks the row's endpoint patterns, `acyclic` (for
  `opened-from`, `would_create_cycle`), that both nodes are live and at the same level, and that no live edge of that
  type already joins the pair (for `link`, either order). Then it writes by `storage`: `edit_parent(from, Some(to))`
  for a field edge, a created Link node for a node edge.
- `disconnect(edge, from, to)`: `edit_parent(from, None)`, or a queued delete of the Link node.
- `edges_of(node) -> Vec<EdgeRef>`: the live edges touching a node (its own opener, the nodes it opened, its links),
  for the "Disconnect from X" entries.

`DropEffect::Reparent` calls `connect(opened-from, …)`; its own cycle check is replaced by the shared one.

### Reading and lifecycle

- **A Link node is shown only while both endpoints are live nodes at the viewed level** (the spec's `Clear` read rule;
  nothing is written when an endpoint closes).
- **Delete** of a node also queues the delete of every Link node naming it. The plan finds the one funnel (`remove_node`,
  `drop_node`, `destroy_node`) and the debug invariant `check_graph_matches_document` catches a missed path.
- **Recently Closed** (`closed.rs`) snapshots a closed subtree with fresh ids on reopen. A Link node is recorded in the
  entry only when both endpoints are inside it; a link to a node outside the closed subtree is lost when the node is
  closed (**correction to the node-model spec**, which assumed the `closed` field of its plan 3; until then a close is a
  delete).
- **Duplicate and import** copy a link only when both ends are inside the copied subtree; **export to a file** drops
  links whose far end is not in the file (node-model spec, section 3).
- **A dangling Link node** (an endpoint deleted on another replica after the link was made) is hidden by the read
  rule. Whether it is also swept is left to the plan (it is harmless if not).

### Drawing and layout

`level_edges()` gets a second source, each shown Link node, and `LevelEdge` gains a `kind` so links draw in a distinct
style from derived edges (same curve machinery). The layout's spring list (`graph_nav.rs`, the `edges` vector for
`graph_layout::step`) keeps reading only edges whose row has `affects_layout`, so links never move a node.

### Menus and pick-target mode

- The node context menu (`node_menu.rs`, through `MenuGrid` and `menu_section`) gains a section: one row per
  user-creatable edge type that has at least one valid target at this level, then "Disconnect from X" per live edge.
  The row is gated by the selected node's capability, per the node-model spec's "every selected node allows the verb"
  rule (v1: a single node).
- Choosing a verb enters a **pick-target mode** (`App` UI state): valid targets are highlighted, a click on one calls
  `connect`, Esc or a click on empty canvas cancels. The mode ends on any other action.
- Hidden in a JSON level (its cards are not stored nodes).

## Out of scope / parked (revisit)

- **Drag to connect, and click on a line to disconnect** (the owner wants these next; they call `connect` and
  `disconnect`, so nothing here is thrown away).
- **Directed links** (an arrowhead), **named links** (rename from the line), cross-level links.
- **`contains` and `jar` as table rows** driven by the same verbs (node-model plan 3 and sub-project C).
- **The `closed` field** (node-model plan 3), which lets a link survive its endpoint's close and an undo.
- Multi-select connect ("link these three to X").

## Risks and open questions

- **Two lines on one pair.** A node pair can be `opened-from` and `link`ed at once, drawing two lines. Accepted; the
  styles differ, and the menu says which is which.
- **Moving drag-reparent onto `connect`** changes a path that has tests (`graph_nav`, `level`, `fetched`); the plan
  keeps their behavior and asserts it.
- **`Kind::Link` ripple.** Every `match` on `Kind` and the `TYPES` table (`[…; 9]`) change. A grep guard or the
  compiler's exhaustiveness covers it; the plan lists the sites.
- **Hit areas.** None: v1 has no line interaction.

## Testing

Rust tests, headless (the existing `App::default()` and `app_on` helpers):

- Table: `can_connect` for `opened-from` and `link` over every card type pair; `link` refuses a jar or a link node.
- `connect(link)` creates one Link node; a second in either order is refused; `disconnect` removes it; both syncs
  through the store (save, restore in a second app).
- `connect(opened-from)` sets `parent`, refuses a cycle, and `DropEffect::Reparent` goes through it with the existing
  tests unchanged.
- A link is not shown (and not in `level_edges`) when an endpoint is at another level or not live; deleting an endpoint
  queues the link's delete; closing then reopening a subtree restores a link inside it and drops one to outside.
- Layout: `level_edges` includes links; the spring list does not.
- The pick-target mode (state transitions only: enter, valid targets, complete, Esc).
- Menu: the section's rows appear only with valid targets (`build_node_context_menu` tests as in `node_menu.rs`).

Live: a headless probe that creates two tabs, links them through the debug commands, checks EXPORTSTATE for the Link
node and the drawn edge, restarts and checks both survive; a `chrome-shots` view of the menu section and one of a
linked pair.

## Files expected to change

`caps/edges.rs` (columns, rows), `store/model.rs` (`TYPE_LINK`, `Kind::Link`, `TYPES`), `session_sync.rs` (materialize,
delete funnel), `graph_nav.rs` (`connect`, `disconnect`, the reparent drop, the spring list), `level.rs`
(`level_edges`), `graph_ui.rs` (edge style, pick-target mode), `node_menu.rs` (the section), `closed.rs` (links in an
entry), `store/json.rs` (duplicate, import, export rules), `debug_control.rs` (EXPORTSTATE, probe commands), tests,
`scripts/` probe and chrome-shots views, README.
