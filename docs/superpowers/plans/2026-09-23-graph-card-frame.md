---
status: done
---

# Shared Graph Card Frame Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `build_graph_ui`'s five hand-copied Download/Note/Image/Pdf/Json card loops with one card frame (`App::draw_graph_card`) plus one per-kind body painter, and replace the four separate card-size computations with one `node_card_size`.

**Architecture:** Pure helpers first (`node_card_size`, `node_card_rect`, `card_filter_text`), wired into `bounds_for_level`, the LOD count and the right-click hit test. Then the frame: `draw_graph_card` does cull → `paint_card_body` (a `match` over the five kinds' bodies, moved from their loops) → interacts registered after the body → border → hover ✕ → `finish_draggable_node` → filter dim. One call-site loop walks the kinds in today's draw order and applies each kind's click action. Download loses its hand-rolled ✕/drag/hover and gains group drop and "Remove from Group".

**Tech Stack:** Rust, egui 0.36.2. Nearly everything is in `crates/manifold/src/main.rs` (~14.4k lines). Unit tests live in that file's `#[cfg(test)] mod tests` at the bottom.

**Spec:** `docs/superpowers/specs/2026-09-23-graph-card-frame-design.md`

## Global Constraints

- Keep today's kind layering: Download under Note under Image under Pdf under Json. The frame is called kind by kind in that order, not in one pass over `node_order`. No visible z-order change.
- Download gets full parity: no hand-rolled close button or drag/hover code; it gains drop-onto-group and sticky hover, loses its hover fill-brightening, and its menu offers "Remove from Group" when grouped.
- Right-click picks the topmost card: the hit test checks kinds in reverse draw order: Group, Json, Pdf, Image, Note, Download, Tab.
- One frame method plus a `match` for the body. No trait per kind, no `card_begin`/`card_end` pair.
- Non-goals: the Tab and Group card loops (they only pick up `node_card_size`), sticky hover itself, one-pass creation-order drawing, any visual change beyond Download's dropped hover brightening.
- Line numbers below are from commit `c75c91f` and drift as earlier steps land. Find code by the quoted text, not by line number.
- `cargo test` does not rebuild `target/debug/manifold.exe`. Run `cargo build -p manifold` before any live headless run.
- The codebase is not rustfmt-clean. Don't run `cargo fmt`; match the surrounding style by hand.
- Comment density: this file has long explanatory comments. When code moves, move its comments with it. When a comment describes something this plan changes, rewrite it to be true.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never touch the real `%APPDATA%\manifold`. Every headless run sets an isolated, quoted `APPDATA` (see Task 1 Step 1).

---

### Task 1: Baselines, `node_card_size`, and its three non-loop users

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - new free functions next to `pdf_card_size` (~line 651);
  - `count_visible_nodes` (~664);
  - `fit_all_nodes_scene_rect` and `bounds_for_level` (~5081-5162);
  - the LOD count in `build_graph_ui` (~9507);
  - the right-click hit test (~10008-10107);
  - the minimap's `bounds_for_level` call (~10206);
  - tests module.
- Create (scratch, git-ignored): `target/card-frame-shots/drive.sh`, `target/card-frame-shots/before/*.png`.

**Interfaces:**
- Produces:
  - `fn node_card_size(payload: &NodePayload) -> egui::Vec2`
  - `fn node_card_rect(node: &GraphNode) -> egui::Rect`
  - `fn card_filter_text(payload: &NodePayload) -> &str`
  - `fn count_visible_nodes(nodes: &NodeMap, ids: &[String], viewport: egui::Rect) -> usize` (the `size` parameter is dropped)
  - `fn bounds_for_level<'a>(&self, ids: impl IntoIterator<Item = &'a String>) -> egui::Rect` (replaces the seven slice parameters)
  - Baseline screenshots in `target/card-frame-shots/before/`, used by Task 3.

- [ ] **Step 1: Capture live baselines before any code change**

Nothing in `main.rs` may change before this step is done. Build the current code:

```bash
cargo build -p manifold
```

Create `target/card-frame-shots/drive.sh`. It runs one headless session in a fresh, isolated APPDATA and pipes the commands in `$2` to it:

```bash
#!/usr/bin/env bash
# Usage: drive.sh <out-dir> <commands-file>
# Fresh isolated APPDATA each run, so the first-launch demo scene appears
# and the real %APPDATA%\manifold is never touched.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
OUT="$1"; CMDS="$2"
D="target/card-frame-shots/appdata-$$"
rm -rf "$D"; mkdir -p "$D/appdata" "$OUT"
W=$(cygpath -w "$PWD/$D")
export APPDATA="$W\\appdata"
# One command per line; "sleep N" lines pause the pipe instead of being sent.
while IFS= read -r line; do
  case "$line" in
    sleep*) eval "$line" ;;
    "") ;;
    *) echo "$line" ;;
  esac
done < "$CMDS" | target/debug/manifold.exe --headless
```

Create sample inputs:

```bash
mkdir -p target/card-frame-shots/assets
cp crates/manifold/assets/icon/icon-64.png target/card-frame-shots/assets/small.png
printf '{"name":"demo","items":[1,2,3],"nested":{"a":true}}' > target/card-frame-shots/assets/sample.json
```

For a PDF, print the demo tab with `PRINTTOPDF` in a first run (below). If that doesn't produce a file, use any small PDF already on the machine and write down its path.

Write `target/card-frame-shots/scene.txt`. Paths passed to `ADDIMAGE`/`ADDPDF`/`ADDJSON` must be absolute Windows paths; use `cygpath -w "$PWD/target/card-frame-shots/assets/small.png"` to get them and paste them in:

```
RESIZE 1300 800
sleep 4
PRINTTOPDF <abs-win-path>\target\card-frame-shots\assets\sample.pdf
sleep 4
ADDIMAGE <abs-win-path>\target\card-frame-shots\assets\small.png
ADDPDF <abs-win-path>\target\card-frame-shots\assets\sample.pdf
ADDJSON <abs-win-path>\target\card-frame-shots\assets\sample.json
sleep 5
MOUSEMOVE 5 790
MOUSEMOVE 5 790
SCREENSHOT <abs-win-path>\target\card-frame-shots\before\scene.png
QUIT
```

Run `bash target/card-frame-shots/drive.sh target/card-frame-shots/before target/card-frame-shots/scene.txt`, then open `before/scene.png` with the Read tool. Write each card's center (Note, Image, Pdf, Json, the "Try grouping" group, the Welcome tab) into `target/card-frame-shots/coords.txt`. At 1300x800 the demo Note spans about x167-389, y329-502, with its ✕ near (387, 332).

With those coordinates, write one command file per check. Each file repeats `scene.txt`'s setup lines, then does its action, then one `MOUSEMOVE` to that spot, `SCREENSHOT`, and `QUIT` (one move is enough since pointer commands get a frame before them; see `debug_control.rs`). Save the output as `before/<name>.png`:

- `hover-<kind>.png` for Note, Image, Pdf and Json: move onto the card's center, then screenshot. The hover border and ✕ should show.
- `filter.png`: `GRAPHFILTERTEXT zzz`, then screenshot. Every card should be dimmed.
- `drag-json.png`: `MOUSEDOWN` on the Json card, 8 `MOUSEMOVE`s stepping 20px right, `MOUSEUP`, then screenshot.
- `rclick-json.png`: `CLICK <json center> right`, then screenshot. The menu should show.
- `close-note.png`: hover the Note, `CLICK` its ✕, `CLICK 551 17` (Recently Closed), then screenshot. Its first entry should read the note.

Use `CLICK x y [right]`, never a `MOUSEDOWN`/`MOUSEUP` pair spaced by `sleep` (that's a long press). A drag needs several `MOUSEMOVE`s between down and up.

Download cards can't be spawned headlessly: they only come from an engine's download event. Try once: `ADDRESSBAR file:///<abs path>/target/card-frame-shots/assets/sample.json.zip`, then `ENTER`, then `sleep 8`, then a screenshot. Create that zip first with `cd target/card-frame-shots/assets && python -m zipfile -c sample.json.zip sample.json`. If no download card appears, write "Download: no headless path; verified by unit tests + code review only" into `coords.txt`. Don't spend more on it.

Look at every PNG. Each must show what its name says. Redo any that caught a menu mid-fade or missed a hover.

- [ ] **Step 2: Write the failing tests**

Add to the tests module at the bottom of `main.rs`, next to the existing `node_menu_config_matches_each_kinds_menu`:

```rust
    #[test]
    fn node_card_size_matches_each_kinds_card() {
        let download = NodePayload::Download(DownloadPayload { path: "f.bin".into(), bytes_received: 0, total_bytes: None, completed: false });
        assert_eq!(node_card_size(&download), DOWNLOAD_NODE_SIZE);
        assert_eq!(node_card_size(&NodePayload::Note(NotePayload { text: String::new(), editing: false })), NOTE_NODE_SIZE);
        assert_eq!(node_card_size(&NodePayload::Group(GroupPayload { name: "g".into() })), GROUP_NODE_SIZE);
        let json = NodePayload::Json(JsonPayload { path: "a.json".into(), value: serde_json::json!({}), key: None });
        assert_eq!(node_card_size(&json), JSON_NODE_SIZE);

        // Image: a small picture keeps its own size; an oversized one is
        // scaled down to fit `IMAGE_NODE_MAX_SIZE`, aspect ratio kept.
        let image = |width, height| NodePayload::Image(ImagePayload { path: "p.png".into(), width, height });
        assert_eq!(node_card_size(&image(40, 20)), egui::vec2(40.0, 20.0));
        let big = node_card_size(&image(100_000, 50_000));
        assert!(big.x <= IMAGE_NODE_MAX_SIZE.x && big.y <= IMAGE_NODE_MAX_SIZE.y);
        assert!((big.x / big.y - 2.0).abs() < 1e-3);

        // Pdf: pending and failed use the fixed pending box; rendered uses
        // the page's own size through the same image fitting.
        let pdf = |outcome| NodePayload::Pdf(PdfPayload { path: "d.pdf".into(), outcome });
        assert_eq!(node_card_size(&pdf(None)), PDF_PENDING_NODE_SIZE);
        assert_eq!(node_card_size(&pdf(Some(Err("bad".into())))), PDF_PENDING_NODE_SIZE);
        assert_eq!(node_card_size(&pdf(Some(Ok(PdfOutcome { width: 40, height: 20, page_count: 3 })))), egui::vec2(40.0, 20.0));
        assert_eq!(node_card_size(&pdf(Some(Ok(PdfOutcome { width: 100_000, height: 50_000, page_count: 1 })))), big);
    }

    #[test]
    fn card_filter_text_matches_each_kinds_filter_input() {
        let download = NodePayload::Download(DownloadPayload { path: "C:/dl/f.bin".into(), bytes_received: 0, total_bytes: None, completed: true });
        // Download filters on its full path; the file kinds on the filename only.
        assert_eq!(card_filter_text(&download), "C:/dl/f.bin");
        assert_eq!(card_filter_text(&NodePayload::Note(NotePayload { text: "buy milk".into(), editing: false })), "buy milk");
        assert_eq!(card_filter_text(&NodePayload::Image(ImagePayload { path: "C:/pics/cat.png".into(), width: 1, height: 1 })), "cat.png");
        assert_eq!(card_filter_text(&NodePayload::Pdf(PdfPayload { path: "C:/docs/paper.pdf".into(), outcome: None })), "paper.pdf");
        assert_eq!(card_filter_text(&NodePayload::Json(JsonPayload { path: "C:/data/x.json".into(), value: serde_json::json!(1), key: None })), "x.json");
    }
```

(`download_filename` is `Path::file_name`, which splits on both `/` and `\` on Windows, so these forward-slash paths are fine.)

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p manifold node_card_size_matches card_filter_text_matches`

Expected: compile error, "cannot find function `node_card_size`" / "`card_filter_text`".

- [ ] **Step 4: Add the helpers**

Insert directly after `pdf_card_size`:

```rust
/// Any node's own card size, every kind -- the one source the card
/// frame (`App::draw_graph_card`), `bounds_for_level`, the LOD visible
/// count and the right-click hit test all read, so none of them can
/// disagree about how big a given card is (they used to each carry
/// their own per-kind copy, and the LOD count approximated Image/Pdf).
fn node_card_size(payload: &NodePayload) -> egui::Vec2 {
    match payload {
        NodePayload::Tab(_) => GRAPH_NODE_SIZE,
        NodePayload::Download(_) => DOWNLOAD_NODE_SIZE,
        NodePayload::Note(_) => NOTE_NODE_SIZE,
        NodePayload::Image(image) => image_node_card_size(image.width, image.height),
        NodePayload::Pdf(pdf) => pdf_card_size(pdf),
        NodePayload::Json(_) => JSON_NODE_SIZE,
        NodePayload::Group(_) => GROUP_NODE_SIZE,
    }
}

/// `node`'s card in world space: `node_card_size` centered on its position.
fn node_card_rect(node: &GraphNode) -> egui::Rect {
    egui::Rect::from_center_size(egui::Pos2::new(node.pos.x, node.pos.y), node_card_size(&node.payload))
}

/// What the Graph filter box matches a Download/Note/Image/Pdf/Json card
/// against: a download's full path, a note's text, a file node's
/// filename. Tab and Group cards match their own fields (title + URL,
/// name) in their own loops and never call this; they get `""`.
fn card_filter_text(payload: &NodePayload) -> &str {
    match payload {
        NodePayload::Download(download) => &download.path,
        NodePayload::Note(note) => &note.text,
        NodePayload::Image(image) => download_filename(&image.path),
        NodePayload::Pdf(pdf) => download_filename(&pdf.path),
        NodePayload::Json(json) => download_filename(&json.path),
        NodePayload::Tab(_) | NodePayload::Group(_) => "",
    }
}
```

Run: `cargo test -p manifold node_card_size_matches card_filter_text_matches`

Expected: both PASS. (`card_filter_text` is unused outside tests until Task 2, so a dead-code warning is fine here.)

- [ ] **Step 5: `count_visible_nodes` uses exact sizes**

Replace the function and its doc comment:

```rust
/// How many of `ids` (one kind's own node-id list at the current level)
/// have a card (`node_card_rect`) intersecting `viewport` -- the exact
/// same per-node rect the render loops build for their own cull check.
/// Shared by `build_graph_ui`'s LOD pre-pass (see `LOD_SIMPLIFY_COUNT`)
/// so it counts "visible" precisely the way culling already does, rather
/// than a second, slightly different notion of visibility.
fn count_visible_nodes(nodes: &NodeMap, ids: &[String], viewport: egui::Rect) -> usize {
    ids.iter()
        .filter(|id| nodes.get(id.as_str()).is_some_and(|n| node_card_rect(n).intersects(viewport)))
        .count()
}
```

In `build_graph_ui`, replace the seven-term `let lod_visible_count = count_visible_nodes(...) + ...;` with:

```rust
        let lod_visible_count: usize = [
            &tabs_at_level,
            &downloads_at_level,
            &notes_at_level,
            &images_at_level,
            &pdfs_at_level,
            &jsons_at_level,
            &groups_at_level,
        ]
        .into_iter()
        .map(|ids| count_visible_nodes(&self.nodes, ids, self.graph_scene_rect))
        .sum();
```

The comment above it opens "Level-of-detail: count how many nodes at this level actually intersect the viewport (approximating a variable-size Image/Pdf card's footprint with its own typical/max size -- this only feeds a threshold decision, not real culling, so that's close enough)". Drop the parenthetical approximation clause, since sizes are now exact. Keep the rest.

- [ ] **Step 6: `bounds_for_level` takes one id iterator**

Replace `bounds_for_level`, including its `#[allow(clippy::too_many_arguments)]` and its seven `for` loops:

```rust
    /// Same computation `fit_all_nodes_scene_rect` needs, but taking
    /// already-known ids for a level instead of recomputing them -- lets
    /// the minimap (drawn every frame, well after `build_graph_ui`'s own
    /// per-frame `*_at_level` locals are already in hand) reuse this
    /// without redoing the level scans a second time per frame.
    fn bounds_for_level<'a>(&self, ids: impl IntoIterator<Item = &'a String>) -> egui::Rect {
        const MARGIN: f32 = 60.0;
        let bounds = ids
            .into_iter()
            .filter_map(|id| self.nodes.get(id))
            .fold(egui::Rect::NOTHING, |bounds, node| bounds.union(node_card_rect(node)));
        if !bounds.is_finite() {
            return default_graph_scene_rect();
        }
        bounds.expand(MARGIN)
    }
```

Keep the existing doc comment's first lines if they differ from the above, and update only the parts about seven lists.

`fit_all_nodes_scene_rect` becomes the following (every node at this level is exactly the union of the seven `nodes_at_current_level(kind)` lists):

```rust
    fn fit_all_nodes_scene_rect(&self) -> egui::Rect {
        self.bounds_for_level(
            self.node_order
                .iter()
                .filter(|id| self.nodes.get(*id).is_some_and(|n| n.group == self.current_group)),
        )
    }
```

The minimap call becomes:

```rust
        let world_bounds = self.bounds_for_level(
            [
                &tabs_at_level,
                &downloads_at_level,
                &notes_at_level,
                &images_at_level,
                &pdfs_at_level,
                &jsons_at_level,
                &groups_at_level,
            ]
            .into_iter()
            .flatten(),
        );
```

- [ ] **Step 7: Right-click hit test, topmost first**

Replace everything from `let hit_group = self.nodes_at_current_level(ItemNodeKind::Group)` through the closing `.flatten();` of `hit_tab`, plus the `if let Some(id) = hit_group.or(...)` condition, with:

```rust
                // Reverse draw order, so the card on top takes the
                // right-click, the same card a left-click there would
                // reach: Group is drawn last, then Json, Pdf, Image, Note,
                // Download, and Tab first. Within one kind, later in
                // `node_order` draws later, hence `.rev()`.
                let hit = [
                    ItemNodeKind::Group,
                    ItemNodeKind::Json,
                    ItemNodeKind::Pdf,
                    ItemNodeKind::Image,
                    ItemNodeKind::Note,
                    ItemNodeKind::Download,
                    ItemNodeKind::Tab,
                ]
                .into_iter()
                .find_map(|kind| {
                    self.nodes_at_current_level(kind)
                        .into_iter()
                        .rev()
                        .find(|id| self.nodes.get(id).is_some_and(|n| node_card_rect(n).contains(world_pos)))
                });
                if let Some(id) = hit {
```

The `then`/`else` bodies (`self.node_menu = Some((id, pos)); …` / `self.node_menu = None; …`) stay as they are. The comment above `let world_pos` talks only about a press landing on a group's card. Rewrite it: a right-press landing on any card opens that card's menu instead of starting a marquee. Keep its point about groups not being marquee-selectable.

Before relying on "within one kind, later draws later", check it in the Tab loop (search `for id in tabs_at_level`). If the tab loop reorders (for example, draws the focused tab last), mention that in the comment instead of claiming plain `node_order`.

- [ ] **Step 8: Build, test, clippy**

```bash
cargo test -p manifold
cargo clippy -p manifold 2>&1 | grep -E "^(warning|error)" | sort | uniq -c
```

Expected: every test passes. Clippy reports no new warning in the touched code (compare against `git stash; cargo clippy …; git stash pop` if unsure). The only allowed new warning is dead code for `card_filter_text`.

- [ ] **Step 9: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "One node_card_size for bounds, LOD count and right-click hit test

Right-click now picks the topmost card (reverse draw order), matching
left-click; before, overlapping cards opened the menu of the one under.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The card frame replaces the five loops (with Download parity)

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - new `CardStyle`, `card_style`, `CardBody`, `CardOutput` near `draw_node_close_button` (~line 513);
  - new `App::draw_graph_card` and `App::paint_card_body` right after `finish_draggable_node` (~4806);
  - `finish_draggable_node`'s doc comment;
  - `build_graph_ui`'s five card loops (from the comment "// Download nodes: no thumbnail/title-strip/audio-dot" to just before "// Group nodes: rendered at whichever level").

**Interfaces:**
- Consumes (Task 1): `node_card_rect(&GraphNode) -> egui::Rect`, `card_filter_text(&NodePayload) -> &str`.
- Consumes (existing):
  - `draw_node_close_button(ui: &egui::Ui, deferred_overlays: &mut DeferredOverlays, card_rect: egui::Rect, close_id_salt: (&str, &str), inv_zoom: f32, base_fill: egui::Color32, border_color: egui::Color32, icon_color: egui::Color32) -> bool`
  - `dim_if_filtered_out(deferred_overlays: &mut DeferredOverlays, filter: &str, haystacks: &[&str], rect: egui::Rect, rounding: f32)`
  - `App::finish_draggable_node(&mut self, id: &str, response: &egui::Response, is_hovered: bool)`
  - `truncate_to_width(cache, ctx, text: &str, font_id, max_width) -> String`
  - `GpuState::image_id(&self, key: &str) -> Option<egui::TextureId>`
- Produces:
  - `struct CardOutput { response: egui::Response, close_clicked: bool }`
  - `fn draw_graph_card(&mut self, ui: &mut egui::Ui, deferred_overlays: &mut DeferredOverlays, id: &str, kind: ItemNodeKind, inv_zoom: f32, zoom_scale: f32) -> Option<CardOutput>`

This is a move-and-merge refactor of UI code with no unit-testable seam beyond Task 1's helpers. Its test cycle is `cargo test` + `cargo clippy` + a live smoke check (Step 6). Task 3 does the full before/after comparison.

- [ ] **Step 1: Style and output types**

Insert just above `fn draw_node_close_button`:

```rust
/// The per-kind colors and filter-dim rounding a graph card's shared
/// frame (`App::draw_graph_card`) paints with. Note is the one kind on a
/// light (yellow) card, so its border and ✕ are dark instead of the
/// theme's text colors.
struct CardStyle {
    border_hover: egui::Color32,
    border_idle: egui::Color32,
    close_fill: egui::Color32,
    close_border: egui::Color32,
    close_icon: egui::Color32,
    dim_rounding: f32,
}

fn card_style(kind: ItemNodeKind, visuals: &egui::Visuals) -> CardStyle {
    let themed = CardStyle {
        border_hover: visuals.text_color(),
        border_idle: visuals.weak_text_color(),
        close_fill: visuals.extreme_bg_color,
        close_border: visuals.weak_text_color(),
        close_icon: visuals.text_color(),
        dim_rounding: 4.0,
    };
    match kind {
        ItemNodeKind::Note => CardStyle {
            border_hover: egui::Color32::from_black_alpha(200),
            border_idle: egui::Color32::from_black_alpha(120),
            close_fill: NOTE_CARD_COLOR,
            close_border: egui::Color32::from_black_alpha(160),
            close_icon: egui::Color32::from_black_alpha(200),
            dim_rounding: 6.0,
        },
        ItemNodeKind::Download => CardStyle { dim_rounding: 6.0, ..themed },
        _ => themed,
    }
}

/// What a card's body (`App::paint_card_body`) tells the shared frame:
/// which part of the card is the click/drag handle (the whole card,
/// except a Note being edited, where only its header is, so clicks in
/// the text go to the `TextEdit`), and an optional hover tooltip (a
/// failed PDF render's error).
struct CardBody {
    interact_rect: egui::Rect,
    tooltip: Option<String>,
}

/// What `App::draw_graph_card` hands back to `build_graph_ui`, which
/// applies each kind's own click action.
struct CardOutput {
    response: egui::Response,
    close_clicked: bool,
}
```

- [ ] **Step 2: The frame**

Insert right after `finish_draggable_node`:

```rust
    /// One Download/Note/Image/Pdf/Json card on the graph: everything the
    /// five kinds share (cull, click/drag and hover interacts, border,
    /// hover ✕, drag and drop-onto-group, filter dim), with the per-kind
    /// content painted by `paint_card_body`. `None` when the node is gone
    /// or culled. The caller applies the kind's own click action from the
    /// returned response. Tab and Group cards keep their own loops.
    #[allow(clippy::too_many_arguments)]
    fn draw_graph_card(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        kind: ItemNodeKind,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> Option<CardOutput> {
        let rect = node_card_rect(self.nodes.get(id)?);
        // Viewport culling -- see the tab loop's own comment. Never culls
        // the node being dragged, so a drag off-screen keeps tracking.
        if self.dragging.as_deref() != Some(id) && !rect.intersects(self.graph_scene_rect) {
            return None;
        }
        let body = self.paint_card_body(ui, deferred_overlays, id, kind, rect, inv_zoom, zoom_scale);

        // Both interacts are registered *after* the body, never before:
        // egui (0.36, `interaction.rs`) gives the pointer to the
        // last-registered interactive widget under it, and counts a
        // hover-only widget as hovered only when it sits on top of that
        // widget. Registered first, a body widget (a Note's markdown
        // scope or `TextEdit`) outranked them: a Note card was never
        // hovered at all, and a click on rendered markdown never reached
        // the card. The hover-only one covers the whole card, so hovering
        // a Note's text while editing (where `response` is only the
        // header) still counts; being hover-only it takes no clicks.
        let response = ui.interact(body.interact_rect, egui::Id::new(("graph_card", id)), egui::Sense::click_and_drag());
        let response = match body.tooltip {
            Some(tooltip) => response.on_hover_text(tooltip),
            None => response,
        };
        let hover = ui.interact(rect, egui::Id::new(("graph_card_hover", id)), egui::Sense::hover());
        let is_hovered = self.hovered.as_deref() == Some(id) || hover.hovered() || response.hovered();

        // An inside stroke painted over the body; every body keeps its
        // content inset from the edge.
        let style = card_style(kind, ui.visuals());
        let (border_width, border_color) =
            if is_hovered { (1.5, style.border_hover) } else { (1.0, style.border_idle) };
        ui.painter().rect_stroke(
            rect,
            2.0 * inv_zoom,
            egui::Stroke::new(border_width * inv_zoom, border_color),
            egui::StrokeKind::Inside,
        );

        let close_clicked = is_hovered
            && draw_node_close_button(
                ui,
                deferred_overlays,
                rect,
                ("graph_card_close", id),
                inv_zoom,
                style.close_fill,
                style.close_border,
                style.close_icon,
            );

        self.finish_draggable_node(id, &response, is_hovered);

        if let Some(node) = self.nodes.get(id) {
            dim_if_filtered_out(deferred_overlays, &self.graph_filter, &[card_filter_text(&node.payload)], rect, style.dim_rounding);
        }
        Some(CardOutput { response, close_clicked })
    }
```

- [ ] **Step 3: The bodies**

Insert right after `draw_graph_card`. Each arm is its kind's old loop body minus everything the frame now does: the rect/cull, the `ui.interact`, `is_hovered`, the border, the ✕, drag/hover bookkeeping, click actions, and the dim. Move each arm's explanatory comments along with its code, from the loop named in the arm's comment. Drop comments about the removed parts. Reword "see the Image loop above" to "see the Image arm".

```rust
    /// A graph card's own content, per kind -- everything under the
    /// shared frame's border (see `draw_graph_card`). Returns which part
    /// of the card is the click/drag handle and an optional tooltip.
    #[allow(clippy::too_many_arguments)]
    fn paint_card_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        kind: ItemNodeKind,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let mut body = CardBody { interact_rect: rect, tooltip: None };
        match kind {
            // Download: no thumbnail/title-strip/audio-dot machinery (no
            // live content behind it), just the filename, an icon, and a
            // live progress bar while it's still downloading.
            ItemNodeKind::Download => {
                ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                let Some((download_path, completed, total_bytes, bytes_received)) = self
                    .nodes
                    .get(id)
                    .and_then(|n| n.as_download())
                    .map(|d| (d.path.clone(), d.completed, d.total_bytes, d.bytes_received))
                else {
                    return body;
                };
                let filename = download_filename(&download_path);
                // (move the `egui_phosphor::DOWNLOAD_SIMPLE` / LOD comment here)
                if !self.graph_lod_simplified {
                    let download_icon_pos = rect.center_top();
                    let download_icon_color = ui.visuals().text_color();
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        overlay_painter.text(
                            project(download_icon_pos) + egui::vec2(0.0, 10.0),
                            egui::Align2::CENTER_TOP,
                            egui_phosphor::light::DOWNLOAD_SIMPLE,
                            egui::FontId::proportional(18.0),
                            download_icon_color,
                        );
                    }));
                    // (move the `MIN_LABEL_CARD_SCREEN_WIDTH` comment here)
                    if rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                        let filename_font = egui::FontId::proportional(14.0);
                        let filename_max_width = (rect.width() * zoom_scale - 12.0).max(0.0);
                        let filename =
                            truncate_to_width(&mut self.truncate_cache, ui.ctx(), filename, &filename_font, filename_max_width);
                        let filename_pos = rect.center_bottom();
                        let filename_color = ui.visuals().text_color();
                        deferred_overlays.push(Box::new(move |overlay_painter, project| {
                            overlay_painter.text(
                                project(filename_pos) + egui::vec2(0.0, -8.0),
                                egui::Align2::CENTER_BOTTOM,
                                filename,
                                filename_font,
                                filename_color,
                            );
                        }));
                    }
                }
                // (move the "Live progress bar, only while not yet complete" comment here)
                if !completed {
                    if let Some(total) = total_bytes.filter(|t| *t > 0) {
                        let fraction = (bytes_received as f32 / total as f32).clamp(0.0, 1.0);
                        let track_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 8.0, rect.bottom() - 26.0),
                            egui::pos2(rect.right() - 8.0, rect.bottom() - 22.0),
                        );
                        ui.painter().rect_filled(track_rect, 2.0, ui.visuals().weak_text_color().gamma_multiply(0.4));
                        let fill_rect = egui::Rect::from_min_max(
                            track_rect.min,
                            egui::pos2(track_rect.min.x + track_rect.width() * fraction, track_rect.max.y),
                        );
                        ui.painter().rect_filled(fill_rect, 2.0, ACCENT);
                    }
                }
            }
            // Note: (move the "Note nodes: a plain user-authored text card"
            // comment here, rewritten: the header strip is the drag handle
            // only while editing, see `CardBody`; the frame's hover-only
            // interact covers the whole card)
            ItemNodeKind::Note => {
                let header_h = 20.0 * inv_zoom;
                let header_rect =
                    egui::Rect::from_min_max(rect.min, egui::Pos2::new(rect.max.x, rect.min.y + header_h));
                let text_rect = egui::Rect::from_min_max(egui::Pos2::new(rect.min.x, rect.min.y + header_h), rect.max)
                    .shrink(6.0 * inv_zoom);
                let is_editing = self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);

                ui.painter().rect_filled(rect, 2.0 * inv_zoom, NOTE_CARD_COLOR);
                if !self.graph_lod_simplified {
                    let note_icon_pos = header_rect.left_center();
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        overlay_painter.text(
                            project(note_icon_pos) + egui::vec2(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            egui_phosphor::light::NOTE_PENCIL,
                            egui::FontId::proportional(12.0),
                            egui::Color32::from_black_alpha(160),
                        );
                    }));
                }

                if is_editing {
                    body.interact_rect = header_rect;
                    let mut lost_focus = false;
                    if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                        lost_focus = ui
                            .put(
                                text_rect,
                                egui::TextEdit::multiline(&mut note.text)
                                    .frame(egui::Frame::NONE)
                                    .font(egui::FontId::proportional(13.0))
                                    .text_color(egui::Color32::from_black_alpha(220))
                                    .hint_text("Type a note..."),
                            )
                            .lost_focus();
                    }
                    // (move the "Committed on blur, not on every keystroke" comment here)
                    if lost_focus {
                        if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                            note.editing = false;
                        }
                        self.save_session();
                    }
                } else if !self.graph_lod_simplified {
                    // (move the "`text` is cloned out before the closure" and
                    // "LOD: skip the markdown render" comments here)
                    let text = self.nodes.get(id).and_then(|n| n.as_note()).map(|n| n.text.clone()).unwrap_or_default();
                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::LEFT)),
                        |ui| {
                            ui.style_mut().visuals.override_text_color = Some(egui::Color32::from_black_alpha(220));
                            egui_commonmark::CommonMarkViewer::new().show(ui, &mut self.commonmark_cache, &text);
                        },
                    );
                }
            }
            // Image: (move the "Image nodes: a locally-added picture" comment here)
            ItemNodeKind::Image => {
                let tex_id = self.gpu.as_ref().and_then(|g| g.image_id(id));
                // (move the "LOD: the real texture blit" comment here)
                if !self.graph_lod_simplified {
                    match tex_id {
                        Some(tex_id) => {
                            ui.painter().image(
                                tex_id,
                                rect,
                                egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                        // (move the "Only reachable if the node somehow outlived its own texture" comment here)
                        None => {
                            ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                            let broken_icon_pos = rect.center();
                            let broken_icon_color = ui.visuals().weak_text_color();
                            deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                overlay_painter.text(
                                    project(broken_icon_pos),
                                    egui::Align2::CENTER_CENTER,
                                    egui_phosphor::light::IMAGE_BROKEN,
                                    egui::FontId::proportional(24.0),
                                    broken_icon_color,
                                );
                            }));
                        }
                    }
                } else {
                    ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                }
            }
            // Pdf: (move the "PDF nodes: same card shape as an image node" comment here)
            ItemNodeKind::Pdf => {
                let Some(state) = self.nodes.get(id).and_then(|n| n.as_pdf()).map(|pdf| match &pdf.outcome {
                    None => None,
                    Some(Err(err)) => Some(Err(err.clone())),
                    Some(Ok(outcome)) => {
                        Some(Ok((self.gpu.as_ref().and_then(|g| g.image_id(id)), outcome.page_count)))
                    }
                }) else {
                    return body;
                };
                // (move the "A failed render's error message shown as a hover tooltip" comment here)
                if let Some(Err(err)) = &state {
                    body.tooltip = Some(err.clone());
                }
                // (move the "LOD: same plain solid box" comment here, saying "the Image arm")
                if !self.graph_lod_simplified {
                    match state {
                        Some(Ok((Some(tex_id), page_count))) => {
                            ui.painter().image(
                                tex_id,
                                rect,
                                egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                            // (move the "Shown even for a one-page PDF" comment here)
                            if page_count > 0 && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                                let page_count_pos = rect.right_bottom();
                                let page_count_text =
                                    format!("{page_count} page{}", if page_count == 1 { "" } else { "s" });
                                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                    overlay_painter.text(
                                        project(page_count_pos) + egui::vec2(-4.0, -4.0),
                                        egui::Align2::RIGHT_BOTTOM,
                                        page_count_text,
                                        egui::FontId::proportional(11.0),
                                        egui::Color32::WHITE,
                                    );
                                }));
                            }
                        }
                        // (move the "Reachable the same rare way" comment here)
                        Some(Ok((None, _))) => {
                            ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                            let broken_icon_pos = rect.center();
                            let broken_icon_color = ui.visuals().weak_text_color();
                            deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                overlay_painter.text(
                                    project(broken_icon_pos),
                                    egui::Align2::CENTER_CENTER,
                                    egui_phosphor::light::IMAGE_BROKEN,
                                    egui::FontId::proportional(24.0),
                                    broken_icon_color,
                                );
                            }));
                        }
                        Some(Err(_)) => {
                            ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                            let error_icon_pos = rect.center();
                            deferred_overlays.push(Box::new(move |overlay_painter, project| {
                                overlay_painter.text(
                                    project(error_icon_pos),
                                    egui::Align2::CENTER_CENTER,
                                    egui_phosphor::light::FILE_X,
                                    egui::FontId::proportional(24.0),
                                    DANGER_COLOR,
                                );
                            }));
                        }
                        None => {
                            ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                            ui.put(rect, egui::Spinner::new().size(24.0 * inv_zoom));
                        }
                    }
                } else {
                    ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                }
            }
            // Json: (move the "JSON nodes: always the same fixed `JSON_NODE_SIZE` box" comment here)
            ItemNodeKind::Json => {
                let Some(summary) = self.nodes.get(id).and_then(|n| n.as_json()).map(|json| match &json.key {
                    Some(key) => {
                        let key_display = if key.chars().count() > 16 {
                            format!("{}…", key.chars().take(15).collect::<String>())
                        } else {
                            key.clone()
                        };
                        format!("{key_display}: {}", json_summary(&json.value))
                    }
                    None => json_summary(&json.value),
                }) else {
                    return body;
                };
                ui.painter().rect_filled(rect, 2.0 * inv_zoom, ui.visuals().extreme_bg_color);
                if !self.graph_lod_simplified && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH {
                    let json_center = rect.center();
                    let summary_color = ui.visuals().text_color();
                    deferred_overlays.push(Box::new(move |overlay_painter, project| {
                        overlay_painter.text(
                            project(json_center) + egui::vec2(0.0, -12.0),
                            egui::Align2::CENTER_CENTER,
                            egui_phosphor::light::BRACKETS_CURLY,
                            egui::FontId::proportional(22.0),
                            ACCENT,
                        );
                        overlay_painter.text(
                            project(json_center) + egui::vec2(0.0, 14.0),
                            egui::Align2::CENTER_CENTER,
                            summary,
                            egui::FontId::proportional(12.0),
                            summary_color,
                        );
                    }));
                }
            }
            ItemNodeKind::Tab | ItemNodeKind::Group => {
                unreachable!("Tab and Group cards are drawn by their own loops in build_graph_ui")
            }
        }
        body
    }
```

Every `(move the … comment here)` line is an instruction, not code. Replace it with the named comment block from the old loop (find it by the quoted opening words) and delete the instruction line. None may remain when this step is done.

Watch these differences from the old code:
- **Download fill:** now always `extreme_bg_color`. The hover brightening (`gamma_multiply(1.35)`) is removed on purpose.
- **Image:** `image_id(id)`, not `image_id(&id)`, because `id` is already `&str`.
- **Note:** `is_editing` is read once at the top, before `lost_focus` can clear it.

- [ ] **Step 4: One call-site loop replaces the five loops**

In `build_graph_ui`, delete everything from the comment line `// Download nodes: no thumbnail/title-strip/audio-dot` down to (not including) `// Group nodes: rendered at whichever level`. First confirm that every comment in that range was moved into Step 3's arms. Put this in its place:

```rust
                // Download, Note, Image, Pdf and Json cards: one shared
                // frame each (`draw_graph_card`), kind by kind in this
                // order, which is also their stacking order: a later kind
                // draws, and takes the pointer, on top of an earlier one.
                // Each kind's own click action is applied here.
                for (kind, ids) in [
                    (ItemNodeKind::Download, &downloads_at_level),
                    (ItemNodeKind::Note, &notes_at_level),
                    (ItemNodeKind::Image, &images_at_level),
                    (ItemNodeKind::Pdf, &pdfs_at_level),
                    (ItemNodeKind::Json, &jsons_at_level),
                ] {
                    for id in ids.iter() {
                        // Read before the frame: a Note's `TextEdit` can lose
                        // focus to a click on its own header this very frame,
                        // clearing `editing`, and that click must not
                        // immediately re-enter editing.
                        let note_was_editing = kind == ItemNodeKind::Note
                            && self.nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing);
                        let Some(card) =
                            self.draw_graph_card(ui, &mut deferred_overlays, id, kind, inv_zoom, zoom_scale)
                        else {
                            continue;
                        };
                        if card.close_clicked {
                            node_close_requested = Some(id.clone());
                            continue;
                        }
                        match kind {
                            // Click (not drag -- egui doesn't report
                            // `clicked()` for a drag) reveals the file.
                            ItemNodeKind::Download if card.response.clicked() => download_clicked = Some(id.clone()),
                            // Click on the rendered note enters editing in place.
                            ItemNodeKind::Note if card.response.clicked() && !note_was_editing => {
                                if let Some(note) = self.nodes.get_mut(id).and_then(|n| n.as_note_mut()) {
                                    note.editing = true;
                                }
                            }
                            // Maximizes the image into Item view, the same
                            // gesture a Tab card uses.
                            ItemNodeKind::Image if card.response.clicked() => image_clicked = Some(id.clone()),
                            // Opens the full tree in `build_json_viewer`.
                            ItemNodeKind::Json if card.response.double_clicked() => {
                                self.json_viewer_node = Some(id.clone())
                            }
                            _ => {}
                        }
                    }
                }
```

If the borrow checker rejects `ids.iter()` (the locals are captured by the Scene closure), use `ids.clone()` and `for id in &ids_clone`, the same pattern the old loops used with `.clone()`.

- [ ] **Step 5: `finish_draggable_node`'s doc comment**

Its doc comment says Download stays separate and that "a download's own group-drop was never implemented at all -- see `node_menu_config`'s own doc comment". Rewrite that part: this now covers the five card kinds (Download, Note, Image, Pdf, Json, all through `draw_graph_card`), and only Tab (drop onto a sibling tab) and Group (merge) differ. Also update the inner comment "Note/Image/Pdf/Json have no competing …" to "a card kind has no competing …".

- [ ] **Step 6: Build, test, clippy, live smoke check**

```bash
cargo test -p manifold
cargo clippy -p manifold 2>&1 | grep -E "^(warning|error)" | sort | uniq -c
cargo build -p manifold
```

Expected:
- All tests pass.
- No new clippy warnings. The `card_filter_text` dead-code warning from Task 1 is gone now.
- `grep -n "graph_download_node\|graph_note_node\|graph_image_node\|graph_pdf_node\|graph_json_node" crates/manifold/src/main.rs` prints nothing.
- `grep -n "(move the" crates/manifold/src/main.rs` prints nothing.

Smoke check: run `target/card-frame-shots/scene.txt` into `target/card-frame-shots/after/`. Run the `hover-note` command file (from Task 1 Step 1) the same way. Look at both PNGs: every card renders, and the Note shows its hover border and ✕.

- [ ] **Step 7: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "One graph card frame for Download/Note/Image/Pdf/Json

draw_graph_card owns cull, interacts (registered after the body, so no
body widget can outrank them), border, hover close, drag/drop and filter
dim; paint_card_body holds each kind's content. Download drops its
hand-rolled close and drag code, gaining drop-onto-group and sticky hover,
and loses its hover fill brightening.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Download "Remove from Group", docs, and the before/after comparison

**Files:**
- Modify: `crates/manifold/src/main.rs`: `node_menu_config` and its doc comment (~line 307); test `node_menu_config_matches_each_kinds_menu` (~14344).
- Modify: `docs/superpowers/specs/2026-09-23-node-menu-and-close-design.md` (Follow-ups).
- Modify: `README.md`: the "One close path and one node menu" entry area; add a new entry after it.

**Interfaces:**
- Consumes: Task 1's baselines in `target/card-frame-shots/before/` and `coords.txt`; Task 2's frame.
- Produces: none (last task).

- [ ] **Step 1: Update the test first**

In `node_menu_config_matches_each_kinds_menu`, replace:

```rust
            // Download never offers "Remove from Group", grouped or not.
            assert_eq!(node_menu_config(ItemNodeKind::Download, has_group), simple("download_context_menu", false, true, false, false, "Remove from List", 170.0));
```

with:

```rust
            // A download can join a group like any card (drag onto it), so
            // its menu offers "Remove from Group" when grouped.
            assert_eq!(node_menu_config(ItemNodeKind::Download, has_group), simple("download_context_menu", false, true, false, has_group, "Remove from List", 170.0));
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p manifold node_menu_config_matches_each_kinds_menu`

Expected: FAIL on the `has_group = true` iteration (`has_group: false` vs `true`).

- [ ] **Step 3: Pass the real `has_group`**

In `node_menu_config`, change the Download arm to:

```rust
        ItemNodeKind::Download => Some(NodeMenuConfig {
            delete_label: "Remove from List",
            ..file("download_context_menu", false)
        }),
```

In its doc comment, delete the sentences from "Download hardcodes `has_group: false`" through "(pre-existing, kept as-is)."

Run: `cargo test -p manifold`. Expected: all pass.

- [ ] **Step 4: Before/after comparison, live**

```bash
cargo build -p manifold
```

Re-run every command file from Task 1 Step 1 into `target/card-frame-shots/after/`, using the same coordinates. Look at each `before`/`after` pair with the Read tool. Expected:
- Every pair looks the same, except where a Download card is visible: its fill no longer brightens on hover.
- `close-note`: Recently Closed lists the note, as before.
- `drag-json`: the card moved the same distance.
- `rclick-json`: the same menu.
- `filter`: every card dimmed.

Then these checks, which have no baseline:
- **Image click:** `CLICK` the image card's center, then screenshot. Item view should show the maximized image.
- **Json double-click:** two `CLICK`s at the json center with no sleep between, then screenshot. The JSON viewer popup should be open.
- **Note click-to-edit:** `CLICK` the note body, then screenshot. The `TextEdit` should show (a caret, or the hint "Type a note..." if the note is empty). Then `CLICK` the note's header strip (y ≈ top + 8). The note must stay rendered and must not flip back into editing.
- **Right-click topmost:** drag the Json card (8-step drag) until it overlaps the Note, `CLICK <overlap point> right`, then screenshot. The menu must be the Json menu ("Expand as Graph" present), not the Note menu ("Copy Text").
- **Download** (only if Task 1 found a way to spawn one): drag it onto "Try grouping". It should vanish from the top level. Double-click into the group, right-click the download, and check the menu shows "Remove from Group". If no headless path was found, say so in the report. Don't claim it's verified.

Write down any difference found and fix it before continuing, using the systematic-debugging skill if the cause isn't obvious.

- [ ] **Step 5: Docs**

In `docs/superpowers/specs/2026-09-23-node-menu-and-close-design.md` → Follow-ups:
- Change the bullet "A Download's menu never offers \"Remove from Group\" (pre-existing)." to "**Done** (graph card frame, `docs/superpowers/specs/2026-09-23-graph-card-frame-design.md`): a download joins a group by dragging onto it like any card, and its menu offers \"Remove from Group\" when grouped."
- Change the bullet "The graph card rendering loops are still per kind (the next generalization of this pattern)." to "**Done:** one card frame for Download/Note/Image/Pdf/Json (same spec)."

In `README.md`, directly after the "One close path and one node menu" entry, add one paragraph in the same style as its neighbors: bold lead-in, then what changed and why, then what was verified live. Cover:
- `draw_graph_card` + `paint_card_body` replace five per-kind loops;
- `node_card_size` is now the one size source for 4 sites (the LOD count got exact Image/Pdf sizes);
- interacts are always registered after the body, the Note-hover root cause, now fixed for every kind by construction;
- right-click picks the topmost card;
- Download parity (group drop, Remove from Group, sticky hover, no hover brightening);
- a closed group now also removes its grouped downloads from the Downloads list, since `close_node`'s Download arm runs for group members;
- what Step 4 actually verified live, and anything it couldn't (Download, if applicable).

Before writing that downloads-list claim, check it: find `close_group` and confirm it calls `close_node` on every member, including downloads. If it doesn't, describe what actually happens.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/main.rs README.md docs/superpowers/specs/2026-09-23-node-menu-and-close-design.md
git commit -m "Download menu offers Remove from Group; record the card frame

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review notes

- **Spec coverage:**
  - §1 `node_card_size` at 4 sites → Task 1 Steps 4-7, plus the render loops in Task 2 via `node_card_rect`.
  - §1 `card_filter_text` → Task 1 Step 4.
  - §2 frame steps 1-8 → Task 2 Steps 1-3.
  - §3 call site → Task 2 Step 4.
  - §4 Download parity: hand-rolled code deleted (Task 2 Step 4), group drop (the frame's `finish_draggable_node`), menu `has_group` + test + spec follow-up (Task 3), brightening removed (Task 2 Step 3).
  - §5 hit test → Task 1 Step 7.
  - Testing: unit tables (Task 1 Step 2, Task 3 Step 1); live baselines before any change (Task 1 Step 1) and the comparison (Task 3 Step 4).
- **Beyond the spec, deliberate:**
  - `node_card_rect`: the 4 sites all wanted a rect, not a size.
  - `bounds_for_level` takes an id iterator instead of seven slices, which removes its `too_many_arguments` allow.
  - Within-kind `.rev()` in the hit test: the same topmost-card rule applied inside a kind.
- **Known gap:** Download cards have no headless spawn path. Their live checks may be impossible, and Task 1 Step 1 / Task 3 Step 4 say to report that rather than claim it.
