# Virtual cards: cards without a document node

Status: implemented 2026-10-16 (piece 1 of 2). Piece 1 of two; piece 2 is the trail strip (`2026-10-16-trail-strip-design.md`, not yet written), the first client.

## Intent

The Graph view draws one card per stored node. Some cards have no node: the JSON level's record cards today (drawn by a parallel renderer), a tab's trail pages tomorrow. They are derived from other state, positioned by their provider, and must never reach the store, sync, sessions or undo. This piece adds a seam so the graph draws, hits, selects, drags and drops such cards through the same code as real ones, with a per-card `Abilities` set saying what each may do. Success: a fake provider in tests gets every ability; no site in the graph path asks "is this id in `nodes`" to decide behaviour; the three silent failures the survey found (see Sites) are gone.

Not in scope: folding the JSON level's renderer into the seam (later; it needs a per-entry body painter and event sink, both of which this design provides, but its Tab walk, `JsonEdit` events and `level.rects` drag stay as they are); the trail provider itself (piece 2); any change to stored nodes' behaviour.

## Current state (from a read-only survey, 2026-10-16; line numbers are to be re-read at plan time)

- `level.rs` has `LevelItem { id, kind: LevelItemKind::{Node(CardKind), JsonCard}, rect }` and `level_items_from`, a per-frame list. Fit/Reset View, the settle key, LOD count, minimap, right-click hit test, rectangle select, `EXPORTSTATE` `cards` and `card_within` already read it.
- The card renderer does not: `draw_graph_cards` iterates ids from `level_scan`; `draw_graph_card(id)` begins with `self.card_rect(id)?` and `self.card_view(id, members)?`, both reading `nodes`/the mirror.
- Silent or loud failures for an id with no node: `level_edges`' `rect_of` indexes `self.nodes[id]` (panic); `live_drag`/`end_stale_drag` cancel the drag; `end_stale_selection` drops the id; right-click opens the node menu and calls `on_jar_chip` for any `Node(_)` item; `bounds_for_level` ignores it; layout/settle never place it; search never dims it.
- Not read by the survey and to be read in the plan: `edit_field`, `begin_tabs_shutdown`, `valid_targets`, the Tab-view transition code.

## Design

1. **A virtual card is a card of a type, in the existing capability table.** Cards already have an identity, `CardKind { ty, tags }`, and a MIME-keyed capability registry (`caps::registry`, rows in `caps/rows.rs`; today `Caps { restorable, draw_layer }`). A virtual source registers its own type strings (the trail: `application/x-manifold-trail-page`) with rows in the same table, and its cards carry that `CardKind`. `card_style`, `card_controls`, the draw layer and the drop table (`drop_action`) then treat them like any other kind, by data, not by special cases. `CardEntry { id, kind: CardKind, origin: Origin, rect }` replaces `LevelItem`, where `Origin` is `Stored`, `Json` or `Virtual(SourceId)`. `draw_graph_cards` iterates entries; `draw_graph_card` takes the entry and reaches the node only when `origin` is `Stored`. `card_tab_order`, digit badges and `pass.order` come from entries.

