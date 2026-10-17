---
status: done
---

# Image Item View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Image nodes an animated full-screen "Item view" that reuses Tab view's toolbar frame, and add a shared zoom chip used by the Graph toolbar and the Image view.

**Architecture:** `ViewMode::Tab` becomes `ViewMode::Item`. A new `render_item_toolbar` owns the toolbar frame (layer 2 "Graph" button, layer 1 Universal chip, a right-to-left trailing `Ui`) and dispatches layer-3 content by node kind. The Image view's subject is a new `App::viewed_image` field (`focused` stays tab-only); its body paints the already-resident GPU texture through egui. Pure zoom/pan math lives in small free functions with unit tests.

**Tech Stack:** Rust, egui 0.36.2, egui_phosphor 0.14 (`light` icons), wgpu, serde.

**Spec:** `docs/superpowers/specs/2026-09-23-image-item-view-design.md` (read its "Plan-time corrections" section first — it supersedes the sections below it). Prior architecture: `docs/superpowers/specs/2026-09-23-toolbar-architecture-design.md`.

## Global Constraints

- Files in scope: `crates/manifold/src/main.rs`, `crates/manifold/src/session.rs`, `README.md`, and the two spec files above. No new dependencies.
- `App::focused` stays tab-only. Never store a non-Tab id in it.
- `cargo build -p manifold` must finish with **zero warnings** after every task (the baseline has zero). `cargo test -p manifold` must pass after every task.
- `toolbar_chip` contract: its content closure runs directly against the `ui` it's given — never nested `ui.horizontal`/`ui.new_child`/`allocate_ui*` inside the chip — and returns the union of its widgets' rects.
- Exact values: `ZOOM_STEP = 1.25`; `IMAGE_ZOOM_MIN = 0.1`; `IMAGE_ZOOM_MAX = 16.0`; Graph zoom range stays `GRAPH_ZOOM_MIN = 0.15` / `GRAPH_ZOOM_MAX = 3.0`; `TRANSITION_DURATION` unchanged (220ms). Image Fit = contain within the body, never above 100%.
- Session compatibility: the on-disk field `view_mode_tab` keeps its name; any new session field is `#[serde(default)]`.
- `ItemToolbarActions` is unpacked with an exhaustive destructure (no `..`).
- When moving code, carry its existing comments with it verbatim; rewrite only comments the move makes false. Don't add `main.rs:NNNN` line references in comments (they rot).
- **Live verification** (every task that says "live"): `cargo build -p manifold`, then drive `target/debug/manifold.exe --headless` over stdin with a bash `coproc`, the way `scripts/headless-smoke-test.sh` does — read that script first; it documents the SCREENSHOT path-translation gotcha and timing. **Always** run with `APPDATA` pointed at a fresh scratch dir (`rm -rf target/verify/appdata && mkdir -p target/verify/appdata && APPDATA="$(cygpath -w "$PWD/target/verify/appdata")" target/debug/manifold.exe --headless`) — headless mode otherwise reads and overwrites the user's real `%APPDATA%\manifold\session.json`. Send `RESIZE 1300 800` first. Save screenshots under `target/verify/<task-name>/`. Inspect screenshots by reading the PNGs (the Read tool renders images); crop/zoom with Python if needed. Test images: generate PNGs with Python (`zlib`+`struct`, no PIL needed) — a 1920×1080 image with a distinct gradient/grid and a 64×64 image with a solid color plus a 1px border — into `target/verify/images/`. Add them with `ADDIMAGE <windows-path>`.

---

### Task 1: Shared zoom chip and Graph toolbar migration

**Files:**
- Modify: `crates/manifold/src/main.rs` (new helpers near `toolbar_chip`; Graph toolbar in `build_graph_ui` where `ui.button("Reset View")` and the `%` label are; the `reset_zoom` application block in `build_graph_ui`; the `mod tests` at the end of the file)
- Modify: `README.md` (line starting "Drag empty background to pan…" — the sentence about "Reset View")

**Interfaces:**
- Produces: `const ZOOM_STEP: f32`, `enum ZoomChipAction { Out, In, Reset100, Fit }` (`Clone, Copy, Debug, PartialEq, Eq`), `fn render_zoom_chip(ui: &mut egui::Ui, pct: f32, can_zoom_out: bool, can_zoom_in: bool, fit_hover: &str) -> Option<ZoomChipAction>`, `fn step_scene_rect(rect: egui::Rect, factor: f32, screen: egui::Vec2, min: f32, max: f32) -> egui::Rect`. Task 4 uses `ZoomChipAction`, `ZOOM_STEP`, `render_zoom_chip`.

- [ ] **Step 1: Capture baselines (before any code change)**

With the Global Constraints' live procedure: launch headless (fresh APPDATA), `RESIZE 1300 800`, wait for the Graph view to settle, `SCREENSHOT target/verify/baseline/graph.png`; click the root tab node to enter Tab view (as the smoke script does), wait for the page to load and the transition to finish, `SCREENSHOT target/verify/baseline/tab.png`; `QUIT`. These are used again in Task 3. Confirm both PNGs exist and look right.

- [ ] **Step 2: Write the failing test**

Append inside `mod tests` at the end of `main.rs`:

```rust
    #[test]
    fn step_scene_rect_zooms_about_center_and_clamps() {
        let screen = egui::vec2(1000.0, 800.0);
        let scale_of = |r: egui::Rect| (screen / r.size()).min_elem();

        let at_100 = egui::Rect::from_center_size(egui::pos2(50.0, 60.0), screen);
        let zoomed_in = step_scene_rect(at_100, ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert_eq!(zoomed_in.center(), at_100.center());
        assert!((scale_of(zoomed_in) - ZOOM_STEP).abs() < 1e-4);

        let zoomed_out = step_scene_rect(at_100, 1.0 / ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(zoomed_out) - 1.0 / ZOOM_STEP).abs() < 1e-4);

        let near_max = egui::Rect::from_center_size(egui::pos2(0.0, 0.0), screen / 2.8);
        let clamped = step_scene_rect(near_max, ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(clamped) - GRAPH_ZOOM_MAX).abs() < 1e-3);

        let near_min = egui::Rect::from_center_size(egui::pos2(0.0, 0.0), screen / 0.16);
        let clamped = step_scene_rect(near_min, 1.0 / ZOOM_STEP, screen, GRAPH_ZOOM_MIN, GRAPH_ZOOM_MAX);
        assert!((scale_of(clamped) - GRAPH_ZOOM_MIN).abs() < 1e-3);
    }
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p manifold step_scene_rect`
Expected: compile error — `step_scene_rect` / `ZOOM_STEP` not found.

- [ ] **Step 4: Implement the helpers**

Add right after `in_reading_order` (after `toolbar_chip`):

```rust
/// Multiplicative step for one click of a zoom chip's −/+ (and, in Image
/// view, one `Ctrl+=`/`Ctrl+-`) -- shared so Graph and Image zoom feel the
/// same.
const ZOOM_STEP: f32 = 1.25;

/// What a click on `render_zoom_chip` asked for. The chip holds no zoom
/// state of its own -- each caller applies this to whatever "zoom" means
/// in its own view (Graph: the camera's `graph_scene_rect`; Image view:
/// `ImageViewState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ZoomChipAction {
    Out,
    In,
    Reset100,
    Fit,
}

/// `rect` (a Graph camera's world-space `graph_scene_rect`) resized about
/// its own center so the zoom scale `graph_world_to_screen_transform`
/// derives from it (`screen / rect.size()`, min axis) is multiplied by
/// `factor`, clamped to `min..=max`. The ratio is computed from the raw,
/// unclamped scale so the result lands exactly on the clamped target.
fn step_scene_rect(rect: egui::Rect, factor: f32, screen: egui::Vec2, min: f32, max: f32) -> egui::Rect {
    let raw = (screen / rect.size()).min_elem();
    let new_scale = (raw.clamp(min, max) * factor).clamp(min, max);
    egui::Rect::from_center_size(rect.center(), rect.size() * (raw / new_scale))
}

