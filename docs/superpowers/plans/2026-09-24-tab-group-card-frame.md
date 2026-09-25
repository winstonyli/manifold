# Tab and Group Cards on the Shared Frame — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put all seven Graph-view card kinds through one frame (`App::draw_graph_card`), so the Tab loop (~690 lines) and Group loop (~360) in `build_graph_ui` go away, and per-kind differences live in small `match` tables.

**Architecture:** A data-driven frame. Each card snapshots a `CardView`, reserves a background slot, paints its body, registers its interacts, then fills the slot (glow + fill), strokes a `card_border`, draws its `card_controls`, and handles drag and drop through one `drop_action` table. It lands in five stages, each a commit that builds without warnings and passes the unit tests and the smoke test.

**Tech Stack:** Rust, egui 0.36.2 (`Painter::add(Shape::Noop)` / `Painter::set` for the background slot), wgpu, winit. One crate: `crates/manifold`.

**Spec:** `docs/superpowers/specs/2026-09-24-tab-group-card-frame-design.md`

## Global Constraints

- All code changes are in `crates/manifold/src/main.rs` (plus `crates/manifold/src/debug_control.rs` in Task 1, docs in Task 5). Line numbers below are at commit `42f3a62` and drift as you edit: find code by the quoted anchor text, not the number.
- (Superseded 2026-09-25: the repo is LF everywhere via `.gitattributes`; ignore the CRLF handling below.) `main.rs` and `README.md` used **CRLF** line endings. Edit them with a Python script that reads with `newline=""`, converts each search/replace string's `\n` to `\r\n`, asserts the exact number of matches before replacing, and writes back with `newline=""`. The `Edit` tool silently fails to match CRLF text. `debug_control.rs`, the scripts and the docs under `docs/` are LF.
- **Never run `cargo fmt`** (the file has hand-formatted regions).
- Headless runs must never touch the real `%APPDATA%\manifold`: always go through `scripts/drive.sh` (isolated APPDATA) or `scripts/headless-smoke-test.sh`.
- Every task ends with all three green:
  - `cargo build -q -p manifold` with **no warnings** (the WebView2 crate's pre-existing `unused dependency image` warning is not this crate's and is ignored);
  - `cargo test -q -p manifold`;
  - `timeout 500 bash scripts/headless-smoke-test.sh` (16 checks at the start; all must pass).
- Match the file's comment style: a short doc comment on every new type and function, saying why, not what.
- Commit at the end of each task with the message given, ending with the trailer line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.
- The draw order (`GRAPH_DRAW_ORDER`), card sizes, LOD thresholds and body content designs don't change (spec Non-goals).

## Headless probe helper

Tasks 1–5 each run a headless probe. They share one helper, created in Task 1 Step 10, at `target/cardframe/probe.py` (under the git-ignored `target/`). It writes command files, runs `scripts/drive.sh` with an isolated seed that skips the first-launch demo, and reads `EXPORTSTATE`'s `cards` list (added in Task 1) to find each card's screen position.

Card positions are deterministic between runs: `NEWNOTE`/`NEWGROUP` place a card at `GRAPH_WORLD_CENTER + spawn_jitter(&id)`, the ids come from fresh counters, and the camera starts the same. So a probe runs once to learn the layout, then again with the gestures. Headless runs at scale factor 1, so `EXPORTSTATE`'s points are the pixels `CLICK`/`MOUSEDOWN` take (the smoke test's `DOUBLECLICK 250 125` on a `NEWGROUP` card relies on the same).

Drags follow `debug_control.rs`'s own advice: `MOUSEDOWN`, several `MOUSEMOVE`s, then `MOUSEUP`, with no `sleep` between them.

---

### Task 0: Branch

**Files:** none.

- [ ] **Step 1: Create the branch from `main`**

```bash
git checkout -b tab-group-card-frame
```

Expected: `Switched to a new branch 'tab-group-card-frame'`. The spec and this plan are already committed on `main`.

- [ ] **Step 2: Baseline**

Run: `cargo build -q -p manifold && cargo test -q -p manifold`
Expected: builds with no warnings from `manifold`; `test result: ok. 21 passed`.

---

### Task 1: Frame look for the five frame kinds

The five kinds that already use `draw_graph_card` (Download, Note, Image, Pdf, Json) gain Tab's look: eased hover, glow, fill brightening, tag-colored border. Bodies stop painting their own base fill; the frame paints it into a slot reserved before the body. `EXPORTSTATE` gains a `cards` list for the probes.

**Files:**
- Modify: `crates/manifold/src/main.rs`: `CardStyle`/`card_style` (~678–708), new `CardView` + `card_border` after `CardOutput` (~725), `paint_missing_file_card` (~867), `draw_graph_card` (~5239–5314), `paint_card_body` fills (~5336–5689), the frame-kind call site (~11709), the `EXPORTSTATE` handler (~13155), tests module (~13886).
- Modify: `crates/manifold/src/debug_control.rs`: `ExportState` doc (~195–201).

**Interfaces:**
- Produces:
  - `struct CardView { kind: ItemNodeKind, color: Option<egui::Color32> }` (later tasks add fields);
  - `fn App::card_view(&self, id: &str) -> Option<CardView>`;
  - `fn card_border(view: &CardView, style: &CardStyle, hover_t: f32) -> (f32, egui::Color32)`;
  - `CardStyle.fill: egui::Color32`;
  - `draw_graph_card(&mut self, ui, deferred_overlays, id, inv_zoom, zoom_scale) -> Option<CardOutput>` (the `kind` parameter is gone; the frame reads `view.kind`);
  - test helper `fn test_view(kind: ItemNodeKind) -> CardView` in `mod tests`;
  - `EXPORTSTATE` JSON key `cards`: a list of `{id, kind, group, parent, color, pinned, screen: [x, y], rect: [left, top, right, bottom]}` for the current level's cards, in draw order.

- [ ] **Step 1: Write the failing test**

In `mod tests`, after `node_menu_config_matches_each_kinds_menu`, add:

```rust
    /// A `CardView` for `kind` with nothing set, for tests to override.
    fn test_view(kind: ItemNodeKind) -> CardView {
        CardView { kind, color: None }
    }

    #[test]
    fn card_border_eases_from_resting_to_hover() {
        // Opaque colors, so `lerp_color32` lands exactly on either end.
        let style = CardStyle {
            border_idle: egui::Color32::GRAY,
            border_hover: egui::Color32::WHITE,
            ..card_style(ItemNodeKind::Note, &egui::Visuals::dark())
        };
        let red = egui::Color32::RED;
        let view = |color| CardView { color, ..test_view(ItemNodeKind::Note) };
        // At rest: the tag color, else the kind's idle color.
        assert_eq!(card_border(&view(None), &style, 0.0), (1.0, egui::Color32::GRAY));
        assert_eq!(card_border(&view(Some(red)), &style, 0.0), (1.0, red));
        // Fully hovered: the hover color at 1.5, whatever the resting color.
        assert_eq!(card_border(&view(Some(red)), &style, 1.0), (1.5, egui::Color32::WHITE));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -q -p manifold card_border`
Expected: compile error, `cannot find struct ... CardView` / `cannot find function card_border`.

- [ ] **Step 3: `CardStyle.fill`**

In `struct CardStyle`, add a first field with a doc line:

```rust
    /// The card's background, under its body (bodies paint content only).
    fill: egui::Color32,
```

In `card_style`, add `fill: visuals.extreme_bg_color,` to `themed`, and `fill: NOTE_CARD_COLOR,` to the Note arm. Update `CardStyle`'s doc comment to say it holds the per-kind background, colors and filter-dim rounding.

- [ ] **Step 4: `CardView`, `card_border`, `App::card_view`**

After `struct CardOutput { ... }`, add:

```rust
/// What a graph card's frame (`App::draw_graph_card`) reads from its
/// node, snapshotted once up front (`App::card_view`) so nothing holds a
/// node borrow across the frame's `&mut self` calls.
struct CardView {
    kind: ItemNodeKind,
    color: Option<egui::Color32>,
}

/// A card's border width and color at hover progress `hover_t` (0 at
/// rest, 1 hovered): the tag color at rest, else the kind's idle color,
/// easing toward its hover color, from width 1 to 1.5.
fn card_border(view: &CardView, style: &CardStyle, hover_t: f32) -> (f32, egui::Color32) {
    let resting = view.color.unwrap_or(style.border_idle);
    (1.0 + 0.5 * hover_t, lerp_color32(resting, style.border_hover, hover_t))
}
```

In `impl App`, just before `fn draw_graph_card`, add:

```rust
    /// `id`'s `CardView`, or `None` when the node is gone.
    fn card_view(&self, id: &str) -> Option<CardView> {
        let node = self.nodes.get(id)?;
        Some(CardView { kind: item_node_kind(&node.payload), color: node.color })
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -q -p manifold card_border`
Expected: `1 passed`. (The build warns that `fill` and `card_view` are unused until Step 6.)

- [ ] **Step 6: The frame paints fill, glow and eased border**

In `draw_graph_card`:

1. Remove the `kind: ItemNodeKind,` parameter. Update the doc comment: the frame now also paints each card's background, glow and eased hover.
2. After the cull `if ... { return None; }`, insert:

```rust
        let view = self.card_view(id)?;
        // Glow and fill go under the body, but hover is only known after
        // the interacts, which must follow the body (below). This slot is
        // filled in once it is.
        let background = ui.painter().add(egui::Shape::Noop);
```

3. In the `paint_card_body` call, pass `view.kind` in place of `kind`.
4. Replace this block:

```rust
        // An inside stroke painted over the body; every body keeps its
        // content inset from the edge.
        let style = card_style(kind, ui.visuals());
        let (border_width, border_color) =
            if is_hovered { (1.5, style.border_hover) } else { (1.0, style.border_idle) };
```

with:

