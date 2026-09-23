# Image Item View Design

## Goal

Give Image nodes a full-screen ("maximized") view, the first node kind
besides Tab to get one. In doing so, turn the toolbar architecture's
extension point (`docs/superpowers/specs/2026-09-23-toolbar-architecture-design.md`)
from a documented-but-unused hook into a real, exercised one:

- Generalize Tab view into an **Item view** whose frame is shared by every
  viewable kind, with per-kind toolbar content and per-kind body.
- Resolve both limits that spec's Non-goals recorded: the Universal chip
  living inside Tab's per-kind content instead of layer 1, and the
  Tab-only `.unwrap()` in `build_tab_ui`.
- Add a shared, stateless **zoom chip** used by the Graph toolbar and the
  Image view.

## Non-goals (this pass)

- Full-screen views for Note, Pdf, Json, Download, or Group. Their
  dispatch arms stay empty and `focus_item` refuses them.
- A zoom chip in the Tab toolbar (Tab page zoom stays keyboard-only).
- Keyboard shortcuts for Graph camera zoom.
- Nearest-neighbor (pixel-art) sampling at high image zoom — needs a
  second texture sampler.
- Prev/Next browsing between sibling images, rotate/flip, transparency
  checkerboard, copy-image-to-clipboard.
- Persisting image zoom/pan across sessions (the maximized Image itself
  is persisted; its view state resets to Fit).
- The Settings toolbar's missing Universal chip (parked final-review
  finding #13 from the toolbar pass) — a separate follow-up.

## Decisions (with the alternatives rejected)

- **Generalize to Item view** rather than a separate `ViewMode::Image`
  (duplicates the frame, adds a mode per future kind, bypasses the
  extension point) or a Graph overlay like the JSON popup (never touches
  the toolbar architecture).
- **Single click** on an Image card opens it, matching Tab cards; drag
  still drags. Enter/`GRAPHOPEN` on a keyboard-selected Image also opens
  it.
- **Animated** enter/leave, matching Tab's zoom transition.
- **Zoom is a shared widget, not a shared control.** "Zoom" means a
  different thing per view (Graph camera scale, Tab page zoom that
  reflows text, Image magnification), each with its own state and its
  own meaning of "fit". So the zoom chip is a reusable layer-3 chip that
  kinds opt into, not a layer-1 universal control.

## 1. Item view frame

**Rename.** `ViewMode::Tab` → `ViewMode::Item`, `build_tab_ui` →
`build_item_ui`, at every site. The on-disk session field
`view_mode_tab` keeps its name so existing `session.json` files load; a
comment notes it now means Item view.

**Layers become real.** `build_item_ui` owns the toolbar row:

1. **Layer 2** — `render_return_to_graph` at the leading edge (unchanged).
2. **Layer 1** — the frame creates the right-to-left trailing child `Ui`
   and renders `render_universal_chip` into it *first*, so the chip is
   always rightmost for every kind.
3. **Layer 3** — `render_item_toolbar_content(id, ui, &mut trailing_ui)`.
   Each kind adds leading widgets to `ui` and trailing widgets to
   `trailing_ui`. Tab still measures `trailing_ui` to size its address
   bar; only where the trailing `Ui` is created changes.

The frame then advances the cursor past `trailing_ui.min_rect()`.

**Tab-only work moves under the kind match.** Everything in today's
`build_tab_ui` that is really Tab behavior — address-bar text sync (the
`.unwrap()` becomes `if let Some(tab)`), the suggestions dropdown,
Enter/navigate resolution, the find bar, split, the tab context menu —
moves into Tab-only code reached via the kind match. The frame keeps only
kind-agnostic work: the shortcuts-help popup, Escape/Ctrl+G to leave, and
the save toast.

**Body dispatch.** Tab keeps its GPU pane path: `content_panes_px` yields
a pane only when the focused node is a Tab. Image paints through egui
(section 2).

**Entry.** New `focus_item(id)`:
- Tab id → existing `focus_tab`.
- Image id → hide the previously focused tab (logic extracted from
  `focus_tab`, which already does this), set `focused`, switch to
  `ViewMode::Item`, reset `image_view` to Fit.
- Any other kind → no-op, with a `debug_assert`.

`start_transition_to_tab` becomes `start_transition_to_item`. Session
restore's focus validation accepts Image ids as well as Tab ids.
`update_window_title` shows the filename for a focused Image.

## 2. Image view

**Body.** Paints the already-resident full-resolution texture
(`gpu.image_id(id)`) with `ui.painter().image(...)` in the area below
the toolbar, over the app's dark background. No re-decode or re-upload.

**View state.** `image_view: ImageViewState { zoom: ImageZoom, pan: Vec2 }`
on `App`, where `enum ImageZoom { Fit, Scale(f32) }`. Reset to Fit on
every entry; not persisted.

