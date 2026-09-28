# JSON Node Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Double-clicking a Json node enters its level, where every non-empty object or array is a record card in a tree layout, with an outline panel on the right, search, copy, and saved view overrides.

**Architecture:** A behaviour-preserving refactor first puts a *level source* seam (`level.rs`) under the shared graph chrome (fit, minimap, LOD count, settle key, right-click hit test, `EXPORTSTATE` cards, edges, layout). A JSON level then becomes the seam's second source: `App::json_level: Option<JsonLevel>` holds cards computed from the node's value and its saved `VIEW` overrides. It never enters `self.nodes`, so no all-node walk can see it. The pure model (`json_view.rs`) holds pointers, labels, visibility, layout, search and outline rows. `json_level.rs` holds the App-bound state, and `json_level_ui.rs` handles painting and input.

**Tech Stack:** Rust, egui 0.36 (`Scene`, `Panel`, `ScrollArea::show_rows`), serde_json (with `preserve_order`), the existing store (`store/model.rs` field table, `session_sync.rs`).

**Spec:** `docs/superpowers/specs/2026-09-27-json-node-rework-design.md`

## Global Constraints

- Work on `dev`, never `main`. Never push. Never `git stash`. Don't touch `.claude/worktrees/great-easley-621017`.
- Before each commit, run `cargo fmt --all`. `cargo fmt --all --check` and `cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings` must pass. Never use `--no-verify`. An `#[allow]` needs a comment saying why.
- Build and test with `-j 12`, for example `cargo test -p manifold -j 12 <filter>`.
- No test may read or write the real `%APPDATA%\manifold`. Tests use `App::default()` (where `profile::dir()` is `None`) and `store::testutil::temp_dir`.
- End commit messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Run Python only as `python -P script.py < /dev/null`, never `python -` or `python -P -`.
- Run headless code only through `scripts/drive.sh`, `scripts/headless-smoke-test.sh`, `scripts/two-instance-test.sh` or `scripts/scale-test.sh`, each via `rtk proxy bash <script>`. Build (`cargo build -p manifold -j 12`) before running them, because they run the existing exe. Kill only PIDs you started.
- Card geometry (spec): width 220; header 26; 22 per row; columns at `x = depth × (220 + 60)`; 16 between siblings; 20 rows per page; default expansion up to ~150 visible cards or depth 3; rows turn into gray bars below zoom 0.5.
- The label rule, verbatim from the spec: "the first of `name`, `title`, `label`, `id` or `key` whose value is a scalar, otherwise the first string of 40 characters or fewer. An array's label is an inline preview such as `[-0.12, 51.5]`, of 30 characters or fewer."
- `VIEW = 14` is a Str field that is diffed, not explicit. It holds compact JSON keyed by pointer, with `pos`, `collapsed`, `color` and `pages`. An empty view is stored as `""`.
- A card's id is `<json node id>#<pointer>`. Its position is its top-left corner in level coordinates, with the root card at the origin.
- Read-only: nothing on a JSON level may change `value`, and nothing about it may enter `self.nodes`.

## Rulings made while planning

1. **Level-source seam, staged, refactor first** (the user's choice, 2026-09-27). Task 1 changes no behaviour, and JSON is the second source from Task 5 on.
2. **`preserve_order`.** Turn on serde_json's `preserve_order` feature so keys show in file order. `indexmap` is already in `Cargo.lock`, so no new crate comes in. This changes key order wherever the app serializes a `serde_json::Map`. Task 2 runs the whole test suite to catch any test that depended on alphabetical order.
3. **`insertion_group()`.** Every creation site that used `current_group` calls this instead. On a JSON level it returns the Json node's own group. It is based on `current_group` naming a file-backed Json node, not on `json_level` being built, so it is right even before the first frame.
4. **Colour** is stored as `0x00RRGGBB`, the same way `session_sync::color_value` stores a node's colour, and picked with the existing `color_swatch_row`. The spec's example `"color": 3` becomes an RGB number. Task 11 updates the spec.
5. **Default expansion is a function of the value alone** (`default_expanded`), computed once per value. A card's collapsed state is its override if there is one, else the default. Expanding one card therefore never collapses another by using up the budget.
6. **Only non-empty containers get cards.** `{}` and `[]` render inline as rows. The root is always a card: a scalar root is a header showing the value.
7. **Null never labels:** a preferred key whose value is `null` is skipped. An array preview gives up (`None`) as soon as it meets a nested container or passes 30 characters.
8. **The outline shows every entry of an open container, without paging.** `show_rows` virtualizes it. During a search it shows exactly the matches and their ancestors, all open.
9. **Search matches object keys and scalar values**, not array indices, which would match every `0`.
10. **Entering a Json node whose file failed to load** (`error` is set) is not allowed. Double-click does nothing on the error card, and Retry stays as it is.
11. **Text** is painted through `deferred_overlays` at `12 × zoom` screen points, which works out to 12 world units. Each overlay closure derives the scale from `project` itself, so the text size is never a frame stale. The font is proportional (the app's font), and icons come from `egui_phosphor`, following the existing tofu-box rule.
12. **`graph_world_rect_to_screen`** subtracts the outline's width while a JSON level shows it. Otherwise `EXPORTSTATE` screen points would be off by the panel's width.
13. **Keyboard:** egui-winit turns Ctrl+C and Ctrl+Shift+C into `egui::Event::Copy` with no `Key` event (verified in egui-winit 0.36.2 `lib.rs`). The copy shortcut therefore reads `Event::Copy` and uses `modifiers.shift` to tell the two apart. JSON-level keys are skipped while `egui_wants_keyboard_input()` is true, for example when the search box has focus.
14. **Headless root pointer:** `JSONSELECT`, `JSONTOGGLE` and `JSONMORE` accept an empty argument, meaning the root (`""`).

## File map

| File | Change |
|---|---|
| `crates/manifold/src/level.rs` (new) | `LevelItem`, `LevelItemKind`, `LevelEdge`, `App::level_items`, `level_edges`, `layout_current_level`, `insertion_group` |
| `crates/manifold/src/json_view.rs` (new) | The pure model: constants, `CardOverride`, `JsonView`, pointers, text, labels, `Card`, `default_expanded`, `visible_cards`, `toggle`, `show_more`, `reveal`, `tree_layout`, `search`, `outline_rows`, `card_title`, `preview_size` |
| `crates/manifold/src/json_level.rs` (new) | `JsonLevel`, `Outline`, `Search`, `App::sync_json_level`, `edit_json_view`, `apply_json_edit`, headless handlers |
| `crates/manifold/src/json_level_ui.rs` (new) | The painter and palette, `draw_json_level`, the top-bar items, the context menu, keys, the outline panel |
| `crates/manifold/Cargo.toml` | `serde_json = { version = "1", features = ["preserve_order"] }` |
| `main.rs` | `mod` lines, `App` fields, `fit_all_nodes_scene_rect`/`bounds_for_rects`, `current_level_settle_key`, creation sites → `insertion_group`, removals |
| `graph_ui.rs` | build loop through the seam, minimap, hit test, edges, toolbar gating, Json double-click → `EnterGroup` |
| `card.rs` | Json card size and preview body |
| `node.rs` | `JsonPayload.view` |
| `store/model.rs` | `field::VIEW = 14` |
| `session_sync.rs` | `kind_fields`, `shared_nodes`, `materialize`, restore |
| `local_state.rs` | outline visibility and width, the path form |
| `debug_control.rs`, `event_loop.rs` | `JSON*` commands, the `EXPORTSTATE` json section |
| `popups.rs`, `tab_view.rs` | removals |
| `scripts/headless-smoke-test.sh`, `scripts/json-level-probe.txt` (new) | headless coverage |
| `README.md`, the spec | docs |

---

### Task 1: The level-source seam (behaviour-preserving)

**Files:**
- Create: `crates/manifold/src/level.rs`
- Modify: `crates/manifold/src/main.rs` (the `mod` list; `fit_all_nodes_scene_rect` ~3262, `bounds_for_level` ~3273, `current_level_settle_key` ~3326)
- Modify: `crates/manifold/src/graph_ui.rs` (`count_visible_nodes` ~97, `switch_group_level` ~416, `build_graph_ui` ~427, `handle_rect_select` ~1045, `draw_graph_minimap` ~1146, `draw_graph_edges` ~1337)
- Modify: `crates/manifold/src/event_loop.rs` (`DebugCommand::ExportState`, ~827)

**Interfaces:**
- Produces:
  - `pub(crate) enum LevelItemKind { Node(ItemNodeKind) }` (Task 5 adds `JsonCard`)
  - `pub(crate) struct LevelItem { id: String, kind: LevelItemKind, rect: egui::Rect }`
  - `pub(crate) enum EdgeCurve { Bow, Horizontal }` and `pub(crate) struct LevelEdge { from: egui::Pos2, to: egui::Pos2, curve: EdgeCurve, highlight: bool }`
  - `App::level_items(&self) -> Vec<LevelItem>`, `App::level_edges(&self) -> Vec<LevelEdge>` and `App::layout_current_level(&mut self)`
  - `App::bounds_for_rects(&self, rects: impl IntoIterator<Item = egui::Rect>) -> egui::Rect`

- [ ] **Step 1: Write the failing test**

Create `crates/manifold/src/level.rs` holding only the test module below, and add `mod level;` to `main.rs`'s alphabetical `mod` list, between `mod json_file;` and `mod local_state;`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn download(path: &str) -> NodePayload {
        NodePayload::Download(DownloadPayload {
            path: path.to_string(),
            bytes_received: 0,
            total_bytes: None,
            completed: true,
        })
    }

    fn group(name: &str) -> NodePayload {
        NodePayload::Group(GroupPayload { name: name.to_string(), from_json: false })
    }

    #[test]
    fn level_items_list_the_current_level_bottom_first_in_draw_order() {
        let mut app = App::default();
        app.insert_placeholder_node("g".into(), egui::vec2(0.0, 0.0), group("G"));
        app.insert_placeholder_node("d".into(), egui::vec2(300.0, 0.0), download("a.bin"));
        app.insert_placeholder_node("inner".into(), egui::vec2(0.0, 0.0), download("b.bin"));
        app.nodes.get_mut("inner").unwrap().group = Some("g".into());

        let top: Vec<(String, LevelItemKind)> = app.level_items().into_iter().map(|i| (i.id, i.kind)).collect();
        // Downloads draw before groups (`GRAPH_DRAW_ORDER`), whatever the insertion order.
        assert_eq!(
            top,
            [
                ("d".to_string(), LevelItemKind::Node(ItemNodeKind::Download)),
                ("g".to_string(), LevelItemKind::Node(ItemNodeKind::Group)),
            ]
        );
        assert_eq!(app.level_items()[0].rect, node_card_rect(&app.nodes["d"]));

        app.current_group = Some("g".into());
        let inside: Vec<String> = app.level_items().into_iter().map(|i| i.id).collect();
        assert_eq!(inside, ["inner"]);
    }

    #[test]
    fn level_edges_join_parent_and_child_centers_on_this_level_only() {
        let mut app = App::default();
        app.insert_placeholder_node("p".into(), egui::vec2(0.0, 0.0), download("p.bin"));
        app.insert_placeholder_node("c".into(), egui::vec2(100.0, 50.0), download("c.bin"));
        app.insert_placeholder_node("elsewhere".into(), egui::vec2(9.0, 9.0), download("e.bin"));
        app.nodes.get_mut("c").unwrap().parent = Some("p".into());
        app.nodes.get_mut("elsewhere").unwrap().parent = Some("p".into());
        app.nodes.get_mut("elsewhere").unwrap().group = Some("other".into());

        let edges = app.level_edges();
        assert_eq!(edges.len(), 1);
        assert_eq!((edges[0].from, edges[0].to), (egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0)));
        assert_eq!(edges[0].curve, EdgeCurve::Bow);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p manifold -j 12 level::tests`
Expected: a compile error, because `level_items`, `LevelItemKind` and `level_edges` don't exist yet.

- [ ] **Step 3: Write `level.rs`**

Put this above the test module:

```rust
//! What a graph level contains, whatever supplies it (spec
//! `2026-09-27-json-node-rework-design.md`, "Level sources"). Shared
//! graph chrome (fit and Reset View, the minimap, the LOD visible count,
//! the settle key, the right-click hit test, `EXPORTSTATE`'s `cards`, the
//! edges and the layout) reads the level through here instead of walking
//! `self.nodes` and `current_group` itself.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LevelItemKind {
    /// A stored node's card.
    Node(ItemNodeKind),
}

/// One card on the current level, as the shared chrome sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LevelItem {
    pub(crate) id: String,
    pub(crate) kind: LevelItemKind,
    /// World space.
    pub(crate) rect: egui::Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeCurve {
    /// `draw_graph_edge`'s sideways bow, for stored nodes' `parent` links.
    Bow,
    /// Leaves and enters horizontally.
    Horizontal,
}

/// One edge on the current level, in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LevelEdge {
    pub(crate) from: egui::Pos2,
    pub(crate) to: egui::Pos2,
    pub(crate) curve: EdgeCurve,
    pub(crate) highlight: bool,
}

impl App {
    /// The current level's items, bottom-first in draw order: stored
    /// nodes kind by kind in `GRAPH_DRAW_ORDER`, and each kind in
    /// `node_order`.
    pub(crate) fn level_items(&self) -> Vec<LevelItem> {
        GRAPH_DRAW_ORDER
            .iter()
            .flat_map(|&kind| self.nodes_at_current_level(kind))
            .filter_map(|id| {
                let node = self.nodes.get(&id)?;
                Some(LevelItem { kind: LevelItemKind::Node(item_node_kind(&node.payload)), rect: node_card_rect(node), id })
            })
            .collect()
    }

    /// The current level's edges: each stored node's `parent` link whose
    /// two ends are both on this level.
    pub(crate) fn level_edges(&self) -> Vec<LevelEdge> {
        self.nodes
            .values()
            .filter(|node| node.group == self.current_group)
            .filter_map(|node| {
                let parent = self.nodes.get(node.parent.as_ref()?).filter(|p| p.group == self.current_group)?;
                Some(LevelEdge {
                    from: parent.pos.to_pos2(),
                    to: node.pos.to_pos2(),
                    curve: EdgeCurve::Bow,
                    highlight: false,
                })
            })
            .collect()
    }

    /// Lays out the current level with its source's own strategy: the
    /// force settle for stored nodes.
    pub(crate) fn layout_current_level(&mut self) {
        self.settle_current_level();
    }
}
```

- [ ] **Step 4: Route the shared chrome through the seam**

Make each of these changes. Behaviour must stay exactly the same.

1. `main.rs`: replace `fit_all_nodes_scene_rect` and `bounds_for_level` with the code below. Keep their doc comments, and add one line to `bounds_for_rects`'s saying it is the shared fold.

```rust
    fn fit_all_nodes_scene_rect(&self) -> egui::Rect {
        self.bounds_for_rects(self.level_items().into_iter().map(|item| item.rect))
    }

    fn bounds_for_level<'a>(&self, ids: impl IntoIterator<Item = &'a String>) -> egui::Rect {
        self.bounds_for_rects(ids.into_iter().filter_map(|id| self.nodes.get(id)).map(node_card_rect))
    }

    pub(crate) fn bounds_for_rects(&self, rects: impl IntoIterator<Item = egui::Rect>) -> egui::Rect {
        let bounds = rects.into_iter().fold(egui::Rect::NOTHING, egui::Rect::union);
        if !bounds.is_finite() {
            return default_graph_scene_rect();
        }
        bounds.expand(GRAPH_FIT_MARGIN)
    }