/// −, a clickable `NN%` readout (back to 100%), +, and Fit, as one chip.
/// Left-to-right rows only (both callers are): in a right-to-left `Ui`
/// its buttons would come out reversed -- see `in_reading_order` for the
/// pattern if a right-to-left caller ever needs it. The readout is a
/// frameless `Button` rather than a `Label` so it has the same height as
/// its icon-button neighbors (a `Label` sits high in the Item toolbar's
/// top-aligned row), with a fixed minimum width so the chip doesn't
/// jitter as the digit count changes.
fn render_zoom_chip(
    ui: &mut egui::Ui,
    pct: f32,
    can_zoom_out: bool,
    can_zoom_in: bool,
    fit_hover: &str,
) -> Option<ZoomChipAction> {
    debug_assert_eq!(
        ui.layout().main_dir(),
        egui::Direction::LeftToRight,
        "render_zoom_chip lays out left to right only"
    );
    let mut action = None;
    toolbar_chip(ui, |ui| {
        let out = chip_icon_button_enabled(
            ui,
            egui::Id::new("icon_btn_zoom_out"),
            egui_phosphor::light::MAGNIFYING_GLASS_MINUS,
            can_zoom_out,
        )
        .on_hover_text("Zoom out");
        if out.clicked() {
            action = Some(ZoomChipAction::Out);
        }
        let readout = ui
            .add(
                egui::Button::new(format!("{}%", pct.round() as i32))
                    .frame(false)
                    .min_size(egui::vec2(44.0, 0.0)),
            )
            .on_hover_text("Reset zoom to 100%")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if readout.clicked() {
            action = Some(ZoomChipAction::Reset100);
        }
        let zoom_in = chip_icon_button_enabled(
            ui,
            egui::Id::new("icon_btn_zoom_in"),
            egui_phosphor::light::MAGNIFYING_GLASS_PLUS,
            can_zoom_in,
        )
        .on_hover_text("Zoom in");
        if zoom_in.clicked() {
            action = Some(ZoomChipAction::In);
        }
        let fit = chip_icon_button(ui, egui::Id::new("icon_btn_zoom_fit"), egui_phosphor::light::FRAME_CORNERS)
            .on_hover_text(fit_hover);
        if fit.clicked() {
            action = Some(ZoomChipAction::Fit);
        }
        out.rect.union(readout.rect).union(zoom_in.rect).union(fit.rect)
    });
    action
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p manifold step_scene_rect`
Expected: PASS. (A `dead_code` warning for `render_zoom_chip` is expected until Step 6.)

- [ ] **Step 6: Migrate the Graph toolbar**

In `build_graph_ui`, next to `let mut reset_view = false; let mut reset_zoom = false;` add:

```rust
        let mut graph_zoom_step: Option<f32> = None;
```

Replace the `if ui.button("Reset View").clicked() { reset_view = true; }` block **and** the following click-to-reset `%` label block (its comment "Click-to-reset (keeping the current pan…" through `reset_zoom = true; }`) with:

```rust
                    // −/+/100%/Fit in one chip (`render_zoom_chip`, shared
                    // with Image view). Fit is the old "Reset View"
                    // (re-fit every node at this level); the `%` readout
                    // keeps the current pan and only resets the scale.
                    let zoom = self.graph_zoom_pct / 100.0;
                    if let Some(action) = render_zoom_chip(
                        ui,
                        self.graph_zoom_pct,
                        zoom > GRAPH_ZOOM_MIN + 1e-3,
                        zoom < GRAPH_ZOOM_MAX - 1e-3,
                        "Reset View (fit all)",
                    ) {
                        match action {
                            ZoomChipAction::Fit => reset_view = true,
                            ZoomChipAction::Reset100 => reset_zoom = true,
                            ZoomChipAction::In => graph_zoom_step = Some(ZOOM_STEP),
                            ZoomChipAction::Out => graph_zoom_step = Some(1.0 / ZOOM_STEP),
                        }
                    }
```

Right after the existing `if reset_zoom { … }` block (the one using `scene_outer_rect.size()`), add:

```rust
        if let Some(factor) = graph_zoom_step {
            self.graph_scene_rect = step_scene_rect(
                self.graph_scene_rect,
                factor,
                scene_outer_rect.size(),
                GRAPH_ZOOM_MIN,
                GRAPH_ZOOM_MAX,
            );
        }
```

Update the Graph toolbar's rule comment (the paragraph beginning "The rule this toolbar actually follows, for any future addition:") to:

```rust
                    // The rule this toolbar actually follows, for any
                    // future addition: the identity/entry-point action that
                    // defines what this toolbar *is* ("+ New Tab") stays
                    // full text; everything else added later to relieve
                    // crowding goes icon-only with a hover tooltip carrying
                    // the old label, per this paragraph's own history.
                    // "Reset View" was the other text holdout until it
                    // became the zoom chip's Fit button (hover text keeps
                    // the name).
```

Also update the comment above the three group-action icons ("These three buttons used to carry full text labels … and, together with "+ New Group"/"+ New Tab"/"Reset View", made this toolbar cramped") only if it now reads false — it's history, so leave it unless it claims "Reset View" still exists.

- [ ] **Step 7: Update README**

In `README.md`, in the paragraph that says `"Reset View" fits every node currently visible at the current level`, change that phrase to: `the zoom chip's Fit button (formerly "Reset View") fits every node currently visible at the current level`, and append one sentence to the same paragraph: `The chip also has −/+ buttons (×1.25 per click, clamped to the 15%–300% zoom range) and a clickable percentage readout that resets to 100% keeping the current pan.` Leave the other historical "Reset View" mentions (they describe past work) untouched.

- [ ] **Step 8: Build, test, live-verify**

Run: `cargo build -p manifold` (zero warnings) and `cargo test -p manifold` (all pass).
Live: headless, `RESIZE 1300 800`, screenshot the Graph view; `CLICK` the zoom chip's + twice, − once, the `%` readout, and Fit, screenshotting after each (`target/verify/task1/`). Check: the chip is vertically aligned with "+ New Tab" and the Groups chip; the readout goes 100%→125%→156%→125%… (starting value is whatever Fit produced; check the ×1.25 ratios); `%` click shows 100%; Fit re-frames all nodes; − is disabled at 15%, + at 300% (reach one bound by repeated clicks). Compare against `target/verify/baseline/graph.png`: only the Reset View/% area should differ.

- [ ] **Step 9: Commit**