- **Fit** = contain within the body, never exceeding 100% (same rule as
  card sizing; avoids blurring small images).
- **Scale** range 0.1×–16×, step ×1.25.
- Mouse wheel / pinch zooms about the cursor (the point under the cursor
  stays fixed). Drag pans. Per axis: if the scaled image is larger than
  the body, pan is clamped so no gap opens past its edges; if it fits,
  it stays centered on that axis.
- Ctrl+= / Ctrl+− / Ctrl+0 zoom the image when an Image is focused
  (they keep driving page zoom for a focused Tab).

**Toolbar content (layer 3)**, left to right:

```
[Graph] [photo.png · 1920×1080] [− 100% + Fit] ········ [Reveal  Copy Path] [Help  Settings]
```

- Identity: filename · pixel dimensions; hover shows the full path.
- Zoom chip on the *leading* (left-to-right) side, same direction as
  Graph's — no reversal logic needed.
- Actions chip in the trailing cluster: Open Containing Folder
  (`reveal_image`), Copy Path.

**Transition.** `Transition.tab_id` → `node_id`. `content_panes_px`
yields a GPU pane only for a Tab. For an Image, its texture is painted on
a foreground layer at the interpolated rect, interpolating between the
card's image rect and the image's Fit rect in the view (not the full
content rect), so aspect ratio is correct throughout. Same
`TRANSITION_DURATION` as Tab.

## 3. Shared zoom chip and Graph migration

```rust
enum ZoomChipAction { Out, In, Reset100, Fit }

fn render_zoom_chip(
    ui: &mut egui::Ui,
    pct: f32,
    can_zoom_out: bool,
    can_zoom_in: bool,
    fit_hover: &str,
) -> Option<ZoomChipAction>
```

- Stateless: reports the click; the caller applies it to its own state.
- Built from `toolbar_chip` + `chip_icon_button_enabled`: − (disabled at
  min), clickable `NN%` readout (→ 100%, today's Graph label idiom moved
  into the chip), + (disabled at max), Fit icon with caller hover text.
- Left-to-right only: documented, plus a `debug_assert` on the layout
  direction.

**Graph migration.** The chip replaces the "Reset View" text button and
the bare `%` label:

- −/+ — new: scale `graph_scene_rect` about its center by ×1.25, clamped
  so the derived zoom stays within `GRAPH_ZOOM_MIN..=GRAPH_ZOOM_MAX`.
- 100% — existing `reset_zoom` path.
- Fit — existing `reset_view` → `fit_all_nodes_scene_rect()` path; hover
  "Reset View (fit all)".
- Update the Graph toolbar's "identity/entry-point actions stay full
  text" rule comment: "+ New Tab" stays text; Reset View becomes the Fit
  icon. Its mention of headless tests using "Reset View" as a coordinate
  anchor is reworded (no such scripts exist in the repo).
- Update `README.md`'s "Reset View" mentions.

## 4. Error handling

- Texture missing (no GPU, or `image_id` is `None`): the body paints the
  card's existing "broken image" glyph; the toolbar still works.
- Focused Image deleted/closed while maximized: leave to Graph, via the
  same guard `build_tab_ui` already has for a missing focused node.
- Re-entry during a transition: the existing guard, made node-generic.

## 5. Testing

**Unit tests** — logic extracted into small pure functions so no `App` is
needed:

- `image_fit_scale(image_size, body_size)`: contains, never exceeds 1.0.
- `zoom_about(...)`: point under the cursor stays fixed; clamps at 0.1×
  and 16×.
- `clamp_image_pan(...)`: per axis, no gap past the edges when larger
  than the body; centered when it fits.
- `step_scene_rect(rect, factor, screen_size, min, max)`: preserves
  center, clamps to 0.15–3.0.
- Session focus validation accepts Tab and Image ids.
- Existing `in_reading_order` test stays.

**Live headless verification** (`debug_control`), with generated test
PNGs (1920×1080 and 64×64):

1. **Baseline before any code change**: screenshot Tab-view and Graph
   toolbars. Afterward the Tab toolbar must match, with the Universal
   chip in the same place despite moving to layer 1.
2. `ADDIMAGE` → `CLICK` the card → screenshots mid- and post-transition;
   toolbar layout; Fit on both sizes (64×64 stays at 100%); −/+/100% via
   `CLICK`; `ESCAPE` animates back to Graph.
3. Graph zoom chip: −, +, Fit, 100%.
4. Session: quit while an Image is maximized, relaunch, it restores into
   Image view.

## Docs

- Amend the toolbar-architecture spec's Non-goals: both recorded limits
  are resolved by this design.
- Update `README.md` for the Graph toolbar and Item view.
