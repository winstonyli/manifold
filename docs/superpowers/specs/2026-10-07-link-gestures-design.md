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

- **Handles:** one per side, a 28 by 16 screen point pill (long side along the edge, scaled by `inv_zoom`) centred on
  the card's edge midpoint. Shown on hover (like Eject and Close), whether or not the card has a valid link target, on
  every card while a connect-drag runs, never in a JSON level, never while another gesture owns the pointer. Hidden
  when the card's edge is under 60 screen px (half the edge must clear the 16 px Audio control plus half a handle) or
  the level is drawn simplified (`graph_lod_simplified`), where they would collide with the corner controls.
- **Gesture:** press on a handle starts a connect-drag; the card does not move, is not selected, and never emits a
  `Drop`. State: `connect_drag: Option<String>` (the source id) beside `connecting`. While it runs, `connecting` is set
  to `(source, LINK)` so the existing ring on `valid_targets` and the brighter ring on the hovered target come free. A
  rubber-band dashed bow is drawn from the source's side facing the pointer to the pointer (to the target's facing
  side once there is one).
- **Release:** the target is the card under the *pointer* (not the dragged card's centre, unlike the drop test),
  found by walking `level_items()` in reverse as right-click does, or else the nearest valid target within the drop
  tolerance (see "Polish"). A valid target calls `finish_connect`; a release on empty canvas or the source cancels
  silently, and an exact hit on a refused card toasts (`finish_connect`'s existing toast for a refused pair). Esc
  cancels.
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
  (`App::card_within`). Only valid targets (`valid_targets`, the cards the ring marks) count in the tolerance pass, so
  a refused neighbour never shadows a farther valid one; with only refused cards in tolerance there is no target. A
  card under the pointer still wins, even if refused (it toasts on release), and if that is the source nothing is
  taken. The target ring and the release read the same `connect_drag_target`.
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

**Parked:** `facing_sides` uses the raw centre delta, not card extents, so two wide cards that overlap horizontally but
sit diagonally (for example 180 by 140 notes with delta (150, 130)) pick right/left although a's right edge lies beyond
b's left edge, and the edge runs backwards through both. Why parked: the rule is stable and simple; a delta scaled by
the cards' half extents is the cheap improvement if it ever bothers.

## Dragging a link's end (2026-10-09)

Pressing on a link curve and dragging retargets one end. Links only (`EdgeKind::Link`); opened-from lines keep today's
behaviour (a drag from one pans). A click without movement still deletes the link, unchanged.

- **One mechanism.** The gesture is a connect-drag from the end that stays. `App::retarget:
  Option<LinkRetarget { link, fixed_is_from }>` rides beside `connect_drag`, which holds the fixed end's card as its
  source; rings, snap (`card_within`, `DROP_TOLERANCE_PX`, valid targets first in the tolerance pass, any other card after them for a dragged end), handles on every
  card, the rubber band, Esc, the right-press rule, the lifecycle resets and the release (`end_connect_drag`) are the
  existing ones, except the release (`end_connect_drag` -> `finish_retarget`). `retarget` is cleared wherever
  `connect_drag` is (`cancel_connect`, `finish_connect`, session replace).
- **Start.** The press on a curve records `line_press` as for a click (same hit-test, `nearest_link` within 6 screen
  points; a handle or card under the pointer takes the press instead). When the Scene background reports
  `drag_started` (egui's own threshold), `start_line_retarget` takes the end nearer the press origin (distance to the
  curve's two ends; a tie takes `from`) as the moving one, starts the connect-drag from the other, and restores the
  camera of the press, undoing the few points the Scene panned on the frame it recognised the drag. While it runs the
  Scene's primary-drag pan is off (`DragPanButtons::empty()`); wheel zoom still works.
- **Drawing.** The link being moved is not drawn (the simpler of not drawing it or dimming it): the dashed rubber band
  stands for it, from the fixed end's facing side to the pointer or the snapped target, bowed in the link's own
  direction (`from` to `to`), so it looks as the finished link will.
- **Release.** The release is read as a `DragEnd` (`connect_drag_end`: `Card`, the fixed card `Source`, `Canvas` for
  a release over the scene with no card under or within the drop tolerance, or `Abandoned` for no real release: off
  the scene, on the minimap or a popup, a click, a lost button-up) and `App::retarget_outcome` turns it into one of
  four outcomes that `finish_retarget` acts on and saves:
  - `Retargeted`: on a card the pair may take, the moving end is rewritten **in place**: one `edit_field` on the Link
    node's `from` or `to`, the same writer `create_link` uses, so the node keeps its id, its other end and its
    direction, and the change syncs as one field edit (no delete plus create). The pair is checked as `connect`
    checks a new one (`check_connect`: live, same level, type table, no existing link on the pair).
  - `Deleted`: on empty canvas, or on a card `check_connect` refuses with `ConnectError::Exists` (a link already
    joins it and the fixed card), the dragged link is deleted (`delete_link`, as the click does). Dropping the end
    off its card removes the link; a duplicate would have nowhere to go.
  - `Refused`: on the fixed card, or a card refused for any other reason (another level, not a live card), the usual
    "Can't connect these cards" toast and the link is kept.
  - `Cancelled`, silently and the link kept: Esc, a focus loss, `Abandoned`, a release on its old end's card, or the
    link deleted or re-ended by another replica mid-drag.
  Only a release with no card under or within the drop tolerance is empty canvas. For this gesture `card_within`
  tries the ring-marked cards first and then any other card in reach, so the end's own old card, a refused card and
  an already-linked card all resolve as that card (silent keep, toast, delete), and the fixed card within reach is
  `Source` (toast).
- **Live feedback.** Every frame of the drag the graph pass resolves the pointer the way the release will
  (`connect_drag_end`, then `retarget_outcome` via `App::retarget_preview`: one resolution, so preview and release
  cannot drift) and stores it in `App::retarget_preview`. `retarget_look` maps the outcome to a look, painted through
  the one `paint_bow` call: `Retargeted` and `Cancelled` the usual accent band; `Deleted` a thinner red band
  (`DANGER_COLOR`) ending in a small "x" at the free end, so colour is not the only signal; `Refused` a cool blue
  (`theme::refused_color`: the sky-blue `TAG_COLOR_PALETTE` entry pulled 20% toward `BG`) ending in a small slashed
  circle (a prohibited sign); the two marks are `RetargetGlyph`, drawn in one place. The card in reach is ringed in the outcome's colour (accent for a retarget, red for a duplicate
  that would delete, blue for a refusal, `TEXT_WEAK` for its own old card), so a ring never promises a retarget the
  release will not do; refused and own cards are ringed too, which they were not before.
- **Debug.** `EXPORTSTATE` `edges[]` gain `near_from` and `near_to` (the curve at t = 0.2 and 0.8, where a probe grabs
  an end), and the state gains `retarget` (the Link node id or null) and `retarget_preview` (`"retarget"`, `"delete"`, `"refused"`
  or `"cancel"` while it runs, else null). `scripts/link-probe.sh` drives the gesture.
- **Changed.** A drag that starts on a link curve used to pan; it now retargets. A drag from the canvas beside the
  curve still pans.

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