```

2. `main.rs` `current_level_settle_key`: change the body to `(self.current_group.clone(), self.level_items().len())`.

3. `graph_ui.rs` `switch_group_level`: change `self.settle_current_level();` to `self.layout_current_level();`. In `build_graph_ui`, change both remaining `self.settle_current_level();` calls to `self.layout_current_level();`: the `frame_restored` branch and the settle-key check.

4. `graph_ui.rs` `build_graph_ui`:
   - Add `let items = self.level_items();` right after the `at_level` line.
   - Replace the LOD count with `let lod_visible_count = items.iter().filter(|item| item.rect.intersects(self.camera.rect)).count();`.
   - Replace the settle key with `let current_key = (self.current_group.clone(), items.len());`, and update its comment: the key now comes from this frame's `items`.
   - Change the minimap call to `self.draw_graph_minimap(ui, &overlay_painter, scene_outer_rect, &items)`.
   - Delete `count_visible_nodes` and its doc comment, since nothing else calls it. Check with `grep -n count_visible_nodes crates/manifold/src/*.rs`.

5. `graph_ui.rs` `draw_graph_minimap`: take `items: &[LevelItem]` instead of `at_level`, and draw:

```rust
        let world_bounds = self.bounds_for_rects(items.iter().map(|item| item.rect));
        // ... unchanged down to the background fill ...
        for item in items {
            let LevelItemKind::Node(kind) = item.kind;
            let (radius, color) = match kind {
                ItemNodeKind::Tab | ItemNodeKind::Unloaded => (2.0, ui.visuals().text_color()),
                ItemNodeKind::Download => (2.0, ui.visuals().weak_text_color()),
                ItemNodeKind::Note => (2.0, NOTE_CARD_COLOR),
                ItemNodeKind::Image | ItemNodeKind::Pdf | ItemNodeKind::Json => (2.0, ACCENT),
                ItemNodeKind::Group => (3.0, ACCENT),
            };
            let failed = self
                .nodes
                .get(&item.id)
                .and_then(|n| n.as_unloaded())
                .is_some_and(|t| matches!(t.state, UnloadedState::Failed(_)));
            let color = if failed { DANGER_COLOR } else { color };
            overlay_painter.circle_filled(world_to_minimap(item.rect.center()), radius, color);
        }
```

A node's `pos` is its card's centre (`node_card_rect`), so `item.rect.center()` is the same point as before. Task 5 turns the `let LevelItemKind::Node(kind) = item.kind;` line into a `match`.

6. `graph_ui.rs` `handle_rect_select`: replace the `hit` computation and its `if let` with the code below. Keep the comment above it, reworded to say "reverse draw order, `level_items`".

```rust
                let hit = self.level_items().into_iter().rev().find(|item| item.rect.contains(world_pos));
                match hit {
                    Some(LevelItem { id, kind: LevelItemKind::Node(_), .. }) => {
                        self.node_menu = Some((id, pos));
                        self.rect_select_start = None;
                    }
                    None => {
                        self.node_menu = None;
                        self.rect_select_start = Some(pos);
                    }
                }
```

7. `graph_ui.rs` `draw_graph_edges`: iterate the seam, keeping the viewport cull:

```rust
    pub(crate) fn draw_graph_edges(&self, ui: &egui::Ui, inv_zoom: f32) {
        for edge in self.level_edges() {
            // (keep the existing viewport-culling comment)
            if !egui::Rect::from_two_pos(edge.from, edge.to).intersects(self.camera.rect) {
                continue;
            }
            let color = if edge.highlight { ACCENT } else { ui.visuals().weak_text_color() };
            let stroke = egui::Stroke::new(2.0 * inv_zoom, color);
            match edge.curve {
                EdgeCurve::Bow => draw_graph_edge(ui.painter(), edge.from, edge.to, stroke),
                EdgeCurve::Horizontal => {
                    let bend = ((edge.to.x - edge.from.x) / 2.0).max(20.0);
                    ui.painter().add(egui::epaint::CubicBezierShape::from_points_stroke(
                        [edge.from, edge.from + egui::vec2(bend, 0.0), edge.to - egui::vec2(bend, 0.0), edge.to],
                        false,
                        egui::Color32::TRANSPARENT,
                        stroke,
                    ));
                }
            }
        }
    }
```

   A horizontal edge's box is from `from` to `to`, and the bend stays inside that x range, so the cull still holds.

8. `event_loop.rs` `ExportState`: build `cards` from `self.level_items()`:

```rust
                let cards: Vec<serde_json::Value> = self
                    .level_items()
                    .into_iter()
                    .filter_map(|item| {
                        let screen = self.graph_world_rect_to_screen(item.rect);
                        let LevelItemKind::Node(kind) = item.kind;
                        let node = self.nodes.get(&item.id)?;
                        Some(serde_json::json!({
                            "id": item.id,
                            "kind": format!("{kind:?}"),
                            "group": node.group,
                            "parent": node.parent,
                            "color": node.color.is_some(),
                            "pinned": node.payload.pinned(),
                            "screen": [screen.center().x, screen.center().y],
                            "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                        }))
                    })
                    .collect();
```

`use super::*;` already brings in `main.rs`'s items. Because `level` is a child module of the crate root, add `use level::{EdgeCurve, LevelItem, LevelItemKind};` next to `main.rs`'s other `use` lines so that `graph_ui.rs` and `event_loop.rs` see them through their own `use super::*;`.

- [ ] **Step 5: Run the tests, clippy and the smoke test**

Run: `cargo test -p manifold -j 12`
Expected: every test passes, including the two new ones.

Run: `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`
Expected: no warnings.

Run: `cargo build -p manifold -j 12`, then `rtk proxy bash scripts/headless-smoke-test.sh`
Expected: the same passes as before this task. The group drill-in, drop and minimap steps exercise fit, the hit test and `EXPORTSTATE`.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/level.rs crates/manifold/src/main.rs crates/manifold/src/graph_ui.rs crates/manifold/src/event_loop.rs
git commit -m "Level sources: the graph chrome reads the level through level.rs

Fit, the minimap, the LOD count, the settle key, the right-click hit
test, EXPORTSTATE cards, edges and layout now go through level_items(),
level_edges() and layout_current_level(). No behaviour change; a JSON
level becomes the second source next.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `json_view.rs`, the model: overrides, pointers, text and labels

**Files:**
- Create: `crates/manifold/src/json_view.rs`
- Modify: `crates/manifold/src/main.rs` (add `mod json_view;` after `mod json_file;`)
- Modify: `crates/manifold/Cargo.toml` (line 75)

**Interfaces:**
- Produces (every item is `pub(crate)`):
  - constants: `CARD_WIDTH`, `HEADER_HEIGHT`, `ROW_HEIGHT`, `COLUMN_GAP`, `SIBLING_GAP`, `PAGE_SIZE`, `EXPANSION_BUDGET`, `EXPANSION_DEPTH`, `LABEL_KEYS`, `LABEL_MAX_CHARS`, `PREVIEW_MAX_CHARS`, `DISPLAY_MAX_CHARS`, `WORKER_SEARCH_NODES`
  - `struct CardOverride { pos: Option<[f32; 2]>, collapsed: Option<bool>, color: Option<u32>, pages: Option<u32> }`
  - `struct JsonView(pub(crate) BTreeMap<String, CardOverride>)`, with `from_field(&str) -> Self`, `to_field(&self) -> String`, `get(&self, &str) -> Option<&CardOverride>`, `entry(&mut self, &str) -> &mut CardOverride`, `pages(&self, &str) -> usize`, `retain_resolvable(&mut self, &Value)` and `clear_positions(&mut self)`
  - `child_pointer(parent: &str, key: &str) -> String`, `pointer_tokens(&str) -> Vec<String>`, `parent_pointer(&str) -> Option<&str>` and `last_key(&str) -> Option<String>`
  - `accessor_path(root: &Value, pointer: &str) -> String`
  - `scalar_text(&Value) -> Option<String>`, `copy_value_text(&Value) -> String` and `display_text(&Value) -> String`
  - `label(&Value) -> Option<String>`
  - `entry_count(&Value) -> usize`, `entries(&Value) -> Box<dyn Iterator<Item = (String, &Value)> + '_>`, `is_card(&Value) -> bool` and `entry_index(&Value, &str) -> Option<usize>`

- [ ] **Step 1: Turn on `preserve_order`, and check that nothing relied on alphabetical keys**

In `crates/manifold/Cargo.toml`, change `serde_json = "1"` to `serde_json = { version = "1", features = ["preserve_order"] }`.

Run: `cargo test -p manifold -j 12`
Expected: all pass. If a test fails because it expected sorted keys, it was asserting serde_json's old iteration order, not a product requirement. Fix the expectation, and say which test in the task report.

- [ ] **Step 2: Write the failing tests**

Create `crates/manifold/src/json_view.rs` with only this test module, and add `mod json_view;` to `main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn view_round_trips_and_stores_nothing_when_empty() {
        assert_eq!(JsonView::default().to_field(), "");
        assert_eq!(JsonView::from_field(""), JsonView::default());
        assert_eq!(JsonView::from_field("not json"), JsonView::default());

        let mut view = JsonView::default();
        view.entry("/users/0").pos = Some([520.0, 20.0]);
        view.entry("/users/0").collapsed = Some(true);
        view.entry("/users").pages = Some(2);
        view.entry("/empty"); // an override with nothing set is not stored
        let text = view.to_field();
        assert_eq!(text, r#"{"/users":{"pages":2},"/users/0":{"pos":[520.0,20.0],"collapsed":true}}"#);
        let back = JsonView::from_field(&text);
        assert_eq!(back.get("/users/0"), view.get("/users/0"));
        assert_eq!(back.pages("/users"), 2);
        assert_eq!(back.pages("/elsewhere"), 1);
    }

    #[test]
    fn stale_pointers_and_positions_are_dropped() {
        let root = json!({"a": [1, 2], "b": {"c": 3}});
        let mut view = JsonView::default();
        view.entry("/a").pages = Some(2);
        view.entry("/a/5").collapsed = Some(true);
        view.entry("/gone").pos = Some([1.0, 2.0]);
        view.entry("").pos = Some([0.0, 0.0]);
        view.retain_resolvable(&root);
        assert_eq!(view.0.keys().collect::<Vec<_>>(), ["", "/a"]);
        view.clear_positions();
        assert_eq!(view.0.keys().collect::<Vec<_>>(), ["/a"]);
    }

    #[test]
    fn pointers_escape_tilde_and_slash() {
        assert_eq!(child_pointer("", "users"), "/users");
        assert_eq!(child_pointer("/a", "x/y~z"), "/a/x~1y~0z");
        assert_eq!(pointer_tokens("/a/x~1y~0z"), ["a", "x/y~z"]);
        assert!(pointer_tokens("").is_empty());
        assert_eq!(parent_pointer("/a/b"), Some("/a"));
        assert_eq!(parent_pointer("/a"), Some(""));
        assert_eq!(parent_pointer(""), None);
        assert_eq!(last_key("/a/x~1y"), Some("x/y".to_string()));
        assert_eq!(last_key(""), None);
    }

    #[test]
    fn accessor_paths_quote_what_isnt_an_identifier() {
        let root = json!({"users": [{"address": {"city": "London"}}], "a b": {"he\"y": 1}, "x/y": [[5]]});
        assert_eq!(accessor_path(&root, "/users/0/address"), "users[0].address");
        assert_eq!(accessor_path(&root, "/a b/he\"y"), r#"["a b"]["he\"y"]"#);
        assert_eq!(accessor_path(&root, "/x~1y/0/0"), r#"["x/y"][0][0]"#);
        assert_eq!(accessor_path(&root, ""), "");
        assert_eq!(accessor_path(&json!([[1]]), "/0/0"), "[0][0]");
    }

    #[test]
    fn copy_and_display_text() {
        assert_eq!(copy_value_text(&json!("plain")), "plain");
        assert_eq!(copy_value_text(&json!(1.5)), "1.5");
        assert_eq!(copy_value_text(&json!(null)), "null");
        assert_eq!(copy_value_text(&json!({"a": 1})), "{\n  \"a\": 1\n}");
        assert_eq!(display_text(&json!("say \"hi\"")), r#""say \"hi\"""#);
        assert_eq!(display_text(&json!({"a": 1, "b": 2})), "{2}");
        assert_eq!(display_text(&json!([1, 2, 3])), "[3]");
        assert_eq!(display_text(&json!(true)), "true");
        let long = "x".repeat(DISPLAY_MAX_CHARS + 5);
        assert_eq!(display_text(&json!(long)).chars().count(), DISPLAY_MAX_CHARS + 3); // two quotes and "…"
    }

    #[test]
    fn labels_follow_the_spec_rule() {
        assert_eq!(label(&json!({"id": 7, "name": "Ada"})), Some("\"Ada\"".into()));
        assert_eq!(label(&json!({"key": "k", "title": "T"})), Some("\"T\"".into()));
        assert_eq!(label(&json!({"label": "L", "id": 1})), Some("\"L\"".into()));
        assert_eq!(label(&json!({"id": 1, "key": "k"})), Some("1".into()));
        assert_eq!(label(&json!({"key": "k"})), Some("\"k\"".into()));
        // Null and containers never label; the fallback takes the first short string.
        assert_eq!(label(&json!({"name": null, "id": {"x": 1}, "note": "short"})), Some("\"short\"".into()));
        assert_eq!(label(&json!({"long": "y".repeat(41), "n": 1})), None);
        assert_eq!(label(&json!({"s": "y".repeat(40)})), Some(format!("\"{}\"", "y".repeat(40))));
        assert_eq!(label(&json!([-0.12, 51.5])), Some("[-0.12, 51.5]".into()));
        assert_eq!(label(&json!([[1], 2])), None);
        assert_eq!(label(&json!(["a very long string indeed", "and more"])), None);
        assert_eq!(label(&json!([])), Some("[]".into()));
        assert_eq!(label(&json!("scalar")), None);
    }

    #[test]
    fn entries_in_file_order_with_indices_for_arrays() {
        let object = json!({"z": 1, "a": 2});
        let keys: Vec<String> = entries(&object).map(|(k, _)| k).collect();
        assert_eq!(keys, ["z", "a"]); // preserve_order: file order, not sorted
        let array = json!(["x", "y"]);
        assert_eq!(entries(&array).map(|(k, _)| k).collect::<Vec<_>>(), ["0", "1"]);
        assert_eq!(entry_index(&object, "a"), Some(1));
        assert_eq!(entry_index(&array, "1"), Some(1));
        assert_eq!(entry_index(&array, "2"), None);
        assert!(is_card(&json!({"a": 1})) && is_card(&json!([0])));
        assert!(!is_card(&json!({})) && !is_card(&json!([])) && !is_card(&json!(3)));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p manifold -j 12 json_view::tests`
Expected: compile errors for the missing items.

- [ ] **Step 4: Write the implementation above the test module**

```rust
//! The JSON level's pure model (spec
//! `docs/superpowers/specs/2026-09-27-json-node-rework-design.md`): the
//! saved view overrides (`VIEW`), JSON Pointers and accessor paths, the
//! text rows show, labels, which cards are visible, the tree layout,
//! search and the outline's rows. Nothing here touches `App`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A card's width in world units (100% zoom).
pub(crate) const CARD_WIDTH: f32 = 220.0;
pub(crate) const HEADER_HEIGHT: f32 = 26.0;
pub(crate) const ROW_HEIGHT: f32 = 22.0;
/// Between depth columns.
pub(crate) const COLUMN_GAP: f32 = 60.0;
/// Between sibling subtrees.
pub(crate) const SIBLING_GAP: f32 = 16.0;
/// Rows per page; a `+ N more` row reveals the next page.
pub(crate) const PAGE_SIZE: usize = 20;
/// Default expansion stops at about this many visible cards...
pub(crate) const EXPANSION_BUDGET: usize = 150;
/// ...and cards this deep start collapsed.
pub(crate) const EXPANSION_DEPTH: usize = 3;
/// Keys whose scalar value names an object, most preferred first.
pub(crate) const LABEL_KEYS: [&str; 5] = ["name", "title", "label", "id", "key"];
/// The longest string the label rule's fallback takes.
pub(crate) const LABEL_MAX_CHARS: usize = 40;
/// The longest inline preview that labels an array.
pub(crate) const PREVIEW_MAX_CHARS: usize = 30;
/// A string row shows at most this many characters; the tooltip has more.
pub(crate) const DISPLAY_MAX_CHARS: usize = 200;
/// A value with more nodes than this is searched on a worker thread.
pub(crate) const WORKER_SEARCH_NODES: usize = 100_000;

/// What the user changed about one card (spec "Saved view state"). Only
/// set fields are stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CardOverride {
    /// Top-left, in level coordinates (the root card at the origin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pos: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) collapsed: Option<bool>,
    /// `0x00RRGGBB`, as `session_sync::color_value` stores a node's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<u32>,
    /// Pages of rows revealed; one when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pages: Option<u32>,
}

impl CardOverride {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A Json node's view overrides by JSON Pointer: the `VIEW` field (14).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct JsonView(pub(crate) BTreeMap<String, CardOverride>);

impl JsonView {
    /// The stored field's text; empty or unreadable text is no overrides.
    pub(crate) fn from_field(text: &str) -> Self {
        if text.is_empty() {
            return Self::default();
        }
        serde_json::from_str(text).map(Self).unwrap_or_default()
    }

    /// Compact JSON of the non-empty overrides, or `""` when there are none.
    pub(crate) fn to_field(&self) -> String {
        let kept: BTreeMap<&String, &CardOverride> = self.0.iter().filter(|(_, o)| !o.is_empty()).collect();
        if kept.is_empty() {
            return String::new();
        }
        serde_json::to_string(&kept).unwrap_or_default()
    }

    pub(crate) fn get(&self, pointer: &str) -> Option<&CardOverride> {
        self.0.get(pointer)
    }

    pub(crate) fn entry(&mut self, pointer: &str) -> &mut CardOverride {
        self.0.entry(pointer.to_string()).or_default()
    }

    /// Pages revealed for the card at `pointer`, at least one.
    pub(crate) fn pages(&self, pointer: &str) -> usize {
        self.get(pointer).and_then(|o| o.pages).map_or(1, |pages| (pages as usize).max(1))
    }

    /// Drops overrides whose pointer no longer resolves in `root` (after
    /// a Refresh), and empty ones.
    pub(crate) fn retain_resolvable(&mut self, root: &Value) {
        self.0.retain(|pointer, o| !o.is_empty() && root.pointer(pointer).is_some());
    }

    /// Relayout: every saved position goes, the rest stays.
    pub(crate) fn clear_positions(&mut self) {
        for o in self.0.values_mut() {
            o.pos = None;
        }
        self.0.retain(|_, o| !o.is_empty());
    }
}

/// `parent`'s child `key`, escaping `~` and `/` (RFC 6901).
pub(crate) fn child_pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// `pointer`'s unescaped reference tokens; none for the root (`""`).
pub(crate) fn pointer_tokens(pointer: &str) -> Vec<String> {
    pointer.split('/').skip(1).map(|t| t.replace("~1", "/").replace("~0", "~")).collect()
}

/// The pointer one level up; `None` for the root.
pub(crate) fn parent_pointer(pointer: &str) -> Option<&str> {
    pointer.rfind('/').map(|i| &pointer[..i])
}

/// The last reference token, unescaped; `None` for the root.
pub(crate) fn last_key(pointer: &str) -> Option<String> {
    pointer_tokens(pointer).pop()
}

/// `pointer` as a script would write it: `users[0].address`, with
/// `["…"]` for keys that aren't identifiers. The root is `""`.
pub(crate) fn accessor_path(root: &Value, pointer: &str) -> String {
    let mut out = String::new();
    let mut at = Some(root);
    for token in pointer_tokens(pointer) {
        match at {
            Some(Value::Array(items)) => {
                out.push_str(&format!("[{token}]"));
                at = token.parse::<usize>().ok().and_then(|i| items.get(i));
            }
            _ => {
                if is_identifier(&token) {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(&token);
                } else {
                    out.push_str(&format!("[{}]", Value::String(token.clone())));
                }
                at = at.and_then(|v| v.get(&token));
            }
        }
    }
    out
}

fn is_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// A scalar's plain text, strings unquoted; `None` for a container.
pub(crate) fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// Copy value (spec "Copy"): a scalar's plain text, or a container's
/// pretty-printed JSON.
pub(crate) fn copy_value_text(value: &Value) -> String {
    scalar_text(value).unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default())
}

/// How a row shows `value`: JSON text (strings quoted and escaped, cut at
/// `DISPLAY_MAX_CHARS` with `…`), and `{n}` / `[n]` for a container.
pub(crate) fn display_text(value: &Value) -> String {
    match value {
        Value::Object(map) => format!("{{{}}}", map.len()),
        Value::Array(items) => format!("[{}]", items.len()),
        Value::String(s) if s.chars().count() > DISPLAY_MAX_CHARS => {
            format!("{}…", Value::String(s.chars().take(DISPLAY_MAX_CHARS).collect()))
        }
        scalar => scalar.to_string(),
    }
}

/// The label rule (spec "Cards"), as display text.
pub(crate) fn label(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            let preferred = LABEL_KEYS
                .iter()
                .filter_map(|key| map.get(*key))
                .find(|v| !v.is_null() && scalar_text(v).is_some());
            let fallback =
                || map.values().find(|v| v.as_str().is_some_and(|s| s.chars().count() <= LABEL_MAX_CHARS));
            preferred.or_else(fallback).map(display_text)
        }
        Value::Array(items) => array_preview(items),
        _ => None,
    }
}

/// `[a, b]` when every element is a scalar and it fits `PREVIEW_MAX_CHARS`.
fn array_preview(items: &[Value]) -> Option<String> {
    let mut text = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if item.is_array() || item.is_object() {
            return None;
        }
        if i > 0 {
            text.push_str(", ");
        }
        text.push_str(&display_text(item));
        if text.chars().count() > PREVIEW_MAX_CHARS {
            return None;
        }
    }
    text.push(']');
    (text.chars().count() <= PREVIEW_MAX_CHARS).then_some(text)
}

pub(crate) fn entry_count(value: &Value) -> usize {
    match value {
        Value::Object(map) => map.len(),
        Value::Array(items) => items.len(),
        _ => 0,
    }
}

/// A container's entries in order, array indices as keys.
pub(crate) fn entries(value: &Value) -> Box<dyn Iterator<Item = (String, &Value)> + '_> {
    match value {
        Value::Object(map) => Box::new(map.iter().map(|(k, v)| (k.clone(), v))),
        Value::Array(items) => Box::new(items.iter().enumerate().map(|(i, v)| (i.to_string(), v))),
        _ => Box::new(std::iter::empty()),
    }
}

/// Whether a child gets its own card: non-empty containers only (plan
/// ruling 6); `{}` and `[]` show inline.
pub(crate) fn is_card(value: &Value) -> bool {
    entry_count(value) > 0
}

/// Where the entry `token` sits among `value`'s entries.
pub(crate) fn entry_index(value: &Value, token: &str) -> Option<usize> {
    match value {
        Value::Object(map) => map.keys().position(|k| k == token),
        Value::Array(items) => token.parse::<usize>().ok().filter(|&i| i < items.len()),
        _ => None,
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p manifold -j 12 json_view::tests`
Expected: 7 passed. The module's items are still unused, so `cargo clippy` warns about dead code. Put `#![cfg_attr(not(test), allow(dead_code))] // Used from Task 5 on; removed in Task 10.` as the first line of `json_view.rs`. Task 10 removes it.

Run: `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/manifold/Cargo.toml Cargo.lock crates/manifold/src/json_view.rs crates/manifold/src/main.rs
git commit -m "json_view: view overrides, pointers, row text and labels

Also turns on serde_json's preserve_order, so a JSON level shows keys in
file order.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 3: `json_view.rs`: visible cards, tree layout, search and outline rows

**Files:**
- Modify: `crates/manifold/src/json_view.rs`

**Interfaces:**
- Consumes: the Task 2 items.
- Produces (every item is `pub(crate)`):
  - `PREVIEW_ROWS: usize = 4`
  - `struct Card { pointer: String, parent: Option<(usize, usize)>, depth: usize, collapsed: bool, rows: usize, more: usize }`, with `height(&self) -> f32` and `size(&self) -> egui::Vec2`
  - `default_expanded(root: &Value) -> HashSet<String>`
  - `is_collapsed(view: &JsonView, expanded: &HashSet<String>, pointer: &str) -> bool`
  - `visible_cards(root: &Value, view: &JsonView, expanded: &HashSet<String>) -> Vec<Card>`
  - `toggle(view: &mut JsonView, expanded: &HashSet<String>, pointer: &str)`
  - `show_more(view: &mut JsonView, pointer: &str)`
  - `reveal(root: &Value, view: &mut JsonView, expanded: &HashSet<String>, pointer: &str) -> bool`
  - `tree_layout(cards: &[Card], view: &JsonView) -> Vec<egui::Pos2>`: each card's top-left
  - `struct SearchResult { matches: Vec<String>, hits: HashSet<String>, keep: HashSet<String> }` and `search(root: &Value, query: &str) -> SearchResult`
  - `count_nodes(&Value) -> usize`
  - `struct OutlineRow { pointer: String, depth: usize }` and `outline_rows(root: &Value, open: &HashSet<String>, keep: Option<&HashSet<String>>) -> Vec<OutlineRow>`
  - `card_title(root: &Value, pointer: &str, root_name: &str) -> String`
  - `preview_size(value: &Value) -> egui::Vec2`

- [ ] **Step 1: Write the failing tests** (append them to `json_view.rs`'s test module)

```rust
    fn users(n: usize) -> Value {
        json!({
            "page": 1,
            "users": (0..n).map(|i| json!({"id": i, "name": format!("u{i}"), "roles": ["a", "b"]})).collect::<Vec<_>>(),
            "meta": {"bbox": [[-0.12, 51.5], [0.33, 51.7]], "empty": {}},
        })
    }

    fn pointers(cards: &[Card]) -> Vec<&str> {
        cards.iter().map(|c| c.pointer.as_str()).collect()
    }

    #[test]
    fn cards_page_objects_and_arrays_and_nest_arrays() {
        let root = users(45);
        let expanded = default_expanded(&root);
        let cards = visible_cards(&root, &JsonView::default(), &expanded);
        let users_card = cards.iter().find(|c| c.pointer == "/users").unwrap();
        assert_eq!((users_card.rows, users_card.more), (20, 25));
        // Only the first page's elements have cards; `{}` gets none.
        assert!(cards.iter().any(|c| c.pointer == "/users/19"));
        assert!(!cards.iter().any(|c| c.pointer == "/users/20"));
        assert!(!cards.iter().any(|c| c.pointer == "/meta/empty"));
        // A nested array is a card whose rows are arrays with cards of their own.
        assert!(cards.iter().any(|c| c.pointer == "/meta/bbox/0"));

        let mut view = JsonView::default();
        show_more(&mut view, "/users");
        let cards = visible_cards(&root, &view, &expanded);
        let users_card = cards.iter().find(|c| c.pointer == "/users").unwrap();
        assert_eq!((users_card.rows, users_card.more), (40, 5));
        assert_eq!(users_card.height(), HEADER_HEIGHT + ROW_HEIGHT * 41.0);

        let big_object: Value = (0..30).map(|i| (format!("k{i}"), json!(i))).collect::<serde_json::Map<_, _>>().into();
        let cards = visible_cards(&big_object, &JsonView::default(), &default_expanded(&big_object));
        assert_eq!((cards[0].rows, cards[0].more), (20, 10));
    }

    #[test]
    fn default_expansion_stops_at_the_depth_and_the_budget() {
        let deep = json!({"a": {"b": {"c": {"d": {"e": 1}}}}});
        let expanded = default_expanded(&deep);
        let cards = visible_cards(&deep, &JsonView::default(), &expanded);
        assert_eq!(pointers(&cards), ["", "/a", "/a/b", "/a/b/c"]);
        assert!(!cards[2].collapsed && cards[3].collapsed); // depth 3 starts collapsed
        assert_eq!(cards[3].height(), HEADER_HEIGHT);

        // 20 users of 1 + 1 cards each fit; the budget stops before 150.
        let root = users(20);
        let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
        assert!(cards.len() <= EXPANSION_BUDGET, "{} cards", cards.len());
        let root = json!((0..20).map(|_| json!((0..20).map(|_| json!({"x": [1]})).collect::<Vec<_>>())).collect::<Vec<_>>());
        let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
        assert!(cards.len() <= EXPANSION_BUDGET + PAGE_SIZE, "{} cards", cards.len());
    }

    #[test]
    fn scalar_and_empty_roots_are_one_header() {
        for root in [json!(3), json!("s"), json!({}), json!([])] {
            let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
            assert_eq!(pointers(&cards), [""]);
            assert_eq!((cards[0].rows, cards[0].more), (0, 0));
        }
    }

    #[test]
    fn toggling_stores_only_a_difference_from_the_default() {
        let root = users(2);
        let expanded = default_expanded(&root);
        let mut view = JsonView::default();
        toggle(&mut view, &expanded, "/users");
        assert_eq!(view.get("/users").unwrap().collapsed, Some(true));
        assert!(is_collapsed(&view, &expanded, "/users"));
        toggle(&mut view, &expanded, "/users");
        assert_eq!(view.to_field(), "");
        // Expanding one card never collapses another (plan ruling 5):
        // `/meta/bbox/0` (depth 3, collapsed by default) holds only
        // scalars, so exactly one fewer card is collapsed afterwards.
        let collapsed = |view: &JsonView| visible_cards(&root, view, &expanded).iter().filter(|c| c.collapsed).count();
        let before = collapsed(&view);
        toggle(&mut view, &expanded, "/meta/bbox/0");
        assert_eq!(collapsed(&view), before - 1);
    }

    #[test]
    fn reveal_expands_ancestors_and_pages_to_the_target() {
        let root = users(45);
        let expanded = default_expanded(&root);
        let mut view = JsonView::default();
        toggle(&mut view, &expanded, "/users");
        assert!(reveal(&root, &mut view, &expanded, "/users/33/roles/1"));
        assert!(!is_collapsed(&view, &expanded, "/users"));
        assert_eq!(view.pages("/users"), 2);
        let cards = visible_cards(&root, &view, &expanded);
        assert!(cards.iter().any(|c| c.pointer == "/users/33/roles"));
        assert!(!reveal(&root, &mut view, &expanded, "/users/33/roles/1"), "already visible");
    }

    #[test]
    fn layout_is_columns_by_depth_with_parents_centred_and_overrides_local() {
        let root = json!({"a": {"x": 1}, "b": {"y": 2, "z": 3}});
        let expanded = default_expanded(&root);
        let cards = visible_cards(&root, &JsonView::default(), &expanded);
        let pos = tree_layout(&cards, &JsonView::default());
        assert_eq!(pos, tree_layout(&cards, &JsonView::default()), "deterministic");
        assert_eq!(pos[0], egui::pos2(0.0, 0.0), "root at the origin");
        let (a, b) = (1, 2);
        assert_eq!(pos[a].x, CARD_WIDTH + COLUMN_GAP);
        assert_eq!(pos[b].y, pos[a].y + cards[a].height() + SIBLING_GAP);
        let span_mid = (pos[a].y + pos[b].y + cards[b].height()) / 2.0;
        assert!((pos[0].y + cards[0].height() / 2.0 - span_mid).abs() < 0.01, "parent centred on its children");

        let mut view = JsonView::default();
        view.entry("/a").pos = Some([900.0, 900.0]);
        let moved = tree_layout(&cards, &view);
        assert_eq!(moved[a], egui::pos2(900.0, 900.0));
        assert_eq!((moved[0], moved[b]), (pos[0], pos[b]), "only that card moves");
    }

    #[test]
    fn search_matches_keys_and_scalars_in_hidden_pages_and_keeps_ancestors() {
        let root = users(45);
        let result = search(&root, "U33");
        assert_eq!(result.matches, ["/users/33/name"]);
        assert!(result.hits.contains("/users/33/name"));
        for kept in ["", "/users", "/users/33", "/users/33/name"] {
            assert!(result.keep.contains(kept), "{kept}");
        }
        assert_eq!(search(&root, "bbox").matches, ["/meta/bbox"]);
        assert!(search(&root, "").matches.is_empty());
        // Array indices don't match (plan ruling 9).
        assert!(search(&json!(["x"]), "0").matches.is_empty());
        assert_eq!(count_nodes(&json!({"a": [1, 2]})), 4);
    }

    #[test]
    fn outline_rows_follow_open_containers_or_the_search() {
        let root = json!({"a": {"x": 1}, "b": [true]});
        let open = HashSet::from([String::new()]);
        let rows: Vec<_> = outline_rows(&root, &open, None).into_iter().map(|r| (r.pointer, r.depth)).collect();
        assert_eq!(rows, [("".into(), 0), ("/a".into(), 1), ("/b".into(), 1)]);
        let open = HashSet::from([String::new(), "/a".to_string()]);
        let rows: Vec<_> = outline_rows(&root, &open, None).into_iter().map(|r| r.pointer).collect();
        assert_eq!(rows, ["", "/a", "/a/x", "/b"]);
        let found = search(&root, "true");
        let rows: Vec<_> = outline_rows(&root, &HashSet::new(), Some(&found.keep)).into_iter().map(|r| r.pointer).collect();
        assert_eq!(rows, ["", "/b", "/b/0"]);
    }

    #[test]
    fn titles_and_preview_sizes() {
        let root = users(3);
        assert_eq!(card_title(&root, "", "users.json"), "users.json");
        assert_eq!(card_title(&root, "/users/0", "users.json"), "users[0]");
        assert_eq!(card_title(&root, "/meta/bbox/1", "users.json"), "bbox[1]");
        assert_eq!(card_title(&root, "/meta", "users.json"), "meta");
        assert_eq!(preview_size(&root), egui::vec2(CARD_WIDTH, HEADER_HEIGHT + ROW_HEIGHT * 3.0));
        let wide = json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5});
        assert_eq!(preview_size(&wide).y, HEADER_HEIGHT + ROW_HEIGHT * 5.0); // 4 rows and "+ 1 more"
        assert_eq!(preview_size(&json!(7)).y, HEADER_HEIGHT + ROW_HEIGHT);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p manifold -j 12 json_view::tests`
Expected: compile errors for the missing items.

- [ ] **Step 3: Write the implementation**

Change `use std::collections::BTreeMap;` to `use std::collections::{BTreeMap, HashSet, VecDeque};`. Add `pub(crate) const PREVIEW_ROWS: usize = 4;` with the doc comment `/// Root rows the Json card shows at its parent's level.`. Then append:

```rust
/// One card on a JSON level: the root, or a non-empty container whose
/// row is visible.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Card {
    pub(crate) pointer: String,
    /// The parent card's index and which of its rows leads here.
    pub(crate) parent: Option<(usize, usize)>,
    pub(crate) depth: usize,
    pub(crate) collapsed: bool,
    /// Entry rows shown: none when collapsed, else the revealed pages.
    pub(crate) rows: usize,
    /// Entries past the revealed pages, shown as one `+ N more` row.
    pub(crate) more: usize,
}

impl Card {
    pub(crate) fn height(&self) -> f32 {
        HEADER_HEIGHT + ROW_HEIGHT * (self.rows + usize::from(self.more > 0)) as f32
    }

    pub(crate) fn size(&self) -> egui::Vec2 {
        egui::vec2(CARD_WIDTH, self.height())
    }
}

/// The cards a value opens with (spec "Default expansion"): breadth-first
/// from the root, each card expanded while the visible count stays within
/// `EXPANSION_BUDGET` and it is shallower than `EXPANSION_DEPTH`. The root
/// is always expanded. Depends on the value alone (plan ruling 5).
pub(crate) fn default_expanded(root: &Value) -> HashSet<String> {
    let mut expanded = HashSet::new();
    let mut visible = 1;
    let mut queue = VecDeque::from([(String::new(), root, 0usize)]);
    while let Some((pointer, value, depth)) = queue.pop_front() {
        let children: Vec<(String, &Value)> = entries(value)
            .take(PAGE_SIZE)
            .filter(|(_, child)| is_card(child))
            .map(|(key, child)| (child_pointer(&pointer, &key), child))
            .collect();
        let is_root = pointer.is_empty();
        if !is_root && (depth >= EXPANSION_DEPTH || visible + children.len() > EXPANSION_BUDGET) {
            continue;
        }
        visible += children.len();
        expanded.insert(pointer);
        queue.extend(children.into_iter().map(|(p, child)| (p, child, depth + 1)));
    }
    expanded
}

/// The saved override if there is one, else the default.
pub(crate) fn is_collapsed(view: &JsonView, expanded: &HashSet<String>, pointer: &str) -> bool {
    view.get(pointer).and_then(|o| o.collapsed).unwrap_or(!expanded.contains(pointer))
}

/// Every visible card, breadth-first from the root (so a parent always
/// comes before its children, and siblings in row order).
pub(crate) fn visible_cards(root: &Value, view: &JsonView, expanded: &HashSet<String>) -> Vec<Card> {
    let mut cards = Vec::new();
    let mut queue = VecDeque::from([(String::new(), root, None, 0usize)]);
    while let Some((pointer, value, parent, depth)) = queue.pop_front() {
        let collapsed = is_collapsed(view, expanded, &pointer);
        let len = entry_count(value);
        let rows = if collapsed { 0 } else { len.min(view.pages(&pointer) * PAGE_SIZE) };
        let index = cards.len();
        for (row, (key, child)) in entries(value).take(rows).enumerate() {
            if is_card(child) {
                queue.push_back((child_pointer(&pointer, &key), child, Some((index, row)), depth + 1));
            }
        }
        let more = if collapsed { 0 } else { len - rows };
        cards.push(Card { pointer, parent, depth, collapsed, rows, more });
    }
    cards
}

/// Flips `pointer`'s collapse, storing an override only where it differs
/// from the default.
pub(crate) fn toggle(view: &mut JsonView, expanded: &HashSet<String>, pointer: &str) {
    let collapsed = !is_collapsed(view, expanded, pointer);
    let default = !expanded.contains(pointer);
    view.entry(pointer).collapsed = (collapsed != default).then_some(collapsed);
    view.0.retain(|_, o| !o.is_empty());
}

/// The `+ N more` row: one more page.
pub(crate) fn show_more(view: &mut JsonView, pointer: &str) {
    let pages = view.pages(pointer) + 1;
    view.entry(pointer).pages = Some(pages as u32);
}

/// Makes `pointer` visible (spec "Outline"): every ancestor expanded and
/// paged far enough to show the next step down. `true` if `view` changed.
pub(crate) fn reveal(root: &Value, view: &mut JsonView, expanded: &HashSet<String>, pointer: &str) -> bool {
    let before = view.clone();
    let mut at = root;
    let mut ancestor = String::new();
    for token in pointer_tokens(pointer) {
        let Some(index) = entry_index(at, &token) else { break };
        if is_collapsed(view, expanded, &ancestor) {
            toggle(view, expanded, &ancestor);
        }
        if index >= view.pages(&ancestor) * PAGE_SIZE {
            view.entry(&ancestor).pages = Some((index / PAGE_SIZE + 1) as u32);
        }
        at = if at.is_array() { &at[index] } else { &at[token.as_str()] };
        ancestor = child_pointer(&ancestor, &token);
    }
    *view != before
}

/// Each card's top-left (spec "Tree layout"): one column per depth; each
/// subtree a band as tall as the larger of its card and its children's
/// bands plus `SIBLING_GAP`s; a parent centred on its children's span.
/// The root is at the origin, and a saved `pos` replaces one card's only.
pub(crate) fn tree_layout(cards: &[Card], view: &JsonView) -> Vec<egui::Pos2> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); cards.len()];
    for (i, card) in cards.iter().enumerate() {
        if let Some((parent, _)) = card.parent {
            children[parent].push(i);
        }
    }
    let span = |band: &[f32], kids: &[usize]| {
        kids.iter().map(|&k| band[k]).sum::<f32>() + SIBLING_GAP * kids.len().saturating_sub(1) as f32
    };
    // Children come after their parent (`visible_cards` is breadth-first),
    // so walking backwards sizes every child before its parent.
    let mut band = vec![0.0; cards.len()];
    for i in (0..cards.len()).rev() {
        band[i] = cards[i].height().max(span(&band, &children[i]));
    }
    let mut top = vec![0.0; cards.len()];
    let mut pos = vec![egui::Pos2::ZERO; cards.len()];
    for i in 0..cards.len() {
        let mut y = top[i] + (band[i] - span(&band, &children[i])) / 2.0;
        for &k in &children[i] {
            top[k] = y;
            y += band[k] + SIBLING_GAP;
        }
        let x = cards[i].depth as f32 * (CARD_WIDTH + COLUMN_GAP);
        pos[i] = egui::pos2(x, top[i] + (band[i] - cards[i].height()) / 2.0);
    }
    let origin = pos.first().map_or(0.0, |p| p.y);
    for (card, p) in cards.iter().zip(&mut pos) {
        p.y -= origin;
        if let Some([x, y]) = view.get(&card.pointer).and_then(|o| o.pos) {
            *p = egui::pos2(x, y);
        }
    }
    pos
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SearchResult {
    /// Matching entries' pointers, in document order.
    pub(crate) matches: Vec<String>,
    /// The same pointers, for highlighting.
    pub(crate) hits: HashSet<String>,
    /// The matches and every ancestor of one.
    pub(crate) keep: HashSet<String>,
}

/// Case-insensitive substring search over object keys and scalar values
/// (spec "Search", plan ruling 9), across the whole value.
pub(crate) fn search(root: &Value, query: &str) -> SearchResult {
    let mut result = SearchResult::default();
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return result;
    }
    // Depth-first, children pushed in reverse so they pop in document order.
    let mut stack: Vec<(String, Option<&str>, &Value)> = vec![(String::new(), None, root)];
    while let Some((pointer, key, value)) = stack.pop() {
        let key_hit = key.is_some_and(|k| k.to_lowercase().contains(&needle));
        let value_hit = scalar_text(value).is_some_and(|t| t.to_lowercase().contains(&needle));
        if key_hit || value_hit {
            for (i, _) in pointer.match_indices('/') {
                result.keep.insert(pointer[..i].to_string());
            }
            result.keep.insert(pointer.clone());
            result.hits.insert(pointer.clone());
            result.matches.push(pointer.clone());
        }
        let children: Vec<(String, Option<&str>, &Value)> = match value {
            Value::Object(map) => map.iter().map(|(k, v)| (child_pointer(&pointer, k), Some(k.as_str()), v)).collect(),
            Value::Array(items) => {
                items.iter().enumerate().map(|(i, v)| (child_pointer(&pointer, &i.to_string()), None, v)).collect()
            }
            _ => Vec::new(),
        };
        stack.extend(children.into_iter().rev());
    }
    result
}

/// Nodes in `value`, itself included: decides whether search runs on a
/// worker (`WORKER_SEARCH_NODES`). serde_json's parser caps nesting at
/// 128, so the recursion is bounded.
pub(crate) fn count_nodes(value: &Value) -> usize {
    1 + match value {
        Value::Object(map) => map.values().map(count_nodes).sum(),
        Value::Array(items) => items.iter().map(count_nodes).sum(),
        _ => 0,
    }
}

/// One outline row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OutlineRow {
    pub(crate) pointer: String,
    pub(crate) depth: usize,
}

/// The outline flattened (plan ruling 8): the root, then every entry of
/// each open container. With a search (`keep`), exactly the kept rows,
/// all open.
pub(crate) fn outline_rows(root: &Value, open: &HashSet<String>, keep: Option<&HashSet<String>>) -> Vec<OutlineRow> {
    let mut rows = Vec::new();
    let mut stack = vec![(String::new(), root, 0usize)];
    while let Some((pointer, value, depth)) = stack.pop() {
        if keep.is_some_and(|keep| !keep.contains(&pointer)) {
            continue;
        }
        let is_open = keep.is_some() || open.contains(&pointer);
        if is_open {
            let children: Vec<_> = entries(value).map(|(k, v)| (child_pointer(&pointer, &k), v, depth + 1)).collect();
            stack.extend(children.into_iter().rev());
        }
        rows.push(OutlineRow { pointer, depth });
    }
    rows
}

/// A card's header title: the file name for the root, `key` for an
/// object's child, `parent[index]` for an array's.
pub(crate) fn card_title(root: &Value, pointer: &str, root_name: &str) -> String {
    let (Some(parent), Some(key)) = (parent_pointer(pointer), last_key(pointer)) else {
        return root_name.to_string();
    };
    if root.pointer(parent).is_some_and(Value::is_array) {
        format!("{}[{key}]", card_title(root, parent, root_name))
    } else {
        key
    }
}

/// The Json card's size at its parent's level (spec "The card at the
/// parent level"): a header, up to `PREVIEW_ROWS` root rows and a
/// `+ N more` row; one value row for a scalar or empty root.
pub(crate) fn preview_size(value: &Value) -> egui::Vec2 {
    let len = entry_count(value);
    let rows = if len == 0 { 1 } else { len.min(PREVIEW_ROWS) + usize::from(len > PREVIEW_ROWS) };
    egui::vec2(CARD_WIDTH, HEADER_HEIGHT + ROW_HEIGHT * rows as f32)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p manifold -j 12 json_view::tests`
Expected: 16 passed. A failure in `default_expansion_stops_at_the_depth_and_the_budget` or `toggling_stores_only_a_difference_from_the_default` means the implementation is off, not the test. Fix the code.

Run: `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/json_view.rs
git commit -m "json_view: visible cards, tree layout, search and outline rows

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The `VIEW` field (14) on Json nodes

**Files:**
- Modify: `crates/manifold/src/store/model.rs` (the `field` consts, `FIELDS`)
- Modify: `crates/manifold/src/node.rs` (`JsonPayload` ~309, test ~638)
- Modify: `crates/manifold/src/session_sync.rs` (`kind_fields` ~37, `shared_nodes` ~345, `materialize` ~470, restore ~546 and ~575)
- Modify: `crates/manifold/src/main.rs` (every `JsonPayload { … }` literal, `reload_fetched_node` ~4337)
- Modify: `crates/manifold/src/card.rs` (the test literals ~1793 and ~1869)

**Interfaces:**
- Consumes: `json_view::JsonView` (Task 2).
- Produces: `JsonPayload.view: JsonView`, and `field::VIEW: u32 = 14`, stored as `"view"`, Str, not explicit.

- [ ] **Step 1: Write the failing tests**

Add to `session_sync.rs`'s test module, which already has `id(counter) -> NodeId`. `shared_nodes()` returns `BTreeMap<NodeId, Shared>` and skips any node whose id doesn't parse as a `NodeId`, so the node's id must be `id(1).to_string()`. `materialize` is `(id: String, kind: Kind, fields: &Fields, group: Option<String>, local: Option<&LocalNode>)`.

```rust
    /// An App holding one Json node, id `id(1)`, loaded from a temp file.
    fn app_with_json_file(name: &str) -> (App, String) {
        let dir = crate::store::testutil::temp_dir(name);
        let path = dir.join("v.json");
        std::fs::write(&path, r#"{"users": [{"name": "a"}]}"#).unwrap();
        let mut app = App::default();
        let node = id(1).to_string();
        app.insert_json_node_with_id(node.clone(), path.to_string_lossy().into_owned(), egui::Vec2::ZERO, None)
            .unwrap();
        (app, node)
    }

    #[test]
    fn a_json_nodes_view_round_trips_through_the_store() {
        let (mut app, node) = app_with_json_file("json_view_field");
        let NodePayload::Json(json) = &mut app.nodes.get_mut(&node).unwrap().payload else { unreachable!() };
        json.view.entry("/users").collapsed = Some(true);

        let shared = app.shared_nodes();
        let fields = &shared[&id(1)].fields;
        assert_eq!(fields.get(&field::VIEW), Some(&FieldValue::Str(r#"{"/users":{"collapsed":true}}"#.into())));

        // Restoring those fields gives the same view back.
        let mut restored = App::default();
        restored.materialize(node.clone(), Kind::Json, fields, None, None);
        let json = restored.nodes[&node].as_json().unwrap();
        assert_eq!(json.view.get("/users").unwrap().collapsed, Some(true));
    }

    #[test]
    fn a_remote_view_change_updates_the_node_in_place() {
        let (mut app, node) = app_with_json_file("json_view_remote");
        let (value, path) = app.nodes[&node].as_json().map(|j| (j.value.clone(), j.path.clone())).unwrap();
        let mut fields = app.shared_nodes()[&id(1)].fields.clone();
        fields.insert(field::VIEW, FieldValue::Str(r#"{"":{"collapsed":true}}"#.into()));
        app.materialize(node.clone(), Kind::Json, &fields, None, None);
        let json = app.nodes[&node].as_json().unwrap();
        assert_eq!(json.view.get("").unwrap().collapsed, Some(true));
        assert_eq!((&json.value, &json.path), (&value, &path), "only the view changed");
    }
```

Also extend `field_numbers_are_unique_and_never_reserved` in `store/model.rs`: add `assert_eq!(field::VIEW, 14);` and check that `FIELDS` has an entry named `"view"` of type `FieldType::Str` with `explicit: false`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p manifold -j 12 view`
Expected: compile errors, because `field::VIEW` and `JsonPayload.view` don't exist yet.

- [ ] **Step 3: Implement**

1. `store/model.rs`: add `pub const VIEW: u32 = 14;` after field 13, with the doc comment `/// A Json node's view overrides (\`json_view::JsonView\`), compact JSON; "" when none.` Make `FIELDS` a `[FieldDef; 14]` and append `FieldDef { no: field::VIEW, name: "view", ty: FieldType::Str, explicit: false }`.

2. `node.rs` `JsonPayload`: add the field below. Also rewrite the `key` doc comment's out-of-date "Never persisted" sentence to say leaves are stored as `JsonLeaf`.

```rust
    /// What the user changed about this node's level (spec "Saved view
    /// state"): the store's `view` field. Empty for a leaf.
    pub(crate) view: crate::json_view::JsonView,
```

3. Add `view: JsonView::default()` to every `JsonPayload { … }` literal (see `grep -n "JsonPayload {" crates/manifold/src/*.rs`). There is one in node.rs, two in card.rs tests, and in main.rs the demo (~2604), `reload_fetched_node` (~4374), `insert_json_node_with_id` (~4688), `expand_json_entries` (~4788) and tests (~5853, ~6275, ~6309). session_sync.rs has three (~550, ~554, ~562). Leave the pattern at main.rs ~4418 (`JsonPayload { path, .. }`) alone. Import `JsonView` where it isn't in scope (`use crate::json_view::JsonView;` in the files that need it).

4. `session_sync.rs`:
   - `kind_fields`: split out Json: `Kind::Image | Kind::Pdf => &[POS, COLOR, URL, PATH],` and `Kind::Json => &[POS, COLOR, URL, PATH, VIEW],`.
   - `shared_nodes`, Json arm, `None` branch: add `fields.insert(field::VIEW, str(&j.view.to_field()));` after the URL insert.
   - `materialize`, in-place update: replace the Json arm with the code below. Keep the catch-all arm after it.

```rust
                NodePayload::Json(p) if p.key.is_none() => {
                    if changed(field::URL) {
                        p.url = url_of(fields);
                    }
                    if changed(field::VIEW) {
                        p.view = JsonView::from_field(&str_of(fields, field::VIEW));
                    }
                }
```

   - Restore: in the block after the kind match that sets `group`, `color` and `source_url_mut`, add the lines below. Every restore branch for `Kind::Json` (placeholder or loaded) then gets its view.

```rust
            if let NodePayload::Json(json) = &mut node.payload {
                if json.key.is_none() {
                    json.view = JsonView::from_field(&str_of(fields, field::VIEW));
                }
            }
```

5. `main.rs` `reload_fetched_node`: carry the view across the reload, and drop overrides that no longer resolve (spec "Saved view state"). Before `self.nodes.remove(id)`, add `let view = node.as_json().map(|j| j.view.clone()).unwrap_or_default();`, taking it while `node` is still borrowed alongside `pos`, `group`, `parent` and `color`. In the block that restores `parent`, `color` and `group`, add:

```rust
            if let NodePayload::Json(json) = &mut node.payload {
                json.view = view;
                json.view.retain_resolvable(&json.value);
            }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p manifold -j 12`
Expected: all pass. That includes the two new tests, the model test, and every existing sync test: Json nodes now carry a 5th field, and an empty view is `""`.

Run: `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "Store field 14 (view): a Json node's level overrides

Diffed Str, compact JSON keyed by pointer; restored, updated in place
from remote replicas, and carried across a Refresh with stale pointers
dropped.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The JSON level on `App`: entering, the seam's second source, creation gating and headless

**Files:**
- Create: `crates/manifold/src/json_level.rs`
- Modify: `crates/manifold/src/main.rs` (`mod json_level;`, the `App` fields and `Default`, creation sites, `group_breadcrumb` ~3226, `rename_current_group_shortcut` ~4094)
- Modify: `crates/manifold/src/level.rs` (the `JsonCard` arms and `insertion_group`)
- Modify: `crates/manifold/src/graph_ui.rs` (`switch_group_level`, the start of `build_graph_ui`, the Json double-click ~1453, toolbar gating, the empty-canvas menu ~1108, `graph_world_rect_to_screen` ~263, the minimap and hit-test arms)
- Modify: `crates/manifold/src/session_sync.rs` (`materialize` marks the level stale; the ImportRoot ~1221 uses `insertion_group`)
- Modify: `crates/manifold/src/debug_control.rs`, `crates/manifold/src/event_loop.rs`

**Interfaces:**
- Consumes: Task 1's seam, Tasks 2–3's `json_view` and Task 4's `JsonPayload.view`.
- Produces:
  - `pub(crate) struct JsonLevel { node_id, value: Arc<Value>, expanded, cards: Vec<Card>, rects: Vec<egui::Rect>, index: HashMap<String, usize>, selected: Option<String>, stale: bool, outline: Outline, search: Search, menu: Option<(String, egui::Pos2)> }`, with `relayout(&mut self, &JsonView)`, `card_id(&self, usize) -> String` and `row_at(&self, card: usize, world: egui::Pos2) -> Option<String>`
  - `pub(crate) struct Outline { open: HashSet<String>, rows: Vec<OutlineRow>, dirty: bool, scroll_to: Option<usize> }` and `pub(crate) struct Search { query: String, result: Option<SearchResult>, current: usize, pending: Option<(String, mpsc::Receiver<SearchResult>)>, focus: bool }`, both `Default`
  - `pub(crate) enum JsonEdit { Select(String), Toggle(String), More(String), Moved(String, egui::Pos2), Relayout, Color(String, Option<u32>) }`
  - `App::current_json_node(&self) -> Option<&JsonPayload>`, `App::sync_json_level(&mut self)`, `App::edit_json_view(&mut self, impl FnOnce(&mut JsonView, &Value, &HashSet<String>))`, `App::apply_json_edit(&mut self, JsonEdit)`, `App::insertion_group(&self) -> Option<String>` and `App::json_level_state(&self) -> serde_json::Value`
  - `App` fields: `json_level: Option<JsonLevel>`, `json_outline_visible: bool` (default `true`) and `json_outline_width: f32` (default `260.0`)
  - `LevelItemKind::JsonCard`
  - `DebugCommand::{JsonSelect(String), JsonToggle(String), JsonMore(String), JsonRelayout, JsonSearch(String)}`

- [ ] **Step 1: Write the failing tests**

Create `crates/manifold/src/json_level.rs` holding only this test module, and add `mod json_level;` to `main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Real node ids: `shared_nodes()` skips any id that doesn't parse as
    // a `NodeId`, which would make the store test pass vacuously.
    const OUTER: &str = "0000000000000001-1";
    const J: &str = "0000000000000001-2";

    /// An App with a Json node `J` inside group `OUTER`, loaded from a temp
    /// file holding `text`.
    fn app_with_json(text: &str) -> (App, String) {
        let dir = crate::store::testutil::temp_dir("json_level");
        let path = dir.join("level.json");
        std::fs::write(&path, text).unwrap();
        let mut app = App::default();
        app.insert_placeholder_node(
            OUTER.into(),
            egui::Vec2::ZERO,
            NodePayload::Group(GroupPayload { name: "outer".into(), from_json: false }),
        );
        app.insert_json_node_with_id(J.into(), path.to_string_lossy().into_owned(), egui::Vec2::ZERO, Some(OUTER.into()))
            .unwrap();
        (app, J.into())
    }

    #[test]
    fn entering_builds_the_level_and_leaving_drops_it() {
        let (mut app, j) = app_with_json(r#"{"a": {"b": 1}, "c": [1, 2]}"#);
        app.switch_group_level(Some(j.clone()));
        let level = app.json_level.as_ref().expect("entering a Json node builds its level");
        assert_eq!(level.node_id, j);
        let ids: Vec<String> = app.level_items().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, [format!("{J}#"), format!("{J}#/a"), format!("{J}#/c")]);
        assert!(app.level_items().iter().all(|i| i.kind == LevelItemKind::JsonCard));
        assert_eq!(app.level_edges().len(), 2);
        app.switch_group_level(Some(OUTER.into()));
        assert!(app.json_level.is_none());
    }

    #[test]
    fn new_nodes_land_beside_the_json_node_not_inside_it() {
        let (mut app, j) = app_with_json("[1]");
        app.current_group = Some(j.clone()); // before any frame syncs the level
        assert_eq!(app.insertion_group(), Some(OUTER.to_string()));
        app.current_group = Some(OUTER.into());
        assert_eq!(app.insertion_group(), Some(OUTER.to_string()));
    }

    #[test]
    fn the_store_sees_nothing_of_the_level_but_view() {
        let (mut app, j) = app_with_json(r#"{"a": {"b": [1, 2, 3]}, "c": [1, 2]}"#);
        // (id, kind, group, fields without VIEW) for every stored node.
        let stored = |app: &App| {
            app.shared_nodes()
                .into_iter()
                .map(|(id, mut s)| {
                    s.fields.remove(&field::VIEW);
                    (id, s.kind, s.group, s.fields)
                })
                .collect::<Vec<_>>()
        };
        let before = stored(&app);
        assert_eq!(before.len(), 2, "both nodes are stored, so the comparison means something");
        app.switch_group_level(Some(j.clone()));
        app.apply_json_edit(JsonEdit::Toggle("/a".into()));
        app.apply_json_edit(JsonEdit::More("/c".into()));
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(500.0, 40.0)));
        app.apply_json_edit(JsonEdit::Select("/a/b".into()));
        app.apply_json_edit(JsonEdit::Relayout);
        assert_eq!(stored(&app), before);
        // The toggle is still there after Relayout; the drag is not.
        let view = &app.nodes[&j].as_json().unwrap().view;
        assert_eq!(view.get("/a").and_then(|o| o.collapsed), Some(true));
        assert_eq!(view.get("/a").and_then(|o| o.pos), None);
        app.switch_group_level(Some(OUTER.into()));
        assert_eq!(stored(&app), before);
    }

    #[test]
    fn a_drag_saves_only_that_cards_position() {
        let (mut app, j) = app_with_json(r#"{"a": {"x": 1}, "b": {"y": 2}}"#);
        app.switch_group_level(Some(j.clone()));
        let b_before = app.json_level.as_ref().unwrap().rects[2];
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(700.0, 10.0)));
        let level = app.json_level.as_ref().unwrap();
        assert_eq!(level.rects[1].min, egui::pos2(700.0, 10.0));
        assert_eq!(level.rects[2], b_before);
    }

    #[test]
    fn rows_are_hit_by_their_world_position() {
        let (mut app, j) = app_with_json(r#"{"a": 1, "b": {"c": 2}}"#);
        app.switch_group_level(Some(j));
        let level = app.json_level.as_ref().unwrap();
        let root = level.rects[0];
        let row = |i: f32| egui::pos2(root.center().x, root.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (i + 0.5));
        assert_eq!(level.row_at(0, row(0.0)), Some("/a".into()));
        assert_eq!(level.row_at(0, row(1.0)), Some("/b".into()));
        assert_eq!(level.row_at(0, egui::pos2(root.center().x, root.top() + 5.0)), None, "the header is the card, not a row");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p manifold -j 12 json_level::tests`
Expected: compile errors.

- [ ] **Step 3: Write `json_level.rs`**

Put this above the tests:

```rust
//! A Json node's level (spec "The JSON level"): its record cards laid out
//! as a tree, the selection, the outline's state and search. Present
//! exactly while `current_group` names a file-backed Json node; never in
//! `self.nodes`, so nothing here can reach the store but the node's own
//! `view` field.

use std::sync::{mpsc, Arc};

use super::*;
use crate::json_view::{self, Card, JsonView, OutlineRow, SearchResult};

pub(crate) struct JsonLevel {
    /// The Json node shown.
    pub(crate) node_id: String,
    /// The node's value when the level was built; a Refresh rebuilds it.
    pub(crate) value: Arc<serde_json::Value>,
    /// `json_view::default_expanded`, fixed for this value.
    pub(crate) expanded: HashSet<String>,
    pub(crate) cards: Vec<Card>,
    /// World rects, one per card.
    pub(crate) rects: Vec<egui::Rect>,
    /// Card index by pointer.
    pub(crate) index: HashMap<String, usize>,
    /// The selected card's or row's pointer.
    pub(crate) selected: Option<String>,
    /// The view changed from elsewhere (another replica); relaid out on
    /// the next sync.
    pub(crate) stale: bool,
    pub(crate) outline: Outline,
    pub(crate) search: Search,
    /// A context menu: the pointer it acts on and where it opened (screen).
    pub(crate) menu: Option<(String, egui::Pos2)>,
}

/// The outline's own state (spec "Outline"): not saved, separate from
/// the canvas's collapse.
#[derive(Default)]
pub(crate) struct Outline {
    pub(crate) open: HashSet<String>,
    pub(crate) rows: Vec<OutlineRow>,
    /// `open` or the search changed; `rows` is rebuilt before the next draw.
    pub(crate) dirty: bool,
    /// Scroll this row into view on the next draw.
    pub(crate) scroll_to: Option<usize>,
}

#[derive(Default)]
pub(crate) struct Search {
    pub(crate) query: String,
    pub(crate) result: Option<SearchResult>,
    /// The match Enter or ↓ is on.
    pub(crate) current: usize,
    /// A worker's search of a big value, with the query it ran.
    pub(crate) pending: Option<(String, mpsc::Receiver<SearchResult>)>,
    /// Ctrl+F: focus the search box on the next draw.
    pub(crate) focus: bool,
}

/// Something the user did on a JSON level, applied once drawing is done.
pub(crate) enum JsonEdit {
    Select(String),
    Toggle(String),
    More(String),
    /// A dragged card's new top-left.
    Moved(String, egui::Pos2),
    Relayout,
    Color(String, Option<u32>),
}

impl JsonLevel {
    fn new(node_id: String, value: serde_json::Value, view: &JsonView) -> Self {
        let expanded = json_view::default_expanded(&value);
        let mut level = Self {
            node_id,
            value: Arc::new(value),
            expanded,
            cards: Vec::new(),
            rects: Vec::new(),
            index: HashMap::new(),
            selected: None,
            stale: false,
            outline: Outline { open: HashSet::from([String::new()]), dirty: true, ..Outline::default() },
            search: Search::default(),
            menu: None,
        };
        level.relayout(view);
        level
    }

    /// Recomputes the visible cards and their rects from `view`.
    pub(crate) fn relayout(&mut self, view: &JsonView) {
        self.cards = json_view::visible_cards(&self.value, view, &self.expanded);
        let positions = json_view::tree_layout(&self.cards, view);
        self.rects =
            self.cards.iter().zip(positions).map(|(card, pos)| egui::Rect::from_min_size(pos, card.size())).collect();
        self.index = self.cards.iter().enumerate().map(|(i, card)| (card.pointer.clone(), i)).collect();
    }

    /// A card's level-item id, `<json node id>#<pointer>` (spec "Model").
    pub(crate) fn card_id(&self, card: usize) -> String {
        format!("{}#{}", self.node_id, self.cards[card].pointer)
    }

    /// The pointer of the entry row under `world` on card `card`, or
    /// `None` on its header or its `+ N more` row.
    pub(crate) fn row_at(&self, card: usize, world: egui::Pos2) -> Option<String> {
        let rect = self.rects.get(card)?;
        let offset = world.y - rect.top() - json_view::HEADER_HEIGHT;
        if offset < 0.0 || !rect.contains(world) {
            return None;
        }
        let row = (offset / json_view::ROW_HEIGHT) as usize;
        let pointer = &self.cards[card].pointer;
        let value = self.value.pointer(pointer)?;
        let (key, _) = json_view::entries(value).take(self.cards[card].rows).nth(row)?;
        Some(json_view::child_pointer(pointer, &key))
    }

    /// Whether `pointer` is the `+ N more` row's position on card `card`.
    pub(crate) fn more_row_at(&self, card: usize, world: egui::Pos2) -> bool {
        let (Some(rect), Some(c)) = (self.rects.get(card), self.cards.get(card)) else { return false };
        let top = rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * c.rows as f32;
        c.more > 0 && rect.contains(world) && world.y >= top
    }
}

impl App {
    /// The file-backed Json node `current_group` names, if it does.
    pub(crate) fn current_json_node(&self) -> Option<&JsonPayload> {
        let node = self.nodes.get(self.current_group.as_ref()?)?;
        node.as_json().filter(|json| json.key.is_none())
    }

    /// Makes `json_level` match `current_group`: built on entering a Json
    /// node, relaid out when marked stale, dropped on leaving. Runs at the
    /// start of `build_graph_ui` and in `switch_group_level`.
    pub(crate) fn sync_json_level(&mut self) {
        let Some(json) = self.current_json_node() else {
            self.json_level = None;
            return;
        };
        let id = self.current_group.clone().unwrap_or_default();
        match &mut self.json_level {
            Some(level) if level.node_id == id => {
                if level.stale {
                    level.stale = false;
                    level.relayout(&json.view);
                }
            }
            _ => self.json_level = Some(JsonLevel::new(id, json.value.clone(), &json.view)),
        }
    }

    /// Applies `edit` to the shown node's view, drops overrides that no
    /// longer resolve, relays the level out and saves (`VIEW` is diffed).
    pub(crate) fn edit_json_view(
        &mut self,
        edit: impl FnOnce(&mut JsonView, &serde_json::Value, &HashSet<String>),
    ) {
        let Some(level) = self.json_level.as_mut() else { return };
        let Some(NodePayload::Json(json)) = self.nodes.get_mut(&level.node_id).map(|n| &mut n.payload) else {
            return;
        };
        edit(&mut json.view, &level.value, &level.expanded);
        json.view.retain_resolvable(&level.value);
        level.relayout(&json.view);
        self.save_session();
    }

    pub(crate) fn apply_json_edit(&mut self, edit: JsonEdit) {
        match edit {
            JsonEdit::Select(pointer) => {
                if let Some(level) = self.json_level.as_mut() {
                    level.selected = Some(pointer);
                }
            }
            JsonEdit::Toggle(pointer) => self.edit_json_view(|view, _, expanded| json_view::toggle(view, expanded, &pointer)),
            JsonEdit::More(pointer) => self.edit_json_view(|view, _, _| json_view::show_more(view, &pointer)),
            JsonEdit::Moved(pointer, top_left) => {
                self.edit_json_view(|view, _, _| view.entry(&pointer).pos = Some([top_left.x, top_left.y]))
            }
            JsonEdit::Relayout => self.edit_json_view(|view, _, _| view.clear_positions()),
            JsonEdit::Color(pointer, color) => self.edit_json_view(|view, _, _| view.entry(&pointer).color = color),
        }
    }

    /// `EXPORTSTATE`'s `json_level` (spec "Headless"): the root id, the
    /// visible cards' pointers and positions, the selection and the
    /// search's matches.
    pub(crate) fn json_level_state(&self) -> serde_json::Value {
        let Some(level) = &self.json_level else { return serde_json::Value::Null };
        let cards: Vec<serde_json::Value> = level
            .cards
            .iter()
            .zip(&level.rects)
            .map(|(card, rect)| {
                serde_json::json!({
                    "pointer": card.pointer,
                    "pos": [rect.min.x, rect.min.y],
                    "collapsed": card.collapsed,
                    "rows": card.rows,
                    "more": card.more,
                })
            })
            .collect();
        serde_json::json!({
            "root": level.node_id,
            "cards": cards,
            "selected": level.selected,
            "matches": level.search.result.as_ref().map(|r| r.matches.clone()).unwrap_or_default(),
            "outline_rows": level.outline.rows.len(),
        })
    }
}
```

`Select` only records the selection for now. Task 8 adds the outline sync and the reveal.

- [ ] **Step 4: Wire it into `App` and the seam**

1. `main.rs` `App`: add the fields below, and the defaults `json_level: None`, `json_outline_visible: true`, `json_outline_width: 260.0` in `App`'s `Default`/constructor next to `current_group: None` (~1979). Import them with `use json_level::{JsonEdit, JsonLevel};` next to the Task 1 `use level::…` line.

```rust
    /// The level of the Json node `current_group` names (spec "Level
    /// sources"); `None` on every other level.
    json_level: Option<JsonLevel>,
    /// The JSON outline panel shows (local state).
    json_outline_visible: bool,
    /// Its width in points, as last drawn (local state).
    json_outline_width: f32,
```

2. `level.rs`:
   - Add a `JsonCard` variant: `/// A JSON level's card (\`json_level.rs\`).` above `JsonCard,`.
   - Give each method a JSON arm at its top:

```rust
    // level_items:
        if let Some(level) = &self.json_level {
            return (0..level.cards.len())
                .map(|i| LevelItem { id: level.card_id(i), kind: LevelItemKind::JsonCard, rect: level.rects[i] })
                .collect();
        }

    // level_edges: from the parent's row port (its right edge, at the
    // row's middle) to the child's header.
        if let Some(level) = &self.json_level {
            return level
                .cards
                .iter()
                .zip(&level.rects)
                .filter_map(|(card, rect)| {
                    let (parent, row) = card.parent?;
                    let from_rect = level.rects[parent];
                    let row_y = from_rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (row as f32 + 0.5);
                    Some(LevelEdge {
                        from: egui::pos2(from_rect.right(), row_y),
                        to: egui::pos2(rect.left(), rect.top() + json_view::HEADER_HEIGHT / 2.0),
                        curve: EdgeCurve::Horizontal,
                        highlight: level.selected.as_deref() == Some(card.pointer.as_str()),
                    })
                })
                .collect();
        }

    // layout_current_level: the tree layout; the force settle never runs here.
        if let Some(level) = self.json_level.as_mut() {
            if let Some(json) = self.nodes.get(&level.node_id).and_then(|n| n.as_json()) {
                level.relayout(&json.view);
            }
            return;
        }
```

   - Add `insertion_group`:

```rust
    /// Where a new node goes (spec "Level sources"): the level being
    /// viewed, except on a JSON level, where it's the Json node's own
    /// group. Based on `current_group`, not `json_level`, so it is right
    /// before the first frame builds the level (plan ruling 3).
    pub(crate) fn insertion_group(&self) -> Option<String> {
        if self.current_json_node().is_some() {
            return self.current_group.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.group.clone());
        }
        self.current_group.clone()
    }
```

3. Replace `self.current_group.clone()` with `self.insertion_group()` at the creation sites, and nowhere else:
   - `graph_ui.rs`: `GraphAction::NewTab` (~299) and `GraphAction::GroupSelected`'s `group:` (~333)
   - `main.rs`: the import root (~2322), `insert_group_at` (~4124), `insert_note_at` (~4152) and `add_file_at` (~4834)
   - `session_sync.rs`: the import `ImportRoot` (~1221)

   The settle key, the breadcrumb, local state and the rename shortcut keep `current_group`.

4. `graph_ui.rs`:
   - `switch_group_level`: after `self.current_group = group;`, call `self.sync_json_level();`.
   - `build_graph_ui`: its first statement becomes `self.sync_json_level();`.
   - `draw_graph_cards` (~1453): change the Json arm to the code below. The `json_viewer_node` popup stays reachable only from the tab's "View as JSON" until Task 10 removes both.

```rust
                    ItemNodeKind::Json
                        if card.response.double_clicked()
                            && self.nodes.get(&id).and_then(|n| n.as_json()).is_some_and(|j| j.key.is_none() && j.error.is_none()) =>
                    {
                        actions.push(GraphAction::EnterGroup(id.clone()));
                    }
```

   Check that `GraphAction::EnterGroup` exists as the group arm uses it: `grep -n "EnterGroup" crates/manifold/src/graph_ui.rs`. Use the same form.
   - `render_graph_toolbar`: add `let on_json_level = self.json_level.is_some();` at the top. Wrap each of these in `if !on_json_level { … }`: the "+ New Tab" button, the three group chips (Import Group, Ungroup All, Collapse/Expand Groups), and the selection cluster ("Group Selected" and "Close Selected"). Then, where the breadcrumb renders the *current* crumb (`if i + 1 == crumb_count`), allow a click to start a rename only when the current level is a group: add `&& !on_json_level` to the condition that opens the rename.
   - `handle_rect_select` (~1108): open the empty-canvas card only when `self.json_level.is_none()`. Add that to the condition that sets `self.pending_new_node = Some(…)`.
   - `handle_rect_select`'s hit match: add `Some(LevelItem { kind: LevelItemKind::JsonCard, .. }) => { self.rect_select_start = None; }` for now (Task 7 opens the JSON menu there). The minimap's `let LevelItemKind::Node(kind) = item.kind;` becomes:

```rust
            let (radius, color) = match item.kind {
                LevelItemKind::JsonCard => (2.0, ACCENT),
                LevelItemKind::Node(kind) => match kind { /* the existing arms */ },
            };
