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

## Open for review

- **D1. The pane is the card's menu laid open (recommended).** Its sections are drawn from the same data the popups
  draw: `node_menu_config(&node_menu_view(id), ..)` items through `draw_menu_item`, and the Label menu's rows. Both
  draw functions are split so the popup and the pane share them; no second list of what a card offers. Alternative:
  a hand-built pane with its own rows (more freedom of layout, a second list to keep in step with the menu).
- **D2. The selection bar stays as it is (recommended).** It floats by the cards and carries the bulk verbs (Pin, Group,
  Label, Close); the pane carries the details. Its Label button then opens a menu whose rows the pane also shows.
  Alternative: drop the bar's Label button while the pane is open (one place for labels, but the bar changes shape
  with the window width).

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

- **Header:** the kind's word and the name through `names::display_name`; for a tab or file, its URL or path.
- **Labels:** the Label menu's rows (tick, rename, recolour, delete) and its "New label" field, acting on the selection
  as the menu does (`GraphAction::ToggleLabel`, `NewLabel`, `RenameLabel`, `RecolorLabel`, `DeleteLabel`).
- **Menu:** the card's menu items in order (`NodeMenuConfig::items`), with the Jar page's rows inline rather than behind
  a page row. Rename and New Jar swap in their name field in the pane, as they do in the menu.
- **Text:** a note's text, or a tab's last stored page text (`text_store.get(id)`), read-only, scrolled, capped in
  height.

Several stored cards selected: the count and the Labels section (tri-state ticks, as the menu shows them). The bulk verbs
stay on the selection bar. Virtual cards (a JSON level's rows) never reach the pane: that level shows the outline.

## Layout and code

- `graph_ui/right_panel.rs` (new): `build_right_panel(ui)` owns the one `egui::Panel::right` (id, resize, width,
  160..=640). It draws the outline's contents in a JSON level and the card pane's otherwise, and nothing (no panel) when
  neither applies. `scene.rs:219` calls it in place of `build_json_outline`. `graph_scene_outer_rect` (`camera.rs:175`)
  reads one predicate, `right_panel_shown()`, shared with `build_right_panel`.
- `graph_ui/card_pane.rs` (new): the pane's contents from the selection, pushing `GraphAction`s and returning a
  `NodeAction` the caller applies.
- `node_menu.rs`: `draw_menu_item` and the items are reused as they are; the jar page's items are drawn inline.
- `label_menu.rs`: the row list and the field move into one function both the popup and the pane call.
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
