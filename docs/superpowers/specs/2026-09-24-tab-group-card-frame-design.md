---
status: done
---

# Tab and Group Cards on the Shared Frame

## Goal

Finish what `2026-09-23-graph-card-frame-design.md` started. That spec
put Download, Note, Image, Pdf and Json on one card frame
(`App::draw_graph_card`) and left Tab and Group as a non-goal. The Tab
loop (~690 lines) and the Group loop (~360) in `build_graph_ui` still
repeat the frame's skeleton, and they carry features the frame lacks:
eased hover with glow, a focused or overloaded border, extra on-card
controls and five drag-and-drop variants. After this change all seven
kinds go through one frame. Per-kind differences live in small `match`
tables, and the card pass is one loop over `GRAPH_DRAW_ORDER`.

This is README survey item 7 (2026-09-24).

## Decisions

- **Full migration**, rather than only sharing helpers between the two
  loops.
- **Approach:** a data-driven frame. The alternatives were a `CardKind`
  trait (every method needs `&mut App`, and nothing else in the codebase
  models kinds as types) and a frame with per-kind drag callbacks
  (leaves the drop rules scattered). It lands in five stages (see
  Staging).
- **All seven kinds ease their hover**, with glow and fill brightening
  (Tab's current look). The five frame kinds gain it. The previous
  spec's "no hover brightening" consistency rule is superseded: all
  seven now brighten.
- **Eject and swatch on every kind.** (*Swatch and Colors row superseded 2026-10-24: a card has no colour of its own; labels give it one, see the properties spec.*) Every card inside a group gets the
  hover eject button. Every card gets the color swatch, always visible
  (a faint ring when uncolored). Every node menu gets the Colors row.
- **One drop table** for every kind (see Drag and drop). Tabs can join a
  nested group's card, and every kind can be ejected through the back
  button.
- **Middle-click does the ✕ action on every kind** (today only tabs).
- **The group swatch moves** from bottom-left to bottom-right, matching
  Tab.

## Current state (for reference)

See the old spec for the frame's pipeline. Tab and Group differ from it
as follows (line numbers at `2eaa943`, `crates/manifold/src/main.rs`):

- Both loops register `ui.interact` *before* painting the body. The
  frame registers after, which is its Note-hover fix. Neither loop has
  the frame's second, hover-only whole-card interact.
- Eased `hover_t` (`animate_bool_responsive`) drives fill brightening
  (`1 + 0.35·t`), glow intensity (`1 + 0.8·t`), border width
  (`1 + 0.5·t`) and a border color lerp from resting to `text_color`
  (Tab ~11062-11090, Group ~11813-11820).
- Border overrides: a focused tab gets ACCENT at 2.5, not eased. A tag
  color replaces the resting color. An overloaded group gets
  DANGER_COLOR.
- Tab only: the rectangle-select wash; pin (bottom-left, always), audio
  indicator, eject (top-left, hover, inside a group) and swatch
  (bottom-right, always), each with its own `ui.interact` and excluded
  from the card's click by a `*_clicked` flag; middle-click close;
  multi-select drag.
- Group only: a live pre-release drop-target ring (`is_drag_target`);
  swatch at bottom-left; a member thumbnail grid or name and count, with
  collapsed and LOD variants.
- Drops. Tab at the top level: onto a group card → join (the whole
  selection if the dragged tab is selected); onto a tab card → new group
  of both. Tab inside a group: onto the back button → eject; onto a
  sibling tab → reparent (`would_create_cycle`). Group onto group →
  merge. Frame kinds (`finish_draggable_node`): onto a group card at the
  current level → join, applied immediately and saved inline.
- Tab and Group hand-copy the ✕ that `draw_node_close_button` draws, and
  keep their own `self.hovered` bookkeeping off plain
  `response.hovered()`.

## Design

### Frame pipeline

`draw_graph_card(ui, deferred_overlays, id, kind, inv_zoom, zoom_scale,
drop_preview) -> Option<CardOutput>`, for every kind:

1. **Cull**, as today (skipped for the node being dragged).
2. **Snapshot** a `CardView` (see Controls) from the node, so nothing
   holds a node borrow across `&mut self` calls.
3. **Reserve a background slot** with `ui.painter().add(Shape::Noop)`.
   Hover is only known after the interacts, which must follow the body.
   The slot lets glow and fill go *under* the body later
   (`painter.set`).
4. **Body:** `paint_card_body(kind)` paints content only. Bodies no
   longer paint their own base fill: `CardStyle.fill` does
   (`NOTE_CARD_COLOR` for Note, `extreme_bg_color` otherwise). Tab and
   Group gain arms that hold their current content code: the thumbnail
   or spinner, title strip, engine dot and favicon; the member grid or
   name and count, collapsed and LOD states. LOD stays per arm.
5. **Interacts:** the card's `click_and_drag` on `body.interact_rect`,
   then the whole-card hover-only interact, as today.
6. **Hover:** `is_hovered = self.hovered == id || hover.hovered() ||
   response.hovered()`, then `hover_t =
   animate_bool_responsive(("graph_card_hover_t", id), is_hovered)`.
7. **Fill the slot:** glow (`card_glow_shapes`, intensity `1 + 0.8·t`,
   in the border color) and `style.fill` brightened by `1 + 0.35·t`.
8. **Border:** `card_border(&view, &style, hover_t) -> (width, color)`,
   first match wins:
   - a focused tab: ACCENT at 2.5, not eased;
   - otherwise the resting color is the tag color, else DANGER_COLOR
     for an overloaded group, else `style.border_idle`. It lerps toward
     `style.border_hover`, at width `1 + 0.5·t`.
   Group's `border_idle` is ACCENT, its resting color today. A hovered
   group now lerps toward `text_color` like every other kind. Today it
   only thickens.
9. **Overlays:** the selection wash (a tab in `selected_tabs`), and the
   drop ring when `drop_preview == Some(id)`.
10. **Controls** (next section), registered after the interacts in step
    5, so they take the pointer.
11. **Drag, drop and hover bookkeeping** (Drag and drop).
12. **Filter dim:** `dim_if_filtered_out(card_filter_text(payload),
    style.dim_rounding)`. `card_filter_text` gains Tab (title and URL)
    and Group (name). It returns the texts to match as a list, since Tab
    matches two. `card_style` gains Tab and Group arms (themed, with
    rounding 6.0).
13. **Returns** `CardOutput { response, control: Option<NodeAction> }`.
    The caller keeps each kind's own click (Tab → Open; Group
    double-click → EnterGroup; Json double-click → viewer; Note → edit;
    Image → Open; Download → Reveal) and skips it when `control` is
    `Some`.