```

   - `graph_world_rect_to_screen` (plan ruling 12): subtract the outline's width from the scene rect's width. It is drawn on the right, so the rect's left stays at 0.

```rust
        let outline = if self.json_level.is_some() && self.json_outline_visible { self.json_outline_width } else { 0.0 };
        let scene_outer_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, GRAPH_TOOLBAR_HEIGHT_POINTS),
            egui::vec2((screen.x - outline).max(0.0), (screen.y - GRAPH_TOOLBAR_HEIGHT_POINTS).max(0.0)),
        );
```

5. `main.rs` `group_breadcrumb` (~3226): where a crumb's name is taken from a group, name a Json node by its file: `NodePayload::Json(json) => download_filename(&json.path).to_string()`. Read the function first and fit this in however it matches on the payload. `rename_current_group_shortcut` (~4094): return early when `self.current_json_node().is_some()`.

6. `session_sync.rs` `materialize`: in the Json arm added in Task 4, after `p.view = …`, the level must be marked stale. `p` borrows `self.nodes`, so do it after the payload `match`: `if changed(field::VIEW) { if let Some(level) = self.json_level.as_mut().filter(|l| l.node_id == id) { level.stale = true; } }`. `id` is the `String` `materialize` was given; use whatever that variable is called there. In `reload_fetched_node`, after the node is reinserted, drop the level if it showed this node so that the next sync rebuilds it from the new value: `if self.json_level.as_ref().is_some_and(|l| l.node_id == id) { self.json_level = None; }`.

7. The EXPORTSTATE card arm (`event_loop.rs`): `level_items` now yields `JsonCard`s too. Replace the `let LevelItemKind::Node(kind) = item.kind;` and `let node = …?` lines with:

```rust
                        if item.kind == LevelItemKind::JsonCard {
                            return Some(serde_json::json!({
                                "id": item.id,
                                "kind": "JsonCard",
                                "screen": [screen.center().x, screen.center().y],
                                "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                            }));
                        }
                        let LevelItemKind::Node(kind) = item.kind else { return None };
                        let node = self.nodes.get(&item.id)?;
```

   Also add `"json_level": self.json_level_state(),` to the exported `state` object.

- [ ] **Step 5: Headless commands**

`debug_control.rs`:
- Add the variants below to `DebugCommand` near `ExpandJsonAsGraph`, with doc comments.
- Add parse arms after `"CLOSEGROUP"`, using `rest` as the other arms do.
- Add help rows next to `EXPANDJSONASGRAPH`'s.

```rust
    /// Select the entry at this JSON Pointer on the current JSON level
    /// (empty: the root).
    JsonSelect(String),
    /// Collapse or expand the card at this pointer.
    JsonToggle(String),
    /// Reveal the next page of the card at this pointer.
    JsonMore(String),
    /// Clear every saved card position (the bar's Relayout).
    JsonRelayout,
    /// Search the current JSON level.
    JsonSearch(String),
```

```rust
        "JSONSELECT" => Some(DebugCommand::JsonSelect(rest.to_string())),
        "JSONTOGGLE" => Some(DebugCommand::JsonToggle(rest.to_string())),
        "JSONMORE" => Some(DebugCommand::JsonMore(rest.to_string())),
        "JSONRELAYOUT" => Some(DebugCommand::JsonRelayout),
        "JSONSEARCH" => Some(DebugCommand::JsonSearch(rest.to_string())),
```

```rust
    ("JSONSELECT [pointer]", "Select an entry on the current JSON level (no pointer: the root)"),
    ("JSONTOGGLE [pointer]", "Collapse or expand a card on the current JSON level"),
    ("JSONMORE [pointer]", "Reveal the next page of a card on the current JSON level"),
    ("JSONRELAYOUT", "Relayout the current JSON level (clears saved card positions)"),
    ("JSONSEARCH text", "Search the current JSON level's keys and values"),
```

Add a parse test beside `parses_addfile_with_an_optional_trailing_point`:

```rust
    #[test]
    fn parses_json_level_commands_with_an_optional_pointer() {
        assert!(matches!(parse_line("JSONSELECT /users/0"), Some(DebugCommand::JsonSelect(p)) if p == "/users/0"));
        assert!(matches!(parse_line("JSONTOGGLE"), Some(DebugCommand::JsonToggle(p)) if p.is_empty()));
        assert!(matches!(parse_line("JSONSEARCH two words"), Some(DebugCommand::JsonSearch(q)) if q == "two words"));
        assert!(matches!(parse_line("JSONRELAYOUT"), Some(DebugCommand::JsonRelayout)));
    }
```

`event_loop.rs`, next to `ExpandJsonAsGraph`'s arm:

```rust
            DebugCommand::JsonSelect(pointer) => self.apply_json_edit(JsonEdit::Select(pointer)),
            DebugCommand::JsonToggle(pointer) => self.apply_json_edit(JsonEdit::Toggle(pointer)),
            DebugCommand::JsonMore(pointer) => self.apply_json_edit(JsonEdit::More(pointer)),
            DebugCommand::JsonRelayout => self.apply_json_edit(JsonEdit::Relayout),
            DebugCommand::JsonSearch(text) => self.set_json_search(text),
```

`set_json_search` belongs to Task 8. Until then, add a minimal version to `json_level.rs`; Task 8 replaces its body:

```rust
    /// Runs a search on the current JSON level (Task 8 adds the worker
    /// and the outline).
    pub(crate) fn set_json_search(&mut self, query: String) {
        if let Some(level) = self.json_level.as_mut() {
            level.search.result = (!query.is_empty()).then(|| json_view::search(&level.value, &query));
            level.search.query = query;
            level.search.current = 0;
        }
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p manifold -j 12`
Expected: all pass, including the five `json_level::tests` and the parse test.

Run: `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`
Expected: clean. The level has no painter yet, so entering a JSON level shows an empty canvas with edges. Task 6 paints the cards.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "JSON level: double-click a Json node to enter it

App::json_level is the level source's second implementation: its cards
feed fit, the minimap, the hit test, EXPORTSTATE and edges; layout is
the tree layout. New nodes go beside the Json node (insertion_group),
creation controls hide on the level, and JSONSELECT/JSONTOGGLE/JSONMORE/
JSONRELAYOUT/JSONSEARCH drive it headlessly.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 6: Painting and handling input on JSON cards

**Files:**
- Create: `crates/manifold/src/json_level_ui.rs`
- Modify: `crates/manifold/src/main.rs` (`mod json_level_ui;`)
- Modify: `crates/manifold/src/graph_ui.rs` (`build_graph_ui`'s Scene closure)

**Interfaces:**
- Consumes: `JsonLevel` and `JsonEdit` (Task 5), and `json_view` (Tasks 2–3).
- Produces:
  - `ValueKind { String, Number, Literal, Container }` and `value_kind(&Value) -> ValueKind`
  - `Palette { key, string, number, literal, container, muted }`, `palette(dark: bool) -> Palette` and `Palette::of(ValueKind) -> Color32`
  - `rgb(u32) -> Color32` and `rgb_u32(Color32) -> u32` (the `0x00RRGGBB` of plan ruling 4)
  - `RowParts` and `row_parts(parent: &Value, key: &str, value: &Value) -> RowParts`
  - `HeaderParts` and `header_parts(root: &Value, card: &Card, root_name: &str) -> HeaderParts`
  - `CardLook<'a>` and `paint_json_content(painter, deferred_overlays, rect, value, header, rows, more, look)`, which Task 9 reuses for the parent-level preview
  - `App::draw_json_level(&mut self, ui, deferred_overlays, edits: &mut Vec<JsonEdit>, inv_zoom, zoom_scale)`

- [ ] **Step 1: Write the failing tests**

Create `json_level_ui.rs` holding only this test module, and add `mod json_level_ui;` after `mod json_level;` in `main.rs`:

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::json_view::{default_expanded, visible_cards, JsonView};

    #[test]
    fn rows_show_keys_or_indices_labels_and_typed_values() {
        let obj = json!({"name": "Ada", "n": 3, "ok": true, "none": null, "roles": ["a"], "empty": {}});
        let row = |k: &str| row_parts(&obj, k, &obj[k]);
        assert_eq!(
            row("name"),
            RowParts {
                key: Some("name".into()),
                index: None,
                label: None,
                value: "\"Ada\"".into(),
                kind: ValueKind::String,
                port: false,
            }
        );
        assert_eq!(row("n").kind, ValueKind::Number);
        assert_eq!((row("ok").kind, row("none").kind), (ValueKind::Literal, ValueKind::Literal));
        // An object's container row has a port but no label (spec mockup).
        let roles = row("roles");
        assert_eq!((roles.value.as_str(), roles.kind, roles.port, roles.label), ("[1]", ValueKind::Container, true, None));
        assert!(!row("empty").port, "{{}} shows inline, with no card");

        let arr = json!([{"name": "Ada"}, [-0.12, 51.5], 7]);
        let element = row_parts(&arr, "0", &arr[0]);
        assert_eq!((element.key, element.index.as_deref(), element.label.as_deref()), (None, Some("0"), Some("\"Ada\"")));
        assert_eq!(row_parts(&arr, "1", &arr[1]).label.as_deref(), Some("[-0.12, 51.5]"));
        assert_eq!(row_parts(&arr, "2", &arr[2]).label, None, "scalars don't label");
    }

    #[test]
    fn headers_show_title_count_and_a_label_when_collapsed() {
        let root = json!({"users": [{"name": "Ada", "id": 1}]});
        let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
        let header = header_parts(&root, &cards[0], "users.json");
        assert_eq!((header.title.as_str(), header.count.as_str(), header.label), ("users.json", "{1}", None));
        let mut user = cards[2].clone();
        assert_eq!(user.pointer, "/users/0");
        user.collapsed = true;
        let header = header_parts(&root, &user, "users.json");
        assert_eq!((header.title.as_str(), header.count.as_str(), header.label.as_deref()), ("users[0]", "{2}", Some("\"Ada\"")));

        let scalar = json!(42);
        let cards = visible_cards(&scalar, &JsonView::default(), &default_expanded(&scalar));
        let header = header_parts(&scalar, &cards[0], "n.json");
        assert_eq!((header.count.as_str(), header.count_kind), ("42", ValueKind::Number));
    }

    #[test]
    fn palettes_follow_the_theme_and_colours_round_trip() {
        assert_eq!(palette(true).key, egui::Color32::from_rgb(0x9c, 0xdc, 0xfe));
        assert_eq!(palette(false).key, egui::Color32::from_rgb(0x04, 0x51, 0xa5));
        let c = egui::Color32::from_rgb(1, 2, 3);
        assert_eq!(rgb_u32(c), 0x010203);
        assert_eq!(rgb(rgb_u32(c)), c);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p manifold -j 12 json_level_ui::tests`
Expected: compile errors for the missing items.

- [ ] **Step 3: Write the model half of `json_level_ui.rs`**

Put this above the tests:

```rust
//! A JSON level's painter and input (spec "Cards"): record cards drawn
//! in the Scene, their text queued through `deferred_overlays` (plan
//! ruling 11). The bar, the menu, keys (Task 7) and the outline (Task 8)
//! live here too.

use serde_json::Value;

use super::*;
use crate::json_level::JsonEdit;
use crate::json_view::{self, Card};

/// Text height in world units (plan ruling 11).
const TEXT_SIZE: f32 = 12.0;
/// Inset of a card's text from its edges.
const PAD: f32 = 8.0;
/// An object row's key column.
const KEY_WIDTH: f32 = 84.0;
/// An array row's index column, right-aligned.
const INDEX_WIDTH: f32 = 22.0;
/// A container row's `{n}` / `[n]` column.
const COUNT_WIDTH: f32 = 44.0;
/// Below this zoom, rows are gray bars without text (spec "Level of
/// detail").
const LOD_TEXT_ZOOM: f32 = 0.5;
/// A search hit's border and row wash: blue, apart from the selection's
/// ACCENT gold.
const SEARCH_HIT: egui::Color32 = egui::Color32::from_rgb(90, 150, 230);
/// Mixed into an array card's fill so array and object cards read apart.
const ARRAY_TINT: egui::Color32 = egui::Color32::from_rgb(90, 100, 160);
/// A row tooltip's cap, in characters.
const TOOLTIP_MAX_CHARS: usize = 2000;

/// Which colour a value's text takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    String,
    Number,
    /// `true`, `false` and `null`.
    Literal,
    Container,
}

pub(crate) fn value_kind(value: &Value) -> ValueKind {
    match value {
        Value::String(_) => ValueKind::String,
        Value::Number(_) => ValueKind::Number,
        Value::Bool(_) | Value::Null => ValueKind::Literal,
        Value::Array(_) | Value::Object(_) => ValueKind::Container,
    }
}

/// `0x00RRGGBB` to a colour (plan ruling 4).
pub(crate) fn rgb(hex: u32) -> egui::Color32 {
    let [_, r, g, b] = hex.to_be_bytes();
    egui::Color32::from_rgb(r, g, b)
}

/// A colour as `0x00RRGGBB`, the way `session_sync::color_value` stores one.
pub(crate) fn rgb_u32(color: egui::Color32) -> u32 {
    u32::from_be_bytes([0, color.r(), color.g(), color.b()])
}

/// The level's text colours: VS Code's JSON colours for the theme.
pub(crate) struct Palette {
    pub(crate) key: egui::Color32,
    pub(crate) string: egui::Color32,
    pub(crate) number: egui::Color32,
    pub(crate) literal: egui::Color32,
    pub(crate) container: egui::Color32,
    /// Indices, labels, counts and `+ N more`.
    pub(crate) muted: egui::Color32,
}

pub(crate) fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            key: rgb(0x9cdcfe),
            string: rgb(0xce9178),
            number: rgb(0xb5cea8),
            literal: rgb(0x569cd6),
            container: rgb(0xc586c0),
            muted: rgb(0x8a8a8a),
        }
    } else {
        Palette {
            key: rgb(0x0451a5),
            string: rgb(0xa31515),
            number: rgb(0x098658),
            literal: rgb(0x0000ff),
            container: rgb(0xaf00db),
            muted: rgb(0x6e6e6e),
        }
    }
}

impl Palette {
    pub(crate) fn of(&self, kind: ValueKind) -> egui::Color32 {
        match kind {
            ValueKind::String => self.string,
            ValueKind::Number => self.number,
            ValueKind::Literal => self.literal,
            ValueKind::Container => self.container,
        }
    }
}

/// What one row shows (spec "Cards").
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowParts {
    /// An object entry's key.
    pub(crate) key: Option<String>,
    /// An array element's index.
    pub(crate) index: Option<String>,
    /// A container element's label (the label rule).
    pub(crate) label: Option<String>,
    /// `display_text`: a scalar as JSON, or `{n}` / `[n]`.
    pub(crate) value: String,
    pub(crate) kind: ValueKind,
    /// A child card hangs off this row's port.
    pub(crate) port: bool,
}

/// The row for entry `key` (an index for an array) of `parent`.
pub(crate) fn row_parts(parent: &Value, key: &str, value: &Value) -> RowParts {
    let in_array = parent.is_array();
    let kind = value_kind(value);
    RowParts {
        key: (!in_array).then(|| key.to_string()),
        index: in_array.then(|| key.to_string()),
        label: if in_array && kind == ValueKind::Container { json_view::label(value) } else { None },
        value: json_view::display_text(value),
        kind,
        port: json_view::is_card(value),
    }
}

/// What a card's header shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HeaderParts {
    pub(crate) icon: &'static str,
    pub(crate) title: String,
    /// The label rule, only while collapsed.
    pub(crate) label: Option<String>,
    /// `{n}` / `[n]`, or a scalar root's value.
    pub(crate) count: String,
    pub(crate) count_kind: ValueKind,
}

