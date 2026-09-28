# JSON node rework: record cards and an outline

Status: implemented, 2026-09-27. Mockups:
`.superpowers/brainstorm/16377-1790563937/content/json-level-v2.html`
(git-ignored).

## Goal

A JSON node's graph *is* its viewer. Double-clicking one enters its
level, the same way double-clicking a group does. Every object and
array on that level is a record card, and an outline panel beside the
canvas gives the same tree as text for scanning, search and copying.
The level is read-only. Nothing about it is stored except the view
overrides the user makes: position, colour, collapse and pages.

## Today

- The Json card (`card.rs` `paint_json_body`, a fixed 160×90) shows one
  line: an icon and `{ N }` / `[ N ]`.
- Double-clicking it opens `build_json_viewer` (`popups.rs`), a
  560×520 popup holding a collapsible text tree (`draw_json_value`) that
  starts with only the root open.
- "Expand as Graph" (`expand_json_as_graph`, headless
  `EXPANDJSONASGRAPH`) is opt-in and one-way. It replaces the node with
  a real group tree: containers become `from_json` groups and scalars
  become `JsonLeaf` nodes, all stored, all laid out by the O(n²) force
  settle, and all placed by jitter around their parent.
- A tab whose page is JSON has a "View as JSON" chip, even though
  content dispatch now converts such a tab into a Json node.

## The JSON level

Double-clicking a Json node sets `current_group` to its id. The canvas
then shows that node's value as cards, and the outline appears.
Leaving works exactly as it does for a group.

### Cards

- **One card for each container.** An object's rows are its entries,
  keyed by name. An array's rows are its elements, keyed by index (gray)
  and tinted slightly bluer so the two kinds read apart.
- **Scalar rows** show the value coloured by type: string, number,
  bool or null.
- **Container rows** show a label and `{n}` / `[n]`, with a port on the
  right edge. An edge runs from that port to the child's card. Nested
  arrays need no special case.
- **Label rule**, used for element rows and collapsed headers: the
  first of `name`, `title`, `label`, `id` or `key` whose value is a
  scalar, otherwise the first string of 40 characters or fewer. An
  array's label is an inline preview such as `[-0.12, 51.5]`, of 30
  characters or fewer. If neither exists there is no label.
- **Paging.** A container with more than 20 entries shows 20 rows and
  then a `+ N more` row, which reveals 20 more per click. This applies
  to objects as well as arrays. A child card exists only for a visible
  row.
- **Collapse.** Double-clicking a card's header toggles it. A collapsed
  card is just its header with its label, and its subtree is hidden.
- **Default expansion.** When a level opens, a breadth-first pass
  expands cards until about 150 are visible or depth 3 is reached.
  Everything beyond starts collapsed. Saved overrides win.
- **Sizing.** Width is fixed at 220. Height is the header plus 22 px per
  row. Long values are ellipsized and shown in full on hover, with a
  capped tooltip.
- **Level of detail.** Below zoom 0.5, rows render as gray bars with no
  text. Off-screen cards are culled.
- **Selection.** Clicking selects a row or a card. A selection is a
  JSON pointer. The level's top bar shows the selected path.

### Tree layout

`json_layout(value, view) -> positions` is a pure function. It gives
one column per depth, with `x = depth × (220 + 60)`. Each subtree gets
a vertical band equal to the larger of its own height and the sum of
its children's bands, with a 16 px gap between siblings, and the parent
is centred on its children's span. A saved `pos` replaces the computed
position for that card only.

The layout runs on enter and whenever collapse or paging changes. The
force settle never runs on a JSON level. Dragging a card saves only that
card's position. **Relayout** in the bar clears every saved `pos`.

### Outline

- The outline is an egui `SidePanel::right`. It is resizable, has a
  toggle in the bar, and appears on JSON levels only. Its width and
  visibility are local state.
- It is a flattened list virtualized with `show_rows`, rebuilt only when
  its expansion or the search changes. Rows match the cards: key or
  index, label, and a value or `{n}` / `[n]`.