```rust
        // One eased hover drives the glow, the fill's brightening and the
        // border, so the three move together.
        let hover_t = ui.ctx().animate_bool_responsive(egui::Id::new(("graph_card_hover_t", id)), is_hovered);
        let style = card_style(view.kind, ui.visuals());
        let (border_width, border_color) = card_border(&view, &style, hover_t);
        let mut background_shapes = card_glow_shapes(rect, 2.0, border_color, inv_zoom, 1.0 + hover_t * 0.8).to_vec();
        background_shapes.push(egui::Shape::rect_filled(rect, 2.0 * inv_zoom, style.fill.gamma_multiply(1.0 + hover_t * 0.35)));
        ui.painter().set(background, egui::Shape::Vec(background_shapes));
        // An inside stroke painted over the body; every body keeps its
        // content inset from the edge.
```

(The `ui.painter().rect_stroke(rect, ..., border_width ..., border_color ...)` call right after stays as it is.)

5. At the call site in `build_graph_ui` (the `for (kind, ids) in [(ItemNodeKind::Download, downloads_at_level), ...]` loop), change `self.draw_graph_card(ui, &mut deferred_overlays, id, kind, inv_zoom, zoom_scale)` to `self.draw_graph_card(ui, &mut deferred_overlays, id, inv_zoom, zoom_scale)`.

- [ ] **Step 7: Bodies stop painting their base fill**

With the CRLF Python helper, in `main.rs`:

1. Replace the regex `\} else \{\r\n\s*ui\.painter\(\)\.rect_filled\(rect, 2\.0 \* inv_zoom, ui\.visuals\(\)\.extreme_bg_color\);\r\n\s*\}` with `}`. Assert **2** matches (the LOD `else` branches of Image and Pdf).
2. Delete every remaining line that is exactly (after indentation) `ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);`. Assert **8** matches: `paint_missing_file_card`, Download, Image's missing-texture case, Pdf's three cases, Json. (If the counts differ, stop and look: the 10 original sites are at lines 874, 5336, 5547, 5562, 5629, 5643, 5656, 5661, 5689 at `42f3a62`.)
3. Delete the line `ui.painter().rect_filled(rect, 2.0 * inv_zoom, NOTE_CARD_COLOR);` in the Note arm. Assert **1** match.
4. `paint_missing_file_card` no longer uses `ui`: remove its `ui: &egui::Ui,` parameter, and `ui, ` from its two calls (`paint_missing_file_card(ui, deferred_overlays, rect, inv_zoom, self.graph_lod_simplified)`, 2 matches).
5. In the Image arm, the comment that begins `// LOD: the real texture blit (and the broken-image` ends with a sentence saying the simplified state "gets a plain solid box instead of nothing". Replace that sentence with: `The simplified state shows the frame's plain fill, matching every other kind's "background + border" simplified look.`

- [ ] **Step 8: `EXPORTSTATE` lists the level's cards**

In the `DebugCommand::ExportState(path)` handler in `main.rs`, before `let state = serde_json::json!({`, add:

```rust
                // The current level's cards, in draw order, with where each
                // one's center is on screen: what a headless test needs to
                // aim a click or drag at a card.
                let cards: Vec<serde_json::Value> = GRAPH_DRAW_ORDER
                    .iter()
                    .flat_map(|&kind| self.nodes_at_current_level(kind))
                    .filter_map(|id| {
                        let node = self.nodes.get(&id)?;
                        let screen = self.graph_world_rect_to_screen(node_card_rect(node));
                        Some(serde_json::json!({
                            "id": id,
                            "kind": format!("{:?}", item_node_kind(&node.payload)),
                            "group": node.group,
                            "parent": node.parent,
                            "color": node.color.is_some(),
                            "pinned": node.as_tab().is_some_and(|t| t.pinned),
                            "screen": [screen.center().x, screen.center().y],
                            "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                        }))
                    })
                    .collect();
```

and add `"cards": cards,` after `"current_group": self.current_group,`.

In `debug_control.rs`, extend `ExportState`'s doc comment: after the sentence describing `current_group`, add: `` `cards` lists the current level's cards in draw order, each as `id`, `kind` (`"Tab"`, `"Note"`, ...), `group`, `parent`, `color` (whether it has a tag color), `pinned`, `screen` (its center) and `rect` (`[left, top, right, bottom]`), in points, which are pixels headless. ``

- [ ] **Step 9: Build, test, smoke**

Run: `cargo build -q -p manifold && cargo test -q -p manifold`
Expected: no warnings; `22 passed`.

Run: `timeout 500 bash scripts/headless-smoke-test.sh`
Expected: every check passes.

- [ ] **Step 10: Create the probe helper**

Create `target/cardframe/probe.py`:

```python
"""Headless probes for docs/superpowers/plans/2026-09-24-tab-group-card-frame.md.

Run from the repo root, after `cargo build -p manifold`. Everything lands
in target/cardframe/. The seed skips the first-launch demo, so a probe
starts from an empty graph.
"""
import json
import os
import pathlib
import subprocess

OUT = pathlib.Path("target/cardframe")
SEED = OUT / "seed"
SEED.mkdir(parents=True, exist_ok=True)
(SEED / "settings.json").write_text('{"has_shown_first_launch_demo": true}')


def run(name, lines):
    """Runs one headless session from `lines`, ending with QUIT."""
    path = OUT / f"{name}.txt"
    path.write_text("\n".join(lines + ["QUIT"]) + "\n")
    subprocess.run(["bash", "scripts/drive.sh", str(path)], check=True, timeout=300,
                   env={**os.environ, "SEED": str(SEED)})


def export(name):
    return f"EXPORTSTATE {{repo}}\\target\\cardframe\\{name}.json"


def shot(name):
    return f"SCREENSHOT {{repo}}\\target\\cardframe\\{name}.png"


def state(name):
    return json.loads((OUT / f"{name}.json").read_text())


def cards(name, kind=None):
    return [c for c in state(name)["cards"] if kind is None or c["kind"] == kind]


def at(card):
    """`card`'s screen center."""
    return tuple(card["screen"])


def corner(card, which):
    """One of `card`'s screen corners: "lt", "rt", "lb" or "rb", where the
    controls are centered (eject, close, pin, swatch)."""
    left, top, right, bottom = card["rect"]
    return (left if which[0] == "l" else right, top if which[1] == "t" else bottom)


def middle_click(point):
    x, y = point
    return [f"MOUSEDOWN {x:.0f} {y:.0f} middle", f"MOUSEUP {x:.0f} {y:.0f} middle"]


def click(point):
    return f"CLICK {point[0]:.0f} {point[1]:.0f}"


def drag(start, end, steps=6):
    (x0, y0), (x1, y1) = start, end
    lines = [f"MOUSEDOWN {x0:.0f} {y0:.0f}"]
    for i in range(1, steps + 1):
        t = i / steps
        lines.append(f"MOUSEMOVE {x0 + (x1 - x0) * t:.0f} {y0 + (y1 - y0) * t:.0f}")
    return lines + [f"MOUSEUP {x1:.0f} {y1:.0f}"]


def check(ok, what):
    print(("PASS " if ok else "FAIL ") + what)
    if not ok:
        raise SystemExit(1)
```

- [ ] **Step 11: Probe — each kind at rest and hovered**

Create `target/cardframe/stage1.py`:

```python
from probe import *
import pathlib

pathlib.Path("target/cardframe/a.json").write_text('{"a": [1, 2]}')
SETUP = [
    "NEWNOTE",
    "NEWGROUP",
    "ADDIMAGE {repo}\\crates\\manifold\\assets\\icon\\icon-64.png",
    "ADDJSON {repo}\\target\\cardframe\\a.json",
    "sleep 1",
]
run("stage1-layout", SETUP + [export("stage1-layout"), shot("stage1-rest")])
lines = list(SETUP)
for kind in ["Note", "Image", "Json"]:
    card = cards("stage1-layout", kind)[0]
    lines += [f"MOUSEMOVE {card['screen'][0]:.0f} {card['screen'][1]:.0f}", "sleep 1", shot(f"stage1-hover-{kind}")]
run("stage1", lines)
check(len(cards("stage1-layout")) == 4, "four cards at the top level")
```