pub(crate) fn header_parts(root: &Value, card: &Card, root_name: &str) -> HeaderParts {
    let value = root.pointer(&card.pointer).unwrap_or(&Value::Null);
    HeaderParts {
        icon: if value.is_array() {
            egui_phosphor::light::BRACKETS_SQUARE
        } else {
            egui_phosphor::light::BRACKETS_CURLY
        },
        title: json_view::card_title(root, &card.pointer, root_name),
        label: if card.collapsed { json_view::label(value) } else { None },
        count: json_view::display_text(value),
        count_kind: value_kind(value),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p manifold -j 12 json_level_ui::tests`
Expected: 3 passed.

- [ ] **Step 5: Write the painter**

Append to `json_level_ui.rs`, above the tests:

```rust
/// One piece of a card's text, in world units.
struct TextItem {
    pos: egui::Pos2,
    align: egui::Align2,
    text: String,
    color: egui::Color32,
    /// Cut to this width with `…`.
    max_width: f32,
}

/// Queues a card's text as one overlay. The scale comes from `project`
/// at paint time, so the text never lags the card by a frame.
fn push_card_text(deferred_overlays: &mut DeferredOverlays, items: Vec<TextItem>) {
    if items.is_empty() {
        return;
    }
    deferred_overlays.push(Box::new(move |painter, project| {
        let scale = project(egui::pos2(1.0, 0.0)).x - project(egui::Pos2::ZERO).x;
        let font = egui::FontId::proportional(TEXT_SIZE * scale);
        for item in items {
            let mut job = egui::text::LayoutJob::simple_singleline(item.text, font.clone(), item.color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(item.max_width * scale);
            let galley = painter.layout_job(job);
            let min = item.align.anchor_size(project(item.pos), galley.size()).min;
            painter.galley(min, galley, item.color);
        }
    }));
}

/// How one card looks this frame.
pub(crate) struct CardLook<'a> {
    pub(crate) palette: &'a Palette,
    pub(crate) text_color: egui::Color32,
    pub(crate) inv_zoom: f32,
    /// Text, or gray bars (level of detail).
    pub(crate) text: bool,
    pub(crate) selected_row: Option<usize>,
    pub(crate) hit_rows: &'a [usize],
}

/// A JSON level card's background, header tint and border.
fn paint_json_frame(
    painter: &egui::Painter,
    rect: egui::Rect,
    fill: egui::Color32,
    tint: Option<egui::Color32>,
    border: (f32, egui::Color32),
    inv_zoom: f32,
) {
    let rounding = 2.0 * inv_zoom;
    painter.rect_filled(rect, rounding, fill);
    if let Some(tint) = tint {
        let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), json_view::HEADER_HEIGHT));
        painter.rect_filled(header, rounding, tint.gamma_multiply(0.35));
    }
    painter.rect_stroke(
        rect,
        rounding,
        egui::Stroke::new(border.0 * inv_zoom, border.1),
        egui::StrokeKind::Inside,
    );
}