```bash
git add crates/manifold/src/main.rs README.md
git commit -m "Add shared zoom chip; use it in the Graph toolbar

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Rename Tab view to Item view (mechanical)

**Files:**
- Modify: `crates/manifold/src/main.rs`

**Interfaces:**
- Produces: `ViewMode::Item` (replaces `ViewMode::Tab`), `fn build_item_ui(&mut self, ui)` (replaces `build_tab_ui`), `fn start_transition_to_item(&mut self, id: &str)` (replaces `start_transition_to_tab`), `TransitionKind::ToItem` (replaces `ToTab`), `Transition { node_id: String, kind, start }` (field renamed from `tab_id`). No behavior change.

- [ ] **Step 1: Rename the enum variant and its uses**

Change the variant and add a doc comment:

```rust
enum ViewMode {
    Graph,
    /// One node shown full-screen ("maximized"): a Tab's live page, or --
    /// while `App::viewed_image` is `Some` -- an Image. Was `Tab` before a
    /// second kind could enter it; `session::Session::view_mode_tab` keeps
    /// the old name on disk.
    Item,
```

Replace every `ViewMode::Tab` with `ViewMode::Item` in `main.rs` (about 20 sites: initial `view_mode`, `settings_return_view`, `chrome_height_points`, session save/restore, `focus_tab`, `finish_transition_if_done`, `toggle_view_mode_shortcut`, `focus_address_bar_shortcut`, `open_find_bar_shortcut`, `build_ui`, `window_event`'s `in_tab_view`, `content_panes_pts`, the `DebugCommand::Enter/Escape/ArrowDown/ArrowUp` guards).

- [ ] **Step 2: Rename the functions and transition names**

- `fn build_tab_ui` → `fn build_item_ui`, and every mention of `build_tab_ui` in comments/doc comments → `build_item_ui`.
- `fn start_transition_to_tab` → `fn start_transition_to_item` and its two call sites (`open_focused_graph_node`, the `clicked_focus` handling in `build_graph_ui`).
- `TransitionKind::ToTab` → `TransitionKind::ToItem` (definition, `start_transition_to_item`, `finish_transition_if_done`, `transition_content_rect_points`). Update the variant doc: "`node_id`'s graph-node spot grows to fill the screen as Item view's content."; `ToGraph` doc: "`node_id`" instead of "`tab_id`".
- `struct Transition`'s field `tab_id` → `node_id`, and its uses: `leave_to_graph`, `start_transition_to_item`, `finish_transition_if_done` (rename the local `tab_id` there to `node_id` too), `transition_content_rect_points` (`transition.tab_id`), and `content_panes_pts` (`transition.tab_id.clone()`). Update `Transition`'s doc comment ("for one specific node", "`node_id`'s live content"). **Do not** touch the unrelated `tab_id` fields on redirect-fetch results, frame updates, or suggestions.

- [ ] **Step 3: Session field comment**

In `session.rs`, add a doc comment above `pub view_mode_tab: bool,`:

```rust
    /// `true` = `ViewMode::Item` (named before that variant was renamed
    /// from `Tab`; kept so existing `session.json` files still load).
```

- [ ] **Step 4: Build and test**

Run: `cargo build -p manifold` (zero warnings), `cargo test -p manifold` (all pass). Then `grep -n "ViewMode::Tab\|build_tab_ui\|start_transition_to_tab\|ToTab\b\|transition\.tab_id" crates/manifold/src/main.rs` → no matches.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/main.rs crates/manifold/src/session.rs
git commit -m "Rename Tab view to Item view (no behavior change)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Item toolbar frame (Universal chip to layer 1; guard the Tab-only unwrap)

**Files:**
- Modify: `crates/manifold/src/main.rs` (`ItemToolbarActions`, `render_item_toolbar_content`, `render_tab_toolbar_content`, `build_item_ui`)
- Modify: `docs/superpowers/specs/2026-09-23-toolbar-architecture-design.md` (Non-goals)

**Interfaces:**
- Consumes: `ViewMode::Item`, `build_item_ui` (Task 2).
- Produces: `struct ItemToolbarFrame { back_to_graph: bool, shortcuts_help_button_rect: Option<egui::Rect>, actions: ItemToolbarActions }`; `fn render_item_toolbar(&mut self, node_id: &str, ui: &mut egui::Ui) -> ItemToolbarFrame`; `fn render_item_toolbar_content(&mut self, node_id: &str, ui: &mut egui::Ui, trailing_ui: &mut egui::Ui) -> ItemToolbarActions` (new `trailing_ui` parameter); `fn build_tab_view_ui(&mut self, ui: &mut egui::Ui)` (today's `build_item_ui` body); `build_item_ui` becomes a dispatcher (Task 4 adds the Image arm). `ItemToolbarActions` loses `shortcuts_help_button_rect`.

- [ ] **Step 1: Split `build_item_ui`**

Rename the existing `fn build_item_ui` to `fn build_tab_view_ui` (update comment mentions that refer to the Tab-specific function), and add above it:

```rust
    /// `ViewMode::Item`: whichever node is maximized -- today always the
    /// focused Tab.
    fn build_item_ui(&mut self, ui: &mut egui::Ui) {
        self.build_tab_view_ui(ui);
    }
```

(Task 4 Step 7 replaces this whole function.)

- [ ] **Step 2: Add the frame**

Add a struct next to `ItemToolbarActions`:

```rust
/// What `render_item_toolbar` (the Item toolbar's shared frame) hands back:
/// the frame's own two results plus whatever the per-kind content reported.
struct ItemToolbarFrame {
    /// Layer 2's "Graph" button was clicked.
    back_to_graph: bool,
    /// Layer 1's Help button -- the shortcuts-help popup anchors to it.
    shortcuts_help_button_rect: Option<egui::Rect>,
    actions: ItemToolbarActions,
}
```

Remove the `shortcuts_help_button_rect` field from `ItemToolbarActions` (and from its doc comment if mentioned).

Add this method just above `render_item_toolbar_content`, moving the long `Align::Min` comment ("Not `horizontal_centered` (`Align::Center`): measured live that `egui::ComboBox`…") verbatim from `build_tab_view_ui`'s `toolbar_panel` closure into the marked spot:

```rust
    /// The Item toolbar's shared frame -- the design spec's three layers,
    /// outer to inner. Layer 2 ("Graph") sits at the leading edge. Layer 1
    /// (the Universal chip) is drawn *first* into a right-to-left trailing
    /// child `Ui` so it's always rightmost, for every kind. Layer 3 (the
    /// focused kind's own content) then gets both the row `ui` for leading
    /// widgets and `trailing_ui` for trailing ones; a kind that needs the
    /// space in between (Tab's address bar) measures `trailing_ui` to size
    /// it. The frame finally advances the row past the trailing cluster.
    fn render_item_toolbar(&mut self, node_id: &str, ui: &mut egui::Ui) -> ItemToolbarFrame {
        let mut back_to_graph = false;
        let mut shortcuts_help_button_rect = None;
        let mut actions = ItemToolbarActions::default();
        toolbar_panel("toolbar", ui, |ui| {
            // <moved verbatim: the "Not `horizontal_centered`…" comment>
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                if render_return_to_graph(ui) {
                    back_to_graph = true;
                }
                let mut trailing_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(ui.available_rect_before_wrap())
                        .layout(egui::Layout::right_to_left(egui::Align::Min)),
                );
                shortcuts_help_button_rect = Some(self.render_universal_chip(&mut trailing_ui));
                actions = self.render_item_toolbar_content(node_id, ui, &mut trailing_ui);
                ui.advance_cursor_after_rect(trailing_ui.min_rect());
            });
        });
        ItemToolbarFrame { back_to_graph, shortcuts_help_button_rect, actions }
    }
```

- [ ] **Step 3: Thread `trailing_ui` through the content**

`render_item_toolbar_content` gains `trailing_ui: &mut egui::Ui` (rename its first parameter from `focused_id` to `node_id`) and passes it on: `ItemNodeKind::Tab => self.render_tab_toolbar_content(node_id, ui, trailing_ui),`.

In `render_tab_toolbar_content(&mut self, focused_id: &str, ui: &mut egui::Ui, trailing_ui: &mut egui::Ui)`:
- Delete `let remaining_rect = ui.available_rect_before_wrap();` and the `let mut trailing_ui = ui.new_child(…);` statement.
- Delete the `render_universal_chip` call, the comment above it ("`render_universal_chip` handles this `Ui`'s right-to-left direction itself…") and the whole `NOTE:` paragraph, and `actions.shortcuts_help_button_rect = Some(shortcuts_help_rect);`.
- In the comment "The trailing cluster (Bookmark, View-as-JSON, Split + Close Split, Tab actions, Shortcuts help, Settings gear) is drawn *first*, into its own right-to-left child `Ui` anchored to this row's remaining space, so its real consumed width can be measured…", replace the first sentence with: "The trailing cluster (Bookmark, View-as-JSON, Split + Close Split, Tab actions) is drawn *first*, into the frame's right-to-left `trailing_ui` (`render_item_toolbar`, which has already put the Universal chip at its far right), so its real consumed width can be measured" — keep the rest of that comment.
- Replace `&mut trailing_ui` with `trailing_ui` (e.g. `icon_button_eased(trailing_ui, …)`, `toolbar_chip(trailing_ui, …)`); `trailing_ui.button(…)` is unchanged.
- Replace the width computation with:

```rust
        let trailing_width = trailing_ui.min_rect().width();
        // …existing comment "Now the address bar gets whatever's actually left…" stays…
        let text_width = (ui.available_rect_before_wrap().width() - trailing_width).max(40.0);