Run (from the repo root; the helper's paths are repo-relative): `PYTHONPATH=target/cardframe python target/cardframe/stage1.py`
Expected: `PASS four cards at the top level`. Then look at `target/cardframe/stage1-rest.png` and the three `stage1-hover-*.png` (Read tool): every frame card has a soft glow; the hovered card's glow is stronger, its fill brighter and its border thicker and lighter; the Note is still yellow with its dark border; the group card (still on its own loop) looks as before.

- [ ] **Step 12: Commit**

```bash
git add crates/manifold/src/main.rs crates/manifold/src/debug_control.rs
git commit -m "Graph cards: the frame paints fill, glow and eased hover for its five kinds

Bodies stop painting their own base fill; the frame fills a slot reserved
before the body once hover is known. EXPORTSTATE lists the level's cards
with their screen centers, for headless tests.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Controls for the frame kinds

Every frame card gets the swatch (always) and the eject button (hover, in a group) beside the ✕, drawn from one `card_controls` list. Middle-click closes. Every node menu gets the Colors row, and the group menu gains Remove from Group.

**Files:**
- Modify: `crates/manifold/src/main.rs`: `NodeMenuConfig` (~330–340) and `node_menu_config` (~441–490), the menu popup call (~362), `CardView`/`card_view`, `CardOutput` (~722), `draw_node_close_button` (~738, removed), new `CardControl` + `card_controls` + `paint_card_control`, `draw_graph_card`, the frame-kind call site, tests.

**Interfaces:**
- Consumes: Task 1's `CardView`, `card_view`, `test_view`.
- Produces:
  - `CardView.in_group: bool`;
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] enum CardControl { Swatch, Eject, Close }` with `fn hover_only(self) -> bool`, `fn action(self, kind: ItemNodeKind) -> NodeAction`, `fn rect(self, card: egui::Rect, inv_zoom: f32) -> egui::Rect`;
  - `fn card_controls(view: &CardView) -> Vec<CardControl>` (registration order: a later control takes the pointer where two overlap);
  - `fn paint_card_control(ui, deferred_overlays, control, card_rect, id, view, style, inv_zoom) -> egui::Response`;
  - `CardOutput { response: egui::Response, control: Option<NodeAction> }` (replaces `close_clicked`);
  - `NodeMenuConfig` without `max_width` (every menu grows to fit the swatch row).

- [ ] **Step 1: Write the failing tests**

In `mod tests`, change `test_view` to:

```rust
    fn test_view(kind: ItemNodeKind) -> CardView {
        CardView { kind, color: None, in_group: false }
    }
```

Add after `card_border_eases_from_resting_to_hover`:

```rust
    #[test]
    fn card_controls_list_each_cards_buttons_in_pointer_order() {
        use CardControl::*;
        for kind in GRAPH_DRAW_ORDER {
            assert_eq!(card_controls(&test_view(kind)), vec![Swatch, Close], "{kind:?}");
            let grouped = CardView { in_group: true, ..test_view(kind) };
            assert_eq!(card_controls(&grouped), vec![Swatch, Eject, Close], "{kind:?}");
            let close = if kind == ItemNodeKind::Group { NodeAction::Dissolve } else { NodeAction::Delete };
            assert_eq!(Close.action(kind), close, "{kind:?}");
        }
        assert_eq!(Eject.action(ItemNodeKind::Note), NodeAction::RemoveFromGroup);
        assert_eq!(Swatch.action(ItemNodeKind::Note), NodeAction::CycleColor);
        assert!(Close.hover_only() && Eject.hover_only() && !Swatch.hover_only());
    }
```

Replace the body of `node_menu_config_matches_each_kinds_menu` with:

```rust
        fn menu(kind: ItemNodeKind, has_group: bool, pinned: bool, has_json: bool, headless: bool) -> (&'static str, f32, String) {
            let view = NodeMenuView { kind, has_group, color: None, pinned, has_json, renaming: None };
            let config = node_menu_config(&view, headless);
            let items: Vec<&str> = config
                .items
                .iter()
                .map(|item| match item {
                    NodeMenuItem::Button(label, _) => *label,
                    NodeMenuItem::Colors => "<colors>",
                    NodeMenuItem::Separator => "---",
                })
                .collect();
            (config.area_id, config.min_width, items.join(" | "))
        }
        for has_group in [false, true] {
            // Every kind can join a group (drag onto it), so every menu
            // offers "Remove from Group" when grouped.
            let g = if has_group { " | --- | Remove from Group" } else { "" };
            let menu = |kind, pinned, has_json| menu(kind, has_group, pinned, has_json, false);
            assert_eq!(
                menu(ItemNodeKind::Download, false, false),
                ("download_context_menu", 170.0, format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Remove from List"))
            );
            assert_eq!(menu(ItemNodeKind::Note, false, false), ("note_context_menu", 150.0, format!("Copy Text | <colors>{g} | --- | Delete")));
            assert_eq!(
                menu(ItemNodeKind::Image, false, false),
                ("image_context_menu", 170.0, format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Pdf, false, false),
                ("pdf_context_menu", 170.0, format!("Open Containing Folder | Copy Path | <colors>{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Json, false, false),
                ("json_context_menu", 170.0, format!("Open Containing Folder | Copy Path | Expand as Graph | <colors>{g} | --- | Delete"))
            );
            assert_eq!(
                menu(ItemNodeKind::Tab, false, false),
                ("tab_context_menu", 150.0, format!("Pin | Duplicate | Copy URL | Fit Subtree | <colors>{g} | --- | Close"))
            );
            assert_eq!(
                menu(ItemNodeKind::Tab, true, true).2,
                format!("Unpin | Duplicate | Copy URL | View as JSON | Fit Subtree | <colors>{g} | --- | Close")
            );
            assert_eq!(
                menu(ItemNodeKind::Group, false, false),
                ("group_context_menu", 150.0, format!("Rename | <colors>{g} | --- | Dissolve | Close Group | Export Group..."))
            );
        }
        // Export Group's file dialog can't run headless.
        assert_eq!(
            menu(ItemNodeKind::Group, false, false, false, true).2,
            "Rename | <colors> | --- | Dissolve | Close Group"
        );
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -q -p manifold card_controls node_menu_config`
Expected: compile errors: `CardControl`, `card_controls` and the `in_group` field are unknown.

- [ ] **Step 3: Menus — Colors everywhere, no fixed width**

In `struct NodeMenuConfig`, remove the `max_width: f32,` field, and change its doc comment's last sentence to: `Every menu has a minimum width and no maximum, so it grows to fit the swatch row.`

In the popup call `build_list_popup_frame(ui.ctx(), config.area_id, anchor, config.min_width, config.max_width, fade, ...)`, replace `config.max_width` with `f32::INFINITY`.

In `node_menu_config`, replace everything from `// Every menu but the group's ends in Remove from Group` to the end of the function with:

```rust
    // Every menu ends in the Colors row, Remove from Group (when the node
    // is grouped), then the kind's delete items.
    let finish = |area_id, min_width, mut items: Vec<NodeMenuItem>, delete_items: Vec<NodeMenuItem>| {
        items.push(Colors);
        if view.has_group {
            items.extend([Separator, Button("Remove from Group", A::RemoveFromGroup)]);
        }
        items.push(Separator);
        items.extend(delete_items);
        NodeMenuConfig { area_id, min_width, items }
    };
    let delete = |label| vec![Button(label, A::Delete)];
    match view.kind {
        ItemNodeKind::Download => finish("download_context_menu", 170.0, file_items(), delete("Remove from List")),
        ItemNodeKind::Note => finish("note_context_menu", 150.0, vec![Button("Copy Text", A::CopyText)], delete("Delete")),
        ItemNodeKind::Image => finish("image_context_menu", 170.0, file_items(), delete("Delete")),
        ItemNodeKind::Pdf => finish("pdf_context_menu", 170.0, file_items(), delete("Delete")),
        ItemNodeKind::Json => {
            let mut items = file_items();
            items.push(Button("Expand as Graph", A::ExpandAsGraph));
            finish("json_context_menu", 170.0, items, delete("Delete"))
        }
        ItemNodeKind::Tab => {
            let mut items = vec![
                Button(if view.pinned { "Unpin" } else { "Pin" }, A::TogglePin),
                Button("Duplicate", A::Duplicate),
                Button("Copy URL", A::CopyUrl),
            ];
            if view.has_json {
                items.push(Button("View as JSON", A::ViewJson));
            }
            items.push(Button("Fit Subtree", A::FitSubtree));
            finish("tab_context_menu", 150.0, items, delete("Close"))
        }
        ItemNodeKind::Group => {
            let mut delete_items = vec![Button("Dissolve", A::Dissolve), Button("Close Group", A::Delete)];
            if !headless {
                delete_items.push(Button("Export Group...", A::ExportGroup));
            }
            finish("group_context_menu", 150.0, vec![Button("Rename", A::Rename)], delete_items)
        }
    }
```

(Export Group sits after the delete items, where it is today.)

- [ ] **Step 4: `CardView.in_group`, `CardControl`, `card_controls`**

Add `in_group: bool,` to `struct CardView` with the doc line `/// In a group: its cards show the eject button.`, and `in_group: node.group.is_some()` to `App::card_view`.

After `card_border`, add:

```rust
/// An on-card button of a graph card's frame. `card_controls` lists them
/// in registration order, so where two overlap the later one takes the
/// pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CardControl {
    Swatch,
    Eject,
    Close,
}

impl CardControl {
    /// Revealed on hover, rather than always shown.
    fn hover_only(self) -> bool {
        matches!(self, CardControl::Eject | CardControl::Close)
    }

    fn action(self, kind: ItemNodeKind) -> NodeAction {
        match self {
            CardControl::Swatch => NodeAction::CycleColor,
            CardControl::Eject => NodeAction::RemoveFromGroup,
            CardControl::Close if kind == ItemNodeKind::Group => NodeAction::Dissolve,
            CardControl::Close => NodeAction::Delete,
        }
    }

    /// Its hit and paint rect, centered on one of the card's corners.
    fn rect(self, card: egui::Rect, inv_zoom: f32) -> egui::Rect {
        let (anchor, size) = match self {
            CardControl::Swatch => (card.right_bottom(), 16.0),
            CardControl::Eject => (card.left_top(), 18.0),
            CardControl::Close => (card.right_top(), 18.0),
        };
        egui::Rect::from_center_size(anchor, egui::Vec2::splat(size) * inv_zoom)
    }
}

/// The buttons `view`'s card shows, in registration order (see
/// `CardControl`): the swatch on every card, eject on a grouped one, and
/// the ✕ last, so it wins over anything under it.
fn card_controls(view: &CardView) -> Vec<CardControl> {
    let mut controls = vec![CardControl::Swatch];
    if view.in_group {
        controls.push(CardControl::Eject);
    }
    controls.push(CardControl::Close);
    controls
}
```

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: `cargo test -q -p manifold card_controls node_menu_config`
Expected: `2 passed`.

- [ ] **Step 6: `paint_card_control` replaces `draw_node_close_button`**

Delete `fn draw_node_close_button` and its doc comment. In its place add:

```rust
/// One of a card's `card_controls`, hit-tested and drawn at its corner.
/// Returns its response: the frame turns a click into
/// `control.action(kind)` and counts its hover as the card's. Icons are
/// painted on the overlay layer (see `DeferredOverlays`), like card text.
#[allow(clippy::too_many_arguments)]
fn paint_card_control(
    ui: &egui::Ui,
    deferred_overlays: &mut DeferredOverlays,
    control: CardControl,
    card_rect: egui::Rect,
    id: &str,
    view: &CardView,
    style: &CardStyle,
    inv_zoom: f32,
) -> egui::Response {
    let rect = control.rect(card_rect, inv_zoom);
    let response = ui
        .interact(rect, egui::Id::new(("graph_card_control", control, id)), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    match control {
        CardControl::Swatch => {
            match view.color {
                Some(color) => {
                    ui.painter().circle_filled(rect.center(), 6.0 * inv_zoom, color);
                }
                // A faint ring, so an uncolored card still shows where to
                // click.
                None => {
                    ui.painter().circle_stroke(rect.center(), 6.0 * inv_zoom, egui::Stroke::new(1.5 * inv_zoom, style.border_idle));
                }
            }
        }
        CardControl::Eject => {
            paint_control_square(ui, deferred_overlays, rect, response.hovered(), ACCENT, egui_phosphor::light::SIGN_OUT, style, inv_zoom);
        }
        CardControl::Close => {
            paint_control_square(ui, deferred_overlays, rect, response.hovered(), DANGER_COLOR, egui_phosphor::light::X, style, inv_zoom);
        }
    }
    response
}

/// The ✕ and eject buttons' look: a small square in the card's close
/// colors (`card_style`, so it matches a Note's dark-on-yellow palette),
/// filled `hover_fill` while hovered.
#[allow(clippy::too_many_arguments)]
fn paint_control_square(
    ui: &egui::Ui,
    deferred_overlays: &mut DeferredOverlays,
    rect: egui::Rect,
    hovered: bool,
    hover_fill: egui::Color32,
    icon: &'static str,
    style: &CardStyle,
    inv_zoom: f32,
) {
    ui.painter().rect_filled(rect, 2.0 * inv_zoom, if hovered { hover_fill } else { style.close_fill });
    ui.painter().rect_stroke(
        rect,
        2.0 * inv_zoom,
        egui::Stroke::new(1.0 * inv_zoom, style.close_border),
        egui::StrokeKind::Inside,
    );
    let icon_pos = rect.center();
    let icon_color = style.close_icon;
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        overlay_painter.text(project(icon_pos), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(14.0), icon_color);
    }));
}
```

Change `struct CardOutput` to:

```rust
/// What `App::draw_graph_card` hands back to `build_graph_ui`, which
/// applies each kind's own click action unless a control took the click.
struct CardOutput {
    response: egui::Response,
    /// The action of the control clicked this frame (a middle-click on
    /// the card counts as its ✕).
    control: Option<NodeAction>,
}
```

- [ ] **Step 7: The frame draws the controls**

In `draw_graph_card`, replace everything from `let close = is_hovered.then(|| {` through `let still_hovered = pointer_on_card || close.as_ref().is_some_and(|c| c.hovered());` with:

```rust
        // Registered after both interacts above, so they take the pointer.
        let mut control = response.clicked_by(egui::PointerButton::Middle).then(|| CardControl::Close.action(view.kind));
        let mut control_hovered = false;
        for card_control in card_controls(&view) {
            if card_control.hover_only() && !is_hovered {
                continue;
            }
            let control_response = paint_card_control(ui, deferred_overlays, card_control, rect, id, &view, &style, inv_zoom);
            control_hovered |= control_response.hovered();
            if control_response.clicked() && control.is_none() {
                control = Some(card_control.action(view.kind));
            }
        }

        // This frame's hover only: passing `is_hovered` (which includes
        // `self.hovered`) made a card stay hovered, ✕ and all, after the
        // pointer left it, until another card was hovered.
        let still_hovered = pointer_on_card || control_hovered;
```

(Keep the existing "This frame's hover only" comment once; delete its old copy above the old `still_hovered`.) Change the final `Some(CardOutput { response, close_clicked })` to `Some(CardOutput { response, control })`.

At the frame-kind call site, replace:

```rust
                        if card.close_clicked {
                            // The ✕ consumed this frame's click, so the
                            // kind's own click action below (e.g. Image
                            // maximize) is skipped.
                            actions.push(GraphAction::Node(id.clone(), NodeAction::Delete));
                            continue;
                        }
```

with:

```rust
                        if let Some(action) = card.control {
                            // A control consumed this frame's click, so
                            // the kind's own click action below (e.g.
                            // Image maximize) is skipped.
                            actions.push(GraphAction::Node(id.clone(), action));
                            continue;
                        }
```

- [ ] **Step 8: Build, test, smoke**

Run: `cargo build -q -p manifold && cargo test -q -p manifold`
Expected: no warnings (`grep -n draw_node_close_button crates/manifold/src/main.rs` prints nothing); `23 passed`.

Run: `timeout 500 bash scripts/headless-smoke-test.sh`
Expected: every check passes.

- [ ] **Step 9: Probe — swatch, eject and middle-click on a Note**

Create `target/cardframe/stage2.py`:

```python
from probe import *

SETUP = ["NEWGROUP", "NEWNOTE", "NEWNOTE", "sleep 1"]
run("stage2-layout", SETUP + [export("stage2-layout")])
group = cards("stage2-layout", "Group")[0]
note, spare = cards("stage2-layout", "Note")

# The swatch (bottom-right corner, always shown) colors the spare note;
# then a middle-click closes it.
lines = SETUP + [click(corner(spare, "rb")), export("stage2-colored")]
lines += middle_click(at(spare)) + [export("stage2-closed")]
# The other note joins the group (the frame's existing drop-onto-group),
# and the group is entered.
lines += drag(at(note), at(group))
lines += ["sleep 1", f"DOUBLECLICK {group['screen'][0]:.0f} {group['screen'][1]:.0f}", "sleep 2", export("stage2-in")]
run("stage2a", lines)

# Inside, hover the note, then click its eject button (top-left corner).
inside = cards("stage2-in", "Note")[0]
lines += [f"MOUSEMOVE {inside['screen'][0]:.0f} {inside['screen'][1]:.0f}", click(corner(inside, "lt")), "sleep 1", export("stage2-ejected")]
run("stage2b", lines)

check(next(c for c in cards("stage2-colored") if c["id"] == spare["id"])["color"], "the swatch colored the note")
check(not any(c["id"] == spare["id"] for c in cards("stage2-closed")), "middle-click closed it")
check(state("stage2-in")["current_group"] == group["id"], "entered the group")
check([c["id"] for c in cards("stage2-in", "Note")] == [note["id"]], "the dragged note is inside it")
check(cards("stage2-ejected", "Note") == [], "eject moved the note out of the group's level")
```

Run: `PYTHONPATH=target/cardframe python target/cardframe/stage2.py`
Expected: five `PASS` lines. If a check fails, screenshot that moment (`shot(...)`) and look before changing code.

- [ ] **Step 10: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "Graph cards: swatch, eject and middle-click on the frame's kinds

One card_controls list drives every on-card button; draw_node_close_button
folds into paint_card_control. Every node menu gets the Colors row, and a
nested group's menu gets Remove from Group.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: One drop table

Drops go through `drop_action` (pure) and `App::drop_target` (which card or button the dragged card is over). The frame kinds release into one `GraphAction::Drop`, applied by `App::apply_drop`. The live ring marks the target. `finish_draggable_node` goes.

Interim, until Task 5: the tab loop keeps its own drop code, so a tab dragged over a group card *inside* a group shows the ring (the table says Join) but its release does nothing yet. Tabs and groups release through the table from Tasks 5 and 4.

**Files:**
- Modify: `crates/manifold/src/main.rs`: `GraphAction` (~299), new `DropTarget`/`DropEffect`/`drop_action` and `CardPass` (after `card_controls`), `finish_draggable_node` (~5197, removed), `draw_graph_card`, `apply_graph_action`'s `MergeGroups` arm (~10164), `build_graph_ui` (drop preview before the tab loop, `CardPass` around the frame-kind loop, the group loop's `is_drag_target`), tests.

