# Link gestures: click a line to disconnect, drag a handle to connect (design)

Status: Implemented 2026-10-07 (commits f0fa529, 9dbeece, dfc01f8, add755e and "Link gestures: probe, screenshots,
docs"); see the plan's Outcome (`docs/superpowers/plans/2026-10-07-link-gestures.md`) for deviations. Follows `2026-10-06-links-design.md` (its "Parked: drag to connect, click a
line to disconnect"). Adds two gestures on top of `App::connect` / `disconnect`; no store, sync or edge-table change.

## Decisions (agreed 2026-10-07)

1. **Links only.** Both gestures act on `link` edges. `opened-from` stays menu-only (node menu "Opened from…" and
   "Disconnect from X", plus the existing tab-onto-tab reparent drop). Reasons: opened-from is mostly set
   automatically, is directed with a child-to-parent direction opposite to a natural drag, silently replaces an
   existing parent, and already has two user routes; clearing one can also drop a tab's idle-sleep protection while a
   download child is in flight.
2. **Connect-drag starts from an edge handle on all four sides of a card.** No modifier key. *Amended 2026-10-08
   (see "Polish" below): lines no longer run centre to centre; every edge between cards is one bowed curve that
   leaves and enters the facing side midpoints, where the handles are.*
3. **Build order: click-to-disconnect (B) first, then connect-drag (A)**, as two tasks of one plan.

## B. Click a link line to disconnect

- `LevelEdge` gains the data a hit-test and a disconnect need: `from_id`, `to_id`, and `link: Option<String>` (the
  Link node's id; `None` for opened-from lines). `level_edges()` fills them. Opened-from lines stay non-interactive.
- A hit-test helper takes the pointer (world coordinates) and returns the nearest link edge within a tolerance of
  `max(6 screen px, line half-width) * inv_zoom`, by distance to the flattened curve (originally to a straight
  segment; see "Polish").
- **When it applies:** a primary *click* (not a drag, so pan is unaffected) in the graph scene, with no card, card
  control, node menu, minimap or pick-target mode claiming it; not while `dragging`, `connecting` or a connect-drag
  is active; never in a JSON level. Where a card is under the pointer the card wins (lines pass under cards).
- **Effect:** `delete_link(link)` (exactly the Link node under the line, as the node-menu row does), then save. Two
  links on one pair are two lines on the same segment: the nearest wins, ties go to the most recently created (as built: the greatest id, which is the
  newest only within one launch; see the plan's Outcome); the
  other stays and can be clicked next.
- **Feedback:** the nearest link line under the pointer is drawn highlighted and the cursor becomes a pointer, so a
  click is never a surprise. An undo path is not added (the node menu can re-link).
- Whether the hit-test runs inside the Scene closure (a `Sense::click` region per edge) or after it
  (as `handle_minimap_click` does) is a plan decision: the plan checks that a region under the Scene does not steal
  the primary-drag pan.

## A. Drag from an edge handle to connect

- **Handles:** one per side, centred on the card's edge midpoint, ~12 screen px (scaled by `inv_zoom`). Shown on hover
  only (like Eject and Close), only on cards that are valid link ends, never in a JSON level, never while another
  gesture owns the pointer. Hidden when the card's edge is under 44 screen px (half the edge must clear the 16 px Audio control plus
  half a handle) or the level is drawn simplified
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
  moving ends the gesture. Losing window focus cancels it (`on_window_focus`; added in the final review, I2), since
  the button-up may go to another window; `scripts/focus-loss-probe.sh` checks this in the real app.

## Polish (2026-10-08)

Supersedes the handle, release and drawing details above where they differ.

- **Handles** are pills 28 by 16 screen points (`HANDLE_LONG_PX`, `HANDLE_SHORT_PX`), the long side along the card
  edge, centred on the edge midpoint. They show on hover whether or not the card has a valid link target (a drag
  from one with none cancels as an empty release does), and on **every** card while a connect-drag runs. The
  minimum on-screen edge length is `HANDLE_MIN_EDGE_PX` = 60 (was 44): the same derivation, 2 * (16 px Audio
  control + half the pill's long side) = 2 * (16 + 14).
- **Drop tolerance:** a release on no card takes the nearest card other than the source within
  `DROP_TOLERANCE_PX` = 24 screen points (24 / zoom world units) of its rect, the topmost of equals
  (`App::card_within`). A card under the pointer still wins, and if that is the source nothing is taken. The target
  ring and the release read the same `connect_drag_target`. Every card other than the source is eligible, valid
  target or not (a refused pair toasts, as before).
- **Drawing:** one routine (`link_gestures::paint_bow`) draws every edge between cards, links (dashed accent) and
  opened-from (solid) alike, as the quadratic bow of `bow_control` (control point = chord midpoint + perpendicular
  * min(15% of the length, 30 world units)) between **facing side midpoints**. Facing rule (`facing_sides`): take
  the centre-to-centre delta from `a` to `b`; if |dx| >= |dy| the pair is right/left (right of `a` and left of `b`
  when dx >= 0, mirrored otherwise), else bottom/top by the sign of dy. A tie goes horizontal, so the choice
  depends only on the two centres and is stable while neither card moves. JSON-level edges are unchanged
  (`EdgeCurve::Horizontal`), and layout is unchanged: links never feed it, opened-from still feeds the springs from
  the centres.
- **Everything that read the straight line follows the curve:** the click hit-test is the distance to the curve
  flattened into `BOW_SEGMENTS` = 24 pieces (same tolerance and tie rule); culling uses the box of both ends and the
  control point; `EXPORTSTATE` `edges[].screen` is the curve at t = 0.5; the rubber band is the same bow from the
  source side facing the pointer, ending on the pointer or, with a target (within tolerance or under the pointer),
  on the target's facing side midpoint.

## Not in scope

- Directed or named links, opened-from by drag or line click,
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