The card pass becomes: parent edges (as today), then `drop_preview =
self.dragging.and_then(drop_target)`, then `for kind in GRAPH_DRAW_ORDER
{ for id in at_level[kind] { draw_graph_card(...) } }`. The draw order
is unchanged.

### Controls

`CardView { kind, color, pinned, muted, audio_playing, focused,
in_group, overloaded, selected }` is cloned per card.
`card_controls(&CardView) -> Vec<CardControl>` lists what a card shows:

| Control | Kinds | Shown | Where | Action |
|---|---|---|---|---|
| Close | all | hover | top-right | Group: `Dissolve`; others: `Delete` |
| Eject | all, when `in_group` | hover | top-left | `RemoveFromGroup` |
| Swatch | all | always | bottom-right | `CycleColor` (*removed 2026-10-24, labels replace it*) |
| Pin | Tab | always | bottom-left | `TogglePin` |
| Audio | Tab, when playing or muted | always | where it is today | `ToggleMute` |

- `paint_control(ui, control, rect, &view, inv_zoom)` is one `match`
  holding today's drawing code for each control. Close now goes
  through `paint_control_square` (via `paint_card_control`) and its
  deferred overlay.
- A middle-click on the card returns the Close action.
- The first clicked control becomes `CardOutput.control`. This replaces
  Tab's `pin_response`, `eject_clicked`, `swatch_clicked` and
  `audio_clicked` exclusions.
