# Link gestures: click a line to disconnect, drag a handle to connect (design)

Status: design for review, 2026-10-07. Follows `2026-10-06-links-design.md` (its "Parked: drag to connect, click a
line to disconnect"). Adds two gestures on top of `App::connect` / `disconnect`; no store, sync or edge-table change.

## Decisions (agreed 2026-10-07)

1. **Links only.** Both gestures act on `link` edges. `opened-from` stays menu-only (node menu "Opened from…" and
   "Disconnect from X", plus the existing tab-onto-tab reparent drop). Reasons: opened-from is mostly set
   automatically, is directed with a child-to-parent direction opposite to a natural drag, silently replaces an
   existing parent, and already has two user routes; clearing one can also drop a tab's idle-sleep protection while a
   download child is in flight.
2. **Connect-drag starts from an edge handle on all four sides of a card.** No modifier key. Lines keep running
   centre to centre; the handles only start a gesture.
3. **Build order: click-to-disconnect (B) first, then connect-drag (A)**, as two tasks of one plan.

## B. Click a link line to disconnect

- `LevelEdge` gains the data a hit-test and a disconnect need: `from_id`, `to_id`, and `link: Option<String>` (the
  Link node's id; `None` for opened-from lines). `level_edges()` fills them. Opened-from lines stay non-interactive.
- A hit-test helper takes the pointer (world coordinates) and returns the nearest link edge within a tolerance of
  `max(6 screen px, line half-width) * inv_zoom`, by point-to-segment distance (links are straight).
- **When it applies:** a primary *click* (not a drag, so pan is unaffected) in the graph scene, with no card, card
  control, node menu, minimap or pick-target mode claiming it; not while `dragging`, `connecting` or a connect-drag
  is active; never in a JSON level. Where a card is under the pointer the card wins (lines pass under cards).
- **Effect:** `delete_link(link)` (exactly the Link node under the line, as the node-menu row does), then save. Two
  links on one pair are two lines on the same segment: the nearest wins, ties go to the most recently created; the
  other stays and can be clicked next.
- **Feedback:** the nearest link line under the pointer is drawn highlighted and the cursor becomes a pointer, so a
  click is never a surprise. An undo path is not added (the node menu can re-link).
- Whether the hit-test runs inside the Scene closure (a `Sense::click` region per edge) or after it
  (as `handle_minimap_click` does) is a plan decision: the plan checks that a region under the Scene does not steal
  the primary-drag pan.

## A. Drag from an edge handle to connect

- **Handles:** one per side, centred on the card's edge midpoint, ~12 screen px (scaled by `inv_zoom`). Shown on hover
  only (like Eject and Close), only on cards that are valid link ends, never in a JSON level, never while another
  gesture owns the pointer. Hidden when the card's edge is under ~40 screen px or the level is drawn simplified
  (`graph_lod_simplified`), where they would collide with the corner controls.
- **Gesture:** press on a handle starts a connect-drag; the card does not move, is not selected, and never emits a
  `Drop`. State: `connect_drag: Option<String>` (the source id) beside `connecting`. While it runs, `connecting` is set
  to `(source, LINK)` so the existing ring on `valid_targets` and the brighter ring on the hovered target come free. A
  rubber-band dashed line is drawn from the source centre to the pointer.
- **Release:** the target is the card under the *pointer* (not the dragged card's centre, unlike the drop test),
  found by walking `level_items()` in reverse as right-click does. A valid target calls `finish_connect`; a release on
  empty canvas, the source, or an invalid card cancels silently (no toast for empty canvas; `finish_connect`'s existing
  toast for a refused pair). Esc cancels.
- **Lifecycle:** `connect_drag` is reset wherever `dragging` and `connecting` are reset (`end_stale_drag`,
  `end_stale_connect`, session replace, source removed or off level). The pick-target "any primary click cancels"
  rule fires on `primary_clicked`, so a press-drag-release is not cancelled by it; a release on the handle without
  moving ends the gesture.

## Not in scope

- Drawing link lines to the side handles (side anchors), directed or named links, opened-from by drag or line click,
  touch-specific handles, a modifier-key variant, multi-selection connect, cross-level targets, an undo for a line
  click.

## Risks

- **Pointer ownership:** a handle half outside the card, or a per-edge click region under the Scene, could take the
  card drag or the canvas pan. The plan proves neither regresses (existing drag/pan tests plus new ones).
- **Coordinates:** the Scene's pointer is documented as world-space in some paths and raw screen in others; the plan
  verifies each use before relying on it.
- **Thin cards** (small images/PDFs) and low zoom: handles are gated by the on-screen edge length.
- **Probes:** `MOUSEDOWN`/`MOUSEUP` need several `PointerMoved` frames for a drag; `EXPORTSTATE` gains an `edges`
  list (kind, link id, ends, screen midpoint) and the handle rects so a probe can aim.

## Testing

- Headless egui harness (as `a_click_finishes_or_cancels_pick_target_mode`): click on a link line deletes exactly
  that Link node; click beside it, on a card, on an opened-from line, in pick mode or in a JSON level does nothing;
  two links on one pair delete one per click; drag from a handle onto a valid card creates one link, onto empty
  canvas/self/invalid card creates none, Esc cancels; the card does not move and no `Drop` runs; `connect_drag` is
  cleared when its source is removed.
- Hit-test unit tests (distance, tolerance by zoom, nearest of several).
- Live: `link-probe.sh` extended to click a line and to drag a handle; `chrome-shots` view with the handles and the
  rubber band; look at the images.