**Interfaces:**
- Consumes: Task 2's frame.
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq)] enum DropTarget { BackButton, Card(ItemNodeKind) }`;
  - `#[derive(Clone, Copy, Debug, PartialEq)] enum DropEffect { Eject, Merge, Join, GroupWith, Reparent }`;
  - `fn drop_action(dragged: ItemNodeKind, target: DropTarget, inside_group: bool) -> Option<DropEffect>`;
  - `fn App::drop_target(&self, id: &str, pointer: Option<egui::Pos2>, leave_button: Option<egui::Rect>) -> Option<(DropEffect, Option<String>)>`;
  - `GraphAction::Drop { id: String, target: Option<(DropEffect, Option<String>)> }`;
  - `fn App::apply_drop(&mut self, id: &str, target: Option<(DropEffect, Option<String>)>)`;
  - `fn App::take_drag_unit(&mut self, id: &str) -> Vec<String>`;
  - `fn App::merge_groups(&mut self, source: &str, target: &str)`;
  - `struct CardPass<'a> { deferred_overlays: &'a mut DeferredOverlays, actions: &'a mut Vec<GraphAction>, inv_zoom: f32, zoom_scale: f32, drop_preview: &'a Option<(DropEffect, Option<String>)> }`;
  - `draw_graph_card(&mut self, ui: &mut egui::Ui, pass: &mut CardPass, id: &str) -> Option<CardOutput>`.

- [ ] **Step 1: Write the failing test**

After `card_controls_list_each_cards_buttons_in_pointer_order`, add:

```rust
    #[test]
    fn drop_action_matches_the_drop_table() {
        use ItemNodeKind as K;
        let card = DropTarget::Card;
        for kind in GRAPH_DRAW_ORDER {
            // The back button only exists inside a group.
            assert_eq!(drop_action(kind, DropTarget::BackButton, true), Some(DropEffect::Eject), "{kind:?}");
            assert_eq!(drop_action(kind, DropTarget::BackButton, false), None, "{kind:?}");
            let onto_group = if kind == K::Group { DropEffect::Merge } else { DropEffect::Join };
            for inside_group in [false, true] {
                assert_eq!(drop_action(kind, card(K::Group), inside_group), Some(onto_group), "{kind:?}");
            }
        }
        assert_eq!(drop_action(K::Tab, card(K::Tab), false), Some(DropEffect::GroupWith));
        assert_eq!(drop_action(K::Tab, card(K::Tab), true), Some(DropEffect::Reparent));
        // Only a tab drops onto a tab, and nothing drops onto the file kinds.
        assert_eq!(drop_action(K::Note, card(K::Tab), false), None);
        assert_eq!(drop_action(K::Group, card(K::Tab), true), None);
        assert_eq!(drop_action(K::Tab, card(K::Note), false), None);
        assert_eq!(drop_action(K::Image, card(K::Json), true), None);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -q -p manifold drop_action`
Expected: compile error, `drop_action` / `DropTarget` not found.

- [ ] **Step 3: The table**

After `card_controls`, add:

```rust
/// What a dragged card was released over (see `App::drop_target`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum DropTarget {
    /// The toolbar's back button, inside a group.
    BackButton,
    Card(ItemNodeKind),
}

/// What a drop does (`App::apply_drop`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum DropEffect {
    /// Out of its group, one level up.
    Eject,
    /// A group onto a group: its members move into the target, and it goes.
    Merge,
    /// Into the target group.
    Join,
    /// A tab onto a tab at the top level: a new group of both.
    GroupWith,
    /// A tab onto a sibling tab inside a group: the target becomes its
    /// parent.
    Reparent,
}

/// Every drag-and-drop rule in Graph view, for every kind.
fn drop_action(dragged: ItemNodeKind, target: DropTarget, inside_group: bool) -> Option<DropEffect> {
    use ItemNodeKind::{Group, Tab};
    match (dragged, target, inside_group) {
        (_, DropTarget::BackButton, true) => Some(DropEffect::Eject),
        (Group, DropTarget::Card(Group), _) => Some(DropEffect::Merge),
        (_, DropTarget::Card(Group), _) => Some(DropEffect::Join),
        (Tab, DropTarget::Card(Tab), false) => Some(DropEffect::GroupWith),
        (Tab, DropTarget::Card(Tab), true) => Some(DropEffect::Reparent),
        _ => None,
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -q -p manifold drop_action`
Expected: `1 passed` (with dead-code warnings until Step 7).

- [ ] **Step 5: `drop_target`, `apply_drop` and friends**

In `impl App`, next to `would_create_cycle`, add:

```rust
    /// Where the dragged card `id` would drop right now, and what that
    /// would do (`drop_action`). The back button is tested first, by
    /// pointer, since it's a screen-space toolbar widget. Then the card at
    /// this level containing the dragged card's center, a group card before
    /// a tab card. Feeds both the live ring and the release, so the two
    /// can't disagree.
    fn drop_target(
        &self,
        id: &str,
        pointer: Option<egui::Pos2>,
        leave_button: Option<egui::Rect>,
    ) -> Option<(DropEffect, Option<String>)> {
        let node = self.nodes.get(id)?;
        let dragged = item_node_kind(&node.payload);
        let inside_group = self.current_group.is_some();
        if leave_button.zip(pointer).is_some_and(|(button, pointer)| button.contains(pointer)) {
            return drop_action(dragged, DropTarget::BackButton, inside_group).map(|effect| (effect, None));
        }
        let center = egui::Pos2::new(node.pos.x, node.pos.y);
        [ItemNodeKind::Group, ItemNodeKind::Tab].into_iter().find_map(|kind| {
            let effect = drop_action(dragged, DropTarget::Card(kind), inside_group)?;
            let target = self.nodes_at_current_level(kind).into_iter().find(|other| {
                other != id && self.nodes.get(other).is_some_and(|n| node_card_rect(n).contains(center))
            })?;
            Some((effect, Some(target)))
        })
    }

    /// What a drop moves along with `id`: the whole rectangle selection
    /// when `id` is in it (it acts as one unit, as "Group Selected" does),
    /// which it empties, else just `id`.
    fn take_drag_unit(&mut self, id: &str) -> Vec<String> {
        if self.selected_tabs.contains(id) {
            self.selected_tabs.drain().collect()
        } else {
            vec![id.to_string()]
        }
    }

    /// A card released after a drag (`GraphAction::Drop`), with where it
    /// landed, if anywhere. Saves the session on every release, since the
    /// drag itself moved the card.
    fn apply_drop(&mut self, id: &str, target: Option<(DropEffect, Option<String>)>) {
        match target {
            Some((DropEffect::Eject, _)) => remove_from_group(&mut self.nodes, id),
            Some((DropEffect::Join, Some(group))) => {
                for member in self.take_drag_unit(id) {
                    if let Some(node) = self.nodes.get_mut(&member) {
                        node.group = Some(group.clone());
                    }
                }
            }
            Some((DropEffect::GroupWith, Some(other))) => {
                let group = self.alloc_group_id();
                let pos = self.nodes.get(&other).map_or(egui::Vec2::ZERO, |n| n.pos);
                self.insert_group_at(group.clone(), pos);
                let unit = self.take_drag_unit(id);
                for member in std::iter::once(other).chain(unit) {
                    if let Some(node) = self.nodes.get_mut(&member) {
                        node.group = Some(group.clone());
                    }
                }
            }
            Some((DropEffect::Reparent, Some(parent))) => {
                if !self.would_create_cycle(id, &parent) {
                    if let Some(node) = self.nodes.get_mut(id) {
                        node.parent = Some(parent);
                    }
                }
            }
            Some((DropEffect::Merge, Some(target))) => self.merge_groups(id, &target),
            _ => {}
        }
        self.save_session();
    }
```

Move the body of `apply_graph_action`'s `GraphAction::MergeGroups { source, target } => { ... }` arm into a new method, adjusting it for `&str` parameters (`target.to_string()` where it needs an owned `String`, `cur == source` comparing `String` with `&str`):

```rust
    /// `source`'s members move into `target`, and `source` goes: a group
    /// card dropped onto another. Refused if `target` is nested inside
    /// `source` (directly or transitively, walking the same `.group` chain
    /// `group_breadcrumb` does): reassigning source's own members to target
    /// while target is one of them would leave it (or a group between them)
    /// pointing at itself once source is removed, a self-reference cycle.
    fn merge_groups(&mut self, source: &str, target: &str) {
        let mut ancestor = Some(target.to_string());
        while let Some(cur) = ancestor {
            if cur == source {
                return;
            }
            ancestor = self.nodes.get(&cur).and_then(|n| n.group.clone());
        }
        for node in self.nodes.values_mut() {
            if node.group.as_deref() == Some(source) {
                node.group = Some(target.to_string());
            }
        }
        self.nodes.remove(source);
        self.node_order.retain(|g| g != source);
    }
```

and make the arm `GraphAction::MergeGroups { source, target } => self.merge_groups(&source, &target),`.

In `enum GraphAction`, after `MergeGroups { .. }`, add:

```rust
    /// A card released after a drag, with where it landed
    /// (`App::drop_target`), if anywhere.
    Drop { id: String, target: Option<(DropEffect, Option<String>)> },
```

and in `apply_graph_action`, add the arm `GraphAction::Drop { id, target } => self.apply_drop(&id, target),`.

- [ ] **Step 6: `CardPass`**

After `drop_action`, add:

```rust
/// What every card in one Graph-view card pass shares
/// (`App::draw_graph_card`).
struct CardPass<'a> {
    deferred_overlays: &'a mut DeferredOverlays,
    actions: &'a mut Vec<GraphAction>,
    inv_zoom: f32,
    zoom_scale: f32,
    /// Where the card being dragged would drop right now
    /// (`App::drop_target`), computed once before the pass.
    drop_preview: &'a Option<(DropEffect, Option<String>)>,
}
```

- [ ] **Step 7: The frame drags and drops through the table**

Change `draw_graph_card`'s signature to:

```rust
    fn draw_graph_card(&mut self, ui: &mut egui::Ui, pass: &mut CardPass, id: &str) -> Option<CardOutput> {
        let (inv_zoom, zoom_scale) = (pass.inv_zoom, pass.zoom_scale);
        let deferred_overlays = &mut *pass.deferred_overlays;
```

(the rest of the body keeps using `inv_zoom`, `zoom_scale` and `deferred_overlays`). Update its doc comment to mention drag and drop through `drop_target`.

After the border's `rect_stroke`, add the drop ring:

```rust
        // The card the dragged one would drop onto: a bold ring, expanded
        // well past this card so it still shows around a larger dragged
        // card on top of it. Tracks the pointer directly, not eased.
        if pass.drop_preview.as_ref().and_then(|(_, target)| target.as_deref()) == Some(id) {
            ui.painter().rect_stroke(
                rect.expand(16.0 * inv_zoom),
                10.0 * inv_zoom,
                egui::Stroke::new(4.0 * inv_zoom, ACCENT),
                egui::StrokeKind::Outside,
            );
        }
```

Replace `self.finish_draggable_node(id, &response, still_hovered);` with:

```rust
        if response.dragged() {
            self.dragging = Some(id.to_string());
            if let Some(node) = self.nodes.get_mut(id) {
                node.pos += response.drag_delta();
            }
        } else if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
            // `drop_preview` was computed this frame for this card, and
            // nothing moved it since (no drag delta on a release frame).
            pass.actions.push(GraphAction::Drop { id: id.to_string(), target: pass.drop_preview.clone() });
        }
        if still_hovered {
            self.hovered = Some(id.to_string());
        } else if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
```

Delete `fn finish_draggable_node` and its doc comment.

- [ ] **Step 8: `build_graph_ui` computes the preview and builds the pass**

Just before the tab loop `for id in tabs_at_level.clone() {`, add:

```rust
                // One drop target per frame, shared by the ring and the
                // release (see `drop_target`).
                let pointer = ui.input(|i| i.pointer.interact_pos());
                let drop_preview =
                    self.dragging.clone().and_then(|dragged| self.drop_target(&dragged, pointer, leave_group_button_rect));
```

Around the frame-kind loop (`for (kind, ids) in [ ... ] { ... }`), build the pass first and use it inside:

```rust
                let mut pass = CardPass {
                    deferred_overlays: &mut deferred_overlays,
                    actions: &mut actions,
                    inv_zoom,
                    zoom_scale,
                    drop_preview: &drop_preview,
                };
```

In that loop: `self.draw_graph_card(ui, &mut deferred_overlays, id, inv_zoom, zoom_scale)` becomes `self.draw_graph_card(ui, &mut pass, id)`, and every `actions.push(` becomes `pass.actions.push(`. The pass must end before the group loop, which still uses `deferred_overlays` and `actions` directly: add `drop(pass);` right after the frame-kind loop's closing brace if the borrow checker needs it.

In the group loop, replace the whole `let is_drag_target = self.dragging.as_deref().is_some_and(|dragging_id| { ... });` statement (and its long comment) with:

```rust
                        // See the frame's own drop ring.
                        let is_drag_target =
                            drop_preview.as_ref().and_then(|(_, target)| target.as_deref()) == Some(id.as_str());
```

- [ ] **Step 9: Build, test, smoke**

Run: `cargo build -q -p manifold && cargo test -q -p manifold`
Expected: no warnings (`grep -n finish_draggable_node crates/manifold/src/main.rs` prints nothing); `24 passed`.

Run: `timeout 500 bash scripts/headless-smoke-test.sh`
Expected: every check passes.

- [ ] **Step 10: Probe — a Note into a group card, then out through the back button**

Create `target/cardframe/stage3.py`:

```python
from probe import *

SETUP = ["NEWGROUP", "NEWNOTE", "sleep 1"]
run("stage3-layout", SETUP + [export("stage3-layout")])
group = cards("stage3-layout", "Group")[0]
note = cards("stage3-layout", "Note")[0]
lines = SETUP + drag(at(note), at(group)) + ["sleep 1", export("stage3-joined")]
lines += [f"DOUBLECLICK {group['screen'][0]:.0f} {group['screen'][1]:.0f}", "sleep 2", export("stage3-in")]
run("stage3a", lines)
inside = cards("stage3-in", "Note")[0]
# The back button is at (22, 17).
lines += drag(at(inside), (22, 17)) + ["sleep 1", export("stage3-ejected"), "CLICK 22 17", "sleep 2", export("stage3-top")]
run("stage3b", lines)

check(not any(c["kind"] == "Note" for c in cards("stage3-joined")), "the note left the top level")
check(cards("stage3-in", "Note")[0]["group"] == group["id"], "the note is in the group")
check(cards("stage3-ejected", "Note") == [], "the back-button drop ejected it")
top = cards("stage3-top", "Note")
check(len(top) == 1 and top[0]["group"] is None, "it's back at the top level")
```

Run: `PYTHONPATH=target/cardframe python target/cardframe/stage3.py`
Expected: four `PASS` lines. Also take one screenshot mid-drag (add `shot("stage3-ring")` after the fourth `MOUSEMOVE` of the first drag) and check the group card shows the ACCENT ring.

- [ ] **Step 11: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "Graph cards: one drop table for the frame's kinds

drop_action holds every drag-and-drop rule; drop_target finds the card or
back button under a dragged card, for both the live ring and the release.
The frame's kinds release into GraphAction::Drop. They can now also leave
a group through the back button. finish_draggable_node goes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Group onto the frame

The group loop goes. Group cards get a body arm, a style arm (ACCENT idle border, 6.0 dim rounding), an overloaded border, a filter-text arm, and the frame's controls (swatch moves to bottom-right; eject when nested). Merges go through `GraphAction::Drop`, so `GraphAction::MergeGroups` goes.

**Files:**
- Modify: `crates/manifold/src/main.rs`: `CardView`/`card_view`, `card_border`, `card_style`, `card_filter_text` (~936), `paint_card_body` (signature, Group arm), `draw_graph_card`, `GraphAction::MergeGroups` (~314, removed) and its apply arm, comments naming `GraphAction::MergeGroups` (~5929, ~6133), `build_graph_ui`'s frame-kind loop and group loop (~11760–12105, removed), tests.

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces:
  - `CardView.overloaded: bool`, `CardView.members: Vec<String>` (Group only; empty otherwise);
  - `fn card_filter_text(payload: &NodePayload) -> Vec<&str>` (Tab: title and URL; Group: name);
  - `paint_card_body(&mut self, ui, deferred_overlays, id, view: &CardView, rect, inv_zoom, zoom_scale) -> CardBody`;
  - the frame-kind loop iterates `GRAPH_DRAW_ORDER.iter().zip(at_level.iter()).skip(1)` (every kind but Tab).

- [ ] **Step 1: Write the failing tests**

Change `test_view` to:

```rust
    fn test_view(kind: ItemNodeKind) -> CardView {
        CardView { kind, color: None, in_group: false, overloaded: false, members: Vec::new() }
    }
```

In `card_border_eases_from_resting_to_hover`, before the "Fully hovered" line, add:

```rust
        // An overloaded group rests on DANGER_COLOR, unless tagged.
        let overloaded = |color| CardView { color, overloaded: true, ..test_view(ItemNodeKind::Group) };
        assert_eq!(card_border(&overloaded(None), &style, 0.0), (1.0, DANGER_COLOR));
        assert_eq!(card_border(&overloaded(Some(red)), &style, 0.0), (1.0, red));
```

Rename the test to `card_border_resolves_tag_overload_idle_and_hover`.

Replace `card_filter_text_matches_each_kinds_filter_input`'s body with (Tab can't be constructed in a unit test, see the first test's comment; its arm is checked live in Task 5):

```rust
        let download = NodePayload::Download(DownloadPayload { path: "C:/dl/f.bin".into(), bytes_received: 0, total_bytes: None, completed: true });
        // Download filters on its full path; the file kinds on the filename only.
        assert_eq!(card_filter_text(&download), ["C:/dl/f.bin"]);
        assert_eq!(card_filter_text(&NodePayload::Note(NotePayload { text: "buy milk".into(), editing: false })), ["buy milk"]);
        assert_eq!(card_filter_text(&NodePayload::Image(ImagePayload { path: "C:/pics/cat.png".into(), width: 1, height: 1, error: None })), ["cat.png"]);
        assert_eq!(card_filter_text(&NodePayload::Pdf(PdfPayload { path: "C:/docs/paper.pdf".into(), outcome: None })), ["paper.pdf"]);
        assert_eq!(card_filter_text(&NodePayload::Json(JsonPayload { path: "C:/data/x.json".into(), value: serde_json::json!(1), key: None, error: None })), ["x.json"]);
        assert_eq!(card_filter_text(&NodePayload::Group(GroupPayload { name: "Reading".into() })), ["Reading"]);
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -q -p manifold card_border card_filter_text`
Expected: compile errors (`overloaded`/`members` unknown fields; `Vec<&str>` vs `&str`).

- [ ] **Step 3: `CardView`, `card_border`, `card_style`, `card_filter_text`**

Add to `struct CardView`:

```rust
    /// A group over `GROUP_OVERLOAD_THRESHOLD` members: its border warns.
    overloaded: bool,
    /// Group only: its members' ids, for its thumbnail grid and count.
    members: Vec<String>,
```

In `App::card_view`, compute them:

```rust
        let kind = item_node_kind(&node.payload);
        let members: Vec<String> = if kind == ItemNodeKind::Group {
            self.nodes.iter().filter(|(_, n)| n.group.as_deref() == Some(id)).map(|(member, _)| member.clone()).collect()
        } else {
            Vec::new()
        };
        Some(CardView {
            kind,
            color: node.color,
            in_group: node.group.is_some(),
            overloaded: members.len() > GROUP_OVERLOAD_THRESHOLD,
            members,
        })
```

In `card_border`, change the resting line to:

```rust
    let resting = view.color.unwrap_or(if view.overloaded { DANGER_COLOR } else { style.border_idle });
```

and its doc comment's "else the kind's idle color" to "else DANGER_COLOR for an overloaded group, else the kind's idle color".

In `card_style`, change `ItemNodeKind::Download => CardStyle { dim_rounding: 6.0, ..themed },` to:

```rust
        ItemNodeKind::Download | ItemNodeKind::Tab => CardStyle { dim_rounding: 6.0, ..themed },
        // ACCENT at rest, as group cards have always been.
        ItemNodeKind::Group => CardStyle { border_idle: ACCENT, dim_rounding: 6.0, ..themed },
```

keeping `_ => themed,` for Image, Pdf and Json.

Change `card_filter_text` to:

```rust
/// What the Graph filter box matches a card against: a tab's title and
/// URL, a download's full path, a note's text, a file node's filename, a
/// group's name.
fn card_filter_text(payload: &NodePayload) -> Vec<&str> {
    match payload {
        NodePayload::Tab(tab) => vec![&tab.title, &tab.url],
        NodePayload::Download(download) => vec![&download.path],
        NodePayload::Note(note) => vec![&note.text],
        NodePayload::Image(image) => vec![download_filename(&image.path)],
        NodePayload::Pdf(pdf) => vec![download_filename(&pdf.path)],
        NodePayload::Json(json) => vec![download_filename(&json.path)],
        NodePayload::Group(group) => vec![&group.name],
    }
}
```

