---
status: done
---

# The trail popup as a miniature graph view Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (the tasks depend on each other in order, so one context runs them). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Draw the Back/Forward branch popup with the Graph view's own card drawing, camera and `Scene`, so it is a pannable, zoomable miniature of the strip.

**Architecture:** Four small camera/drawing pieces become shared (`GraphCamera::{transform, begin_frame, pan_to}`, `show_scene`, a viewport on `CardPass`, a free `paint_edges`); `TrailSource` splits into an ungated `trail_cards`/`trail_edges` generator and its gating shell; the popup owns a `GraphCamera` (recentred on every open) and draws the strip's cards through `virtual_card_click` inside a `Scene`.

**Tech Stack:** Rust, egui 0.36 (`Scene`, `Area`), the manifold crate (`crates/manifold`).

**Spec:** `docs/superpowers/specs/2026-10-16-trail-popup-canvas-design.md`

## Global Constraints

- Work on `main`; run `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold --all-targets -- -D warnings` must pass; never `--no-verify`. Lints are fixed or `#[allow(..., reason = "...")]`-ed, never silenced bare.
- Tests never touch the real profile; use `App::default()` and `store::testutil::temp_dir` for files.
- Python, if any script is unavoidable, runs as `python -P f.py < /dev/null`. Change files with the Edit tool.
- Commit messages end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- No compatibility shims: when a mechanism is replaced, delete the old one and fix its callers in the same task.
- Test command for the crate: `cargo test -j 8 -p manifold` (add a name filter for a single test). Build for the gate: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`.
- Text rules from the spec: the popup viewport is the cards' bounding box plus 8 per side, clamped to 160..=480 wide and 60..=240 high; page sizes and gaps are the strip's (`PAGE`, `MIN_PAGE_WIDTH`, `COLUMN_GAP`, `ROW_GAP`).

## Review Focus

Failure modes the tasks' tests must pin (each has a test in the task named):
1. A one-page trail (`tree().len() == 1`) still opens a small, valid popup (Task 3).
2. A very long trail clamps the viewport and recentres on the cursor page, with the first page off-screen (Tasks 3 and 5).
3. The trail changes under an open popup (an eviction renumbers entries and retires ids): the popup keeps drawing the new cards and does not panic (Task 3).
4. A right-click menu in Tab view survives until it is used, and a click on it does not close the popup (Task 4).
5. A click that closes the menu from outside does not also close the popup in the same frame (Task 4).

---

### Task 1: Shared camera pieces (pure refactor)

**Files:**
- Modify: `crates/manifold/src/graph_ui.rs` (camera methods, `show_scene`, `paint_edges`, `card_tab_step`, call sites)
- Modify: `crates/manifold/src/card.rs` (`CardPass` fields, cull, focus pan)
- Modify: `crates/manifold/src/link_gestures.rs`, `crates/manifold/src/debug_control.rs`, `crates/manifold/src/link_gestures/tests.rs`, `crates/manifold/src/card/tests.rs`

**Interfaces:**
- Produces (all `pub(crate)`):
  - `GraphCamera::transform(&self, outer: egui::Rect) -> (f32, egui::Vec2)` (`screen = translation + scale * world`)
  - `GraphCamera::begin_frame(&mut self, ctx: &egui::Context, outer: egui::Rect) -> (f32, egui::Vec2)`
  - `GraphCamera::pan_to(&mut self, card: egui::Rect)`
  - `graph_ui::show_scene<R>(ui: &mut egui::Ui, rect: &mut egui::Rect, drag_pan: bool, body: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R>`
  - `graph_ui::paint_edges(ui: &egui::Ui, viewport: egui::Rect, edges: &[LevelEdge], inv_zoom: f32)`
  - `graph_ui::card_tab_step(ui: &egui::Ui, order: &[String], rect_of: impl Fn(&str) -> Option<egui::Rect>) -> Option<egui::Rect>` (Tab/Shift+Tab: moves focus, returns the rect to pan to)
  - `CardPass` gains `viewport: egui::Rect` (the world rect the cull tests) and `pan_target: Option<egui::Rect>` (set when a card gains focus; the caller pans).
- Deleted: `App::graph_world_to_screen_transform`, `App::pan_to_card`, `App::draw_graph_dot_grid`, `App::draw_graph_edges`, `App::handle_card_tab`.

- [ ] **Step 1: Write the failing tests** — in the `tests` module of `graph_ui.rs`, next to `tab_steps_through_cards_in_creation_order`:

```rust
    #[test]
    fn the_camera_transform_maps_its_rect_onto_the_outer_rect() {
        let mut camera = GraphCamera::new();
        camera.rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 200.0));
        let outer = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(800.0, 400.0));
        let (scale, translation) = camera.transform(outer);
        assert_eq!(scale, 2.0);
        let screen = translation + scale * camera.rect.center().to_vec2();
        assert_eq!(screen, outer.center().to_vec2());
        camera.rect = egui::Rect::from_center_size(egui::Pos2::ZERO, egui::vec2(8.0, 4.0));
        assert_eq!(camera.transform(outer).0, GRAPH_ZOOM_MAX, "the zoom range clamps");
    }

    #[test]
    fn begin_frame_fills_the_rect_to_the_outer_shape_and_sets_the_readout() {
        let ctx = egui::Context::default();
        let mut camera = GraphCamera::new();
        camera.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        let outer = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
        let (scale, _) = camera.begin_frame(&ctx, outer);
        assert_eq!(scale, 2.0);
        assert_eq!(camera.rect.size(), egui::vec2(200.0, 100.0));
        assert_eq!(camera.zoom_pct, 200.0);
    }

    #[test]
    fn pan_to_eases_only_when_the_card_is_not_fully_in_view() {
        let mut camera = GraphCamera::new();
        camera.rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        camera.pan_to(egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 20.0)));
        assert!(camera.tween.is_none(), "already visible");
        let far = egui::Rect::from_min_size(egui::pos2(500.0, 0.0), egui::vec2(20.0, 20.0));
        camera.pan_to(far);
        let tween = camera.tween.expect("off screen: eases");
        assert_eq!(tween.to, egui::Rect::from_center_size(far.center(), camera.rect.size()));
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 8 -p manifold the_camera_transform begin_frame_fills pan_to_eases`
Expected: compile errors — no method `transform`, `begin_frame`, `pan_to` on `GraphCamera`.

- [ ] **Step 3: Add the camera methods.** In `graph_ui.rs`, inside `impl GraphCamera` after `fill`:

```rust
    /// `(scale, translation)` such that `screen = translation + scale * world` for `rect` fitted into `outer`: the same
    /// "fit the visible rect into the area, keep the aspect, clamp to the zoom range" that `egui::Scene` applies
    /// (private to egui), needed outside its closure for animation targets, hit tests and drop points.
    pub(crate) fn transform(&self, outer: egui::Rect) -> (f32, egui::Vec2) {
        let scale = (outer.size() / self.rect.size()).min_elem().clamp(GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        (scale, outer.center().to_vec2() - scale * self.rect.center().to_vec2())
    }

    /// A frame's camera work before its `Scene`: advance any tween or glide, keep repainting while one runs, fill
    /// `rect` to `outer`'s shape (`fill`) and set the zoom readout. Returns `transform(outer)`.
    pub(crate) fn begin_frame(&mut self, ctx: &egui::Context, outer: egui::Rect) -> (f32, egui::Vec2) {
        self.advance();
        if self.tween.is_some() || self.momentum.is_some() {
            ctx.request_repaint();
        }
        self.fill(outer);
        let transform = self.transform(outer);
        self.zoom_pct = transform.0 * 100.0;
        transform
    }

    /// Eases the camera so `card` is in view, centred on it when it isn't already fully visible.
    pub(crate) fn pan_to(&mut self, card: egui::Rect) {
        if !self.rect.contains_rect(card) {
            self.ease_to(egui::Rect::from_center_size(card.center(), self.rect.size()));
        }
    }
```

Then delete `App::graph_world_to_screen_transform` and its doc comment (`graph_ui.rs`, the first item in the `impl App` block after `GraphCamera`'s). Replace every call: `self.graph_world_to_screen_transform(X)` and `app.graph_world_to_screen_transform(X)` become `self.camera.transform(X)` / `app.camera.transform(X)`, at `graph_ui.rs` (in `graph_world_rect_to_screen`, `graph_screen_point_to_world`, `build_graph_ui` twice), `link_gestures.rs`, `debug_control.rs`, and `link_gestures/tests.rs` (four sites). Doc comments that name the old function (`graph_ui.rs` `zoom_pct`, `zoom_chip.rs`, `link_gestures.rs`, `apply_zoom_requests`) now name `GraphCamera::transform`.

- [ ] **Step 4: Use `begin_frame` in `build_graph_ui`.** Replace

```rust
        self.camera.advance();
        if self.camera.tween.is_some() || self.camera.momentum.is_some() {
            ui.ctx().request_repaint();
        }
        self.camera.fill(scene_outer_rect);
        let (zoom_scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
        self.camera.zoom_pct = zoom_scale * 100.0;
        let inv_zoom = 1.0 / zoom_scale;
```

(with its call already renamed by Step 3) by

```rust
        let (zoom_scale, translation) = self.camera.begin_frame(ui.ctx(), scene_outer_rect);
        let inv_zoom = 1.0 / zoom_scale;
```

- [ ] **Step 5: `show_scene`.** Add after `impl GraphCamera`:

```rust
/// The graph's `Scene`: its zoom range, and the primary button pans unless `drag_pan` is off (while a link retargets).
pub(crate) fn show_scene<R>(
    ui: &mut egui::Ui,
    rect: &mut egui::Rect,
    drag_pan: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Scene::new()
        .zoom_range(GRAPH_ZOOM_MIN..=GRAPH_ZOOM_MAX)
        .drag_pan_buttons(if drag_pan { egui::DragPanButtons::PRIMARY } else { egui::DragPanButtons::empty() })
        .show(ui, rect, body)
}
```

In `build_graph_ui` replace the `egui::Scene::new() … .show(ui, &mut scene_rect, |ui| {` chain (and its explanatory comment, kept above the call) with `show_scene(ui, &mut scene_rect, self.link.retarget.is_none(), |ui| {`.

- [ ] **Step 6: Viewport, edges, grid, pan target.**
  - `card.rs` `CardPass`: add after `members`:

```rust
    /// The world rect a card must meet to be drawn (the camera's, in Graph view).
    pub(crate) viewport: egui::Rect,
    /// Set to the rect of a card that gained keyboard focus this frame; the caller pans its camera to it.
    pub(crate) pan_target: Option<egui::Rect>,
```

  - `draw_graph_card` (`card.rs`): the cull `!rect.intersects(self.camera.rect)` becomes `!rect.intersects(pass.viewport)`; `if response.gained_focus() { self.pan_to_card(rect); }` becomes `if response.gained_focus() { pass.pan_target = Some(rect); }`.
  - `graph_ui.rs` `draw_graph_cards`: the `CardPass { … }` literal gains `viewport: self.camera.rect, pan_target: None`; replace `self.handle_card_tab(ui, &tab_order);` by

```rust
        let tab_pan = card_tab_step(ui, &tab_order, |id| self.level_card_rect(id));
        if let Some(rect) = tab_pan {
            self.camera.pan_to(rect);
        }
```

  and right after the `for entry in self.level_items_from(at_level) { … }` loop, before `self.card_nav_active = pass.any_focused;` add `if let Some(rect) = pass.pan_target { self.camera.pan_to(rect); }`.
  - Turn `handle_card_tab` into the free function `card_tab_step` (same body, no `&mut self`): its tail `if let (true, Some(target)) = (tab, target) { request_focus; if let Some(rect) = self.level_card_rect(..) { self.pan_to_card(rect) } }` becomes `if let (true, Some(target)) = (tab, target) { ui.memory_mut(|m| m.request_focus(card_id(&order[target]))); return rect_of(&order[target]); } None`, with the signature from Interfaces.
  - `handle_card_digits`: `self.pan_to_card(rect)` becomes `self.camera.pan_to(rect)`. Delete `App::pan_to_card`.
  - `draw_graph_edges` becomes the free `paint_edges(ui, viewport, edges, inv_zoom)` (body unchanged, `self.camera.rect` → `viewport`); its call becomes `paint_edges(ui, self.camera.rect, &edges, inv_zoom);`. Delete `draw_graph_dot_grid`; its call becomes `paint_dot_grid(ui.painter(), self.camera.rect, zoom_scale, inv_zoom, ui.visuals().weak_text_color());`.
  - Test updates: `card/tests.rs` `CardPass` literal gains `viewport: egui::Rect::EVERYTHING, pan_target: None`; `graph_ui.rs` test `tab_steps_through_cards_in_creation_order` calls `card_tab_step(ui, &order, |_| None)` instead of `app.handle_card_tab(ui, &order)`.

- [ ] **Step 7: Run the three new tests and the whole crate**

Run: `cargo test -j 8 -p manifold the_camera_transform begin_frame_fills pan_to_eases` — Expected: 3 passed.
Run: `cargo test -j 8 -p manifold` — Expected: all pass (the suite was 1344 before the three new ones).

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold --all-targets -- -D warnings
git add -A crates/manifold/src
git commit -m "Share the graph camera pieces: transform, begin_frame, pan_to, show_scene, a card viewport

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `trail_cards` and `trail_edges`, ungated

**Files:**
- Modify: `crates/manifold/src/trail_strip.rs`

**Interfaces:**
- Consumes: `NavHistory::{tree, epoch}`, `page_sizes`, `page_rects`, `TrailSource::page_id`.
- Produces: `trail_strip::trail_cards(app: &App, tab: &str, origin: egui::Pos2) -> Vec<CardEntry>` (the tab's page cards with their bounding box's top-left at `origin`; empty if the tab has no trail) and `trail_strip::trail_edges(app: &App, tab: &str, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge>`.

- [ ] **Step 1: Write the failing test** — in `trail_strip.rs` `mod tests` (the `forked()` fixture there has the same tree):

```rust
    #[test]
    fn trail_cards_lay_out_any_tabs_trail_from_the_origin_in_any_view() {
        let mut rig = Rig::new();
        let tab = rig.tab();
        rig.app.nav_history.insert(
            tab.clone(),
            NavHistory::with_tree(
                &[("/a", None, Some(3)), ("/b", Some(0), Some(2)), ("/c", Some(1), None), ("/d", Some(0), None)],
                3,
            ),
        );
        rig.app.view_mode = crate::ViewMode::Item;
        rig.app.focused = None;
        rig.frame(vec![], egui::Modifiers::NONE);
        assert!(rig.app.level_items().iter().all(|e| !e.id.starts_with("virtual:trail:")), "no strip here");
        let origin = egui::pos2(5.0, 7.0);
        let cards = trail_cards(&rig.app, &tab, origin);
        assert_eq!(cards.len(), 4);
        assert_eq!(cards.iter().fold(egui::Rect::NOTHING, |b, e| b.union(e.rect)).min, origin);
        let rect_of = |id: &str| cards.iter().find(|e| e.id == id).map(|e| e.rect);
        assert_eq!(trail_edges(&rig.app, &tab, &rect_of).len(), 3);
        assert!(trail_cards(&rig.app, "no-such-tab", origin).is_empty());
    }
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -j 8 -p manifold trail_cards_lay_out`
Expected: compile error — `trail_cards` and `trail_edges` not found.

- [ ] **Step 3: Implement.** In `trail_strip.rs`, add before `impl VirtualSource for TrailSource`:

```rust
/// Tab `tab`'s page cards, their bounding box's top-left at `origin`: the strip hangs it under the tab's card, the Back
/// and Forward popups use their own origin. Empty when the tab has no trail.
pub(crate) fn trail_cards(app: &App, tab: &str, origin: egui::Pos2) -> Vec<CardEntry> {
    let Some(history) = app.nav_history.get(tab) else { return Vec::new() };
    let (pages, epoch) = (history.tree(), history.epoch());
    let kind = CardKind { ty: TYPE_TRAIL_PAGE, tags: crate::caps::TagSet::EMPTY };
    pages
        .iter()
        .zip(page_rects(&pages, &page_sizes(&app.egui_ctx, &pages, PAGE), COLUMN_GAP, ROW_GAP))
        .map(|(page, rect)| CardEntry {
            id: TrailSource::page_id(tab, epoch, page.entry),
            kind,
            origin: Origin::Virtual("trail"),
            rect: rect.translate(origin.to_vec2()),
        })
        .collect()
}

/// The edges between `tab`'s pages (`rect_of` finds a card's rect by id); those on the path to the cursor are
/// highlighted.
pub(crate) fn trail_edges(app: &App, tab: &str, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
    let Some(history) = app.nav_history.get(tab) else { return Vec::new() };
    let (pages, epoch) = (history.tree(), history.epoch());
    pages
        .iter()
        .filter_map(|page| {
            let parent = &pages[page.parent?];
            let (from_id, to_id) =
                (TrailSource::page_id(tab, epoch, parent.entry), TrailSource::page_id(tab, epoch, page.entry));
            Some(LevelEdge {
                from: rect_of(&from_id)?.right_center(),
                to: rect_of(&to_id)?.left_center(),
                highlight: parent.on_path && page.on_path,
                from_id,
                to_id,
                curve: EdgeCurve::Horizontal,
                kind: EdgeKind::Opened,
                link: None,
            })
        })
        .collect()
}
```

and make the source's methods the gating shell:

```rust
    fn entries(&self, app: &App) -> Vec<CardEntry> {
        let Some(tab) = app.trail_strip_tab() else { return Vec::new() };
        let Some(card) = app.card_rect(&tab) else { return Vec::new() };
        trail_cards(app, &tab, card.left_bottom() + egui::vec2(0.0, STRIP_GAP))
    }

    fn edges(&self, app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
        app.trail_strip_tab().map_or_else(Vec::new, |tab| trail_edges(app, &tab, rect_of))
    }
```

(replacing the two bodies; `use crate::{App, CardKind, GraphAction}` already imports `CardKind`).

- [ ] **Step 4: Run the new test and the strip's existing ones**

Run: `cargo test -j 8 -p manifold trail_strip trail_cards_lay_out`
Expected: all pass (the strip's own tests are the regression for the gated path).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold --all-targets -- -D warnings
git add -A crates/manifold/src
git commit -m "Trail strip: trail_cards and trail_edges lay out any tab's trail, the source only gates

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The popup body is a Scene of strip cards

**Files:**
- Modify: `crates/manifold/src/popups.rs` (`MenuPopups.trail_canvas`, `build_trail_branches_popup`, `popup_centres_cursor` doc)
- Modify: `crates/manifold/src/card.rs` (`CardPass.carry`, `draw_graph_card` sense and carry)
- Modify: `crates/manifold/src/graph_ui.rs` (`virtual_card_click` returns the response; `CardPass` literal)
- Modify: `crates/manifold/src/trail_strip.rs` (`paint_trail_page` loses `card`; delete `POPUP_*`)
- Modify: `crates/manifold/src/card/tests.rs` (`CardPass` literal)
- Test: `crates/manifold/src/trail_nav.rs` (its popup tests move to the scene)

**Interfaces:**
- Consumes: Task 1 (`begin_frame`, `show_scene`, `paint_edges`, `CardPass.viewport/pan_target`), Task 2 (`trail_cards`, `trail_edges`).
- Produces:
  - `MenuPopups.trail_canvas: Option<(AnchoredPopup, GraphCamera)>`
  - `CardPass.carry: bool` (a virtual card is a drag source only when its caps say so and `carry` is true; the graph passes `true`)
  - `App::virtual_card_click(..) -> Option<egui::Response>`, now `pub(crate)` (the card's response, `None` when culled)
  - `popups::popup_start_rect(bounds: egui::Rect, cursor: Option<egui::Rect>, viewport: egui::Vec2) -> egui::Rect`
  - `paint_trail_page(painter, rect, page, hovered)` (the `card` parameter is gone; always the card path).

- [ ] **Step 1: Write the failing tests** in `trail_nav.rs` `mod tests`, replacing `the_branch_list_draws_a_foreground_popup` and `clicking_a_branch_row_moves_the_tab_and_closes_the_popup`, and adding the others. Shared helpers first:

```rust
    /// Every leaf shape in `output`, flattened.
    fn leaves(output: &egui::FullOutput) -> Vec<egui::Shape> {
        fn walk(s: &egui::Shape, out: &mut Vec<egui::Shape>) {
            match s {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                other => out.push(other.clone()),
            }
        }
        let mut out = Vec::new();
        output.shapes.iter().for_each(|s| walk(&s.shape, &mut out));
        out
    }

    const ANCHOR: egui::Rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 20.0));

    /// One frame of the Back popup over `tab`; returns what it drew.
    fn popup_run(app: &mut App, tab: &str, events: Vec<egui::Event>) -> egui::FullOutput {
        let ctx = app.egui_ctx.clone();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_trail_branches_popup(ui, Dir::Back, ANCHOR, tab, 1.0));
        output.textures_delta.clear();
        output
    }

    /// The popup's scene area in screen points: the area's rect inside the frame's 5-point margin.
    fn scene_min(app: &App) -> egui::Pos2 {
        let id = egui::Id::new(("trail_branches_popup", AnchoredPopup::BackBranches));
        app.egui_ctx.memory(|m| m.area_rect(id)).expect("the popup's area").min + egui::vec2(5.0, 5.0)
    }

    fn open_back_popup(name: &str) -> (App, String, Peer) {
        let (mut app, tab, peer) = tab_with_a_sibling(name);
        app.menus.open_popup = Some(AnchoredPopup::BackBranches);
        for _ in 0..3 {
            popup_run(&mut app, &tab, vec![]); // measure, draw, settle the recentred camera
        }
        (app, tab, peer)
    }
```

Tests:

```rust
    #[test]
    fn the_branch_popup_draws_the_strips_cards_edges_and_grid() {
        let (mut app, tab, _peer) = open_back_popup("trail-popup-draw");
        let output = popup_run(&mut app, &tab, vec![]);
        let shapes = leaves(&output);
        let accent = egui::epaint::ColorMode::Solid(crate::theme::ACCENT);
        let curves = shapes.iter().filter(|s| matches!(s, egui::Shape::CubicBezier(_))).count();
        let on_path = shapes
            .iter()
            .filter(|s| matches!(s, egui::Shape::CubicBezier(b) if b.stroke.color == accent))
            .count();
        let dots = shapes.iter().filter(|s| matches!(s, egui::Shape::Circle(_))).count();
        assert!(curves > 0, "the tree's lines are the level's bezier edges");
        assert!(on_path > 0, "the path to the cursor is drawn in the accent, as in the strip");
        assert!(dots > 0, "the graph's dot grid is behind the tree");
        let id = egui::Id::new(("graph_card", crate::trail_strip::trail_cards(&app, &tab, egui::Pos2::ZERO)[1].id));
        assert!(app.egui_ctx.read_response(id).is_some(), "a page is a real graph card");
        assert_eq!(app.menus.open_popup, Some(AnchoredPopup::BackBranches), "no click: it stays open");
    }

    #[test]
    fn clicking_a_page_moves_the_tab_and_closes_the_popup() {
        let (mut app, tab, mut peer) = open_back_popup("trail-popup-click");
        let cards = crate::trail_strip::trail_cards(&app, &tab, egui::Pos2::ZERO);
        let camera = app.menus.trail_canvas.as_ref().expect("the popup's camera").1.rect;
        // Scale 1 (the viewport is the camera's size): screen = scene origin + (world - camera origin).
        let at = scene_min(&app) + (cards[1].rect.center() - camera.min);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        popup_run(&mut app, &tab, vec![egui::Event::PointerMoved(at)]);
        popup_run(&mut app, &tab, vec![button(true), button(false)]);
        assert_eq!(app.menus.open_popup, None, "a pick closes the popup");
        assert!(peer.sent().iter().any(|m| matches!(m, HostToView::Navigate { url, .. } if url == "https://b/")));
    }

    #[test]
    fn dragging_empty_canvas_pans_the_popup_and_does_not_carry_a_page() {
        let (mut app, tab, _peer) = open_back_popup("trail-popup-pan");
        let before = app.menus.trail_canvas.as_ref().unwrap().1.rect.center();
        let start = scene_min(&app) + egui::vec2(3.0, 3.0); // inside the 8-point padding: empty canvas
        let press = |pressed, pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        popup_run(&mut app, &tab, vec![egui::Event::PointerMoved(start), press(true, start)]);
        for step in 1..=4 {
            let at = start + egui::vec2(8.0 * step as f32, 0.0);
            popup_run(&mut app, &tab, vec![egui::Event::PointerMoved(at)]);
        }
        popup_run(&mut app, &tab, vec![press(false, start + egui::vec2(32.0, 0.0))]);
        let after = app.menus.trail_canvas.as_ref().unwrap().1.rect.center();
        assert!(after.x < before.x - 10.0, "dragging right pans the view left: {before:?} -> {after:?}");
        assert!(app.interaction.dragging.is_none(), "nothing was carried");
    }

    #[test]
    fn the_popup_recentres_on_every_opening_and_not_in_between() {
        let (mut app, tab, _peer) = open_back_popup("trail-popup-recentre");
        let first = app.menus.trail_canvas.as_ref().unwrap().1.rect;
        app.menus.trail_canvas.as_mut().unwrap().1.rect = first.translate(egui::vec2(40.0, 0.0)); // the user panned
        popup_run(&mut app, &tab, vec![]);
        assert_ne!(app.menus.trail_canvas.as_ref().unwrap().1.rect.center(), first.center(), "later frames keep the pan");
        app.menus.open_popup = None;
        popup_run(&mut app, &tab, vec![]);
        app.menus.open_popup = Some(AnchoredPopup::BackBranches);
        popup_run(&mut app, &tab, vec![]);
        assert_eq!(app.menus.trail_canvas.as_ref().unwrap().1.rect, first, "reopened: back to the start");
    }

    #[test]
    fn a_one_page_trail_opens_a_small_valid_popup() {
        let mut app = app_on(&session("trail-popup-one"), 1);
        let (tab, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://a/");
        app.nav_history.insert(tab.clone(), NavHistory::visited(&["https://a/"]));
        app.menus.open_popup = Some(AnchoredPopup::BackBranches);
        for _ in 0..3 {
            popup_run(&mut app, &tab, vec![]);
        }
        let id = egui::Id::new(("trail_branches_popup", AnchoredPopup::BackBranches));
        let rect = app.egui_ctx.memory(|m| m.area_rect(id)).expect("the popup's area");
        assert!(rect.width() >= 160.0 && rect.height() >= 60.0, "{rect:?}");
    }

    #[test]
    fn a_long_trail_clamps_the_popup_and_starts_on_the_cursor_page() {
        let mut app = app_on(&session("trail-popup-long"), 1);
        let (tab, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://0/");
        let urls: Vec<String> = (0..12).map(|i| format!("https://example.test/page-{i}")).collect();
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        app.nav_history.insert(tab.clone(), NavHistory::visited(&refs));
        app.menus.open_popup = Some(AnchoredPopup::BackBranches);
        for _ in 0..3 {
            popup_run(&mut app, &tab, vec![]);
        }
        let id = egui::Id::new(("trail_branches_popup", AnchoredPopup::BackBranches));
        let rect = app.egui_ctx.memory(|m| m.area_rect(id)).expect("the popup's area");
        assert!(rect.width() <= 480.0 + 20.0 && rect.height() <= 240.0 + 20.0, "{rect:?}");
        let cards = crate::trail_strip::trail_cards(&app, &tab, egui::Pos2::ZERO);
        let camera = app.menus.trail_canvas.as_ref().unwrap().1.rect;
        assert!(camera.contains(cards.last().unwrap().rect.center()), "the cursor page (the last) is in view");
        assert!(!camera.contains(cards[0].rect.center()), "the first page is off screen");
    }

    #[test]
    fn the_trail_changing_under_an_open_popup_redraws_its_cards() {
        let (mut app, tab, _peer) = open_back_popup("trail-popup-changes");
        app.nav_history.insert(tab.clone(), NavHistory::visited(&["https://x/", "https://y/", "https://z/"]));
        let output = popup_run(&mut app, &tab, vec![]);
        assert!(!leaves(&output).is_empty());
        let cards = crate::trail_strip::trail_cards(&app, &tab, egui::Pos2::ZERO);
        assert_eq!(cards.len(), 3);
        popup_run(&mut app, &tab, vec![]);
        let id = egui::Id::new(("graph_card", cards[1].id.clone()));
        assert!(app.egui_ctx.read_response(id).is_some(), "the new trail's pages are registered");
    }

    #[test]
    fn the_popup_start_rect_centres_a_fitting_tree_and_the_cursor_of_a_long_one() {
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 80.0));
        let view = egui::vec2(200.0, 100.0);
        let cursor = egui::Rect::from_min_size(egui::pos2(900.0, 0.0), egui::vec2(10.0, 10.0));
        assert_eq!(popup_start_rect(bounds, Some(cursor), view).center(), bounds.center(), "fits: the whole tree");
        let long = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 80.0));
        assert_eq!(popup_start_rect(long, Some(cursor), view).center(), cursor.center(), "does not fit: the cursor");
        assert_eq!(popup_start_rect(long, None, view).center(), long.center(), "no cursor: the middle");
        assert_eq!(popup_start_rect(long, Some(cursor), view).size(), view);
    }
```

(`NavHistory`, `Peer`, `app_on`, `session`, `live_tab`, `tab_with_a_sibling`, `ViewKind` are already imported in this module; add `use crate::popups::popup_start_rect;` if the module's imports need it.) Remove the old `the_branch_list_draws_a_foreground_popup` and `clicking_a_branch_row_moves_the_tab_and_closes_the_popup`; keep `the_popup_centres_the_cursor_page_once_per_opening_not_every_frame` (the helper is unchanged).

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 8 -p manifold trail_nav`
Expected: compile errors — `popup_start_rect` and `menus.trail_canvas` missing.

- [ ] **Step 3: `carry` and the response.**
  - `card.rs` `CardPass`: add `pub(crate) carry: bool,` with the doc "Whether a virtual card that may be a drag source is carried by a press-drag on it (the graph); off in the popup, where a drag pans."
  - `draw_graph_card`: `let draggable = entry.caps().drag_source;` becomes `let draggable = entry.caps().drag_source && (stored || pass.carry);` and `let carried = !stored && entry.caps().drag_source;` becomes `let carried = !stored && entry.caps().drag_source && pass.carry;`.
  - `graph_ui.rs` `CardPass` literal gains `carry: true`; `card/tests.rs` literal gains `carry: true`.
  - `virtual_card_click` returns the card's response: signature `-> Option<egui::Response>`; body: `let card = self.draw_graph_card(ui, pass, entry)?;` then the existing click logic unchanged, ending with `Some(card.response)` (the early `return;`s become `return Some(card.response)`). Its call site in `draw_graph_cards` ignores the value (`let _ = …` is not needed: it is `Option`, not `#[must_use]`).

- [ ] **Step 4: `paint_trail_page` loses `card`.** In `trail_strip.rs`:

```rust
/// One page, inside the card loop's frame and border: a hover fills it, the cursor's ring is drawn outside the page
/// (clear of the card's own border), the label is clipped to the page and dimmed off the path.
pub(crate) fn paint_trail_page(painter: &egui::Painter, rect: egui::Rect, page: &TrailPage, hovered: bool) {
    if hovered {
        painter.rect_filled(rect, 6.0, SURFACE_HOVER);
    }
    if page.cursor {
        painter.rect_stroke(rect, 6.0, egui::Stroke::new(2.0, ACCENT), egui::StrokeKind::Outside);
    }
    … (the label part is unchanged)
```

Its one remaining caller (`TrailSource::paint_body`) drops the `true`; remove the `SURFACE_RAISED` import and the constants `POPUP_PAGE`, `POPUP_COLUMN_GAP`, `POPUP_ROW_GAP` with their comment. Fix any strip test that passed `card`.

- [ ] **Step 5: The popup.** In `popups.rs`: delete the `use crate::trail_strip::{POPUP_COLUMN_GAP, POPUP_PAGE, POPUP_ROW_GAP};` line; add to `MenuPopups` after `trail_menu`:

```rust
    /// The Back/Forward branch popup's camera and which popup it belongs to: recentred every time the popup opens
    /// (`popup_centres_cursor`), kept while it stays open.
    pub(crate) trail_canvas: Option<(AnchoredPopup, crate::graph_ui::GraphCamera)>,
```

Update `popup_centres_cursor`'s doc to "…should recentre its camera on the cursor page this frame: the first frame it is open…". Add the constants and helper above `build_trail_branches_popup`:

```rust
/// Canvas room around the trail's cards, and the popup viewport's least and most size.
const POPUP_PAD: f32 = 8.0;
const POPUP_MIN: egui::Vec2 = egui::vec2(160.0, 60.0);
const POPUP_MAX: egui::Vec2 = egui::vec2(480.0, 240.0);

/// The popup camera's rect on opening: `viewport` big, centred on the whole tree when it fits, else on the cursor page
/// (the tree's middle if there is none).
pub(crate) fn popup_start_rect(bounds: egui::Rect, cursor: Option<egui::Rect>, viewport: egui::Vec2) -> egui::Rect {
    let fits = bounds.width() <= viewport.x && bounds.height() <= viewport.y;
    let centre = if fits { bounds.center() } else { cursor.map_or(bounds.center(), |c| c.center()) };
    egui::Rect::from_center_size(centre, viewport)
}
```

and replace the whole body of `build_trail_branches_popup` (keep its signature) with:

```rust
        let which = match dir {
            Dir::Back => AnchoredPopup::BackBranches,
            Dir::Forward => AnchoredPopup::ForwardBranches,
        };
        let cards = crate::trail_strip::trail_cards(self, tab, egui::Pos2::ZERO);
        if cards.is_empty() {
            return;
        }
        let edges = crate::trail_strip::trail_edges(self, tab, &|id| cards.iter().find(|e| e.id == id).map(|e| e.rect));
        let bounds = cards.iter().fold(egui::Rect::NOTHING, |b, e| b.union(e.rect));
        let view = (bounds.size() + egui::Vec2::splat(2.0 * POPUP_PAD)).clamp(POPUP_MIN, POPUP_MAX);
        let cursor = self
            .nav_history
            .get(tab)
            .and_then(|h| h.tree().iter().position(|p| p.cursor))
            .and_then(|i| cards.get(i))
            .map(|e| e.rect);
        let mut camera = match self.menus.trail_canvas.take() {
            Some((w, camera)) if w == which => camera,
            _ => crate::graph_ui::GraphCamera::new(),
        };
        if popup_centres_cursor(ui.ctx(), which, self.menus.open_popup == Some(which)) {
            camera.jump_to(popup_start_rect(bounds, cursor, view));
        }
        let order: Vec<String> = cards.iter().map(|e| e.id.clone()).collect();
        let mut clicked = false;
        let area = egui::Area::new(egui::Id::new(("trail_branches_popup", which)))
            .fixed_pos(anchor_rect.left_bottom())
            .order(egui::Order::Foreground)
            .interactable(self.menus.open_popup == Some(which))
            .show(ui.ctx(), |ui| {
                ui.set_opacity(fade);
                popup_frame(ui).show(ui, |ui| {
                    let (outer, _) = ui.allocate_exact_size(view, egui::Sense::hover());
                    ui.painter().rect_filled(outer, 4.0, crate::theme::BG);
                    let (zoom_scale, _) = camera.begin_frame(ui.ctx(), outer);
                    let inv_zoom = 1.0 / zoom_scale;
                    let viewport = camera.rect;
                    let weak = ui.visuals().weak_text_color();
                    let mut rect = camera.rect;
                    let mut overlays: crate::graph_ui::DeferredOverlays = Vec::new();
                    let mut actions = Vec::new();
                    let members = std::collections::HashMap::new();
                    let mut pass = crate::card::CardPass {
                        deferred_overlays: &mut overlays,
                        actions: &mut actions,
                        inv_zoom,
                        zoom_scale,
                        drop_preview: &None,
                        pick_targets: &[],
                        connect_target: None,
                        connect_ring: None,
                        show_order: false,
                        order: &order,
                        any_focused: false,
                        members: &members,
                        viewport,
                        pan_target: None,
                        carry: false,
                    };
                    ui.scope_builder(egui::UiBuilder::new().max_rect(outer), |ui| {
                        crate::graph_ui::show_scene(ui, &mut rect, true, |ui| {
                            crate::graph_ui::paint_dot_grid(ui.painter(), viewport, zoom_scale, inv_zoom, weak);
                            crate::graph_ui::paint_edges(ui, viewport, &edges, inv_zoom);
                            for entry in &cards {
                                if let Some(response) = self.virtual_card_click(ui, &mut pass, entry, "trail", false) {
                                    clicked |= response.clicked();
                                }
                            }
                        });
                    });
                    camera.rect = rect;
                    if let Some(target) = pass.pan_target {
                        camera.pan_to(target);
                    }
                    debug_assert!(overlays.is_empty(), "a trail page queues no overlay text: the popup has no layer for it");
                    debug_assert!(actions.is_empty(), "a trail page's click returns no GraphAction");
                });
            });
        self.menus.trail_canvas = Some((which, camera));
        let clicked_outside = clicked_outside_popup(ui, area.response.rect, Some(anchor_rect));
        if clicked || clicked_outside {
            close_popup(&mut self.menus.open_popup, which);
        }
```


- [ ] **Step 6: Run the new tests and the whole crate**

Run: `cargo test -j 8 -p manifold trail_nav` — Expected: the seven new tests and the kept centring test pass. If a layout assumption fails (the 5-point frame margin in `scene_min`, `Scene` filling `max_rect`), the failure message names the rect: fix `scene_min`/the scope to match what egui does, then rerun; if the scene does not nest in the `Area` at all, stop and report (spec risks 1 and 2).
Run: `cargo test -j 8 -p manifold` — Expected: all pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold --all-targets -- -D warnings
git add -A crates/manifold/src
git commit -m "Trail popup: a pannable, zoomable Scene of the strip's cards, recentred on every open

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Right-click menu in the popup

**Files:**
- Modify: `crates/manifold/src/popups.rs` (`build_trail_branches_popup`)
- Modify: `crates/manifold/src/trail_strip.rs` (`build_trail_menu` condition)
- Test: `crates/manifold/src/trail_nav.rs`, `crates/manifold/src/trail_strip.rs` tests

**Interfaces:**
- Consumes: Task 3's popup, `TrailSource::on_menu` (sets `menus.trail_menu`), `App::build_trail_menu`.
- Produces: `build_trail_menu` keeps the menu while the strip shows or a branch popup is open.

- [ ] **Step 1: Write the failing tests** in `trail_nav.rs` `mod tests`:

```rust
    /// A secondary press and release at `at`.
    fn right_click(at: egui::Pos2) -> Vec<egui::Event> {
        let secondary = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(at), secondary(true), secondary(false)]
    }

    fn page_centre(app: &App, tab: &str, entry: usize) -> egui::Pos2 {
        let cards = crate::trail_strip::trail_cards(app, tab, egui::Pos2::ZERO);
        let camera = app.menus.trail_canvas.as_ref().expect("the popup's camera").1.rect;
        scene_min(app) + (cards[entry].rect.center() - camera.min)
    }

    #[test]
    fn a_right_click_on_a_page_opens_its_menu_in_tab_view_and_keeps_the_popup_open() {
        let (mut app, tab, _peer) = open_back_popup("trail-popup-menu");
        app.view_mode = crate::ViewMode::Item; // the popup lives in Tab view, where no strip hosts the menu
        let at = page_centre(&app, &tab, 1);
        popup_run(&mut app, &tab, vec![egui::Event::PointerMoved(at)]);
        popup_run(&mut app, &tab, right_click(at));
        let (id, _) = app.menus.trail_menu.clone().expect("the page's menu is open");
        assert!(id.starts_with("virtual:trail:"), "{id}");
        for _ in 0..3 {
            popup_run(&mut app, &tab, vec![]);
        }
        assert!(app.menus.trail_menu.is_some(), "the popup keeps the menu alive in Tab view");
        assert_eq!(app.menus.open_popup, Some(AnchoredPopup::BackBranches));
    }

    #[test]
    fn a_click_that_closes_the_menu_from_outside_does_not_also_close_the_popup() {
        let (mut app, tab, _peer) = open_back_popup("trail-popup-menu-outside");
        let at = page_centre(&app, &tab, 1);
        popup_run(&mut app, &tab, vec![egui::Event::PointerMoved(at)]);
        popup_run(&mut app, &tab, right_click(at));
        for _ in 0..2 {
            popup_run(&mut app, &tab, vec![]);
        }
        let away = egui::pos2(700.0, 500.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: away,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        popup_run(&mut app, &tab, vec![egui::Event::PointerMoved(away), press(true), press(false)]);
        assert!(app.menus.trail_menu.is_none(), "the click closed the menu");
        assert_eq!(app.menus.open_popup, Some(AnchoredPopup::BackBranches), "and only the menu");
        popup_run(&mut app, &tab, vec![egui::Event::PointerMoved(away), press(true), press(false)]);
        assert_eq!(app.menus.open_popup, None, "the next click outside closes the popup");
    }
```

and in `trail_strip.rs` tests:

```rust
    #[test]
    fn the_page_menu_stays_while_a_branch_popup_is_open_and_closes_when_neither_shows() {
        let mut rig = forked();
        let tab = rig.tab();
        let id = TrailSource::page_id(&tab, rig.app.nav_history[&tab].epoch(), 1);
        rig.app.view_mode = crate::ViewMode::Item;
        rig.app.menus.trail_menu = Some((id.clone(), egui::pos2(5.0, 5.0)));
        let run = |rig: &mut Rig| {
            let app = &mut rig.app;
            let mut out = rig.ctx.run_ui(egui::RawInput::default(), |ui| app.build_trail_menu(ui));
            out.textures_delta.clear();
        };
        run(&mut rig);
        assert!(rig.app.menus.trail_menu.is_none(), "no strip and no popup: closed");
        rig.app.menus.trail_menu = Some((id, egui::pos2(5.0, 5.0)));
        rig.app.menus.open_popup = Some(crate::AnchoredPopup::BackBranches);
        run(&mut rig);
        assert!(rig.app.menus.trail_menu.is_some(), "a branch popup is open: kept");
    }
```

(Use whatever path `AnchoredPopup` is imported by in this test module.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 8 -p manifold a_right_click_on_a_page a_click_that_closes_the_menu the_page_menu_stays`
Expected: FAIL — the menu is never opened by the popup; the last test fails on "kept" (the condition requires `trail_strip_tab()`).

- [ ] **Step 3: The menu's condition.** In `trail_strip.rs` `build_trail_menu`, replace `page.filter(|_| self.trail_strip_tab().is_some())` by

```rust
        let hosted = self.trail_strip_tab().is_some()
            || matches!(self.menus.open_popup, Some(AnchoredPopup::BackBranches | AnchoredPopup::ForwardBranches));
        let Some((tab, page)) = page.filter(|_| hosted) else {
```

(importing `AnchoredPopup` into the file) and update the method's doc to "…or when its page is gone or neither the strip nor a branch popup is showing".

- [ ] **Step 4: The popup opens and hosts the menu.** In `build_trail_branches_popup`:
  - Before the `Area`, add `let menu_was_open = self.menus.trail_menu.is_some();`.
  - In the card loop, extend the response handling:

```rust
                            for entry in &cards {
                                if let Some(response) = self.virtual_card_click(ui, &mut pass, entry, "trail", false) {
                                    clicked |= response.clicked();
                                    if response.secondary_clicked() {
                                        let at = ui.input(|i| i.pointer.interact_pos()).unwrap_or(response.rect.center());
                                        if let Some(source) = self.virtual_source("trail") {
                                            source.on_menu(self, entry, at);
                                        }
                                    }
                                }
                            }
```

  - After the `Area` and the camera store, call `self.build_trail_menu(ui);` is not possible with `ui: &egui::Ui` (it takes `&mut Ui`): change `build_trail_branches_popup`'s `ui` parameter to `&mut egui::Ui` (its callers `tab_view.rs` and the tests already hold a `&mut Ui`; `popup_run` in the tests passes the closure's `ui`), then `self.build_trail_menu(ui);`.
  - The outside-click rule: `let clicked_outside = clicked_outside_popup(ui, area.response.rect, Some(anchor_rect)) && !menu_was_open;`. (The menu's own click-elsewhere closes it in `build_trail_menu` the same frame; `menu_was_open` was read before that, so that click does not also close the popup.)

- [ ] **Step 5: Run the new tests and the crate**

Run: `cargo test -j 8 -p manifold trail_nav trail_strip` — Expected: all pass.
Run: `cargo test -j 8 -p manifold` — Expected: all pass.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold --all-targets -- -D warnings
git add -A crates/manifold/src
git commit -m "Trail popup: right-click opens the page menu in Tab view; its outside click is the menu's, not the popup's

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Keyboard in the popup

**Files:**
- Modify: `crates/manifold/src/popups.rs` (`build_trail_branches_popup`)
- Test: `crates/manifold/src/trail_nav.rs`

**Interfaces:**
- Consumes: `graph_ui::card_tab_step`, `CardPass.pan_target`, `GraphCamera::pan_to` (Task 1), the popup of Task 3.
- Produces: Tab/Shift+Tab, Enter and Shift+F10 on a focused page inside the popup.

- [ ] **Step 1: Write the failing tests** in `trail_nav.rs` `mod tests`:

```rust
    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    fn card_id(app: &App, tab: &str, entry: usize) -> egui::Id {
        egui::Id::new(("graph_card", crate::trail_strip::trail_cards(app, tab, egui::Pos2::ZERO)[entry].id.clone()))
    }

    #[test]
    fn tab_steps_through_the_popups_pages_and_pans_an_off_screen_one_into_view() {
        let mut app = app_on(&session("trail-popup-tab"), 1);
        let (tab, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://0/");
        let urls: Vec<String> = (0..12).map(|i| format!("https://example.test/page-{i}")).collect();
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        app.nav_history.insert(tab.clone(), NavHistory::visited(&refs));
        app.menus.open_popup = Some(AnchoredPopup::BackBranches);
        for _ in 0..3 {
            popup_run(&mut app, &tab, vec![]);
        }
        let ctx = app.egui_ctx.clone();
        ctx.memory_mut(|m| m.request_focus(card_id(&app, &tab, 0))); // the first page is always registered
        popup_run(&mut app, &tab, vec![]);
        popup_run(&mut app, &tab, vec![key(egui::Key::Tab, egui::Modifiers::NONE)]);
        assert!(ctx.memory(|m| m.has_focus(card_id(&app, &tab, 1))), "Tab: first -> second");
        let camera = &app.menus.trail_canvas.as_ref().unwrap().1;
        assert!(camera.tween.is_some(), "the second page is off screen: the camera eases to it");
    }

    #[test]
    fn enter_on_a_focused_page_picks_it_and_shift_f10_opens_its_menu() {
        let (mut app, tab, mut peer) = open_back_popup("trail-popup-keys");
        let ctx = app.egui_ctx.clone();
        ctx.memory_mut(|m| m.request_focus(card_id(&app, &tab, 1)));
        popup_run(&mut app, &tab, vec![]);
        popup_run(&mut app, &tab, vec![key(egui::Key::F10, egui::Modifiers::SHIFT)]);
        assert!(app.menus.trail_menu.is_some(), "Shift+F10 opens the focused page's menu");
        app.menus.trail_menu = None;
        popup_run(&mut app, &tab, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
        assert_eq!(app.menus.open_popup, None, "Enter picks the page, which closes the popup");
        assert!(peer.sent().iter().any(|m| matches!(m, HostToView::Navigate { url, .. } if url == "https://b/")));
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 8 -p manifold tab_steps_through_the_popups enter_on_a_focused_page`
Expected: FAIL — focus does not move on Tab (nothing calls `card_tab_step` in the popup).

- [ ] **Step 3: Implement.** In `build_trail_branches_popup`'s scene body, before the `for entry in &cards` loop:

```rust
                            if let Some(target) =
                                crate::graph_ui::card_tab_step(ui, &order, |id| cards.iter().find(|e| e.id == id).map(|e| e.rect))
                            {
                                pass.pan_target = Some(target);
                            }
```

(the existing code after the scene already pans to `pass.pan_target`). If Enter does not arrive as `clicked()` (the graph relies on it for a focused card), the failing assertion says so; then treat `response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))` as a click in the loop — but only if the test shows it is needed.

- [ ] **Step 4: Run the new tests and the crate**

Run: `cargo test -j 8 -p manifold trail_nav` — Expected: all pass. Then `cargo test -j 8 -p manifold` — Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold --all-targets -- -D warnings
git add -A crates/manifold/src
git commit -m "Trail popup: Tab, Enter and Shift+F10 on its pages, as in the graph

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Docs, gate and a look

**Files:**
- Modify: `docs/superpowers/specs/2026-10-16-trail-strip-design.md`, `docs/superpowers/specs/2026-10-16-trail-popup-canvas-design.md`
- Temporary (reverted): `crates/gate/src/trail.rs` for screenshots

- [ ] **Step 1: Docs.** In the strip spec: decision 8 (the popup body) becomes "the popup draws the strip's own cards in a pannable, zoomable `Scene` (`2026-10-16-trail-popup-canvas-design.md`)"; its popup-sizing, hover and gesture lines point there; delete lines that name `POPUP_PAGE`, the popup's `ScrollArea` or its own row hover. In the popup spec: change "Status: draft" to "implemented", note the plan, correct decision 1 (`pan_to_card` became `GraphCamera::pan_to`; the card loop returns `pan_target` and `card_tab_step` returns the rect instead of taking the camera), decision 6 and the "shape of the work" line (six tasks), and record the first-task results of the six risks (what held, what needed the fallback). Grep the docs and code comments for `POPUP_PAGE`, `graph_world_to_screen_transform`, `draw_graph_edges`, `handle_card_tab`, `pan_to_card` and fix any remaining mention.

- [ ] **Step 2: Whole-tree checks**

Run: `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` — Expected: clean.
Run: `cargo test -j 8 -p manifold` — Expected: all pass; record the count.

- [ ] **Step 3: Gate `ui`.** Build (`cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`), then run `target/debug/gate.exe all LOG ui` detached at BelowNormal (as CLAUDE.md "Gate"), and report CPU load and Defender state with the result.

- [ ] **Step 4: A look.** With `crates/gate/src/trail.rs` temporarily extended (as in earlier screenshot rounds) take `TOGGLEPOPUP back` screenshots of a short trail (padding, recentred), a long one (clamped; cursor page in view) and a zoomed-out one (scroll over the popup), and check by eye that: the cards match the strip's, the menu shows over a page (spec risk 2) and a plain scroll over the popup does not move the page behind (risk 4). Revert with `git checkout crates/gate/src/trail.rs` and confirm `git status` shows only intended files.

- [ ] **Step 5: Commit**

```bash
git add -A docs
git commit -m "Docs: the trail popup is a Scene of strip cards; strip spec points at it

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

## Self-review

- Spec coverage: decision 1 → Task 1 (with the `pan_target`/`card_tab_step` refinement recorded in Task 6); 2 → Tasks 2–3; 3, 4 → Task 3; 5 → Tasks 3–4; 6 → Task 5; 7 (empty overlays) → Task 3's `debug_assert` run by every popup test; 8 (deletions) → Tasks 1, 3, 6; 9 → nothing to build. Risks 1–6 → Task 3's tests and Task 6's look.
- Types: `trail_cards`/`trail_edges` (Task 2) are what Task 3 calls; `carry` and `viewport`/`pan_target` fields are added in Tasks 1 and 3 and every `CardPass` literal (graph, popup, `card/tests.rs`) gets them in the task that adds the field; `build_trail_branches_popup`'s `&mut Ui` parameter change (Task 4) touches `tab_view.rs`'s call and the tests' `popup_run`.