/// A card's contents (the frame is the caller's): the header, then the
/// first `rows` entries of `value`, then `+ N more` when `more > 0`.
/// Shared by the JSON level and the Json card at its parent's level.
#[allow(clippy::too_many_arguments)] // one card's full description; a struct for it would only rename these
pub(crate) fn paint_json_content(
    painter: &egui::Painter,
    deferred_overlays: &mut DeferredOverlays,
    rect: egui::Rect,
    value: &Value,
    header: &HeaderParts,
    rows: usize,
    more: usize,
    look: &CardLook,
) {
    use json_view::{HEADER_HEIGHT, ROW_HEIGHT};
    let pal = look.palette;
    let row_rect = |row: usize| {
        egui::Rect::from_min_size(
            rect.min + egui::vec2(0.0, HEADER_HEIGHT + ROW_HEIGHT * row as f32),
            egui::vec2(rect.width(), ROW_HEIGHT),
        )
    };
    if rows + usize::from(more > 0) > 0 {
        painter.hline(rect.x_range(), rect.top() + HEADER_HEIGHT, egui::Stroke::new(look.inv_zoom, pal.muted.gamma_multiply(0.4)));
    }
    for &row in look.hit_rows {
        painter.rect_filled(row_rect(row), 0.0, SEARCH_HIT.gamma_multiply(0.18));
    }
    if let Some(row) = look.selected_row {
        painter.rect_filled(row_rect(row), 0.0, ACCENT.gamma_multiply(0.22));
    }

    let mut text = Vec::new();
    let inner = rect.width() - 2.0 * PAD;
    let left = rect.left() + PAD;
    let right = rect.right() - PAD;
    if look.text {
        let y = rect.top() + HEADER_HEIGHT / 2.0;
        let count_color = if header.count_kind == ValueKind::Container { pal.muted } else { pal.of(header.count_kind) };
        text.push(TextItem { pos: egui::pos2(left, y), align: egui::Align2::LEFT_CENTER, text: header.icon.into(), color: pal.container, max_width: 16.0 });
        let title_width = if header.label.is_some() { 88.0 } else { inner - 18.0 - COUNT_WIDTH };
        text.push(TextItem { pos: egui::pos2(left + 18.0, y), align: egui::Align2::LEFT_CENTER, text: header.title.clone(), color: look.text_color, max_width: title_width });
        if let Some(label) = &header.label {
            text.push(TextItem { pos: egui::pos2(left + 110.0, y), align: egui::Align2::LEFT_CENTER, text: label.clone(), color: pal.muted, max_width: inner - 110.0 - COUNT_WIDTH });
        }
        text.push(TextItem { pos: egui::pos2(right, y), align: egui::Align2::RIGHT_CENTER, text: header.count.clone(), color: count_color, max_width: COUNT_WIDTH });
    }
    for (row, (key, child)) in json_view::entries(value).take(rows).enumerate() {
        let r = row_rect(row);
        let y = r.center().y;
        let parts = row_parts(value, &key, child);
        if parts.port {
            painter.circle_filled(egui::pos2(r.right(), y), 3.5, pal.container);
        }
        if !look.text {
            painter.rect_filled(r.shrink2(egui::vec2(PAD, 7.0)), 2.0, pal.muted.gamma_multiply(0.35));
            continue;
        }
        let mut used = 0.0;
        if let Some(key) = parts.key {
            text.push(TextItem { pos: egui::pos2(left, y), align: egui::Align2::LEFT_CENTER, text: key, color: pal.key, max_width: KEY_WIDTH });
            used = KEY_WIDTH + 6.0;
        }
        if let Some(index) = parts.index {
            text.push(TextItem { pos: egui::pos2(left + INDEX_WIDTH, y), align: egui::Align2::RIGHT_CENTER, text: index, color: pal.muted, max_width: INDEX_WIDTH });
            used = INDEX_WIDTH + 6.0;
        }
        let value_width = if parts.kind == ValueKind::Container { COUNT_WIDTH } else { inner - used };
        if let Some(label) = parts.label {
            text.push(TextItem { pos: egui::pos2(left + used, y), align: egui::Align2::LEFT_CENTER, text: label, color: pal.muted, max_width: inner - used - COUNT_WIDTH - 6.0 });
        }
        text.push(TextItem { pos: egui::pos2(right, y), align: egui::Align2::RIGHT_CENTER, text: parts.value, color: pal.of(parts.kind), max_width: value_width });
    }
    if more > 0 {
        let r = row_rect(rows);
        if look.text {
            text.push(TextItem { pos: egui::pos2(left, r.center().y), align: egui::Align2::LEFT_CENTER, text: format!("+ {more} more"), color: pal.muted, max_width: inner });
        } else {
            painter.rect_filled(r.shrink2(egui::vec2(PAD, 7.0)), 2.0, pal.muted.gamma_multiply(0.2));
        }
    }
    push_card_text(deferred_overlays, text);
}