- `node_menu_config` gives every kind the Colors row. The group menu
  gains Remove from Group when `has_group` (closes item 4's gap).

### Drag and drop

A pure table:

```
enum DropTarget { BackButton, Card(ItemNodeKind) }
enum DropEffect { Eject, Merge, Join, GroupWith, Reparent }

fn drop_action(dragged: ItemNodeKind, target: DropTarget, inside_group: bool) -> Option<DropEffect>
  (_,     BackButton,  true)  → Eject
  (Group, Card(Group), _)     → Merge
  (_,     Card(Group), _)     → Join
  (Tab,   Card(Tab),   false) → GroupWith
  (Tab,   Card(Tab),   true)  → Reparent
  _                           → None
```

- `App::drop_target(dragged_id) -> Option<(DropTarget, Option<String>)>`
  tests, in order:
  - the back button (`leave_group_button_rect`), by screen-space
    pointer, as today;
  - the card whose `node_card_rect` contains the dragged card's center,
    at the current level, excluding the dragged card itself. Group
    cards are tried before tab cards.

  It is kept only when `drop_action` returns `Some`. The same call feeds
  the live ring (`drop_preview`) and the release, so the two cannot
  disagree.
- Drag step:
  - while dragging, apply the delta to the card, or to every tab in
    `selected_tabs` if the dragged tab is selected;
  - on release, push the effect as a `GraphAction`;
  - update `self.hovered` from this frame's pointer only (the frame's
    current `still_hovered` rule).

  `finish_draggable_node` is removed.
- One new `GraphAction::Drop { id, target: Option<(DropEffect,
  Option<String>)> }`, pushed on every release. `App::apply_drop` applies
  it, then saves the session (as `finish_draggable_node` saved on every
  release):
  - Join: the drag unit (the whole selection when the dragged tab is
    selected, as today; applied members leave `selected_tabs`) moves into
    the target group.
  - GroupWith: today's inline new-group code, moved. The group is created
    at the target tab's position, and the target and the drag unit join
    it.
  - Reparent: the target becomes the dragged tab's parent, unless
    `would_create_cycle`.
  - Merge: the body of today's `MergeGroups` apply arm, moved.
    `GraphAction::MergeGroups` goes once groups are on the frame.
  - Eject: `remove_from_group` on the dragged card.
- Group's `is_drag_target` code is removed. Its ring now marks the
  `drop_preview` card, whatever the card's kind.

## Staging

Five commits. Before the next stage begins, each must build without
warnings, pass `cargo test -p manifold` and pass the headless smoke test.

1. **Frame look** for the five current kinds: the background slot,
   `CardStyle.fill` (bodies stop filling), eased hover, glow,
   brightening, `card_border` with tag colors.
2. **Controls** for the five kinds: `CardView`, `card_controls`,
   `paint_control`, `CardOutput.control`, middle-click, and the menu
   Colors row plus the group menu's Remove from Group.
3. **Drops** for the five kinds: `drop_action`, `drop_target`, the new
   `GraphAction`s, the live ring, `finish_draggable_node` removed.
4. **Group** onto the frame: its body arm and its style, border and
   filter arms. The group loop is removed.
5. **Tab** onto the frame: its body arm, pin and audio, the focused
   border, the selection wash, multi-select drag. The tab loop is
   removed.

The last stage also marks README survey item 7 Done and points the old
spec's Non-goals at this one.

## Testing

- Unit tests (no GPU): a `card_controls` table test for every kind, in
  and out of a group, and a Tab playing, muted and pinned. A
  `drop_action` test covering every row and a sample of `None` cases.
  `node_menu_config_matches_each_kinds_menu` updated for the Colors row
  and the nested group's Remove from Group.
- Headless, per stage, with an isolated seed:
  - Stage 1: screenshots of each kind at rest and hovered.
  - Stage 2: click eject and swatch on a Note in a group.
  - Stage 3: drag a Note onto a group card, then out through the back
    button.
  - Stage 4: groups with 0, 1, 4 and 5+ members; a group-onto-group
    merge; smoke (enter and leave).
  - Stage 5: tab-on-tab → new group; reparent inside a group; a
    rectangle-select drag into a group; pin toggle; ✕ and middle-click
    close; the full smoke test.

## Risks

- **Interact order flips for Tab and Group** (body first, then
  interacts). Controls register last, so they still win the pointer.
  Stage 5's headless clicks on each control check it.
- **Multi-select drag ordering.** Deltas apply during the pass, but
  drops resolve as actions afterwards, so no drop reads half-applied
  positions.
- **Performance** is unchanged: the per-group member scan stays, and
  `CardView` is a small clone per card.

## Non-goals

- Changing z-order, card sizes, LOD thresholds or any body's content
  design.
- Tab-onto-non-tab drops, or multi-select for non-tab kinds.