In `draw_graph_card`, the dim call becomes `dim_if_filtered_out(deferred_overlays, &self.graph_filter, &card_filter_text(&node.payload), rect, style.dim_rounding);`.

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -q -p manifold card_border card_filter_text`
Expected: `2 passed`.

- [ ] **Step 5: The Group body arm**

Change `paint_card_body`'s `kind: ItemNodeKind,` parameter to `view: &CardView,`, and its `match kind {` to `match view.kind {`. In `draw_graph_card`, pass `&view` in place of `view.kind`.

Replace the `ItemNodeKind::Tab | ItemNodeKind::Group => { unreachable!(...) }` arm with:

```rust
            // A member thumbnail grid (or the name and member count, when
            // empty or with "Collapse Groups" on). LOD: skipped once
            // simplified, leaving the frame's plain card.
            ItemNodeKind::Group => {
                let member_ids = &view.members;
                let member_count = member_ids.len();
                <MOVED BLOCK>
            }
            ItemNodeKind::Tab => unreachable!("Tab cards are drawn by their own loop in build_graph_ui"),
```

`<MOVED BLOCK>` is the group loop's content block, moved verbatim: from the line `if !self.graph_lod_simplified {` that follows the comment `// LOD: the whole member-thumbnail-grid/name/count` through its matching closing `}` (the doubled `}` / `}` just before `if is_hovered {` and the group's close button). Drop the old `// LOD: ...` comment (the new arm comment replaces it). Inside the moved block, `self.nodes.get(&id)` becomes `self.nodes.get(id)` (`id` is `&str` here). Everything else it reads (`rect`, `inv_zoom`, `zoom_scale`, `deferred_overlays`, `self.truncate_cache`, `self.groups_collapsed`, `self.gpu`) is already in scope.

- [ ] **Step 6: Groups go through the frame; the group loop goes**

At the frame-kind loop, replace the header:

```rust
                for (kind, ids) in [
                    (ItemNodeKind::Download, downloads_at_level),
                    (ItemNodeKind::Note, notes_at_level),
                    (ItemNodeKind::Image, images_at_level),
                    (ItemNodeKind::Pdf, pdfs_at_level),
                    (ItemNodeKind::Json, jsons_at_level),
                ] {
```

with:

```rust
                // Tab cards still have their own loop, above.
                for (&kind, ids) in GRAPH_DRAW_ORDER.iter().zip(at_level.iter()).skip(1) {
```

and update the comment above it to say it covers every kind but Tab, in `GRAPH_DRAW_ORDER` (Group last). In its `match kind {`, add before `_ => {}`:

```rust
                            // Drills into the group's own subgraph.
                            ItemNodeKind::Group if card.response.double_clicked() => {
                                pass.actions.push(GraphAction::EnterGroup(id.clone()))
                            }
```

Delete the whole group loop: from the comment `// Group nodes: rendered at whichever level `self.current_group`` through the loop's closing brace (just before `// The empty-canvas "New Tab/Note/Group/File" card`). Its `drop(pass);` from Task 3, if added, goes too.

If `downloads_at_level`, `notes_at_level`, `images_at_level`, `pdfs_at_level`, `jsons_at_level` are now unused, rename them to `_` in the `let [tabs_at_level, ...] = &at_level;` destructure (keep `tabs_at_level` and `groups_at_level`, which other code reads).

- [ ] **Step 7: `GraphAction::MergeGroups` goes**

Remove the `MergeGroups { source: String, target: String },` variant and its doc comment, and its `apply_graph_action` arm. Replace `GraphAction::MergeGroups` with `App::merge_groups` in the two comments that name it (`grep -n "GraphAction::MergeGroups" crates/manifold/src/main.rs`; in `group_breadcrumb` the phrase becomes "the merge-drag gesture, already refuses a cycle at the source -- see `App::merge_groups`'s own ancestor check").

- [ ] **Step 8: Build, test, smoke**

Run: `cargo build -q -p manifold && cargo test -q -p manifold`
Expected: no warnings; `24 passed`. `grep -n '"graph_group' crates/manifold/src/main.rs` prints nothing.

Run: `timeout 500 bash scripts/headless-smoke-test.sh`
Expected: every check passes (including `group_in`/`group_out`: double-click enters, the back button leaves).

- [ ] **Step 9: Probe — groups with 0, 1 and 5 members, and a merge**

Create `target/cardframe/stage4.py`:

```python
from probe import *

SETUP = ["NEWGROUP", "NEWGROUP", "NEWGROUP"] + ["NEWNOTE"] * 6 + ["sleep 1"]
run("stage4-layout", SETUP + [export("stage4-layout")])
empty, one, five = cards("stage4-layout", "Group")
notes = cards("stage4-layout", "Note")
lines = list(SETUP) + drag(at(notes[0]), at(one))
for note in notes[1:]:
    lines += drag(at(note), at(five))
lines += ["sleep 1", export("stage4-filled"), shot("stage4-groups")]
lines += drag(at(one), at(empty)) + ["sleep 1", export("stage4-merged"), shot("stage4-merged")]
run("stage4", lines)

check(len(cards("stage4-filled", "Group")) == 3 and cards("stage4-filled", "Note") == [], "all six notes joined a group")
merged = cards("stage4-merged", "Group")
check([g["id"] for g in merged] == [empty["id"], five["id"]], "the one-member group merged into the empty one")
```

Run: `PYTHONPATH=target/cardframe python target/cardframe/stage4.py`
Expected: two `PASS` lines. Look at `stage4-groups.png`: the empty group shows its name and "0 members"; the one-member group a single grid cell; the five-member group four cells and "+1"; every group has the ACCENT border, its swatch at the bottom-right, and a glow. `stage4-merged.png`: two groups, the first now showing one cell.

- [ ] **Step 10: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "Graph cards: Group onto the shared frame

The group loop goes: a Group body arm, style, overloaded border and filter
text. Its swatch moves to the bottom-right; a nested group gets the eject
button; a hovered group's border lerps to the text color like every other
kind. Merges go through GraphAction::Drop, so MergeGroups goes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Tab onto the frame, and docs

The tab loop goes. Tab cards get a body arm, the pin and audio controls, the focused border, the selection wash and multi-select drag. Every card now goes through one loop over `GRAPH_DRAW_ORDER`. README item 7 is marked Done.

**Files:**
- Modify: `crates/manifold/src/main.rs`: `CardView`/`card_view`, `card_border`, `CardControl`/`card_controls`/`paint_card_control`, `paint_card_body` (Tab arm), `draw_graph_card`, `draw_card_glow` (~2122, removed), `build_graph_ui` (tab loop ~11004–11688 removed, frame-kind loop header), tests.
- Modify: `README.md` (item 7, line ~333), `docs/superpowers/specs/2026-09-23-graph-card-frame-design.md` (Non-goals, line ~149).

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces:
  - `CardView.{focused, pinned, muted, audio_playing, selected}: bool`;
  - `CardControl::{Pin, Audio}`; `card_controls` order `[Swatch, Pin?, Audio?, Eject?, Close]`;
  - `paint_card_control(..., inv_zoom: f32, lod_simplified: bool)`.

- [ ] **Step 1: Write the failing tests**

Change `test_view` to:

```rust
    fn test_view(kind: ItemNodeKind) -> CardView {
        CardView {
            kind,
            color: None,
            in_group: false,
            overloaded: false,
            members: Vec::new(),
            focused: false,
            pinned: false,
            muted: false,
            audio_playing: false,
            selected: false,
        }
    }
```

In `card_border_resolves_tag_overload_idle_and_hover`, add at the end, and rename the test `card_border_resolves_focus_tag_overload_idle_and_hover`:

```rust
        // The open tab: ACCENT at 2.5 over everything, not eased.
        let focused = CardView { color: Some(red), focused: true, ..test_view(ItemNodeKind::Tab) };
        assert_eq!(card_border(&focused, &style, 0.0), (2.5, ACCENT));
        assert_eq!(card_border(&focused, &style, 1.0), (2.5, ACCENT));
```

In `card_controls_list_each_cards_buttons_in_pointer_order`, change the loop header to `for kind in GRAPH_DRAW_ORDER.into_iter().filter(|&k| k != ItemNodeKind::Tab) {` and add after the loop:

```rust
        // A tab always shows its pin; its audio indicator while playing or
        // muted. Close still registers last: it overlaps the indicator.
        let tab = |f: fn(&mut CardView)| {
            let mut view = test_view(ItemNodeKind::Tab);
            f(&mut view);
            card_controls(&view)
        };
        assert_eq!(tab(|_| {}), vec![Swatch, Pin, Close]);
        assert_eq!(tab(|v| v.pinned = true), vec![Swatch, Pin, Close]);
        assert_eq!(tab(|v| v.audio_playing = true), vec![Swatch, Pin, Audio, Close]);
        assert_eq!(tab(|v| v.muted = true), vec![Swatch, Pin, Audio, Close]);
        assert_eq!(tab(|v| { v.muted = true; v.in_group = true; }), vec![Swatch, Pin, Audio, Eject, Close]);
        assert_eq!(Close.action(ItemNodeKind::Tab), NodeAction::Delete);
        assert_eq!(Pin.action(ItemNodeKind::Tab), NodeAction::TogglePin);
        assert_eq!(Audio.action(ItemNodeKind::Tab), NodeAction::ToggleMute);
        assert!(!Pin.hover_only() && !Audio.hover_only());
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -q -p manifold card_border card_controls`
Expected: compile errors (unknown fields and variants).

- [ ] **Step 3: `CardView`, `card_view`, `card_border`, controls**

Add to `struct CardView`:

```rust
    /// The open tab: its border is ACCENT.
    focused: bool,
    /// Tab only.
    pinned: bool,
    /// Tab only.
    muted: bool,
    /// Tab only.
    audio_playing: bool,
    /// In the rectangle selection (tabs only): washed in ACCENT, and
    /// dragged as one unit.
    selected: bool,
```

In `App::card_view`, add:

```rust
        let tab = node.as_tab();
```

and the fields `focused: self.focused.as_deref() == Some(id)`, `pinned: tab.is_some_and(|t| t.pinned)`, `muted: tab.is_some_and(|t| t.muted)`, `audio_playing: tab.is_some_and(|t| t.audio_playing)`, `selected: self.selected_tabs.contains(id)`.

In `card_border`, add as the first line:

```rust
    // The strongest, least ambiguous signal: not eased, and over any tag.
    if view.focused {
        return (2.5, ACCENT);
    }
```

and add "A focused tab's is ACCENT at 2.5." to its doc comment.

Add `Pin` and `Audio` to `enum CardControl` (order: `Swatch, Pin, Audio, Eject, Close`). In `impl CardControl`: `action` gains `CardControl::Pin => NodeAction::TogglePin,` and `CardControl::Audio => NodeAction::ToggleMute,`; `rect` gains `CardControl::Pin => (card.left_bottom(), 16.0),` and `CardControl::Audio => (card.right_top() + egui::vec2(-8.0, 8.0) * inv_zoom, 16.0),`. (`hover_only` is unchanged: pin and audio are always shown.)

In `card_controls`, after `let mut controls = vec![CardControl::Swatch];`, add:

```rust
    if view.kind == ItemNodeKind::Tab {
        controls.push(CardControl::Pin);
        // Also while muted after audio stops, so a mute can always be
        // seen and undone from Graph view.
        if view.audio_playing || view.muted {
            controls.push(CardControl::Audio);
        }
    }
```

and extend its doc comment: "a tab's pin, and its audio indicator while playing or muted".

- [ ] **Step 4: `paint_card_control` draws pin and audio**

(`match control` in `paint_card_control` is non-exhaustive until this step, so the tests run after it.)

Add a `lod_simplified: bool` parameter after `inv_zoom`, and pass `self.graph_lod_simplified` from `draw_graph_card`. Add two arms, moving the tab loop's code:

```rust
        // A `PUSH_PIN` icon, ACCENT when pinned. A pinned node is left out
        // of the force-directed layout (see `TabPayload::pinned`).
        CardControl::Pin => {
            let pin_color = if view.pinned { ACCENT } else { ui.visuals().weak_text_color() };
            if !lod_simplified {
                let pin_icon_pos = rect.center();
                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                    overlay_painter.text(
                        project(pin_icon_pos),
                        egui::Align2::CENTER_CENTER,
                        egui_phosphor::light::PUSH_PIN,
                        egui::FontId::proportional(14.0),
                        pin_color,
                    );
                }));
            }
        }
        CardControl::Audio => {
            <MOVED AUDIO BLOCK>
        }
```

`<MOVED AUDIO BLOCK>` is the tab loop's `if node_muted { ... } else { ... }` statement, moved verbatim with its comments (the muted `SPEAKER_X` icon, and the animated three-bar waveform), inside `if node_audio_playing || node_muted {` after `let audio_response = ...;`. In it, `node_muted` becomes `view.muted`, `self.graph_lod_simplified` becomes `lod_simplified`, and `audio_rect` becomes `rect`. Also move the comment block above `let mut audio_clicked = false;` (it explains the painter-drawn indicator and why it shows while muted) to sit above the `CardControl::Audio` arm, trimming its first line to "Audio-playing/muted indicator:".

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: `cargo test -q -p manifold card_border card_controls`
Expected: `2 passed`.

- [ ] **Step 6: The Tab body arm**

Replace `ItemNodeKind::Tab => unreachable!(...)` in `paint_card_body` with:

```rust
            // A live thumbnail (or a spinner until the first frame), a
            // title strip with the engine's color dot, and the favicon.
            ItemNodeKind::Tab => {
                let Some((title, url, engine_kind)) =
                    self.nodes.get(id).and_then(|n| n.as_tab()).map(|t| (t.title.clone(), t.url.clone(), t.tab.kind))
                else {
                    return body;
                };
                <MOVED TAB BLOCK>
            }
```

`<MOVED TAB BLOCK>` is the tab loop's content block, moved verbatim: the comment starting `// LOD: the thumbnail texture blit and the title/`, then `if !self.graph_lod_simplified {` through its matching `}` (just before the `// Audio-playing/muted indicator` comment). In it, `g.thumbnail_id(&id)` becomes `g.thumbnail_id(id)`. The LOD comment's end ("leaving the plain background/border/color-swatch painted above and below as the simplified card's whole appearance") becomes "leaving the frame's plain card and controls as the simplified card's whole appearance".

- [ ] **Step 7: The frame: selection wash and multi-select drag**

In `draw_graph_card`, after the drop ring, add:

```rust
        // Rectangle-select's highlight: a translucent ACCENT wash, distinct
        // from the focus border (an outline only), so a selected tab still
        // reads differently from the focused one when both apply.
        if view.selected {
            ui.painter().rect_filled(
                rect,
                2.0 * inv_zoom,
                egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 45),
            );
        }
```

In the drag step, replace:

```rust
            if let Some(node) = self.nodes.get_mut(id) {
                node.pos += response.drag_delta();
            }
```

with:

```rust
            // Dragging one selected tab moves the whole selection, as one
            // unit (see `take_drag_unit`).
            let delta = response.drag_delta();
            let moved = if view.selected { self.selected_tabs.iter().cloned().collect() } else { vec![id.to_string()] };
            for moved_id in moved {
                if let Some(node) = self.nodes.get_mut(&moved_id) {
                    node.pos += delta;
                }
            }
```

Move the tab loop's viewport-culling comment (starting `// Viewport culling: skip every bit of interact/paint`) into `draw_graph_card`, replacing the frame's `// Viewport culling -- see the tab loop's own comment. ...` lines.

- [ ] **Step 8: One card loop; the tab loop goes**

Delete the tab loop: from `for id in tabs_at_level.clone() {` through its closing brace (just before the comment above the frame-kind loop). Move the `drop_preview` computation (Task 3 Step 8) so it stays before the card loop. In the frame-kind loop header, drop `.skip(1)` and the "Tab cards still have their own loop" comment, and update the comment above: every card, kind by kind in `GRAPH_DRAW_ORDER`, which is also their stacking order. In its `match kind {`, add before `ItemNodeKind::Download if ...`:

```rust
                            // Opens the tab into Tab view.
                            ItemNodeKind::Tab if card.response.clicked() => {
                                pass.actions.push(GraphAction::Node(id.clone(), NodeAction::Open))
                            }
```

Delete `fn draw_card_glow` and its doc comment (its only callers were the two loops). Move the doc's reasoning worth keeping into `card_glow_shapes`'s doc: "A soft glow tinted the card's own border color, grown outward with decreasing alpha: a plain black drop shadow was invisible against this app's near-black background. `intensity` scales the alpha only, not the spread, so a hover brightens it without resizing the card."

Then:
- `grep -n "tab loop\|group loop\|own loop" crates/manifold/src/main.rs` and reword each remaining comment that points at the removed loops (e.g. "see the tab loop's own doc comment" → "see `draw_graph_card`").
- `grep -n '"graph_node' crates/manifold/src/main.rs` must print nothing.
- If `tabs_at_level` is now unused, rename it to `_` in the destructure.

- [ ] **Step 9: Build, test, smoke**

Run: `cargo build -q -p manifold && cargo test -q -p manifold`
Expected: no warnings; `24 passed`.

Run: `timeout 500 bash scripts/headless-smoke-test.sh`
Expected: every check passes. This run exercises Tab cards on the frame: clicking a root tab card enters Tab view, and the group enter/leave checks.

- [ ] **Step 10: Probe — tab gestures**

Tabs need live engines, so this probe seeds a session of four top-level WebView2 tabs on the local `manifold://welcome` page (no network), in its own seed directory, and waits for them with `WAITTABS`. Create `target/cardframe/stage5.py`:

```python
import json
import probe
from probe import *

SEED5 = OUT / "seed5"
SEED5.mkdir(exist_ok=True)
(SEED5 / "settings.json").write_text('{"has_shown_first_launch_demo": true}')
tab = lambda n, x, y: {"id": f"tab-{n}", "parent": None, "url": "manifold://welcome", "engine": "webview2", "pos_x": x, "pos_y": y}
(SEED5 / "session.json").write_text(json.dumps({
    "tabs": [tab(1, -260, -150), tab(2, 260, -150), tab(3, -260, 150), tab(4, 260, 150)],
    "focused": "tab-1",
    "view_mode_tab": False,
    "next_tab_id": 5,
}))
probe.SEED = SEED5

SETUP = ["WAITTABS", "sleep 2"]
run("stage5-layout", SETUP + [export("stage5-layout"), shot("stage5-rest")])
a, b, c, d = cards("stage5-layout", "Tab")
# The pin is on the bottom-left corner. Then tab b onto tab c, and a
# middle-click on tab d.
lines = SETUP + [export("stage5-pre"), click(corner(a, "lb")), export("stage5-pinned")]
lines += drag(at(b), at(c)) + ["sleep 1", export("stage5-grouped")]
lines += middle_click(at(d)) + ["sleep 1", export("stage5-closed")]
run("stage5", lines)

check([t["screen"] for t in cards("stage5-pre", "Tab")] == [t["screen"] for t in cards("stage5-layout", "Tab")],
      "the layout matches between runs")
check(next(t for t in cards("stage5-pinned") if t["id"] == a["id"])["pinned"], "the pin toggled")
check(len(cards("stage5-grouped", "Group")) == 1 and not any(t["id"] in (b["id"], c["id"]) for t in cards("stage5-grouped", "Tab")),
      "tab onto tab made a new group of both")
check(not any(t["id"] == d["id"] for t in cards("stage5-closed")), "middle-click closed a tab")
```

(`run` reads `probe.SEED` when called, so setting it switches this script to the four-tab seed.)

Run: `PYTHONPATH=target/cardframe python target/cardframe/stage5.py`
Expected: four `PASS` lines. Look at `stage5-rest.png`: tab cards look as before (thumbnail or spinner, title strip, engine dot, favicon, pin at the bottom-left, swatch at the bottom-right, glow), and `tab-1` has the focused ACCENT border. If the layout check fails, the force layout moved the seeded tabs between runs: aim the gestures from `stage5-pre` in a third run instead.

Two more gestures, extending the same script's `lines` (and re-running it) once the four checks pass:
- **Reparent inside a group:** after the tab-onto-tab drop, `DOUBLECLICK` the new group's card (from `stage5-grouped`), export, then drag the first member onto the second, export, and check the first's `parent` is the second's id.
- **Selection into a group:** in a fresh run with `NEWGROUP` added to `SETUP` (re-export the layout, since the group is a new card), `MOUSEDOWN` on empty canvas above-left of tabs `a` and `c` (outside every card's `rect`), `MOUSEMOVE` in steps to below-right of both without reaching `b`, `d` or the group, then `MOUSEUP`. Drag `a` onto the group card, export, and check both `a` and `c` have `group` set to the group's id.

If a gesture fails, screenshot it before changing code.

- [ ] **Step 11: Docs**

In `README.md` (CRLF), replace survey item 7's text (line starting `7. **Tab and Group cards onto the shared frame.**`) with:

```
7. **Done: Tab and Group cards onto the shared frame.** All seven kinds now go through `draw_graph_card`: one loop over `GRAPH_DRAW_ORDER`, per-kind differences in `match` tables (`card_style`, `card_border`, `card_controls`, `drop_action`, `paint_card_body`). Every kind eases its hover with glow and fill brightening, shows the swatch (always) and the eject button (hover, in a group), and closes on middle-click; every node menu has the Colors row. One drop table covers every kind, so tabs can join a nested group's card and any card can leave through the back button. See `docs/superpowers/specs/2026-09-24-tab-group-card-frame-design.md`.
```

In `docs/superpowers/specs/2026-09-23-graph-card-frame-design.md`, change the Non-goals bullet `- The Tab and Group card loops (drop-onto-tab reparenting, drill-in, glow, thumbnails). They only pick up `node_card_size`.` to:

```
- The Tab and Group card loops (drop-onto-tab reparenting, drill-in,
  glow, thumbnails). They only pick up `node_card_size`. Done since, in
  `2026-09-24-tab-group-card-frame-design.md`.
```

Check the README's Graph-view feature text for anything the change made stale (`grep -n "swatch\|middle-click\|Middle-click\|eject" README.md`): e.g. a line saying only tabs close on middle-click, or that the group swatch is bottom-left. Update each to the new behavior.

- [ ] **Step 12: Commit**

```bash
git add crates/manifold/src/main.rs README.md docs/superpowers/specs/2026-09-23-graph-card-frame-design.md
git commit -m "Graph cards: Tab onto the shared frame; one card loop

The tab loop goes: a Tab body arm, pin and audio controls, the focused
border, the selection wash and multi-select drag. Every card is now drawn
by one loop over GRAPH_DRAW_ORDER through draw_graph_card. Survey item 7
is done.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