```

- Delete `ui.advance_cursor_after_rect(trailing_ui.min_rect());` (the frame does it).

- [ ] **Step 4: Use the frame in `build_tab_view_ui`**

Delete the `let mut` declarations of `go_back`, `go_forward`, `reload_requested`, `home_requested`, `back_to_graph`, `toggle_bookmark_requested`, `tab_actions_button_rect`, `split_engine_button_rect`, `close_split_requested`, `address_bar_rect`, `shortcuts_help_button_rect`, `enter_pressed` (keep `let mut navigate = None;` and keep the "Whether Enter fired this frame…" comment, moving it above the new statement). Replace the whole `toolbar_panel("toolbar", ui, |ui| { … });` statement with:

```rust
        // Exhaustive (no `..`): a new `ItemToolbarActions` field that isn't
        // wired up here fails to compile.
        let ItemToolbarFrame {
            back_to_graph,
            shortcuts_help_button_rect,
            actions:
                ItemToolbarActions {
                    go_back,
                    go_forward,
                    reload_requested,
                    home_requested,
                    enter_pressed,
                    toggle_bookmark_requested,
                    close_split_requested,
                    tab_actions_button_rect,
                    split_engine_button_rect,
                    address_bar_rect,
                },
        } = self.render_item_toolbar(&focused_id, ui);
```

- [ ] **Step 5: Guard the Tab-only unwrap**

In `build_tab_view_ui`, replace the `.unwrap()` comment and block:

```rust
        if !editing_address {
            self.address_bar_text = self.nodes[&focused_id].as_tab().unwrap().url.clone();
        }
```

with:

```rust
        // `focused` is tab-only (see `App::focused`), so this always finds
        // a Tab; `if let` rather than `.unwrap()` so a future path that
        // breaks that invariant degrades to a stale address bar, not a
        // panic.
        if !editing_address {
            if let Some(tab) = self.nodes.get(&focused_id).and_then(|n| n.as_tab()) {
                self.address_bar_text = tab.url.clone();
            }
        }
```

- [ ] **Step 6: Amend the old spec's Non-goals**

In `docs/superpowers/specs/2026-09-23-toolbar-architecture-design.md`, append to the Non-goals bullet that begins "Making the Universal chip and the Tab-only `.unwrap()`…" this sentence: "Both were resolved by the Image Item View pass (`2026-09-23-image-item-view-design.md`): `render_item_toolbar` now renders the Universal chip as a real layer-1 element before per-kind dispatch, and the `.unwrap()` became an `if let`."

- [ ] **Step 7: Build, test, live-verify against the baseline**

Run: `cargo build -p manifold` (zero warnings), `cargo test -p manifold`.
Live: the Task 1 Step 1 Tab-view sequence → `target/verify/task3/tab.png`. Compare the toolbar strip (top ~`CHROME_HEIGHT_POINTS` × scale-factor pixels) with `target/verify/baseline/tab.png`: the Universal chip, Tab actions, Split, Bookmark chip, and the address bar's left and right edges must be at the same x positions; any differing pixels must be explained (e.g. favicon vs spinner timing). Also click Help (the `?`) and confirm the shortcuts popup still anchors under it.

- [ ] **Step 8: Commit**

```bash
git add crates/manifold/src/main.rs docs/superpowers/specs/2026-09-23-toolbar-architecture-design.md
git commit -m "Extract the Item toolbar frame; Universal chip is layer 1

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Image view (math, state, body, toolbar, entry/exit)

Entry/exit in this task is instant; Task 5 animates it.

**Files:**
- Modify: `crates/manifold/src/main.rs`

**Interfaces:**
- Consumes: `ZOOM_STEP`, `ZoomChipAction`, `render_zoom_chip` (Task 1); `ViewMode::Item`, `start_transition_to_item` (Task 2); `render_item_toolbar`, `ItemToolbarFrame`, `render_item_toolbar_content(node_id, ui, trailing_ui)`, `build_tab_view_ui` (Task 3); existing `in_reading_order`, `toolbar_chip`, `chip_icon_button`, `reveal_image`, `download_filename`, `BG`, `CHROME_HEIGHT_POINTS`, `GpuState::image_id`.
- Produces (used by Tasks 5–6): consts `IMAGE_ZOOM_MIN`, `IMAGE_ZOOM_MAX`; `enum ImageZoom { Fit, Scale(f32) }`; `struct ImageViewState { zoom: ImageZoom, pan: egui::Vec2 }` (`Clone, Copy, Debug, PartialEq`, `Default` = Fit/zero) with `fn effective(&self, image: egui::Vec2, body: egui::Vec2) -> (f32, egui::Vec2)`; free fns `image_fit_scale`, `image_display_rect`, `clamp_image_pan`, `zoom_about`, `image_view_after_action`, `image_pixel_size(&ImagePayload) -> egui::Vec2`; `App` fields `viewed_image: Option<String>`, `image_view: ImageViewState`; methods `showing_tab_view(&self) -> bool`, `showing_image_view(&self) -> bool`, `screen_size_points(&self) -> egui::Vec2`, `item_body_rect(&self) -> egui::Rect`, `hide_focused_tab(&mut self)`, `focus_item(&mut self, id: &str)`, `apply_image_zoom_action(&mut self, action: ZoomChipAction)`, `build_image_view_ui`, `render_image_toolbar_content`.

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    #[test]
    fn image_fit_scale_contains_and_never_upscales() {
        let body = egui::vec2(1000.0, 800.0);
        assert_eq!(image_fit_scale(egui::vec2(2000.0, 1000.0), body), 0.5);
        assert_eq!(image_fit_scale(egui::vec2(1000.0, 4000.0), body), 0.2);
        assert_eq!(image_fit_scale(egui::vec2(64.0, 64.0), body), 1.0);
    }

    #[test]
    fn zoom_about_keeps_the_anchor_point_fixed() {
        let (scale, pan) = (0.5, egui::vec2(30.0, -20.0));
        let anchor = egui::vec2(100.0, 50.0);
        let (new_scale, new_pan) = zoom_about(scale, pan, 2.0, anchor);
        assert_eq!(new_scale, 1.0);
        let before = (anchor - pan) / scale;
        let after = (anchor - new_pan) / new_scale;
        assert!((before - after).length() < 1e-4, "{before:?} vs {after:?}");
    }

    #[test]
    fn zoom_about_clamps_scale() {
        assert_eq!(zoom_about(15.0, egui::Vec2::ZERO, 2.0, egui::Vec2::ZERO).0, IMAGE_ZOOM_MAX);
        assert_eq!(zoom_about(0.11, egui::Vec2::ZERO, 0.5, egui::Vec2::ZERO).0, IMAGE_ZOOM_MIN);
    }

    #[test]
    fn clamp_image_pan_limits_each_axis_independently() {
        let body = egui::vec2(1000.0, 800.0);
        let scaled = egui::vec2(2000.0, 500.0); // wider than the body, shorter than it
        assert_eq!(clamp_image_pan(egui::vec2(900.0, 40.0), scaled, body), egui::vec2(500.0, 0.0));
        assert_eq!(clamp_image_pan(egui::vec2(-120.0, 0.0), scaled, body), egui::vec2(-120.0, 0.0));
    }

    #[test]
    fn image_view_after_action_steps_from_fit_and_resets() {
        let image = egui::vec2(2000.0, 1000.0);
        let body = egui::vec2(1000.0, 800.0);
        let zoomed = image_view_after_action(ImageViewState::default(), ZoomChipAction::In, image, body);
        assert_eq!(zoomed.zoom, ImageZoom::Scale(0.5 * ZOOM_STEP));
        let actual = image_view_after_action(zoomed, ZoomChipAction::Reset100, image, body);
        assert_eq!(actual, ImageViewState { zoom: ImageZoom::Scale(1.0), pan: egui::Vec2::ZERO });
        assert_eq!(image_view_after_action(zoomed, ZoomChipAction::Fit, image, body), ImageViewState::default());
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p manifold image_`
Expected: compile errors — the functions/types don't exist.

- [ ] **Step 3: Implement the math**

Add after `render_zoom_chip`:

```rust
const IMAGE_ZOOM_MIN: f32 = 0.1;
const IMAGE_ZOOM_MAX: f32 = 16.0;
/// Image view mouse-wheel sensitivity: zoom factor per point of vertical
/// scroll, applied as `exp(delta * rate)` so equal scroll amounts in and
/// out cancel exactly.
const IMAGE_WHEEL_ZOOM_RATE: f32 = 0.002;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ImageZoom {
    /// Contain within the body, never above 100% (`image_fit_scale`).
    Fit,
    Scale(f32),
}

/// Image view's zoom/pan. `pan` is the image center's offset from the
/// body center, in points; ignored while `zoom` is `Fit`. Reset to the
/// default (Fit) every time an Image enters Item view; not persisted.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ImageViewState {
    zoom: ImageZoom,
    pan: egui::Vec2,
}