impl App {
    /// The JSON level's cards, inside `build_graph_ui`'s Scene: painted,
    /// culled to the viewport, and their clicks and drags turned into
    /// `edits`, applied once the Scene closure returns. A drag moves the
    /// card live (its rect, so edges and the minimap follow) and saves
    /// its position on release.
    pub(crate) fn draw_json_level(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        edits: &mut Vec<JsonEdit>,
        inv_zoom: f32,
        zoom_scale: f32,
    ) {
        let Some(level) = self.json_level.as_ref() else { return };
        let Some(json) = self.nodes.get(&level.node_id).and_then(|n| n.as_json()) else { return };
        let root_name = download_filename(&json.path).to_string();
        let pal = palette(ui.visuals().dark_mode);
        let base_fill = ui.visuals().extreme_bg_color;
        let idle_border = ui.visuals().weak_text_color();
        let text_color = ui.visuals().text_color();
        let hits = level.search.result.as_ref().map(|r| &r.hits);
        let mut moves = Vec::new();
        for (i, card) in level.cards.iter().enumerate() {
            let rect = level.rects[i];
            if !rect.intersects(self.camera.rect) {
                continue;
            }
            let Some(value) = level.value.pointer(&card.pointer) else { continue };
            let shown: Vec<String> = json_view::entries(value)
                .take(card.rows)
                .map(|(key, _)| json_view::child_pointer(&card.pointer, &key))
                .collect();
            let selected_row = level.selected.as_ref().and_then(|s| shown.iter().position(|p| p == s));
            let hit_rows: Vec<usize> = match hits {
                Some(hits) => shown.iter().enumerate().filter(|(_, p)| hits.contains(*p)).map(|(r, _)| r).collect(),
                None => Vec::new(),
            };
            let border = if level.selected.as_deref() == Some(card.pointer.as_str()) {
                (2.0, ACCENT)
            } else if hits.is_some_and(|h| h.contains(&card.pointer)) || !hit_rows.is_empty() {
                (2.0, SEARCH_HIT)
            } else {
                (1.0, idle_border)
            };
            let fill = if value.is_array() { base_fill.lerp_to_gamma(ARRAY_TINT, 0.12) } else { base_fill };
            let tint = json.view.get(&card.pointer).and_then(|o| o.color).map(rgb);
            paint_json_frame(ui.painter(), rect, fill, tint, border, inv_zoom);
            let look = CardLook {
                palette: &pal,
                text_color,
                inv_zoom,
                text: zoom_scale >= LOD_TEXT_ZOOM,
                selected_row,
                hit_rows: &hit_rows,
            };
            let header = header_parts(&level.value, card, &root_name);
            paint_json_content(ui.painter(), deferred_overlays, rect, value, &header, card.rows, card.more, &look);

            let response =
                ui.interact(rect, egui::Id::new(("json_card", &level.node_id, &card.pointer)), egui::Sense::click_and_drag());
            let at = response.interact_pointer_pos().or(response.hover_pos());
            let in_header = at.is_some_and(|p| p.y < rect.top() + json_view::HEADER_HEIGHT);
            if response.double_clicked() && in_header && json_view::entry_count(value) > 0 {
                edits.push(JsonEdit::Toggle(card.pointer.clone()));
            } else if response.clicked() {
                if let Some(p) = at {
                    if level.more_row_at(i, p) {
                        edits.push(JsonEdit::More(card.pointer.clone()));
                    } else {
                        edits.push(JsonEdit::Select(level.row_at(i, p).unwrap_or_else(|| card.pointer.clone())));
                    }
                }
            }
            if response.dragged() {
                moves.push((i, response.drag_delta()));
            }
            if response.drag_stopped() {
                edits.push(JsonEdit::Moved(card.pointer.clone(), rect.min));
            }
            // A scalar row's full text; the row itself is cut to fit.
            let tooltip = response
                .hover_pos()
                .filter(|_| !response.dragged())
                .and_then(|p| level.row_at(i, p))
                .and_then(|p| level.value.pointer(&p))
                .and_then(json_view::scalar_text)
                .map(|t| if t.chars().count() > TOOLTIP_MAX_CHARS { format!("{}…", t.chars().take(TOOLTIP_MAX_CHARS).collect::<String>()) } else { t });
            if let Some(tooltip) = tooltip {
                response.on_hover_text(tooltip);
            }
        }
        if let Some(level) = self.json_level.as_mut() {
            for (i, delta) in moves {
                level.rects[i] = level.rects[i].translate(delta);
            }
        }
    }
}
```

`JsonLevel::more_row_at` and `row_at` are Task 5's. Clicking a `+ N more` row pages the card. Clicking a header or an empty spot selects the card, and clicking a row selects that row.

- [ ] **Step 6: Wire it into `build_graph_ui`**

In `build_graph_ui`, declare `let mut json_edits = Vec::new();` before `let scene_response = egui::Scene::new()`. Inside the Scene closure, replace the `draw_graph_cards`, `draw_pending_new_node_menu` and `draw_peek_preview` calls with:

```rust
                if self.json_level.is_some() {
                    self.draw_json_level(ui, &mut deferred_overlays, &mut json_edits, inv_zoom, zoom_scale);
                } else {
                    self.draw_graph_cards(
                        ui,
                        &mut deferred_overlays,
                        &mut actions,
                        &at_level,
                        inv_zoom,
                        zoom_scale,
                        leave_group_button_rect,
                    );
                    self.draw_pending_new_node_menu(ui, &mut deferred_overlays, &mut actions, inv_zoom, &project);
                    self.draw_peek_preview(ui);
                }
```

After `for action in actions { … }`, add `for edit in json_edits { self.apply_json_edit(edit); }`.

The edges need no change: `draw_graph_edges` draws `level_edges()` (Task 1) with the `Horizontal` curve, and `highlight` draws in ACCENT.

- [ ] **Step 7: Build and check it by eye**

Run: `cargo build -p manifold -j 12`, then `cargo test -p manifold -j 12` and `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`.
Expected: all clean.

Then write `target/json-shot.txt` with the lines below and run it with `rtk proxy bash scripts/drive.sh target/json-shot.txt`. Before writing it, create `target/shot.json` holding `{"page": 1, "users": [{"id": 1, "name": "Ada Lovelace", "roles": ["admin", "dev"], "address": {"city": "London"}}, {"id": 2, "name": "Alan Turing"}], "meta": {"bbox": [[-0.12, 51.5], [0.33, 51.7]]}}`.

```
RESIZE 1200 800
ADDFILE {repo}\target\shot.json 400 300
sleep 1
!(while [ ! -f target/shot-state.json ]; do sleep 0.2; done) &
EXPORTSTATE {repo}\target\shot-state.json
```

Read the Json card's `screen` point from `target/shot-state.json` (the card whose `kind` is `Json`). Then append these lines, replacing `X Y` with that point, and run the file again:

```
DOUBLECLICK X Y
sleep 1
CLICK 326 17
sleep 1
SCREENSHOT {repo}\target\shots\json-level.png
```

Open `target/shots/json-level.png` with the Read tool. Check that the cards show typed colours, `users` is an array card with index rows, edges run from row ports to headers, and the text sits inside the cards. Fix what's off before committing. Screenshots are git-ignored under `target/`.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "JSON level: paint record cards and handle clicks and drags

Typed colours, array cards tinted apart, row ports, + N more, gray bars
below zoom 0.5; double-click a header to collapse, click to select or
page, drag to move (saved on release).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: The bar, copying, the context menu and keys

**Files:**
- Modify: `crates/manifold/src/json_level_ui.rs`
- Modify: `crates/manifold/src/json_level.rs` (`card_at`)
- Modify: `crates/manifold/src/graph_ui.rs` (`render_graph_toolbar`, `handle_graph_keys`, the `JsonCard` arm of `handle_rect_select`, `build_graph_ui`)
- Modify: `crates/manifold/src/local_state.rs`, `crates/manifold/src/session_sync.rs` (`local_state`, `apply_local`), `crates/manifold/src/main.rs` (the `App` field)

**Interfaces:**
- Consumes: Tasks 5–6.
- Produces:
  - `App.json_accessor_paths: bool`, saved as `LocalState.json_accessor_paths`
  - `JsonLevel::card_at(&self, world: egui::Pos2) -> Option<usize>`
  - `App::json_copy_text(&self, pointer: &str, what: CopyWhat) -> Option<String>`, with `pub(crate) enum CopyWhat { Value, Key, Path }`
  - `App::render_json_bar(&mut self, ui)`, `App::build_json_menu(&mut self, ui)` and `App::handle_json_keys(&mut self, ui)`

- [ ] **Step 1: Write the failing tests** (add to `json_level.rs`'s tests, which have `app_with_json`)

```rust
    #[test]
    fn copy_texts_follow_the_selection_and_the_path_form() {
        let (mut app, j) = app_with_json(r#"{"users": [{"name": "Ada", "a/b": 1}]}"#);
        app.switch_group_level(Some(j));
        let copy = |app: &App, p: &str, what| app.json_copy_text(p, what);
        assert_eq!(copy(&app, "/users/0/name", CopyWhat::Value).as_deref(), Some("Ada"));
        assert_eq!(copy(&app, "/users/0", CopyWhat::Value).as_deref(), Some("{\n  \"name\": \"Ada\",\n  \"a/b\": 1\n}"));
        assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Key).as_deref(), Some("a/b"));
        assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Path).as_deref(), Some("/users/0/a~1b"));
        app.json_accessor_paths = true;
        assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Path).as_deref(), Some(r#"users[0]["a/b"]"#));
        assert_eq!(copy(&app, "/nope", CopyWhat::Value), None);
    }

    #[test]
    fn the_topmost_card_under_a_point_is_hit() {
        let (mut app, j) = app_with_json(r#"{"a": {"x": 1}}"#);
        app.switch_group_level(Some(j));
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(0.0, 0.0))); // onto the root
        let level = app.json_level.as_ref().unwrap();
        assert_eq!(level.card_at(egui::pos2(5.0, 5.0)), Some(1), "later cards draw on top");
        assert_eq!(level.card_at(egui::pos2(-50.0, -50.0)), None);
    }
