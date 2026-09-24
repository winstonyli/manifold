# Shared Graph Card Frame Design

## Goal

Apply the "shared frame + per-kind content, dispatched on `ItemNodeKind`"
pattern (toolbar, node menu) to the Graph view's card rendering. Today
`build_graph_ui` has one hand-copied loop per kind for Download, Note,
Image, Pdf and Json, each repeating the same skeleton; card size is also
computed separately at four sites. After this change there is one card
frame, one per-kind body painter, and one `node_card_size`.

Warm-ups already landed (`6daaa03`): `nodes_at_current_level(kind)` and
`dim_if_filtered_out`.

## Decisions

- **Keep today's kind layering** (Download under Note under Image under
  Pdf under Json). The frame is called kind by kind in that order, not in
  one pass over `node_order`. No visible z-order change.
- **Download gets full parity** with the other kinds: it loses its
  hand-rolled close button and drag/hover code, gains drop-onto-group and
  the same sticky hover every other card has, loses its hover
  fill-brightening, and its context menu offers "Remove from Group" when
  grouped.
- **Right-click picks the topmost card.** The right-click hit test checks
  kinds in reverse draw order (today it checks bottom-up, so on
  overlapping cards it opened the menu of the card *under* the visible
  one — the opposite of left-click). Visible only where cards overlap.
- **Approach:** one frame method plus a `match` for the body (rejected: a
  `card_begin`/`card_end` helper pair called from five surviving loops —
  keeps the loops; a trait per kind — nothing else in the codebase models
  kinds as types).

## Current state (for reference)

Per card, each loop does: fetch fields → build rect → viewport cull
(skipped for the node being dragged) → `ui.interact(click_and_drag)` →
`is_hovered = self.hovered == id || response.hovered()` → paint body →
border (hover-dependent width/color) → hover ✕ → drag/hover bookkeeping
→ kind action → filter dim. Differences:

| Kind | Size | Body | Interact | Action | Border colors | Dim rounding |
|---|---|---|---|---|---|---|
| Download | `DOWNLOAD_NODE_SIZE` | fill (brightened on hover), icon, filename, progress bar | whole card | click → reveal (`download_clicked`) | text / weak text | 6.0 |
| Note | `NOTE_NODE_SIZE` | yellow fill, icon; `TextEdit` while editing, else markdown | header while editing, else whole card, registered after content; plus hover-only interact | click → enter editing; `TextEdit` lost focus → save | black α200 / α120 | 6.0 |
| Image | `image_node_card_size(w, h)` | texture, broken-image glyph, or LOD box | whole card | click → maximize (`image_clicked`), unless ✕ clicked | text / weak text | 4.0 |
| Pdf | `pdf_card_size(pdf)` | texture + page count, broken glyph, error glyph, spinner, or LOD box | whole card; error → hover tooltip | none | text / weak text | 4.0 |
| Json | `JSON_NODE_SIZE` | fill, icon, summary | whole card | double-click → `json_viewer_node` | text / weak text | 4.0 |

Download alone hand-rolls its close button (visually identical to
`draw_node_close_button` with the Image/Pdf/Json colors) and its
drag/hover (no `finish_draggable_node`, so no drop-onto-group and
non-sticky hover). Card size is also computed in `bounds_for_level`, the
LOD `count_visible_nodes` sum (which approximates Image/Pdf with
`IMAGE_NODE_MAX_SIZE`/`PDF_PENDING_NODE_SIZE`), and the right-click hit
test.

## 1. Pure helpers

- `node_card_size(payload: &NodePayload) -> egui::Vec2` — every kind:
  Tab `GRAPH_NODE_SIZE`, Download `DOWNLOAD_NODE_SIZE`, Note
  `NOTE_NODE_SIZE`, Image `image_node_card_size`, Pdf `pdf_card_size`,
  Json `JSON_NODE_SIZE`, Group `GROUP_NODE_SIZE`. Replaces the per-kind
  sizes in the render loops, `bounds_for_level` (one loop over all level
  lists), `count_visible_nodes` (exact Image/Pdf sizes instead of the
  max/pending approximations — only affects when LOD simplification
  kicks in, and only toward correctness), and the right-click hit test.
- `card_filter_text(payload: &NodePayload) -> String` for the five card
  kinds, with today's values: Download full path, Note text, Image/Pdf/Json
  `download_filename(path)`. (Tab and Group keep their own filter
  inputs.)

## 2. The frame

`App::draw_graph_card(&mut self, ui, deferred_overlays, id, kind, inv_zoom,
zoom_scale) -> Option<CardOutput>`, in this order:

1. Look up the node; `rect = Rect::from_center_size(pos, node_card_size)`.
   Cull exactly as today (skip unless dragging this node or intersecting
   `graph_scene_rect`); return `None` when culled or missing.