impl Default for ImageViewState {
    fn default() -> Self {
        Self { zoom: ImageZoom::Fit, pan: egui::Vec2::ZERO }
    }
}

impl ImageViewState {
    /// The concrete `(scale, pan)` to draw with for an `image`-sized
    /// picture in a `body`-sized area -- Fit resolved, pan re-clamped (the
    /// window may have been resized since it was stored).
    fn effective(&self, image: egui::Vec2, body: egui::Vec2) -> (f32, egui::Vec2) {
        match self.zoom {
            ImageZoom::Fit => (image_fit_scale(image, body), egui::Vec2::ZERO),
            ImageZoom::Scale(scale) => (scale, clamp_image_pan(self.pan, image * scale, body)),
        }
    }
}

/// An Image's pixel size as a `Vec2` (at least 1×1).
fn image_pixel_size(image: &ImagePayload) -> egui::Vec2 {
    egui::vec2(image.width.max(1) as f32, image.height.max(1) as f32)
}

/// Contain `image` within `body`, never above 1.0 -- same "scale down,
/// never up" rule `image_node_card_size` uses, so a small image isn't
/// blurred by magnification it didn't ask for.
fn image_fit_scale(image: egui::Vec2, body: egui::Vec2) -> f32 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    (body.x / image.x).min(body.y / image.y).clamp(IMAGE_ZOOM_MIN, 1.0)
}

fn image_display_rect(body: egui::Rect, image: egui::Vec2, scale: f32, pan: egui::Vec2) -> egui::Rect {
    egui::Rect::from_center_size(body.center() + pan, image * scale)
}

/// Per axis: when the scaled image is larger than the body, no gap may
/// open past its edges; when it fits, it stays centered on that axis.
fn clamp_image_pan(pan: egui::Vec2, scaled: egui::Vec2, body: egui::Vec2) -> egui::Vec2 {
    let axis = |pan: f32, scaled: f32, body: f32| {
        let slack = (scaled - body) / 2.0;
        if slack <= 0.0 { 0.0 } else { pan.clamp(-slack, slack) }
    };
    egui::vec2(axis(pan.x, scaled.x, body.x), axis(pan.y, scaled.y, body.y))
}

/// Zooms by `factor` (clamped to `IMAGE_ZOOM_MIN..=IMAGE_ZOOM_MAX`) so the
/// image point under `anchor` (relative to the body center) stays put.
fn zoom_about(scale: f32, pan: egui::Vec2, factor: f32, anchor: egui::Vec2) -> (f32, egui::Vec2) {
    let new_scale = (scale * factor).clamp(IMAGE_ZOOM_MIN, IMAGE_ZOOM_MAX);
    let ratio = new_scale / scale;
    (new_scale, anchor - (anchor - pan) * ratio)
}