```

And to `local_state.rs`'s tests, following its existing round-trip test:

```rust
    #[test]
    fn json_level_preferences_round_trip_and_default_off() {
        let state = LocalState { json_accessor_paths: true, json_outline_hidden: true, json_outline_width: Some(310.0), ..LocalState::default() };
        let back = LocalState::from_bytes(&state.to_bytes()).unwrap();
        assert_eq!((back.json_accessor_paths, back.json_outline_hidden, back.json_outline_width), (true, true, Some(310.0)));
        let old = LocalState::from_bytes(br#"{}"#).unwrap();
        assert_eq!((old.json_accessor_paths, old.json_outline_hidden, old.json_outline_width), (false, false, None));
    }
```

If `from_bytes` doesn't take plain JSON, build `old` the way the file's other tests build a minimal state. Task 8 uses the two outline fields; adding all three here keeps `LocalState` to a single change.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p manifold -j 12 json_level::tests local_state`
Expected: compile errors.

- [ ] **Step 3: Implement**

1. `local_state.rs` `LocalState`: add these fields, each under `#[serde(default)]` like their neighbours:

```rust
    /// Copy path on a JSON level gives the accessor form
    /// (`users[0].name`) rather than a JSON Pointer.
    #[serde(default)]
    pub json_accessor_paths: bool,
    /// The JSON outline panel is hidden.
    #[serde(default)]
    pub json_outline_hidden: bool,
    /// The JSON outline panel's width, once resized.
    #[serde(default)]
    pub json_outline_width: Option<f32>,
```

   In `session_sync.rs` `local_state()`, add `json_accessor_paths: self.json_accessor_paths, json_outline_hidden: !self.json_outline_visible, json_outline_width: Some(self.json_outline_width),`. In `apply_local`, add `self.json_accessor_paths = state.json_accessor_paths; self.json_outline_visible = !state.json_outline_hidden; if let Some(width) = state.json_outline_width { self.json_outline_width = width; }`. Add `json_accessor_paths: bool` to `App`, with the doc comment `/// Copy path's form on a JSON level (local state).`, defaulting to `false`.

2. `json_level.rs`: add to `impl JsonLevel`:

```rust
    /// The topmost card under `world`: later cards draw on top.
    pub(crate) fn card_at(&self, world: egui::Pos2) -> Option<usize> {
        self.rects.iter().rposition(|rect| rect.contains(world))
    }
```

   Add to `impl App`:

```rust
    /// What a copy puts on the clipboard for the entry at `pointer`
    /// (spec "Copy"); `None` if it doesn't resolve.
    pub(crate) fn json_copy_text(&self, pointer: &str, what: CopyWhat) -> Option<String> {
        let level = self.json_level.as_ref()?;
        let value = level.value.pointer(pointer)?;
        Some(match what {
            CopyWhat::Value => json_view::copy_value_text(value),
            CopyWhat::Key => json_view::last_key(pointer).unwrap_or_default(),
            CopyWhat::Path if self.json_accessor_paths => json_view::accessor_path(&level.value, pointer),
            CopyWhat::Path => pointer.to_string(),
        })
    }
```

   And the enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyWhat {
    Value,
    Key,
    Path,
}
```

3. `json_level_ui.rs`: the bar, the menu and the keys.

```rust
impl App {
    /// The JSON level's part of the graph toolbar, after the breadcrumb
    /// (spec "Copy"): the selected path, Copy value, Copy path with its
    /// form menu, Relayout and the outline toggle.
    pub(crate) fn render_json_bar(&mut self, ui: &mut egui::Ui) {
        let Some(level) = self.json_level.as_ref() else { return };
        let selected = level.selected.clone();
        ui.separator();
        let shown = selected.as_deref().map(|p| if p.is_empty() { "/" } else { p }).unwrap_or("nothing selected");
        ui.add(egui::Label::new(egui::RichText::new(shown).monospace().weak()).truncate())
            .on_hover_text(shown);
        ui.add_enabled_ui(selected.is_some(), |ui| {
            if ui.button("Copy value").on_hover_text("Ctrl+C").clicked() {
                self.copy_json(ui.ctx(), selected.as_deref(), CopyWhat::Value);
            }
            if ui.button("Copy path").on_hover_text("Ctrl+Shift+C").clicked() {
                self.copy_json(ui.ctx(), selected.as_deref(), CopyWhat::Path);
            }
        });
        ui.menu_button("▾", |ui| {
            if ui.selectable_label(!self.json_accessor_paths, "JSON Pointer  /users/0/name").clicked() {
                self.json_accessor_paths = false;
                ui.close();
            }
            if ui.selectable_label(self.json_accessor_paths, "Accessor  users[0].name").clicked() {
                self.json_accessor_paths = true;
                ui.close();
            }
        });
        if ui.button("Relayout").on_hover_text("Forget every moved card's position").clicked() {
            self.apply_json_edit(JsonEdit::Relayout);
        }
        let outline = egui_phosphor::light::LIST_BULLETS;
        if ui.selectable_label(self.json_outline_visible, outline).on_hover_text("Outline").clicked() {
            self.json_outline_visible = !self.json_outline_visible;
        }
    }

    fn copy_json(&mut self, ctx: &egui::Context, pointer: Option<&str>, what: CopyWhat) {
        if let Some(text) = pointer.and_then(|p| self.json_copy_text(p, what)) {
            ctx.copy_text(text);
        }
    }

    /// The right-click menu of a JSON card or row: Copy value / key /
    /// path, Collapse or Expand (cards), and the colour swatches (cards).
    /// Closes on a click elsewhere or Escape.
    pub(crate) fn build_json_menu(&mut self, ui: &mut egui::Ui) {
        let Some((pointer, pos)) = self.json_level.as_ref().and_then(|l| l.menu.clone()) else { return };
        let is_card = self.json_level.as_ref().is_some_and(|l| l.index.contains_key(&pointer));
        let collapsed = self.json_level.as_ref().and_then(|l| l.index.get(&pointer).map(|&i| l.cards[i].collapsed));
        let color = self.current_json_node().and_then(|j| j.view.get(&pointer)).and_then(|o| o.color).map(rgb);
        let mut close = false;
        let mut edit = None;
        let area = egui::Area::new(egui::Id::new("json_context_menu")).order(egui::Order::Foreground).fixed_pos(pos).show(
            ui.ctx(),
            |ui| {
                popup_frame(ui).show(ui, |ui| {
                    ui.set_min_width(150.0);
                    for (label, what) in [("Copy value", CopyWhat::Value), ("Copy key", CopyWhat::Key), ("Copy path", CopyWhat::Path)] {
                        if what == CopyWhat::Key && pointer.is_empty() {
                            continue;
                        }
                        if ui.button(label).clicked() {
                            self.copy_json(ui.ctx(), Some(&pointer), what);
                            close = true;
                        }
                    }
                    if let Some(collapsed) = collapsed.filter(|_| is_card) {
                        ui.separator();
                        if ui.button(if collapsed { "Expand" } else { "Collapse" }).clicked() {
                            edit = Some(JsonEdit::Toggle(pointer.clone()));
                            close = true;
                        }
                    }
                    if is_card {
                        if let Some(picked) = color_swatch_row(ui, color) {
                            edit = Some(JsonEdit::Color(pointer.clone(), picked.map(rgb_u32)));
                            close = true;
                        }
                    }
                });
            },
        );
        if area.response.clicked_elsewhere() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if let Some(edit) = edit {
            self.apply_json_edit(edit);
        }
        if close {
            if let Some(level) = self.json_level.as_mut() {
                level.menu = None;
            }
        }
    }

    /// Keys on a JSON level, in place of the graph's arrow-key focus
    /// moves: copy (plan ruling 13), Ctrl+F for search, Escape to clear
    /// the selection. Skipped while a text field has the keyboard.
    pub(crate) fn handle_json_keys(&mut self, ui: &egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let copy = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)).then_some(i.modifiers.shift));
        if let Some(shift) = copy {
            let selected = self.json_level.as_ref().and_then(|l| l.selected.clone());
            self.copy_json(ui.ctx(), selected.as_deref(), if shift { CopyWhat::Path } else { CopyWhat::Value });
        }
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            self.json_outline_visible = true;
            if let Some(level) = self.json_level.as_mut() {
                level.search.focus = true;
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(level) = self.json_level.as_mut() {
                level.menu = None;
                level.selected = None;
            }
        }
    }
}
```

   `popup_frame` is `theme.rs`'s shared menu frame. Check that it takes `&egui::Ui` and that `ui.close()` is how this codebase closes a `menu_button` in egui 0.36 (`grep -n "menu_button" -A6 crates/manifold/src/*.rs`); if the code closes menus another way, do it that way. `color_swatch_row` is `main.rs`'s. Import both the way the other `json_level_ui.rs` imports come in (`use super::*;` covers items in `main.rs`, and `theme`'s items are re-exported there if the other modules use them unqualified).

4. `graph_ui.rs`:
   - `render_graph_toolbar`: right after the breadcrumb's `for` loop ends (still inside `if !breadcrumb.is_empty()`), add `if self.json_level.is_some() { self.render_json_bar(ui); }`.
   - `handle_graph_keys`: first statement: `if self.json_level.is_some() { self.handle_json_keys(ui); return; }`.
   - `handle_rect_select`: replace Task 5's `JsonCard` arm with:

```rust
                    Some(LevelItem { kind: LevelItemKind::JsonCard, .. }) => {
                        self.rect_select_start = None;
                        if let Some(level) = self.json_level.as_mut() {
                            if let Some(i) = level.card_at(world_pos) {
                                let target = level.row_at(i, world_pos).unwrap_or_else(|| level.cards[i].pointer.clone());
                                level.selected = Some(target.clone());
                                level.menu = Some((target, pos));
                            }
                        }
                    }
```

   and, in the arm for a press on empty canvas, close the menu too: `if let Some(level) = self.json_level.as_mut() { level.menu = None; }`.
   - `build_graph_ui`: after `self.build_node_menu(ui);`, add `self.build_json_menu(ui);`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p manifold -j 12` and `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`.
Expected: all pass, clean.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "JSON level: bar, copy value/key/path, context menu and keys

Ctrl+C copies the selection's value and Ctrl+Shift+C its path, as a JSON
Pointer or accessor (a local preference); right-click gives copy,
collapse and a header colour.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: The outline and search

**Files:**
- Modify: `crates/manifold/src/json_level.rs` (`set_json_search`, `poll_json_search`, `step_json_match`, the `Select` edit, the node count)
- Modify: `crates/manifold/src/json_level_ui.rs` (`build_json_outline`)
- Modify: `crates/manifold/src/graph_ui.rs` (`build_graph_ui`)

**Interfaces:**
- Consumes: Tasks 3, 5 and 7.
- Produces:
  - `JsonLevel.nodes: usize` (`count_nodes` of the value, computed once)
  - `App::set_json_search(&mut self, String)` (replacing Task 5's stand-in), `App::poll_json_search(&mut self)`, `App::step_json_match(&mut self)` and `App::reveal_json(&mut self, pointer: &str)`
  - `JsonEdit::Select` now reveals, pans and syncs the outline
  - `App::build_json_outline(&mut self, ui)`

- [ ] **Step 1: Write the failing tests** (in `json_level.rs`'s tests)

```rust
    #[test]
    fn selecting_a_hidden_entry_reveals_it_and_saves_the_reveal() {
        let users: Vec<_> = (0..45).map(|i| serde_json::json!({"name": format!("u{i}")})).collect();
        let (mut app, j) = app_with_json(&serde_json::json!({ "users": users }).to_string());
        app.switch_group_level(Some(j.clone()));
        app.apply_json_edit(JsonEdit::Toggle("/users".into()));
        app.apply_json_edit(JsonEdit::Select("/users/33/name".into()));
        let level = app.json_level.as_ref().unwrap();
        assert!(level.index.contains_key("/users/33"), "the entry's card is on the canvas");
        assert_eq!(level.selected.as_deref(), Some("/users/33/name"));
        for open in ["", "/users", "/users/33"] {
            assert!(level.outline.open.contains(open), "the outline opens {open:?}");
        }
        let view = &app.nodes[&j].as_json().unwrap().view;
        assert_eq!(view.pages("/users"), 2, "the reveal is saved like a manual one");
    }

    #[test]
    fn search_steps_through_matches_and_filters_the_outline() {
        let (mut app, j) = app_with_json(r#"{"a": {"name": "x1"}, "b": {"name": "x2"}, "c": 3}"#);
        app.switch_group_level(Some(j));
        app.set_json_search("X".into());
        app.poll_json_search();
        let rows = |app: &App| app.json_level.as_ref().unwrap().outline.rows.iter().map(|r| r.pointer.clone()).collect::<Vec<_>>();
        assert_eq!(rows(&app), ["", "/a", "/a/name", "/b", "/b/name"]);
        app.step_json_match();
        assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/a/name"));
        app.step_json_match();
        assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/b/name"));
        app.step_json_match();
        assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/a/name"), "wraps around");
        app.set_json_search(String::new());
        app.poll_json_search();
        // The two selections opened /a and /b in the outline.
        assert_eq!(rows(&app), ["", "/a", "/a/name", "/b", "/b/name", "/c"]);
    }

    #[test]
    fn a_big_value_is_searched_on_a_worker() {
        let big: Vec<u32> = (0..json_view::WORKER_SEARCH_NODES as u32 + 1).collect();
        let (mut app, j) = app_with_json(&serde_json::to_string(&big).unwrap());
        app.switch_group_level(Some(j));
        app.set_json_search("99999".into());
        assert!(app.json_level.as_ref().unwrap().search.pending.is_some());
        for _ in 0..1000 {
            app.poll_json_search();
            if app.json_level.as_ref().unwrap().search.result.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let result = app.json_level.as_ref().unwrap().search.result.clone().expect("the worker answers");
        assert_eq!(result.matches, ["/99999"]);
    }
```

`rows` is rebuilt by `poll_json_search`, so the outline is correct before a frame draws it. `SearchResult` needs `Clone` (Task 3 derives it).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p manifold -j 12 json_level::tests`
Expected: compile errors or failures.

- [ ] **Step 3: Implement in `json_level.rs`**

Add `pub(crate) nodes: usize,` to `JsonLevel` (doc: `/// Nodes in \`value\`, for choosing a worker search.`) and set it in `new` with `nodes: json_view::count_nodes(&value)` before `value` moves into the `Arc`. Replace Task 5's `set_json_search` and the `Select` arm, and add the rest:

```rust
    /// Runs a search (spec "Search"): on the UI thread for a small value,
    /// on a worker above `WORKER_SEARCH_NODES`. An empty query ends it.
    pub(crate) fn set_json_search(&mut self, query: String) {
        let Some(level) = self.json_level.as_mut() else { return };
        level.search.current = 0;
        level.search.pending = None;
        level.outline.dirty = true;
        if query.is_empty() {
            level.search.result = None;
        } else if level.nodes > json_view::WORKER_SEARCH_NODES {
            let (tx, rx) = mpsc::channel();
            let (value, q) = (level.value.clone(), query.clone());
            std::thread::spawn(move || {
                // The level may be gone by now; a closed channel is fine.
                let _ = tx.send(json_view::search(&value, &q));
            });
            level.search.pending = Some((query.clone(), rx));
        } else {
            level.search.result = Some(json_view::search(&level.value, &query));
        }
        level.search.query = query;
        self.poll_json_search();
    }

    /// Takes a worker's answer if it has one, and rebuilds the outline's
    /// rows if they're stale. Runs each frame before the outline draws.
    pub(crate) fn poll_json_search(&mut self) {
        let Some(level) = self.json_level.as_mut() else { return };
        if let Some((query, rx)) = &level.search.pending {
            match rx.try_recv() {
                Ok(result) => {
                    if *query == level.search.query {
                        level.search.result = Some(result);
                    }
                    level.search.pending = None;
                    level.outline.dirty = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => level.search.pending = None,
            }
        }
        if level.outline.dirty {
            let keep = level.search.result.as_ref().map(|r| &r.keep);
            level.outline.rows = json_view::outline_rows(&level.value, &level.outline.open, keep);
            level.outline.dirty = false;
        }
    }

    /// Enter or ↓ in the search box: selects the next match, wrapping.
    pub(crate) fn step_json_match(&mut self) {
        let Some(level) = self.json_level.as_mut() else { return };
        let Some(result) = level.search.result.as_ref().filter(|r| !r.matches.is_empty()) else { return };
        let pointer = result.matches[level.search.current % result.matches.len()].clone();
        level.search.current = (level.search.current + 1) % result.matches.len();
        self.apply_json_edit(JsonEdit::Select(pointer));
    }

    /// Selecting from anywhere (spec "Outline"): the entry's card is
    /// revealed on the canvas (saved, like a manual expand), the camera
    /// pans to it, and the outline opens down to it.
    pub(crate) fn reveal_json(&mut self, pointer: &str) {
        let target = pointer.to_string();
        let changed = self.json_level.as_ref().zip(self.current_json_node()).is_some_and(|(level, json)| {
            let mut view = json.view.clone();
            json_view::reveal(&level.value, &mut view, &level.expanded, &target)
        });
        if changed {
            self.edit_json_view(|view, value, expanded| {
                json_view::reveal(value, view, expanded, &target);
            });
        }
        let Some(level) = self.json_level.as_mut() else { return };
        // The row lives on its parent's card; the root is its own.
        let card = json_view::parent_pointer(pointer).unwrap_or("");
        if let Some(rect) = level.index.get(card).map(|&i| level.rects[i]) {
            let size = self.camera.rect.size();
            self.camera.ease_to(egui::Rect::from_center_size(rect.center(), size));
        }
        let mut ancestor = String::new();
        level.outline.open.insert(String::new());
        for token in json_view::pointer_tokens(json_view::parent_pointer(pointer).unwrap_or("")) {
            ancestor = json_view::child_pointer(&ancestor, &token);
            level.outline.open.insert(ancestor.clone());
        }
        level.outline.dirty = true;
        let keep = level.search.result.as_ref().map(|r| &r.keep);
        let rows = json_view::outline_rows(&level.value, &level.outline.open, keep);
        level.outline.scroll_to = rows.iter().position(|r| r.pointer == pointer);
        level.outline.rows = rows;
        level.outline.dirty = false;
    }
```

The `Select` arm of `apply_json_edit` becomes:

```rust
            JsonEdit::Select(pointer) => {
                if let Some(level) = self.json_level.as_mut() {
                    level.selected = Some(pointer.clone());
                }
                self.reveal_json(&pointer);
            }
```

The dry run in `reveal_json` (`changed`) keeps a click on an already-visible row from saving. `self.camera` is a field of `App`, so `self.json_level.as_mut()` and `self.camera` borrow disjointly. If the borrow checker disagrees because of a method call, take the rect first, drop the level borrow, then ease the camera.

- [ ] **Step 4: The outline panel** (`json_level_ui.rs`)

```rust
/// One outline row's height, in points.
const OUTLINE_ROW_HEIGHT: f32 = 20.0;

impl App {
    /// The outline (spec "Outline"): a resizable right panel holding the
    /// search box and the value as an indented list, virtualized with
    /// `show_rows`. Drawn before the canvas takes the remaining space.
    pub(crate) fn build_json_outline(&mut self, ui: &mut egui::Ui) {
        if self.json_level.is_none() || !self.json_outline_visible {
            return;
        }
        self.poll_json_search();
        let panel = egui::Panel::right("json_outline")
            .resizable(true)
            .default_size(self.json_outline_width)
            .size_range(160.0..=640.0)
            .show(ui, |ui| self.json_outline_contents(ui));
        self.json_outline_width = panel.response.rect.width();
    }

    fn json_outline_contents(&mut self, ui: &mut egui::Ui) {
        let root_name = self.current_json_node().map(|j| download_filename(&j.path).to_string()).unwrap_or_default();
        let Some(level) = self.json_level.as_mut() else { return };
        let mut query = level.search.query.clone();
        let search = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search keys and values").desired_width(f32::INFINITY));
        if std::mem::take(&mut level.search.focus) {
            search.request_focus();
        }
        let status = match (&level.search.pending, &level.search.result) {
            (Some(_), _) => Some("Searching…".to_string()),
            (None, Some(result)) if result.matches.is_empty() => Some("No matches".to_string()),
            (None, Some(result)) => Some(format!("{} matches", result.matches.len())),
            (None, None) => None,
        };
        if let Some(status) = status {
            ui.label(egui::RichText::new(status).weak().small());
        }
        let step = search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::ArrowDown))
            || search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.separator();

        let pal = palette(ui.visuals().dark_mode);
        let rows = std::mem::take(&mut level.outline.rows);
        let searching = level.search.result.is_some();
        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(row) = level.outline.scroll_to.take() {
            scroll = scroll.vertical_scroll_offset((row as f32 * OUTLINE_ROW_HEIGHT - 80.0).max(0.0));
        }
        let mut clicked = None;
        let mut flip = None;
        scroll.show_rows(ui, OUTLINE_ROW_HEIGHT, rows.len(), |ui, range| {
            for row in &rows[range] {
                let Some(value) = level.value.pointer(&row.pointer) else { continue };
                let selected = level.selected.as_deref() == Some(row.pointer.as_str());
                let hit = level.search.result.as_ref().is_some_and(|r| r.hits.contains(&row.pointer));
                ui.horizontal(|ui| {
                    ui.set_height(OUTLINE_ROW_HEIGHT);
                    ui.add_space(row.depth as f32 * 12.0);
                    let open = searching || level.outline.open.contains(&row.pointer);
                    if json_view::is_card(value) && !searching {
                        let caret = if open { egui_phosphor::light::CARET_DOWN } else { egui_phosphor::light::CARET_RIGHT };
                        if ui.small_button(caret).clicked() {
                            flip = Some(row.pointer.clone());
                        }
                    } else {
                        ui.add_space(18.0);
                    }
                    let parent = json_view::parent_pointer(&row.pointer).and_then(|p| level.value.pointer(p));
                    let (name, color) = match (parent, json_view::last_key(&row.pointer)) {
                        (Some(parent), Some(key)) if parent.is_array() => (key, pal.muted),
                        (_, Some(key)) => (key, pal.key),
                        (_, None) => (root_name.clone(), ui.visuals().text_color()),
                    };
                    let mut text = egui::RichText::new(name).color(color);
                    if hit {
                        text = text.background_color(SEARCH_HIT.gamma_multiply(0.3));
                    }
                    if ui.add(egui::Button::selectable(selected, text).frame_when_inactive(false)).clicked() {
                        clicked = Some(row.pointer.clone());
                    }
                    if let Some(label) = parent.filter(|p| p.is_array()).and_then(|_| json_view::label(value)) {
                        ui.add(egui::Label::new(egui::RichText::new(label).color(pal.muted)).truncate());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let kind = value_kind(value);
                        let color = if kind == ValueKind::Container { pal.muted } else { pal.of(kind) };
                        ui.add(egui::Label::new(egui::RichText::new(json_view::display_text(value)).color(color)).truncate());
                    });
                });
            }
        });
        level.outline.rows = rows;
        if let Some(pointer) = flip {
            if !level.outline.open.remove(&pointer) {
                level.outline.open.insert(pointer);
            }
            level.outline.dirty = true;
        }
        let changed_query = query != level.search.query;
        if changed_query {
            self.set_json_search(query);
        } else if step {
            self.step_json_match();
        }
        if let Some(pointer) = clicked {
            self.apply_json_edit(JsonEdit::Select(pointer));
        }
    }
}
```

`root_name` is taken before `level` borrows `self.json_level` mutably. `Button::selectable` and `frame_when_inactive` exist in egui 0.36 (`egui/src/widgets/button.rs`); if either is missing in the pinned version, use `ui.selectable_label(selected, text)`, which draws the same selected fill.

In `build_graph_ui`, call `self.build_json_outline(ui);` on the line before `let scene_outer_rect = ui.available_rect_before_wrap();`, so the canvas takes what the panel leaves.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p manifold -j 12` and `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`.
Expected: all pass, clean.

Rebuild (`cargo build -p manifold -j 12`), rerun Task 6's `target/json-shot.txt` with `JSONSEARCH Ada` and a second `SCREENSHOT` appended, and look at the outline: the search box, the filtered rows, and Ada's card and row in the search colour.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "JSON level: the outline panel and search

A resizable right panel lists the value (show_rows), with its own open
state; selecting anywhere reveals the entry on the canvas (saved), pans
to it and opens the outline down to it. Search matches keys and scalars
across every page, on a worker above 100k nodes; Enter/Down steps.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: The Json card at its parent's level

**Files:**
- Modify: `crates/manifold/src/card.rs` (`node_card_size` ~530, `paint_json_body` ~1314, `JSON_NODE_SIZE`'s doc ~45, the size test ~1777)

**Interfaces:**
- Consumes: `json_view::preview_size` and `PREVIEW_ROWS` (Task 3), `paint_json_content`, `header_parts`, `palette` and `CardLook` (Task 6).
- Produces: a file-backed Json card sized by `preview_size` that shows the file name, `{n}` / `[n]`, up to 4 root rows and `+ N more`.

- [ ] **Step 1: Write the failing test**

In `node_card_size_matches_each_kinds_card`, replace the Json part with:

```rust
        let json = |value: serde_json::Value, key: Option<&str>, error: Option<&str>| {
            NodePayload::Json(JsonPayload {
                path: "a.json".into(),
                value,
                key: key.map(Into::into),
                error: error.map(Into::into),
                url: None,
                view: JsonView::default(),
            })
        };
        let five = serde_json::json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5});
        assert_eq!(node_card_size(&json(five.clone(), None, None)), crate::json_view::preview_size(&five));
        assert_eq!(node_card_size(&json(serde_json::json!({}), None, None)).x, crate::json_view::CARD_WIDTH);
        // A leaf from an old "Expand as Graph" and an error card keep the small size.
        assert_eq!(node_card_size(&json(serde_json::json!(1), Some("k"), None)), JSON_NODE_SIZE);
        assert_eq!(node_card_size(&json(five, None, Some("missing"))), JSON_NODE_SIZE);
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p manifold -j 12 node_card_size_matches_each_kinds_card`
Expected: FAIL, because a file-backed Json card is still `JSON_NODE_SIZE`.

- [ ] **Step 3: Implement**

`node_card_size`:

```rust
        // A file's own card previews its root (spec "The card at the
        // parent level"); a leaf or an error card keeps the small box.
        NodePayload::Json(json) if json.key.is_none() && json.error.is_none() => {
            crate::json_view::preview_size(&json.value)
        }
        NodePayload::Json(_) => JSON_NODE_SIZE,
```

`paint_json_body`: keep the error branch and the leaf's summary as they are, and draw a file-backed node's preview. Replace everything after the error branch with:

```rust
        let Some(json) = self.nodes.get(id).and_then(|n| n.as_json()) else { return body };
        let text = !self.graph_lod_simplified && rect.width() * zoom_scale >= MIN_LABEL_CARD_SCREEN_WIDTH;
        if json.key.is_some() {
            // A leaf from an old "Expand as Graph": its one-line summary.
            let key = json.key.clone().unwrap_or_default();
            let key_display =
                if key.chars().count() > 16 { format!("{}…", key.chars().take(15).collect::<String>()) } else { key };
            let summary = format!("{key_display}: {}", json_summary(&json.value));
            if text {
                let center = rect.center();
                let color = ui.visuals().text_color();
                deferred_overlays.push(Box::new(move |overlay_painter, project| {
                    overlay_painter.text(project(center), egui::Align2::CENTER_CENTER, summary, egui::FontId::proportional(12.0), color);
                }));
            }
            return body;
        }
        let len = crate::json_view::entry_count(&json.value);
        let rows = len.min(crate::json_view::PREVIEW_ROWS);
        let card = crate::json_view::Card { pointer: String::new(), parent: None, depth: 0, collapsed: false, rows, more: len - rows };
        let header = crate::json_level_ui::header_parts(&json.value, &card, download_filename(&json.path));
        let pal = crate::json_level_ui::palette(ui.visuals().dark_mode);
        let look = crate::json_level_ui::CardLook {
            palette: &pal,
            text_color: ui.visuals().text_color(),
            inv_zoom: 1.0 / zoom_scale,
            text,
            selected_row: None,
            hit_rows: &[],
        };
        crate::json_level_ui::paint_json_content(ui.painter(), deferred_overlays, rect, &json.value, &header, rows, len - rows, &look);
        body
```

A scalar or empty root shows its value in the header's count slot (`header_parts` gives `display_text`, for example `42` or `{0}`), and `preview_size` leaves one row of room under it. Rewrite `paint_json_body`'s doc comment and `JSON_NODE_SIZE`'s (card.rs ~45) to say what the card shows now and that double-clicking enters the level. `Card`'s fields are `pub(crate)`, so the literal compiles.

- [ ] **Step 4: Run the tests and look at it**

Run: `cargo test -p manifold -j 12` and `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`.
Expected: all pass, clean.

Rebuild and take the first half of Task 6's `target/json-shot.txt` (up to `EXPORTSTATE`) with a `SCREENSHOT {repo}\target\shots\json-card.png` after it. The card should show `shot.json`, `{3}` and the rows `page`, `users [2]` and `meta {1}`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/card.rs
git commit -m "Json card previews its root at the parent level

220 wide: the file name, {n}/[n], up to four root rows and + N more,
painted by the JSON level's own content painter.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Remove the old viewer and "Expand as Graph"

**Files:**
- Modify: `crates/manifold/src/main.rs`, `popups.rs`, `tab_view.rs`, `node.rs`, `event_loop.rs`, `debug_control.rs`, `card.rs`, `json_view.rs`

**Interfaces:**
- Consumes: everything before; the JSON level now replaces all of it.
- Produces: no new items. `TabPayload.json_body` goes too (the ruling below).

**Ruling:** the spec keeps `TabPayload.json_body` "only as far as content dispatch still needs it". Dispatch (`EngineToHost::JsonBody` in `event_loop.rs`) saves the `body` it receives and never reads the field, and the chip and `ViewJson` were its only readers. With both gone, the field is write-only, so it's removed. A tab kept via Open as tab shows the JSON page itself in its engine.

- [ ] **Step 1: Remove**

Delete each of these, then follow the compiler until the build is clean:

1. `main.rs`:
   - `NodeAction::ExpandAsGraph` and `NodeAction::ViewJson`, their `apply_node_action` arms, and the menu items: "Expand as Graph" in the Json arm and "View as JSON" in the Tab arm of the menu builder.
   - `NodeMenuView.has_json`, its line in `node_menu_view`, and its uses in the menu tests (~6334–6410): drop the parameter and the `View as JSON` entry from the expected strings.
   - `draw_json_value`.
   - `json_viewer_node`: the field, its doc comment, its default, the slot in `expand_json_as_graph`'s loop, and the clearing in the reload path (~5233).
   - `expand_json_as_graph` and `expand_json_entries`.
   - The `self.build_json_viewer(ui);` call.
2. `popups.rs`: `build_json_viewer`.
3. `tab_view.rs`: `has_json` and the `icon_btn_view_json` chip block.
4. `node.rs`: `TabPayload.json_body` and its doc. Rewrite the doc comments that mention the viewer or Expand as Graph (~301, ~315, ~593) to describe what is true now: `JsonLeaf`/`from_json` nodes are what older builds' "Expand as Graph" made, and they still load.
5. `event_loop.rs`: the `ExpandJsonAsGraph` arm, `node.json_body = None;` (~1287), `tab.json_body = Some(body.clone());` (~1377, which leaves `tab.url.clone()` in that `map`), and the `"View as JSON"` sentence in the comment above it. Also the `json_body: None` field in the tab literal at `main.rs` ~2743.
6. `debug_control.rs`: `ExpandJsonAsGraph`, its parse arm, its help row, and the mention in the neighbouring doc comment (~296). Point that comment at a command that still exists, or drop the cross-reference.
7. `card.rs`: the doc lines that say double-click opens `build_json_viewer`.
8. `json_view.rs`: the `#![cfg_attr(not(test), allow(dead_code))]` line from Task 2. Every item is used now; any that clippy reports as dead is a gap in an earlier task, so wire it in or delete it.

Then check for leftovers:

```bash
rtk proxy grep -rn "json_viewer_node\|build_json_viewer\|draw_json_value\|ExpandAsGraph\|ExpandJsonAsGraph\|EXPANDJSONASGRAPH\|expand_json\|ViewJson\|View as JSON\|json_body\|has_json" crates/ scripts/
```

Expected: no matches.

- [ ] **Step 2: Run everything**

Run: `cargo test -p manifold -j 12`, `cargo fmt --all --check` and `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`.
Expected: all pass, clean.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "Remove the JSON viewer popup and Expand as Graph

The JSON level replaces both. Also drops TabPayload.json_body: with the
tab's View as JSON chip gone nothing read it, and content dispatch saves
the body it receives. Old from_json groups and JsonLeaf nodes still
load.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Headless coverage and docs

**Files:**
- Modify: `scripts/headless-smoke-test.sh` (after the drop checks, ~line 472)
- Create: `scripts/json-level-probe.txt`
- Modify: `README.md` (the JSON sections around lines 100 and 259, "Headless mode"'s command list)
- Modify: `docs/superpowers/specs/2026-09-27-json-node-rework-design.md` (the colour example, Status, an Outcome section)

**Interfaces:**
- Consumes: the headless commands and `EXPORTSTATE`'s `json_level` (Task 5).

- [ ] **Step 1: The smoke test**

After the `check_query after_drop "the dropped json's screen center …"` line, add:

```bash
# Entering the dropped JSON file: its level, driven by the JSON commands,
# then back out. `{"a": 1}` is one root card with one row.
send "DOUBLECLICK $(query after_drop "xy('$json_id')")"
settle json_in
check_query json_in "double-clicking a Json card enters its level" \
    "s['json_level'] and s['json_level']['root'] == '$json_id' and s['current_group'] == '$json_id'"
check_query json_in "the JSON level shows the root card" \
    "[c['pointer'] for c in s['json_level']['cards']] == [''] and cards['$json_id#']['kind'] == 'JsonCard'"
send "JSONSELECT /a"
send "JSONSEARCH a"
check_state json_search "JSONSELECT and JSONSEARCH reach the level" \
    "s['json_level']['selected'] == '/a' and s['json_level']['matches'] == ['/a']"
send "CLICK 22 17"
check_state json_out "the back button leaves the JSON level" \
    "s['json_level'] is None and s['current_group'] != '$json_id'"
```

`JSONSEARCH a` matches only the key `a`: the value `1` doesn't contain `a`. Update the script's header comment (~line 18), which lists what the test covers, to add entering a JSON level.

- [ ] **Step 2: The drive.sh probe**

Create `scripts/json-level-probe.txt`. It adds a JSON file with a paged array, enters it, toggles, pages, searches, relayouts and leaves, taking a screenshot and an export at each step. `drive.sh`'s `!` lines compute the Json card's position from a live export, the same way its header comment describes.

```
RESIZE 1200 800
!mkdir -p target/json-probe && python -P -c "import json; json.dump({'page': 1, 'users': [{'id': i, 'name': f'user {i}', 'roles': ['a', 'b']} for i in range(45)], 'meta': {'bbox': [[-0.12, 51.5], [0.33, 51.7]]}}, open('target/json-probe/users.json', 'w'))" < /dev/null
ADDFILE {repo}\target\json-probe\users.json 500 350
sleep 1
EXPORTSTATE {repo}\target\json-probe\0-parent.json
!while [ ! -f target/json-probe/0-parent.json ]; do sleep 0.2; done; python -P -c "import json; s = json.load(open('target/json-probe/0-parent.json')); c = [c for c in s['cards'] if c['kind'] == 'Json'][0]; print('DOUBLECLICK %.0f %.0f' % tuple(c['screen']))" < /dev/null
sleep 1
CLICK 326 17
sleep 1
SCREENSHOT {repo}\target\json-probe\1-entered.png
EXPORTSTATE {repo}\target\json-probe\1-entered.json
JSONTOGGLE /meta
JSONMORE /users
SCREENSHOT {repo}\target\json-probe\2-toggled-paged.png
EXPORTSTATE {repo}\target\json-probe\2-toggled-paged.json
JSONSEARCH user 33
JSONSELECT /users/33/name
SCREENSHOT {repo}\target\json-probe\3-search.png
EXPORTSTATE {repo}\target\json-probe\3-search.json
JSONRELAYOUT
EXPORTSTATE {repo}\target\json-probe\4-relayout.json
CLICK 22 17
sleep 1
EXPORTSTATE {repo}\target\json-probe\5-left.json
```

The `!` line with `python -P -c` passes its program as an argument, which the Global Constraints allow; only `python -` and `python -P -` (a program on stdin) are out. Check `drive.sh`'s `{repo}` substitution also covers `!` lines. If it doesn't, the relative `target/...` paths above already work, because `drive.sh` runs from the repo root.

Build (`cargo build -p manifold -j 12`), run `rtk proxy bash scripts/drive.sh scripts/json-level-probe.txt`, then check the exports:

- `1-entered.json`: `json_level.root` is set, and the `/users` card has `rows` 20 and `more` 25.
- `2-toggled-paged.json`: `/meta` is collapsed, and `/users` has `rows` 40.
- `3-search.json`: `matches` holds `/users/33/name`, and `selected` is `/users/33/name`.
- `5-left.json`: `json_level` is `null`.

Look at the three screenshots with the Read tool.

- [ ] **Step 3: Run the smoke test**

Run: `rtk proxy bash scripts/headless-smoke-test.sh`
Expected: every check passes, the new ones included. A failure in the new block means Tasks 5–8 are off, not the checks. Fix the code.

- [ ] **Step 4: Docs**

1. `README.md`:
   - The JSON node section (~100): replace the viewer popup and Expand as Graph with the JSON level. Cover double-clicking to enter; the record cards, paging and collapse; the tree layout and Relayout; the outline and search; copying, and the path form; the `view` field (14); and that old `from_json`/`JsonLeaf` nodes still load.
   - "Headless mode": add `JSONSELECT`, `JSONTOGGLE`, `JSONMORE`, `JSONRELAYOUT` and `JSONSEARCH`, drop `EXPANDJSONASGRAPH`, and mention `EXPORTSTATE`'s `json_level`.
   - Add the level-source seam (`level.rs`) to the architecture notes, in a sentence or two: what `level_items`, `level_edges` and `layout_current_level` are for, and that a new level kind is one `LevelItemKind` arm plus its painter.
   - Leave the history entries (~259 onward, ~562) alone apart from a note that later work replaced them. They record what was true then.
2. The spec:
   - Set Status to implemented, with the date.
   - Change the example's `"color": 3` to an RGB number (for example `"color": 13936726`) and add "stored as `0x00RRGGBB`, like a node's colour".
   - Add an "Outcome" section. List the rulings that changed or sharpened the spec: plan rulings 2 (preserve_order), 6, 8, 9 and 10, and Task 10's `json_body` removal. Name the commit range by subject lines, not hashes.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all --check
git add scripts/headless-smoke-test.sh scripts/json-level-probe.txt README.md docs/superpowers/specs/2026-09-27-json-node-rework-design.md
git commit -m "JSON level: smoke-test coverage, a drive.sh probe and docs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