- Its expand state is separate from the canvas and is not saved.
- Selection is synced both ways. Selecting a row pans to its card. If
  that card is hidden behind a collapsed ancestor or an unrevealed page,
  the canvas reveals it, and the reveal is saved like a manual expand.
- **Search** (Ctrl+F) is a case-insensitive substring match on keys and
  scalar values across the whole value, including unrevealed pages.
  Matches keep their ancestors. Enter or ↓ steps to the next match, and
  matching cards are highlighted. A value of more than about 100k nodes
  is searched on a worker thread.

### Copy

- **Copy value** (Ctrl+C): a scalar's plain text, with strings
  unquoted, or a container's pretty-printed JSON.
- **Copy path** (Ctrl+Shift+C): a JSON Pointer (`/users/0/address`) or
  accessor form (`users[0].address`). The bar's dropdown picks the form,
  and the choice is remembered locally.
- **Copy key** is also in the context menu of a row or card.

The top bar holds the breadcrumb, the selected path, **Copy value**,
**Copy path ▾**, **Relayout** and the outline toggle.

### The card at the parent level

The Json card grows to 220 wide. It shows a header (the file name and
`{n}` / `[n]`), up to 4 root rows as above, and `+ N more`. A scalar or
empty root shows just the value. A missing file or parse error keeps
today's error card and Retry.

## Model

### Level sources

A level's contents come from a **level source** instead of every piece
of graph chrome reading `self.nodes` and `current_group` directly. This
was the brainstorm's parked refactor (C); the user chose it during
planning (2026-09-27), because JSON is its second implementation
straight away and the shared chrome then works on both kinds of level.

- `LevelItem { id, kind, rect }`, where `kind` is `Node(ItemNodeKind)`
  or `JsonCard`, is what the shared code sees. `App::level_items()`
  lists the current level's items bottom-first, in draw order: stored
  nodes kind by kind in `GRAPH_DRAW_ORDER`, or a JSON level's cards.
- Built on `level_items()`: fit and Reset View, the minimap, the LOD
  visible count, the settle key, the right-click hit test, and
  `EXPORTSTATE`'s `cards`. `level_edges()` gives each source's edges:
  `parent` links for stored nodes, row ports for JSON.
- Layout is a per-source strategy: `layout_current_level()` runs the
  force settle for stored nodes and the tree layout for a JSON level.
- Each source paints and handles input for its own items: stored nodes
  keep `draw_graph_card` unchanged, and JSON cards get their own painter.
- The refactor lands first, in behaviour-preserving steps, before any
  JSON work.

A JSON level is `App::json_level: Option<JsonLevel>`, present exactly
while `current_group` names a Json node. It holds the visible cards,
their world rects, the selection, and the outline's and search's state.
It is rebuilt from the node's `value` and view overrides whenever those
change, and never enters `self.nodes`, so nothing about it can reach the
store, Recently Closed, export, pruning or any other all-node walk. A
card's id is `<json node id>#<pointer>`.

New nodes never land inside a Json node. Every creation site uses
`insertion_group()`, which on a JSON level is the Json node's own group,
and the creation controls (New Tab, New Note, New Group, Import Group,
the empty-canvas menu) are hidden on JSON levels. The breadcrumb names
the Json node by its file name, and it can't be renamed.

### Saved view state

There is one new store field: `VIEW = 14`, of type Str, diffed (not
explicit), added to `kind_fields(Kind::Json)`. It holds compact JSON
keyed by pointer. Only overrides are stored:

```json
{"/users/0": {"pos": [520, 20], "collapsed": true, "color": 40563}, "/users": {"pages": 2}}
```

`color` is stored as `0x00RRGGBB`, like a node's colour (`session_sync::color_value`) — `40563` is `0x00009E73`, `TAG_COLOR_PALETTE`'s teal.

- Positions are in level coordinates, with the root card at the origin.
- The field is last-writer-wins as a whole: if two replicas change the
  view at the same moment, one change is lost. That is acceptable for
  view state (see Future directions).
- An older build ignores the field.
- Entries whose pointer no longer resolves (after a Refresh) are dropped
  on the next save. If array elements are reordered, their overrides
  stay with the index, not the element.

### Removed