2. **Abilities are `Caps` fields.** `Caps` gains `selectable`, `drag_source`, `drop_target`, `menu` and `stored_controls` (Swatch, Pin, Close and Eject act on a stored node, so a virtual kind turns them off; Delete and middle-click Close ask it too). (An earlier draft also had `movable` and `linkable`; no source needs them, so they were deleted: a stored card's drag moves it and its link handles start a Link node, both stored behaviour by nature, not abilities.) Defaults are false; the builtin rows set them true for the stored kinds, so existing behaviour is the registry's output, tested row by row (`caps::rows` tests). A site asks `entry.kind.caps().selectable` instead of `nodes.contains_key`. The JSON card keeps its current behaviour through its own row: a JSON level's entries are of kind `TYPE_JSON_LEVEL` (`application/vnd.manifold.json-level`), whose row grants no ability, so the shared chrome needs no `Origin::Json` filter. A virtual card's abilities are its row: per-kind, not per-instance, which is all the trail needs; a per-entry override is added only when a source needs it.

3. **Providers.** `trait VirtualSource` (one impl per source, registered in `App`): `entries(&self, app_state, level) -> Vec<CardEntry>` (ids `virtual:<source>:<key>`, never colliding with a node id; rects in world space computed by the provider), `edges(...) -> Vec<LevelEdge>`, `paint_body(ui, entry, ...)`, `on_click`, `on_menu`, and `on_drop(entry, target, at) -> Option<GraphAction>`, where `at` is the world point of the release. The graph core never inspects a source's data; it only calls these.

4. **Position and drag.** A virtual card's rect is the provider's; it is never written to `edit_pos` or the layout (`MOVE` unset). With `DRAG_SOURCE`, dragging shows a ghost at the pointer and leaves the card in place; on release, `drop_target` runs over the entries that have `DROP_TARGET` (real nodes' existing rules plus virtual ones), and the result goes to the provider's `on_drop`, which returns a `GraphAction` (or nothing). `end_stale_drag` and `live_drag` test the entry, not `nodes`. Two things to confirm in the plan: whether the graph already draws a drag ghost (if not, a ghost is new, small, and shared by real cards' non-moving drags if any), and that `drop_target` can report "empty canvas" as a target (the trail's open-as-card gesture needs it; today a release on empty canvas may simply return no target, in which case `on_drop` gets `None` and the provider decides).

5. **Selection.** Virtual ids may be in `interaction.selected` when `SELECT` is granted; `end_stale_selection` tests the entry list. Actions that act on the selection (Close Selected, Pin, group, delete) filter to ids that are nodes, so a virtual card in a mixed selection is skipped, not an error.

6. **Edges.** `level_edges` takes rects from entries, not `self.nodes[id]` (removes the panic); providers append their own edges (`LevelEdge`), drawn by the existing `draw_graph_edges`.

7. **Excluded from persistence.** No virtual id reaches `edit_pos`, `edit_field`, store writes, sync, `EXPORTSESSION`, undo, Recently Closed or `tab-history.json`. A test enumerates the write entry points and passes a virtual id to each.

8. **Observability.** `EXPORTSTATE` `cards` lists virtual entries with `"virtual": "<source>"` and their abilities; `gate layout`'s checks skip them unless a stage opts in (implemented: the gate's `State::cards` and `State::card_is` leave out a card with a `"virtual"` key; `all_cards`, which hit tests and layout read, keeps it).

9. **Search/filter, culling, camera.** Culling uses `entry.rect`. `bounds_for_level` reads entries. Search dims by an optional `searchable_text()` on the provider (default none, not dimmed).

## Tests

A fake provider in `#[cfg(test)]` that yields three virtual cards with different ability sets and one edge. Unit tests, each failing before the change: it draws and is culled outside the camera; hover and click reach `on_click`; right-click opens the provider's menu, not the node menu; rectangle select and click select it and `end_stale_selection` keeps it; dragging it leaves its rect unchanged, shows a ghost, and `on_drop` receives the release target; a virtual id given to each persistence entry point writes nothing; `level_edges` draws the provider's edge and does not panic; `EXPORTSTATE` lists it; fit/Reset View include it. Existing graph tests stay green unchanged (they are the check that real nodes did not regress). Gate: `gate layout` and `gate smoke` (cards, selection, drag) through `gate all LOG light`, then `ui`.

## Review rounds

Round 1 (self): found the drag ghost and empty-canvas-target assumptions unverified (added to decision 4); abilities `MOVE` vs `DRAG_SOURCE` consistent with decisions 2 and 4; no TBDs. Round 2 (self, while planning): abilities per card moved from a new per-entry flag set into the existing `Caps` registry (decisions 1 and 2), because the registry already keys capabilities by card type and `CardKind` is what `card_style`, `card_controls` and `drop_action` take; two parallel capability mechanisms would be the thing this piece exists to avoid. Round 3 pending the user's read.

## Risks and rulings to confirm

- The seam touches the interaction code in about 8 files (survey estimate 250-400 lines); the largest risk is a site the survey did not read still assuming `nodes`. Mitigation: grep every `self.nodes` use in the graph modules during planning and classify each.
- A virtual card cannot be dragged to a new position (decision 4). If a later source needs that, `MOVE` with a provider-owned position is the extension.
- Mixed selections skip virtual ids for node actions (decision 5) rather than refusing the action.

## Implementation notes

Rulings made while implementing:

- Sources are held as `Rc<dyn VirtualSource>` (`App.virtual_sources`), single-threaded like the rest of the UI state.
- A virtual drag calls the provider's `on_drop` directly on release; it never produces `GraphAction::Drop`, so `apply_drop` and the stored-node drop paths never see it.
- A virtual dragged kind dropped on a card takes the `Join` effect in `drop_action`.
- The drag ghost is the frame only (translucent rect with an accent stroke at the card rect plus the pointer offset), not the source's `paint_body`, which would register interacts twice.
- The persistence guard is the `is_virtual_id` prefix check (`virtual:`), not `nodes.contains_key`, because link nodes live outside `nodes` and would pass a "not in nodes" test. The selection actions (`take_selected_stored`, `selected_stored_count`) use the same rule.
- `movable` and `linkable` were deleted from `Caps` (decision 2). A JSON level's entries carry their own kind, `TYPE_JSON_LEVEL`, with a no-ability row; the `Origin::Json` ability filters in `drop_target`, `selectable_ids`, the marquee and the draw loop are gone. A JSON level lists no virtual entries (they are drawn only by `draw_graph_cards`, which a JSON level does not run).
- `on_drop` gets `at`, the world position of the release (the ghost's transform applied to the pointer), so a source can place what was dragged out where it was dropped even when `target` is `None`. A release on the back button reaches `on_drop` as `Some((DropEffect::Eject, None))`: `drop_action` ignores the dragged kind for it.
- `on_click` and `on_drop` run with `&mut App` inside the card loop (a stored drop goes through `GraphAction::Drop` instead); a handler returns a `GraphAction` for anything that changes nodes.
- Shift+F10 on a focused card asks `menu`: a stored card opens its node menu, a virtual one calls `on_menu` at the card's bottom-left.
- The settle key counts stored entries only (`level::settle_count`, used by the per-frame check and `current_level_settle_key`), so a source whose entry count changes never re-settles the stored cards.

## Next

Piece 2 (trail strip): lane layout over `NavHistory`, a `VirtualSource` anchored under the focused tab card (drag a page out to open it as a card; click to navigate), and a toolbar painter for tab view that shares the layout.