/// Applies a zoom-chip click (or its keyboard equivalent) to Image view's
/// state. −/+ zoom about the body center.
fn image_view_after_action(
    state: ImageViewState,
    action: ZoomChipAction,
    image: egui::Vec2,
    body: egui::Vec2,
) -> ImageViewState {
    match action {
        ZoomChipAction::Fit => ImageViewState::default(),
        ZoomChipAction::Reset100 => ImageViewState { zoom: ImageZoom::Scale(1.0), pan: egui::Vec2::ZERO },
        ZoomChipAction::In | ZoomChipAction::Out => {
            let factor = if action == ZoomChipAction::In { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
            let (scale, pan) = state.effective(image, body);
            let (scale, pan) = zoom_about(scale, pan, factor, egui::Vec2::ZERO);
            ImageViewState { zoom: ImageZoom::Scale(scale), pan: clamp_image_pan(pan, image * scale, body) }
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p manifold`
Expected: all pass (dead-code warnings for the new items are expected until Step 7).

- [ ] **Step 5: App state and view predicates**

Add fields to `struct App` next to `focused` (with the defaults `viewed_image: None, image_view: ImageViewState::default(),` in the constructor):

```rust
    /// The Image node `ViewMode::Item` is showing, if any. `None` means
    /// Item view (if active) shows `focused`, a Tab. Kept separate from
    /// `focused` because `focused` is tab-only by contract -- see the
    /// Image Item View spec's "Plan-time corrections".
    viewed_image: Option<String>,
    /// Zoom/pan for `viewed_image` -- see `ImageViewState`.
    image_view: ImageViewState,
```

Add to `impl App` (next to `chrome_height_points`):

```rust
    /// Item view showing a Tab -- the case every engine-facing path (input
    /// forwarding, content panes, address bar, find bar) cares about.
    fn showing_tab_view(&self) -> bool {
        self.view_mode == ViewMode::Item && self.viewed_image.is_none()
    }

    fn showing_image_view(&self) -> bool {
        self.view_mode == ViewMode::Item && self.viewed_image.is_some()
    }

    fn screen_size_points(&self) -> egui::Vec2 {
        let screen = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let ppp = self.ppp() as f32;
        egui::vec2(screen.width as f32 / ppp, screen.height as f32 / ppp)
    }

    /// Item view's content area (points, screen space): everything below
    /// the toolbar.
    fn item_body_rect(&self) -> egui::Rect {
        let screen = self.screen_size_points();
        egui::Rect::from_min_max(
            egui::pos2(0.0, CHROME_HEIGHT_POINTS),
            egui::pos2(screen.x, screen.y.max(CHROME_HEIGHT_POINTS)),
        )
    }
```

Gate the engine-facing sites on `showing_tab_view()` instead of `view_mode == ViewMode::Item`:
- `focus_address_bar_shortcut`: `if self.showing_tab_view() {`
- `open_find_bar_shortcut`: `if !self.showing_tab_view() { return; }`
- `window_event`: `let in_tab_view = self.showing_tab_view();`
- `content_panes_pts`: the transition branch becomes

```rust
                let content_panes_pts: Vec<(egui::Rect, String)> = if let Some(transition) = &self.transition {
                    // Only a Tab has a GPU pane; an Image's transition is
                    // painted through egui instead (see `build_ui`).
                    if self.nodes.get(&transition.node_id).is_some_and(|n| n.as_tab().is_some()) {
                        vec![(self.transition_content_rect_points(transition), transition.node_id.clone())]
                    } else {
                        vec![]
                    }
                } else if self.showing_tab_view() {
```

- The `DebugCommand::Enter`, `ArrowDown`, `ArrowUp` guards: `if self.showing_tab_view() && self.transition.is_none()`. `DebugCommand::Escape` keeps `self.view_mode == ViewMode::Item && …` (both kinds handle Escape).

- [ ] **Step 6: Entry and exit**

Extract the "hide the old tab" block from `focus_tab` into:

```rust
    /// Stops the focused tab rendering (and drops its find bar, context
    /// menu, and split) -- what switching the maximized node away from it
    /// needs, whether to another tab (`focus_tab`) or an Image
    /// (`focus_item`). `focused` itself is left alone.
    fn hide_focused_tab(&mut self) {
        self.close_find_bar();
        self.context_menu = None;
        self.close_split();
        if let Some(old_id) = self.focused.clone() {
            if let Some(old) = self.nodes.get_mut(&old_id).and_then(|n| n.as_tab_mut()) {
                old.tab.set_visible(false);
            }
            if let Some(gpu) = &mut self.gpu {
                gpu.free_tab_texture(&old_id);
            }
        }
    }
```

`focus_tab` becomes `if self.focused.as_deref() != Some(id) { self.hide_focused_tab(); }`, followed by the existing rest, plus `self.viewed_image = None;` right before `self.view_mode = ViewMode::Item;` (any jump to a tab — Ctrl+Tab, quick switcher, Ctrl+G — leaves Image view).

Add after `focus_tab`:

```rust
    /// Maximizes `id` into Item view, whatever its kind -- a Tab goes
    /// through `focus_tab`; an Image becomes `viewed_image` (the last tab
    /// stays `focused`, hidden, exactly as in Graph view). Kinds with no
    /// Item view are a no-op (and a debug-build assertion).
    fn focus_item(&mut self, id: &str) {
        match self.nodes.get(id).map(|n| item_node_kind(&n.payload)) {
            Some(ItemNodeKind::Tab) => self.focus_tab(id),
            Some(ItemNodeKind::Image) => {
                self.hide_focused_tab();
                self.viewed_image = Some(id.to_string());
                self.image_view = ImageViewState::default();
                self.view_mode = ViewMode::Item;
                self.save_session();
            }
            Some(kind) => debug_assert!(false, "focus_item: {kind:?} nodes have no Item view"),
            None => {}
        }
    }
```

In `leave_to_graph`, right after `self.context_menu = None;`, add the instant Image exit (Task 5 replaces it with an animation):

```rust
        if self.viewed_image.take().is_some() {
            self.view_mode = ViewMode::Graph;
            self.save_session();
            return;
        }
```

- [ ] **Step 7: Body, toolbar content, dispatch**

`build_item_ui` becomes:

```rust
    /// `ViewMode::Item`: whichever node is maximized -- `viewed_image` if
    /// set, else the focused Tab.
    fn build_item_ui(&mut self, ui: &mut egui::Ui) {
        match self.viewed_image.clone() {
            Some(image_id) => self.build_image_view_ui(ui, &image_id),
            None => self.build_tab_view_ui(ui),
        }
    }
```

`render_item_toolbar_content`'s match: add `ItemNodeKind::Image => self.render_image_toolbar_content(node_id, ui, trailing_ui),` and remove `Image` from the empty-default arm.

Add:

```rust
    /// Image view's layer-3 toolbar content: identity (filename · pixel
    /// size, full path on hover) and the zoom chip on the leading side;
    /// Open Containing Folder + Copy Path in the trailing cluster.
    fn render_image_toolbar_content(
        &mut self,
        image_id: &str,
        ui: &mut egui::Ui,
        trailing_ui: &mut egui::Ui,
    ) -> ItemToolbarActions {
        let Some((path, image)) = self
            .nodes
            .get(image_id)
            .and_then(|n| n.as_image())
            .map(|i| (i.path.clone(), image_pixel_size(i)))
        else {
            return ItemToolbarActions::default();
        };

        // (reveal, copy) clicked
        let mut clicked = (false, false);
        toolbar_chip(trailing_ui, |ui| {
            let (reveal, copy) = in_reading_order(
                ui,
                &mut clicked,
                |clicked, ui| {
                    let response = chip_icon_button(ui, egui::Id::new("icon_btn_image_reveal"), egui_phosphor::light::FOLDER_OPEN)
                        .on_hover_text("Open Containing Folder");
                    clicked.0 = response.clicked();
                    response.rect
                },
                |clicked, ui| {
                    let response = chip_icon_button(ui, egui::Id::new("icon_btn_image_copy_path"), egui_phosphor::light::COPY)
                        .on_hover_text("Copy Path");
                    clicked.1 = response.clicked();
                    response.rect
                },
            );
            reveal.union(copy)
        });

        // Vertically centered in the row (which is top-aligned -- see
        // `render_item_toolbar`), the same problem the Tab toolbar's
        // favicon slot solves; width capped so a long filename truncates
        // instead of pushing the zoom chip off the row.
        let identity = format!("{} · {}×{}", download_filename(&path), image.x as u32, image.y as u32);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width().min(360.0), ui.available_height()),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.add(egui::Label::new(identity).truncate()).on_hover_text(&path),
        );

        let (scale, _) = self.image_view.effective(image, self.item_body_rect().size());
        if let Some(action) = render_zoom_chip(
            ui,
            scale * 100.0,
            scale > IMAGE_ZOOM_MIN + 1e-3,
            scale < IMAGE_ZOOM_MAX - 1e-3,
            "Fit to window",
        ) {
            self.apply_image_zoom_action(action);
        }

        if clicked.0 {
            self.reveal_image(image_id);
        }
        if clicked.1 {
            ui.ctx().copy_text(path);
        }
        ItemToolbarActions::default()
    }

    /// Zoom-chip click or its keyboard equivalent, applied to the Image
    /// currently in Item view. A no-op outside Image view.
    fn apply_image_zoom_action(&mut self, action: ZoomChipAction) {
        let Some(image) = self
            .viewed_image
            .as_ref()
            .and_then(|id| self.nodes.get(id))
            .and_then(|n| n.as_image())
            .map(image_pixel_size)
        else {
            return;
        };
        self.image_view = image_view_after_action(self.image_view, action, image, self.item_body_rect().size());
    }

    /// Image view: the shared Item toolbar plus the image itself, zoomable
    /// (wheel/pinch about the cursor) and pannable (drag) -- see
    /// `ImageViewState`. Leaves to Graph if the image node is gone.
    fn build_image_view_ui(&mut self, ui: &mut egui::Ui, image_id: &str) {
        let Some(image) = self.nodes.get(image_id).and_then(|n| n.as_image()).map(image_pixel_size) else {
            self.viewed_image = None;
            self.view_mode = ViewMode::Graph;
            self.save_session();
            return;
        };
        let frame = self.render_item_toolbar(image_id, ui);
        self.build_shortcuts_help(ui, frame.shortcuts_help_button_rect);
        self.build_save_toast(ui);

        let body = self.item_body_rect();
        let response = ui.interact(body, egui::Id::new("image_view_body"), egui::Sense::drag());
        let (mut scale, mut pan) = self.image_view.effective(image, body.size());
        let mut changed = false;
        if response.hovered() {
            let factor = ui.input(|i| i.zoom_delta() * (i.smooth_scroll_delta.y * IMAGE_WHEEL_ZOOM_RATE).exp());
            if factor != 1.0 {
                let anchor = ui
                    .input(|i| i.pointer.hover_pos())
                    .map_or(egui::Vec2::ZERO, |p| p - body.center());
                (scale, pan) = zoom_about(scale, pan, factor, anchor);
                changed = true;
            }
        }
        if response.dragged() {
            pan += response.drag_delta();
            changed = true;
        }
        if changed {
            pan = clamp_image_pan(pan, image * scale, body.size());
            self.image_view = ImageViewState { zoom: ImageZoom::Scale(scale), pan };
        }

        let painter = ui.painter_at(body);
        painter.rect_filled(body, 0.0, BG);
        let rect = image_display_rect(body, image, scale, pan);
        match self.gpu.as_ref().and_then(|g| g.image_id(image_id)) {
            Some(tex_id) => {
                painter.image(
                    tex_id,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            // Same placeholder the graph card uses for a missing texture.
            None => {
                painter.text(
                    body.center(),
                    egui::Align2::CENTER_CENTER,
                    egui_phosphor::light::IMAGE_BROKEN,
                    egui::FontId::proportional(48.0),
                    ui.visuals().weak_text_color(),
                );
            }
        }

        let escape_pressed = (ui.input(|i| i.key_pressed(egui::Key::Escape))
            || std::mem::take(&mut self.pending_debug_escape))
            && self.quick_switcher.is_none();
        if frame.back_to_graph || escape_pressed {
            self.leave_to_graph();
        }
    }
```

If a borrow-checker error arises from `ui.input` inside the closures, restructure minimally (e.g. read inputs into locals first) without changing behavior.

- [ ] **Step 8: Open an Image by clicking its card**

In `build_graph_ui`, add `let mut image_clicked: Option<String> = None;` next to `image_close_requested`. In the image card loop, right after `self.finish_draggable_node(&id, &response, is_hovered);`, add:

```rust
                    // Click (not drag -- egui doesn't report `clicked()` for
                    // a drag) maximizes the image into Item view, the same
                    // gesture a Tab card uses.
                    if response.clicked() {
                        image_clicked = Some(id.clone());
                    }
```

and next to the `clicked_focus` handling at the end of `build_graph_ui`:

```rust
        if let Some(id) = image_clicked {
            if self.nodes.contains_key(&id) {
                self.selected_tabs.clear();
                self.focus_item(&id);
            }
        }
```

Make sure the close button's click doesn't also open the view: if `draw_node_close_button` returned true this frame, skip setting `image_clicked` (reorder so the close check sets `image_close_requested` and the open check is `if response.clicked() && image_close_requested.as_deref() != Some(id.as_str())`).

- [ ] **Step 9: Build, test, live-verify**

Run: `cargo build -p manifold` (zero warnings), `cargo test -p manifold`.
Live (`target/verify/task4/`): generate the two test PNGs (Global Constraints), `ADDIMAGE` both, screenshot Graph to find the cards, `CLICK` the 1920×1080 card → screenshot. Check: toolbar reads `[Graph] [name · 1920×1080] [− NN% + Fit] … [folder copy] [? gear]` with every element vertically aligned; image is contained and centered; readout shows the Fit %. `CLICK` +, − , `%`, Fit and screenshot each; `SCROLL` over the image (zooms about the cursor); `MOUSEDOWN`/`MOUSEMOVE`/`MOUSEUP` drag (pans, no gap past edges); `ESCAPE` → Graph. Repeat entry with the 64×64 image: shown at 100%, centered. Then enter the Tab view (click the tab node) and confirm the Tab toolbar still matches `target/verify/baseline/tab.png`; then open an image and press the Graph button; then `NEXTTAB` from Image view lands in Tab view with the page visible.

- [ ] **Step 10: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "Add Image view: click an image card to maximize it

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Animated Image enter/leave

**Files:**
- Modify: `crates/manifold/src/main.rs` (`transition_content_rect_points`, `leave_to_graph`, `start_transition_to_item`, `finish_transition_if_done`, `build_ui`, image card click)

**Interfaces:**
- Consumes: Task 4's `screen_size_points`, `item_body_rect`, `focus_item`, `ImageViewState::effective`, `image_display_rect`, `image_pixel_size`, `viewed_image`.
- Produces: `fn graph_world_rect_to_screen(&self, world: egui::Rect) -> egui::Rect`, free fn `transition_eased_rect(transition: &Transition, item_rect: egui::Rect, graph_rect: egui::Rect) -> egui::Rect`, `fn image_transition_frame(&self, transition: &Transition) -> Option<(egui::TextureId, egui::Rect)>`.

- [ ] **Step 1: Factor the shared transition geometry**

Add a free function near `Transition`:

```rust
/// A `Transition`'s current rect: smoothstep-eased between the Item-view
/// end (`item_rect`) and the graph-card end (`graph_rect`), in whichever
/// direction `transition.kind` runs.
fn transition_eased_rect(transition: &Transition, item_rect: egui::Rect, graph_rect: egui::Rect) -> egui::Rect {
    let (from, to) = match transition.kind {
        TransitionKind::ToGraph => (item_rect, graph_rect),
        TransitionKind::ToItem => (graph_rect, item_rect),
    };
    let t = (transition.start.elapsed().as_secs_f32() / TRANSITION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    // Smoothstep: eases in and out, rather than the constant-velocity
    // (and slightly mechanical-looking) feel of plain linear `lerp`.
    let eased = t * t * (3.0 - 2.0 * t);
    egui::Rect::from_min_max(from.min + (to.min - from.min) * eased, from.max + (to.max - from.max) * eased)
}
```

Add to `impl App` (next to `graph_world_to_screen_transform`):

```rust
    /// Where a world-space rect lands on screen in Graph view right now --
    /// the same projection `Scene` applies, computed outside it (see
    /// `graph_world_to_screen_transform`).
    fn graph_world_rect_to_screen(&self, world: egui::Rect) -> egui::Rect {
        let screen = self.screen_size_points();
        let scene_outer_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, GRAPH_TOOLBAR_HEIGHT_POINTS),
            egui::vec2(screen.x, (screen.y - GRAPH_TOOLBAR_HEIGHT_POINTS).max(0.0)),
        );
        let (scale, translation) = self.graph_world_to_screen_transform(scene_outer_rect);
        egui::Rect::from_min_max(
            (translation + scale * world.min.to_vec2()).to_pos2(),
            (translation + scale * world.max.to_vec2()).to_pos2(),
        )
    }
```

Rewrite `transition_content_rect_points`'s body to use them (same result as today — keep its doc comment):

```rust
        let screen = self.screen_size_points();
        let full_content_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, CHROME_HEIGHT_POINTS),
            egui::vec2(screen.x, (screen.y - CHROME_HEIGHT_POINTS).max(0.0)),
        );
        let node_rect = self
            .nodes
            .get(&transition.node_id)
            .map(|node| {
                self.graph_world_rect_to_screen(
                    egui::Rect::from_center_size(egui::pos2(node.pos.x, node.pos.y), GRAPH_NODE_SIZE)
                        .shrink(GRAPH_THUMB_MARGIN),
                )
            })
            .unwrap_or(full_content_rect);
        transition_eased_rect(transition, full_content_rect, node_rect)
```

- [ ] **Step 2: The Image transition frame**

```rust
    /// Image counterpart of `transition_content_rect_points`: the image's
    /// texture and where to paint it this frame, between its graph card
    /// and its current Image-view rect (Fit on entry; whatever zoom/pan
    /// the user left on exit), so the aspect ratio is right throughout.
    /// `None` for a non-Image transition or a missing node/texture.
    fn image_transition_frame(&self, transition: &Transition) -> Option<(egui::TextureId, egui::Rect)> {
        let node = self.nodes.get(&transition.node_id)?;
        let image = node.as_image()?;
        let tex_id = self.gpu.as_ref()?.image_id(&transition.node_id)?;
        let size = image_pixel_size(image);
        let card = self.graph_world_rect_to_screen(egui::Rect::from_center_size(
            egui::pos2(node.pos.x, node.pos.y),
            image_node_card_size(image.width, image.height),
        ));
        let body = self.item_body_rect();
        let (scale, pan) = self.image_view.effective(size, body.size());
        let view = image_display_rect(body, size, scale, pan);
        Some((tex_id, transition_eased_rect(transition, view, card)))
    }
```

In `build_ui`'s transition branch, after `self.build_graph_ui(ui);` and before `return;`:

```rust
            // A Tab's transition quad is a GPU pane (`content_panes_pts`);
            // an Image's is painted here, above the graph backdrop.
            if let Some((tex_id, rect)) = self.transition.as_ref().and_then(|t| self.image_transition_frame(t)) {
                ui.ctx()
                    .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("image_transition")))
                    .image(
                        tex_id,
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
            }
```

- [ ] **Step 3: Animate entry and exit**

- `start_transition_to_item`: call `self.focus_item(id);` instead of `self.focus_tab(id);`, and update its doc comment's first line to "Graph view's node-click equivalent of `focus_item`". Leave the rest.
- Image card click (Task 4 Step 8): call `self.start_transition_to_item(&id);` instead of `self.focus_item(&id);`.
- `leave_to_graph`: delete Task 4's instant `if self.viewed_image.take().is_some() { … return; }` block, and change the existing `match self.focused.clone()` so the transition targets the image when one is shown:

```rust
        match self.viewed_image.clone().or_else(|| self.focused.clone()) {
            Some(id) => {
                self.transition = Some(Transition {
                    node_id: id,
                    kind: TransitionKind::ToGraph,
                    start: std::time::Instant::now(),
                });
            }
```

(the `None =>` arm stays as is.)
- `finish_transition_if_done`'s `ToGraph` arm: add `self.viewed_image = None;` after `self.view_mode = ViewMode::Graph;`.

- [ ] **Step 4: Build, test, live-verify**

Run: `cargo build -p manifold` (zero warnings), `cargo test -p manifold`.
Live (`target/verify/task5/`): add the 1920×1080 image, `CLICK` its card and immediately `SCREENSHOT` (mid-transition: the image is between card and full view, correct aspect ratio, graph visible behind), then after ~300ms screenshot (settled Image view). Zoom in once, `ESCAPE`, screenshot immediately (shrinking from the zoomed rect toward the card) and after settle (Graph, card intact, no leftover image). Repeat a tab-node click → Tab transition still animates as before. Double-click an image card quickly → no glitch (second click ignored during the transition).

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "Animate Image view enter/leave like Tab view

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Session, window title, zoom shortcuts, docs

**Files:**
- Modify: `crates/manifold/src/session.rs` (`Session`)
- Modify: `crates/manifold/src/main.rs` (session snapshot + `apply_session`, `update_window_title`, `zoom_in_shortcut`/`zoom_out_shortcut`/`zoom_reset_shortcut`)
- Modify: `README.md`

**Interfaces:**
- Consumes: `viewed_image`, `showing_image_view`, `showing_tab_view`, `apply_image_zoom_action` (Task 4); `ZoomChipAction` (Task 1).
- Produces: `session::Session::viewed_image: Option<String>`.

- [ ] **Step 1: Session field**

In `session.rs`, after `pub view_mode_tab: bool,`:

```rust
    /// The Image node Item view was showing (`App::viewed_image`), if any
    /// -- `focused` stays tab-only. `#[serde(default)]` so older files
    /// still load.
    #[serde(default)]
    pub viewed_image: Option<String>,
```

Add `viewed_image: self.viewed_image.clone(),` to the `session::Session { … }` snapshot literal in `main.rs` (after `view_mode_tab`). Fix any other `Session { … }` literal the compiler reports.

- [ ] **Step 2: Restore**

In `apply_session`, replace

```rust
        self.view_mode = if session.view_mode_tab { ViewMode::Item } else { ViewMode::Graph };
        if matches!(self.view_mode, ViewMode::Item) {
```

with

```rust
        self.view_mode = if session.view_mode_tab { ViewMode::Item } else { ViewMode::Graph };
        // Images were inserted above, so a still-valid id is already a node.
        self.viewed_image = session
            .viewed_image
            .clone()
            .filter(|_| self.view_mode == ViewMode::Item)
            .filter(|id| self.nodes.get(id).and_then(|n| n.as_image()).is_some());
        if self.showing_tab_view() {
```

(the comment and body inside that `if` are unchanged). An image that no longer exists falls back to the focused Tab, as before.

- [ ] **Step 3: Window title**

In `update_window_title`, compute the image title first and fall back to the existing chain:

```rust
        let image_title = self
            .viewed_image
            .as_ref()
            .filter(|_| self.view_mode == ViewMode::Item)
            .and_then(|id| self.nodes.get(id))
            .and_then(|n| n.as_image())
            .map(|i| download_filename(&i.path).to_owned());
        let title = image_title
            .or_else(|| {
                self.focused
                    .as_ref()
                    .and_then(|id| self.nodes.get(id))
                    .and_then(|n| n.as_tab())
                    .map(|n| if n.title.is_empty() { n.url.clone() } else { n.title.clone() })
            })
            .filter(|t| !t.is_empty())
            .map(|t| format!("{t} — manifold"))
            .unwrap_or_else(|| "manifold".to_string());
```

Add one sentence to its doc comment: "In Image view, the image's filename instead."

- [ ] **Step 4: Zoom shortcuts drive the image in Image view**

At the top of each of `zoom_in_shortcut`, `zoom_out_shortcut`, `zoom_reset_shortcut`:

```rust
        if self.showing_image_view() {
            self.apply_image_zoom_action(ZoomChipAction::In); // Out / Reset100 respectively
            return;
        }
```

Update each doc comment: "In Image view, zooms the image instead (same step as its zoom chip)."

- [ ] **Step 5: README**

- In the "Headless mode" command list, change `ESCAPE` to: "dismisses the suggestion dropdown, or leaves Item view (a maximized tab or image) for Graph view if none is showing." and `ZOOMIN`/`ZOOMOUT`/`ZOOMRESET` if listed: "(zooms the image in Image view)".
- In the "Running" section, before `## Headless mode`, add a paragraph:

```markdown
**Image view**: clicking an image card maximizes it into Item view (the same animated expand/shrink a tab card uses; Escape or the "Graph" button returns). The toolbar is the shared Item toolbar frame -- "Graph" at the left, the Help/Settings chip at the right -- with the image's own content between: its filename and pixel size, a zoom chip (−, percentage, +, Fit; the same chip the Graph toolbar uses), and Open Containing Folder / Copy Path. The image opens at Fit (contained, never above 100%); the mouse wheel zooms about the cursor (10%–1600%), dragging pans (never past the image's edges), and `Ctrl+=`/`Ctrl+-`/`Ctrl+0` zoom it. A maximized image is restored on relaunch (at Fit). Tab-only shortcuts (Ctrl+W, F5, …) keep acting on the last focused tab, as they do from Graph view.
```

- [ ] **Step 6: Build, test, live-verify**

Run: `cargo build -p manifold` (zero warnings), `cargo test -p manifold`.
Live (`target/verify/task6/`), with one scratch APPDATA reused across two launches: launch, add the 1920×1080 image, open it, `ZOOMIN` twice (readout goes Fit%×1.25²), `ZOOMRESET` (100%), `QUIT`. Inspect `target/verify/appdata/manifold/session.json`: `view_mode_tab: true`, `viewed_image` = the image id, `focused` = a tab id. Relaunch with the same APPDATA → screenshot: Image view restored at Fit. `ESCAPE` → Graph → `QUIT`; session now has `view_mode_tab: false`. Check the window title isn't testable headless — instead confirm by code review that `update_window_title` runs every frame (it does, before `build_ui`).

- [ ] **Step 7: Commit**

```bash
git add crates/manifold/src/main.rs crates/manifold/src/session.rs README.md
git commit -m "Persist Image view; route zoom shortcuts and title to it

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Final verification

**Files:** none modified unless a check fails (then fix in the smallest relevant place and note it in the commit).

- [ ] **Step 1: Static checks**

Run: `cargo build -p manifold` → zero warnings. `cargo test -p manifold` → all pass (11: the prior 5 plus 6 added by this plan). `grep -n "ViewMode::Tab\|build_tab_ui\|\.as_tab()\.unwrap()" crates/manifold/src/main.rs` → only `.as_tab().unwrap()` inside `render_tab_toolbar_content` (reached only for a Tab by dispatch) may remain.

- [ ] **Step 2: Smoke test**

Run: `APPDATA` isolated as in Global Constraints, then `bash scripts/headless-smoke-test.sh` → exits 0.

- [ ] **Step 3: End-to-end live pass** (`target/verify/task7/`)

One headless session covering: Graph zoom chip (−, +, %, Fit); open a tab (Tab toolbar identical to `baseline/tab.png` in element positions; Help popup anchors); back to Graph; add both images; open each (animated, correct toolbar, Fit correct for large and small); zoom chip, wheel, drag; `NEXTTAB` from Image view → Tab view; `ESCAPE`; `TOGGLEVIEW` from Graph goes to the focused *tab* (not the image) — expected per the spec. Record each check as PASS/FAIL with its screenshot path in the report.

- [ ] **Step 4: Commit (only if a fix was needed)**

```bash
git add -A crates/manifold/src/main.rs
git commit -m "Fix <what> found in final verification

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