- The "Expand as Graph" menu item, `NodeAction::ExpandAsGraph`,
  `expand_json_as_graph`, `expand_json_entries` and headless
  `EXPANDJSONASGRAPH`.
- `build_json_viewer`, `draw_json_value` (the outline is new code),
  `json_viewer_node` and `NodeAction::ViewJson`.
- The tab's "View as JSON" chip. `TabPayload.json_body` stays only as
  far as content dispatch still needs it.

### Kept as-is

Existing `from_json` groups and `JsonLeaf` nodes still load, sync and
render as ordinary nodes, and are not converted. `FROM_JSON`, `KEY` and
`VALUE` are not retired, so older stores still read.

## Headless

- Entering a level uses the existing `DOUBLECLICK x y`.
- New commands:
  - `JSONSELECT <pointer>` selects on the current JSON level.
  - `JSONTOGGLE <pointer>` collapses or expands.
  - `JSONMORE <pointer>` reveals a page.
  - `JSONRELAYOUT` does what Relayout does.
  - `JSONSEARCH <text>` searches.
- `EXPORTSTATE` reports, for the JSON level: the root id, the visible
  cards' pointers and positions, the selection, and the search matches.
- `scripts/drive.sh` gets a probe that adds a JSON file, enters it,
  toggles, pages, searches and leaves.

## Testing

- **Store untouched:** entering and leaving a JSON level (with toggles,
  pages and a drag undone by Relayout) leaves `shared_nodes()`
  identical to before, apart from `VIEW`.
- **Card generation:** paging of objects and arrays, the default
  expansion budget and depth, nested arrays, and scalar or empty roots.
- **Label rule:** each preferred key, the string fallback, the array
  preview and no label.
- **Layout:** deterministic for the same input, columns by depth,
  parents centred, and a saved `pos` overriding one card only.
- **VIEW:** round-trips through the store, and a stale pointer is
  dropped.
- **Search:** matches in hidden pages, ancestors kept, and the reveal on
  select.
- **Copy:** scalar vs container text, and both path forms, including
  keys that need escaping (`~0`/`~1` in pointers, quotes and brackets
  in the accessor form).
- The headless probe above, and smoke-test coverage for entering a JSON
  level.

## Outcome

Implemented across Tasks 1-11 ("Level sources: the graph chrome reads the level through level.rs" through
"JSON level: smoke-test coverage, a drive.sh probe and docs"). Rulings made
while implementing that changed or sharpened this spec:

- `preserve_order` is on (serde_json), so keys show in file order
  everywhere a `serde_json::Map` is serialized, not just on a JSON level.
- Only non-empty containers get their own card; `{}`/`[]` render inline as
  a row, and a scalar root is a header showing the value.
- The outline shows every entry of an open container without paging (only
  cards page); during a search it shows exactly the matches and their
  ancestors, all open.
- Search matches object keys and scalar values, not array indices (which
  would match every `0`).
- Entering a Json node whose file failed to load is refused; double-click
  does nothing on its error card.
- Selecting an entry that doesn't resolve in the value is ignored (no
  selection, no reveal); selecting one that does pans the camera only when
  it isn't already fully on-screen, not unconditionally.
- The current crumb on a JSON level is a plain label — the spec's
  "can't be renamed" is enforced there, not just left unimplemented.
- The parent-level preview card for a Json node has no port dots (nothing
  for them to point at from outside the level).
- `TabPayload.json_body` is removed (Task 10): the JSON viewer popup was
  its only reader, and it's now write-only once that popup is gone.
  `manifold-engine-webview2/assets/welcome.html`'s "Try it" card, part of
  the same removal, no longer points at "Expand as Graph".

## Future directions

- **Table mode** for arrays of same-shaped objects: columns are keys and
  rows are elements. It is the natural view for API responses.
- **More level sources:** a PDF's pages, a folder or archive listing,
  table mode. Each is one new `LevelItemKind` arm and its painter.
- **Tree layout for tab trees:** the layout strategy is per source, so
  tab forests could opt into the JSON level's tree layout.
- **Per-pointer VIEW merging:** a store map type so concurrent view
  edits on two replicas both survive.
- **Editing** values in place. The level is read-only for now.