2. **Body:** `self.paint_card_body(ui, deferred_overlays, id, kind, rect,
   …) -> CardBody { interact_rect, tooltip: Option<String> }` — a `match
   kind` over one painter per kind, each moved verbatim from its loop.
   Note's painter keeps its `TextEdit` and its lost-focus save; Download's
   drops the hover brightening.
3. **Interacts, registered after the body** (so no body widget outranks
   them in egui's hit test — the root cause of the Note hover bug, now
   fixed for every kind by construction):
   `response = ui.interact(interact_rect, ("graph_card", id),
   click_and_drag)` (plus `on_hover_text` if `tooltip`), then
   `hover = ui.interact(rect, ("graph_card_hover", id), hover)`.
   `is_hovered = self.hovered == id || hover.hovered() ||
   response.hovered()`.
4. **Border:** `card_style(kind, visuals)` gives hover/idle border colors,
   ✕ colors and dim rounding (table above); stroke painted after the body
   (every body keeps content inset from the edge, and Image/Pdf already
   paint their border after the texture).
5. **✕:** `draw_node_close_button` with salt `("graph_card_close", id)`.
6. `finish_draggable_node(id, &response, is_hovered)`.
7. `dim_if_filtered_out(…, &[&card_filter_text(payload)], rect,
   style.dim_rounding)`.
8. Return `CardOutput { response, close_clicked }`.

## 3. Call site

```rust
for kind in [Download, Note, Image, Pdf, Json] {          // draw order
    for id in self.nodes_at_current_level(kind) {          // cached lists today
        let Some(card) = self.draw_graph_card(…) else { continue };
        if card.close_clicked { node_close_requested = Some(id); continue; }
        match kind {
            Download if card.response.clicked() => download_clicked = Some(id),
            Note if card.response.clicked() => /* enter editing */,
            Image if card.response.clicked() => image_clicked = Some(id),
            Json if card.response.double_clicked() => self.json_viewer_node = Some(id),
            _ => {}
        }
    }
}
```

The cached `*_at_level` locals stay (they also feed the settle key and the
minimap). The Note "enter editing" arm fires only when not already editing
(today it lives in the non-editing branch).

## 4. Download parity

- Hand-rolled ✕ and drag/hover code deleted (the frame provides both).
- `finish_draggable_node` gives it drop-onto-group; a drop saves the
  session like any other kind's drop (harmless — downloads aren't in
  `session.json`).
- `node_menu_config(Download, has_group)` passes the real `has_group`
  (was hardcoded `false`), so a grouped download's menu shows "Remove from
  Group". Update `node_menu_config_matches_each_kinds_menu` and the node
  menu spec's follow-up ("A Download's menu never offers Remove from
  Group") to done.
- Hover fill-brightening removed (no other kind has it).

## 5. Right-click hit test

One loop over the full reverse draw order — Group, Json, Pdf, Image,
Note, Download, Tab — each hit by `node_card_size` containment, replacing
the chained `hit_* = (…is_none()).then(…)` blocks. Group (drawn last) and
Tab (drawn first) keep their current first/last priority; only the five
card kinds between them flip to top-first.

## Non-goals

- The Tab and Group card loops (drop-onto-tab reparenting, drill-in,
  glow, thumbnails). They only pick up `node_card_size`.
- Sticky hover itself (`self.hovered` feeding `is_hovered`) — recorded
  follow-up in the node-menu spec; unchanged.
- One-pass creation-order drawing (declined).
- Any visual change beyond Download's dropped hover brightening.

## Testing

- **Unit:** `node_card_size` and `card_filter_text` table tests for every
  kind (Image/Pdf with a small and an oversized source); updated
  `node_menu_config` table (Download grouped → Remove from Group).
- **Live headless** (APPDATA isolated and quoted; `CLICK`, not spaced
  `MOUSEDOWN`/`MOUSEUP`; several `MOUSEMOVE`s per drag), demo scene plus
  `ADDIMAGE`/`ADDPDF`/`ADDJSON`, baseline screenshots captured on `main`
  before any code change and compared after:
  - idle and hovered look of each kind (identical except Download's fill);
  - hover ✕ closes each kind; Recently Closed as before;
  - drag each kind; drag a Download onto a group → joins it, its menu
    shows Remove from Group (not possible at first: nothing could create
    a Download headless. Done later with the `ADDDOWNLOAD` debug
    command: the drop, the menu, and Remove from Group all verified);
  - Image click maximizes; Json double-click opens the viewer; Note click
    enters editing;
  - Filter dims each kind;
  - right-click on two overlapping cards opens the top card's menu.
