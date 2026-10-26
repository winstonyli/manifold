---
status: draft
---

# Card pane — Design

## Question

What a card is, carries and links to is spread over its right-click menu (rename, pin, jar, links, export), the
selection bar's Label menu and the hover peek, and a tab's stored page text is shown nowhere. A JSON level already has a
right-hand panel, the outline (`json_level_ui.rs` `build_json_outline`). This adds a card pane in the same place for
the Graph view: select a card and its details show beside the canvas.

## Decisions (conversation 2026-10-26)

1. **One right panel slot (23a).** A JSON level shows the outline there; any other Graph level shows the card pane.
   One panel id, one width setting, one camera offset.
2. **It opens on selection (24b).** Selecting a stored card shows the pane; clearing the selection hides it. No toolbar
   toggle.
3. **Edits go through the existing actions (25a).** The pane emits only the action types its menu counterparts emit
   today: `NodeAction` (applied by `App::apply_node_action`) for one card's menu rows, `GraphAction` for the
   selection's labels. Undo, sync, the debug commands and the gate already cover both.

4. **The pane is an inspector (D1, option 2 of the survey).** It shows what the card is: its state, each editable
   piece sending the action the menu sends for it. The commands (Sleep, Copy URL, Duplicate, Close, ...) stay in the
   right-click menu. The jar and Links sections reuse the menu's drawing (`NodeMenuItem::Jars`, `Edges` through
   `draw_menu_item`), so the two never list them differently. Rejected: the menu laid open (a second copy of a list
   of verbs, with Close one click away all the time), the pane replacing the menu, hand-built jar and link rows.
5. **The world stays put (R1 below, option 8 of the survey).** Rejected: the pane over the canvas (Fit and the minimap
   would land under it, and the outline would no longer share the slot), today's refit (the view zooms on every click).

## Open for review

- **D2. The selection bar and the pane.** Surveyed 2026-10-26; options in the conversation, not yet chosen.

## Rulings (mine, for review)

- **R1. Opening or closing the right panel does not move or zoom the world.** Today `GraphCamera::fill`
  (`camera.rs:80-86`) refits the camera rect to the scene area keeping its centre, so narrowing the area by the panel
  zooms out until the same world width fits: every card shrinks and slides. With a pane that opens on every selection
  that would happen on every click. Instead the camera remembers the last scene area; when only its right edge moved,
  `fill` keeps the scale and the camera rect's left edge, so the panel covers a strip and reveals it again on close.
  Any other change (a window resize, a tween to a rect of another shape) keeps today's fit. This also applies to the
  JSON outline's toggle, which today zooms the same way.
- **R2. A selected card under the panel is brought into view.** After R1 the camera rect is the visible area left of
  the panel, so `pan_to` (`camera.rs:119`) already does the right thing: when the pane opens for one card selected by
  click or keyboard and the card is not fully in view, the camera eases it in. Not for a marquee selection.
- **R3. The pane takes no graph keys** unless one of its fields has focus: `graph_keys_free` (`keys.rs:313`) is already
  false while egui wants keyboard input.
- **R4. One width setting, `right_panel_width`,** replacing `json_outline_width` in `JsonUi` and `local_state`
  (`LocalState.json_outline_width` renames; no migration, per the no-migration rule: an old saved width is dropped).
  The outline's visibility toggle stays, JSON level only.

## Contents

One stored card selected, top to bottom:

- **Header:** the kind's word; the name through `names::display_name`, edited in place (`NodeAction::Rename`
  then `CommitRename`, as the menu's field and the toolbar's breadcrumb (`toolbar.rs:172`) already do through the one
  draft `renaming_group`); for a tab or file, its URL or path, copyable.
- **Labels:** the card's labels and a way to add or remove one, acting on the selection (`GraphAction::ToggleLabel`,
  `NewLabel`). How this relates to the bar's Label menu is D2.
- **Place:** group (`NodeAction::RemoveFromGroup`, `MoveToGroup`), pinned, jar for a tab or group (the menu's Jars rows,
  `SetJar`).
- **Links:** the menu's Links rows (`Connect`, `Disconnect`).
- **Text:** a note's text, or a tab's last stored page text (`text_store.get(id)`), read-only, scrolled, capped in
  height.

Several stored cards selected: the count and the labels' tri-state. Virtual cards (a JSON level's rows) never reach the
pane: that level shows the outline.

## Layout and code

- `graph_ui/right_panel.rs` (new): `build_right_panel(ui)` owns the one `egui::Panel::right` (id, resize, width,
  160..=640). It draws the outline's contents in a JSON level and the card pane's otherwise, and nothing (no panel) when
  neither applies. `scene.rs:219` calls it in place of `build_json_outline`. `graph_scene_outer_rect` (`camera.rs:175`)
  reads one predicate, `right_panel_shown()`, shared with `build_right_panel`.
- `graph_ui/card_pane.rs` (new): the pane's contents from the selection and `node_menu_view(id)`, pushing
  `GraphAction`s and returning a `NodeAction` the caller applies through `apply_node_action`.
- `node_menu.rs`: `draw_menu_item` is shared with the pane for the Jars and Edges items. The rename draft
  (`renaming_group`, already shared by the menu and the breadcrumb for any kind) gains the pane as a third surface;
  the field shows wherever the rename started. New Jar stays in the menu (its draft is dropped when the menu closes,
  `drop_stray_jar_name`).
- `GraphCamera`: R1's remembered scene area and the anchored refit in `fill`.
- `EXPORTSTATE` gains `right_panel: "outline" | "card" | null` and the described card ids, for the gate.

## Edge cases

- A selected card closes or is deleted on another replica: the selection drops it and the pane follows.
- Tab view: the Graph view is not drawn, so no pane.
- A label renamed or recoloured elsewhere: the pane reads the mirror each frame.
- A narrow window: under 480 points of window width the pane does not open (the canvas would be unusable).
- The right-click menu open for the selected card: both show; an action from either applies once.
- A rename in progress when the selection changes: the draft commits as when the menu loses focus.

## Testing

- `camera.rs` unit tests: a scene area that loses only its right strip keeps the scale and the left edge; one that gets
  it back restores the rect; a window resize keeps today's fit (R1).
- App-level tests: one card selected gives a card pane for it; clearing the selection gives none; a JSON level gives the
  outline; several cards give the common labels' states; a label ticked off from the pane is one undo step.
- Gate (`ui` tier): extend `gate labels` to select a card, read `right_panel` from `EXPORTSTATE`, untick a label through
  the pane's action and check the card.

## Out of scope

Editing arbitrary properties (left alone 2026-10-26), a pane in Tab view, keeping the pane open with nothing selected,
fetching page text for a card that has none stored.

## Review log

- Round 1 (draft against code): R1 misdescribed today's camera (it zooms out to keep the world width, not a half-panel
  shift) and `fill` has no memory of the old area; now it remembers one. Found the menu is data (`NodeMenuConfig`,
  `NodeAction`) and the Label menu emits `GraphAction`s: decision 3 now names both types, and D1 reuses both menus'
  drawing instead of a new list. Page text lives in `search::text_store`, not on the node. `pan_to` needs no change once
  R1 holds.
- Round 2 (adversarial): the selection bar and the pane now always appear together, so their overlap (labels) is raised
  as D2. Added the rename-in-progress and both-menus-open cases. `json_outline_width` is persisted in `local_state`:
  R4 names the rename.
- Round 3 (after choosing the inspector and R1): rename already works for every kind and its draft is already shared by
  two surfaces, so the pane adds no new rename state; New Jar stays menu-only. The Label menu reuse moved under D2.
