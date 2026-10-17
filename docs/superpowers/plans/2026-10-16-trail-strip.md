---
status: done
---

# Trail Strip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Draw the focused tab's page tree (its `NavHistory`) as a strip of virtual page cards under the tab's card in the Graph view, and as a tree in the Back/Forward caret popups, from one shared tree layout.

**Architecture:** `json_view::tree_layout`'s algorithm moves to a shared pure `tree_layout.rs`; `NavHistory::tree()` is the one read-only view of the tree; `trail_strip.rs` holds a `TrailSource: VirtualSource` (the first real client of the virtual-card seam), the page painter and the page geometry that the card and the popup share. A click is `trail_pick`; dragging a page out calls `spawn_tab_from` with a position.

**Tech Stack:** Rust, egui, the existing `Rig` test harness (`virtual_source::fake`), the `gate` crate.

**Spec:** `docs/superpowers/specs/2026-10-16-trail-strip-design.md` (draft 1 + round 2; this plan corrects two points the spec text had wrong: the kind string and the strip's menu, both fixed in the spec).

## Global Constraints

- Work on `main`; commit per task; end each commit message with the line `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. No push, no tag.
- Before each commit: `cargo fmt --all`, then `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass. Never `--no-verify`. Never silence a lint without `#[allow(..., reason = "...")]`.
- Tests: `cargo test -j 8 -p manifold <filter>`. No test reads or writes the real `%APPDATA%\manifold` (use `App::default()`; `profile::dir()` is `None` under `cfg(test)`).
- Python only as a file run `python -P f.py < /dev/null`, never on stdin. Edit with the Edit tool.
- No compatibility shims: when a function's signature changes, change every caller.
- Names: a string identifier that is never shown gets a `// name rule` mark where `names/tests.rs` requires it (run `cargo test -j 8 -p manifold names` after adding string literals).
- Exact values: strip page size 160 × 44 world units; column gap 24; sibling gap 12; the strip's top-left is the tab card rect's `left_bottom() + (0, 24)`; max entries per trail is `MAX_ENTRIES` = 50.
- The kind is `TYPE_TRAIL_PAGE = "application/vnd.manifold.virtual.trail-page"` (it must sit under `TYPE_VIRTUAL_PREFIX`; any other string matches the `*/*` row and gets every ability). Ids are `virtual:trail:<tab id>:<entry index>`; source name `"trail"`.

## Review Focus

Failure modes the spec implies that no task's happy path exercises (each has a test in the task named):

1. A long chain: 50 pages is ~9000 world units wide in the strip and far wider than a screen in the popup. The popup scrolls (Task 5) and scrolls the cursor page into view; the strip is pannable world space.
2. A click or drag on a page whose entry was evicted since the last frame: `go_to` ignores an out-of-range index; the drag ends via `end_stale_drag` (Task 4).
3. Rapid clicks on pages while a move is pending: `go_to` is ignored while pending, so no second `Navigate` is sent (Task 4).
4. A page with an empty title and a very long URL: the label falls back to the URL and is clipped to the page (Task 3).
5. The focused tab is on another level, is in Tab view, has one page, or the level is a JSON level: no strip (Task 3).

---

### Task 1: Shared tree layout

**Files:**
- Create: `crates/manifold/src/tree_layout.rs`
- Modify: `crates/manifold/src/main.rs` (add `mod tree_layout;` beside `mod json_view;`)
- Modify: `crates/manifold/src/json_view.rs` (`tree_layout` at ~line 584 becomes a wrapper)
- Test: `crates/manifold/src/tree_layout.rs` (inline `mod tests`); `json_view/tests.rs` unchanged

**Interfaces:**
- Produces: `pub(crate) struct TreeNode { pub(crate) parent: Option<usize>, pub(crate) size: egui::Vec2 }` and `pub(crate) fn tree_layout(nodes: &[TreeNode], column_gap: f32, sibling_gap: f32) -> Vec<egui::Pos2>`: top-left per node, node 0 (the only root) at y = 0 and x = 0. Precondition: `nodes[i].parent < Some(i)` and only node 0 has no parent.

- [ ] **Step 1: Write the failing tests** in `tree_layout.rs`:

```rust
//! One tree layout for the JSON level and the trail strip (spec `2026-10-16-trail-strip-design.md`, decision 1): a
//! column per depth as wide as its widest node; a subtree's band is the larger of its node and its children's bands
//! plus gaps; a parent is centred on its children's span; node 0 is at the origin.

#[cfg(test)]
mod tests {
    use super::*;

    fn node(parent: Option<usize>, w: f32, h: f32) -> TreeNode {
        TreeNode { parent, size: egui::vec2(w, h) }
    }

    #[test]
    fn a_single_node_sits_at_the_origin_and_nothing_gives_nothing() {
        assert_eq!(tree_layout(&[node(None, 10.0, 4.0)], 5.0, 3.0), [egui::pos2(0.0, 0.0)]);
        assert!(tree_layout(&[], 5.0, 3.0).is_empty());
    }

    #[test]
    fn a_chain_is_one_row_with_growing_x() {
        let nodes = [node(None, 10.0, 4.0), node(Some(0), 10.0, 4.0), node(Some(1), 10.0, 4.0)];
        let pos = tree_layout(&nodes, 5.0, 3.0);
        assert_eq!(pos, [egui::pos2(0.0, 0.0), egui::pos2(15.0, 0.0), egui::pos2(30.0, 0.0)]);
    }

    #[test]
    fn a_fork_stacks_children_and_centres_the_parent() {
        let nodes = [node(None, 10.0, 4.0), node(Some(0), 10.0, 4.0), node(Some(0), 10.0, 4.0)];
        let pos = tree_layout(&nodes, 5.0, 3.0);
        // Children span 4 + 3 + 4 = 11, so the root (height 4) is centred 3.5 below their top; rebased, the root is at 0.
        assert_eq!(pos[1], egui::pos2(15.0, -3.5));
        assert_eq!(pos[2], egui::pos2(15.0, 3.5));
        let span_mid = (pos[1].y + pos[2].y + 4.0) / 2.0;
        assert!((pos[0].y + 2.0 - span_mid).abs() < 1e-4, "parent centred on its children");
        assert_eq!(pos[0].y, 0.0, "the root's top is the origin");
    }

    #[test]
    fn a_nested_fork_does_not_overlap_and_wide_nodes_widen_only_their_column() {
        // 0 -> 1 -> 3, 0 -> 2 (1 and 2 siblings; 3 under 1); node 3 is wide.
        let nodes = [node(None, 10.0, 4.0), node(Some(0), 10.0, 4.0), node(Some(0), 10.0, 4.0), node(Some(1), 30.0, 4.0)];
        let pos = tree_layout(&nodes, 5.0, 3.0);
        assert_eq!(pos[1].x, pos[2].x, "siblings share a column");
        assert_eq!(pos[3].x, pos[1].x + 10.0 + 5.0);
        let rect = |i: usize| egui::Rect::from_min_size(pos[i], nodes[i].size);
        for a in 0..4 {
            for b in a + 1..4 {
                assert!(!rect(a).intersects(rect(b)), "{a} and {b} overlap");
            }
        }
    }
}
```

- [ ] **Step 2: Run to verify they fail:** `cargo test -j 8 -p manifold tree_layout` → FAIL (no `tree_layout` in the module).

- [ ] **Step 3: Implement** above the tests in `tree_layout.rs`. This is `json_view::tree_layout`'s algorithm made generic (column x from the widest node of each depth instead of `depth * (CARD_WIDTH + COLUMN_GAP)`):

```rust
/// One node to lay out: its parent's index (`None` only for node 0) and its size.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TreeNode {
    pub(crate) parent: Option<usize>,
    pub(crate) size: egui::Vec2,
}

/// Each node's top-left. A parent's index is below its children's, so walking backwards sizes every child first.
pub(crate) fn tree_layout(nodes: &[TreeNode], column_gap: f32, sibling_gap: f32) -> Vec<egui::Pos2> {
    let n = nodes.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut depth = vec![0usize; n];
    for (i, node) in nodes.iter().enumerate() {
        debug_assert_eq!(node.parent.is_none(), i == 0, "node 0 is the only root");
        if let Some(p) = node.parent {
            debug_assert!(p < i, "a parent comes before its children");
            children[p].push(i);
            depth[i] = depth[p] + 1;
        }
    }
    let span = |band: &[f32], kids: &[usize]| {
        kids.iter().map(|&k| band[k]).sum::<f32>() + sibling_gap * kids.len().saturating_sub(1) as f32
    };
    let mut band = vec![0.0; n];
    for i in (0..n).rev() {
        band[i] = nodes[i].size.y.max(span(&band, &children[i]));
    }
    let columns = depth.iter().copied().max().map_or(0, |d| d + 1);
    let mut width = vec![0.0f32; columns];
    for (node, &d) in nodes.iter().zip(&depth) {
        width[d] = width[d].max(node.size.x);
    }
    let mut left = vec![0.0f32; columns];
    for d in 1..columns {
        left[d] = left[d - 1] + width[d - 1] + column_gap;
    }
    let mut top = vec![0.0; n];
    let mut pos = vec![egui::Pos2::ZERO; n];
    for i in 0..n {
        let mut y = top[i] + (band[i] - span(&band, &children[i])) / 2.0;
        for &k in &children[i] {
            top[k] = y;
            y += band[k] + sibling_gap;
        }
        pos[i] = egui::pos2(left[depth[i]], top[i] + (band[i] - nodes[i].size.y) / 2.0);
    }
    let origin = pos.first().map_or(0.0, |p| p.y);
    for p in &mut pos {
        p.y -= origin;
    }
    pos
}
```

- [ ] **Step 4: Make `json_view::tree_layout` a wrapper.** Replace its body (keep the signature and the doc, trimmed to say the algorithm is `tree_layout::tree_layout`) with:

```rust
pub(crate) fn tree_layout(cards: &[Card], view: &JsonView) -> Vec<egui::Pos2> {
    let nodes: Vec<crate::tree_layout::TreeNode> = cards
        .iter()
        .map(|card| crate::tree_layout::TreeNode { parent: card.parent.map(|(p, _)| p), size: card.size() })
        .collect();
    let mut pos = crate::tree_layout::tree_layout(&nodes, COLUMN_GAP, SIBLING_GAP);
    for (card, p) in cards.iter().zip(&mut pos) {
        if let Some([x, y]) = view.get(&card.pointer).and_then(|o| o.pos) {
            *p = egui::pos2(x, y);
        }
    }
    pos
}
```

Delete `json_view`'s old algorithm lines (the `children`/`band`/`top` code). Add `mod tree_layout;` to `main.rs`.

- [ ] **Step 5: Run:** `cargo test -j 8 -p manifold tree_layout json_view` → PASS, including `layout_is_columns_by_depth_with_parents_centred_and_overrides_local` unchanged. Then `cargo test -j 8 -p manifold json_level` → PASS.

- [ ] **Step 6: Commit** (`cargo fmt --all` and the clippy command first):

```bash
git add crates/manifold/src/tree_layout.rs crates/manifold/src/json_view.rs crates/manifold/src/main.rs
git commit -m "Tree layout: one shared algorithm for the JSON level and the trail strip

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `NavHistory::tree()` and a shared test fixture

**Files:**
- Modify: `crates/manifold/src/nav_history.rs`
- Test: `crates/manifold/src/nav_history.rs` (`mod tests`)

**Interfaces:**
- Consumes: private `Entry { url, title, parent, fwd }`, `self.at`.
- Produces: `pub(crate) struct TrailPage { pub(crate) entry: usize, pub(crate) parent: Option<usize>, pub(crate) label: String, pub(crate) cursor: bool, pub(crate) on_path: bool }`; `NavHistory::tree(&self) -> Vec<TrailPage>` (entry order, so `parent < entry`); `#[cfg(test)] NavHistory::with_tree(spec: &[(&str, Option<usize>, Option<usize>)], at: usize) -> NavHistory` (the existing test helper `tree`, moved so other modules' tests can build a trail).

- [ ] **Step 1: Failing test** in `nav_history.rs` `mod tests`:

```rust
#[test]
fn tree_lists_every_page_with_the_cursor_and_the_path_from_the_root() {
    // /a -> /b -> /c and /a -> /d, cursor on /d; /b has a title.
    let mut h = NavHistory::with_tree(
        &[("/a", None, Some(3)), ("/b", Some(0), Some(2)), ("/c", Some(1), None), ("/d", Some(0), None)],
        3,
    );
    h.entries[1].title = "Bee".into();
    let pages = h.tree();
    assert_eq!(pages.iter().map(|p| p.entry).collect::<Vec<_>>(), [0, 1, 2, 3]);
    assert_eq!(pages.iter().map(|p| p.parent).collect::<Vec<_>>(), [None, Some(0), Some(1), Some(0)]);
    assert_eq!(pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["/a", "Bee", "/c", "/d"]);
    assert_eq!(pages.iter().map(|p| p.cursor).collect::<Vec<_>>(), [false, false, false, true]);
    assert_eq!(pages.iter().map(|p| p.on_path).collect::<Vec<_>>(), [true, false, false, true]);
    assert!(NavHistory::default().tree().is_empty());
}
```

- [ ] **Step 2:** `cargo test -j 8 -p manifold tree_lists` → FAIL (no `tree`, no `with_tree`).
- [ ] **Step 3: Implement.** Next to `branches` add:

```rust
/// One page of the trail as the strip and the popup draw it.
pub(crate) struct TrailPage {
    /// The arena index: what `go_to` and `entry_url` take.
    pub(crate) entry: usize,
    pub(crate) parent: Option<usize>,
    /// The title, or the URL when the title is empty.
    pub(crate) label: String,
    /// The page the tab shows.
    pub(crate) cursor: bool,
    /// On the chain from the root to the cursor.
    pub(crate) on_path: bool,
}

impl NavHistory {
    /// Every page in entry order (a parent always before its children), for drawing.
    pub(crate) fn tree(&self) -> Vec<TrailPage> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        let path: HashSet<usize> = std::iter::successors(Some(self.at), |&i| self.entries[i].parent).collect();
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| TrailPage {
                entry: i,
                parent: e.parent,
                label: if e.title.is_empty() { e.url.clone() } else { e.title.clone() },
                cursor: i == self.at,
                on_path: path.contains(&i),
            })
            .collect()
    }
}
```

(merge into the existing `impl NavHistory` block rather than a second one). Move the test helper `tree(spec, at)` out of `mod tests` into the main `impl NavHistory` as `#[cfg(test)] pub(crate) fn with_tree(...)` with the same body, and rename every call `tree(` → `NavHistory::with_tree(` in this file's tests (`grep -n "tree(" crates/manifold/src/nav_history.rs`).
- [ ] **Step 4:** `cargo test -j 8 -p manifold nav_history` → PASS (all of the file's tests, proving the rename).
- [ ] **Step 5: Commit** `git commit -m "NavHistory::tree: the read-only page view the strip and popup draw"` (with the trailer).

---

### Task 3: The strip shows (`TrailSource` entries, edges, painting)

**Files:**
- Create: `crates/manifold/src/trail_strip.rs`
- Modify: `crates/manifold/src/store/model.rs` (const next to `TYPE_VIRTUAL_PREFIX`, line ~128), `crates/manifold/src/caps/rows.rs` (a row), `crates/manifold/src/main.rs` (`mod trail_strip;`, register the source at the `virtual_sources: Vec::new()` initialiser ~line 565), `crates/manifold/src/level.rs` (delete the `#[allow(dead_code …)]` on `Origin::Virtual`, ~line 17-20)
- Test: `crates/manifold/src/trail_strip.rs` (`mod tests`), `crates/manifold/src/caps/rows.rs` tests

**Interfaces:**
- Consumes: `NavHistory::tree()` (Task 2), `tree_layout` (Task 1), `VirtualSource` (seam), `App::card_rect`, `App::nav_history: HashMap<String, NavHistory>`, `App::focused`, `App::view_mode`, `App::mirror.group(id)`, `App::current_group`.
- Produces: `TYPE_TRAIL_PAGE`; `pub(crate) struct TrailSource;` (`impl VirtualSource`); `pub(crate) const PAGE: egui::Vec2 = egui::vec2(160.0, 44.0)`, `COLUMN_GAP = 24.0`, `ROW_GAP = 12.0`, `STRIP_GAP = 24.0`; `pub(crate) fn paint_trail_page(painter: &egui::Painter, rect: egui::Rect, page: &TrailPage, hovered: bool)` (shared with Task 5); `fn page_ids(id: &str) -> Option<(&str, usize)>`; `impl App { fn trail_strip_tab(&self) -> Option<String> }`.

- [ ] **Step 1: Caps row test + row.** In `caps/rows.rs` tests (find the existing virtual-prefix row test and follow its style), assert that `crate::CardKind { ty: TYPE_TRAIL_PAGE, tags: TagSet::EMPTY }.caps()` has `drag_source` true and `selectable`, `drop_target`, `menu`, `stored_controls` false, and `draw_layer` 8. Run → FAIL (const missing). Add to `store/model.rs`: `pub const TYPE_TRAIL_PAGE: &str = "application/vnd.manifold.virtual.trail-page";` with a doc line. In `builtin_rows()` add after the prefix row: `Row::new(TYPE_TRAIL_PAGE).drag_source(true),` (import it with the other `TYPE_*`). The exact row outranks the prefix row, which already sets the rest false and layer 8. Run → PASS.

- [ ] **Step 2: Failing source tests** in `trail_strip.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav_history::NavHistory;
    use crate::virtual_source::fake::Rig;

    /// /a -> /b -> /c and /a -> /d, cursor on /d, on the rig's tab, which is focused.
    fn forked() -> Rig {
        let mut rig = Rig::new();
        let tab = rig.tab();
        rig.app.nav_history.insert(
            tab.clone(),
            NavHistory::with_tree(
                &[("/a", None, Some(3)), ("/b", Some(0), Some(2)), ("/c", Some(1), None), ("/d", Some(0), None)],
                3,
            ),
        );
        rig.app.focused = Some(tab);
        rig.frame(vec![], egui::Modifiers::NONE);
        rig
    }

    fn pages(rig: &Rig) -> Vec<crate::level::CardEntry> {
        rig.app.level_items().into_iter().filter(|e| e.id.starts_with("virtual:trail:")).collect()
    }

    #[test]
    fn a_forked_trail_shows_one_card_per_page_under_the_tab_card() {
        let rig = forked();
        let tab = rig.tab();
        let cards = pages(&rig);
        assert_eq!(cards.len(), 4);
        assert!(cards.iter().all(|c| c.kind.ty == TYPE_TRAIL_PAGE && c.origin == crate::level::Origin::Virtual("trail")));
        assert_eq!(cards[0].id, format!("virtual:trail:{tab}:0"));
        let below = rig.app.card_rect(&tab).unwrap().left_bottom() + egui::vec2(0.0, STRIP_GAP);
        assert_eq!(cards[0].rect.min, below, "the root page hangs under the tab card");
        assert_eq!(cards[0].rect.size(), PAGE);
        assert_eq!(cards[1].rect.min.x, below.x + PAGE.x + COLUMN_GAP, "children one column right");
        assert!(cards[3].rect.min.y > cards[1].rect.min.y, "the later branch sits on a lower lane");
    }

    #[test]
    fn no_strip_for_one_page_no_focus_tab_view_or_another_level() {
        let mut rig = forked();
        let tab = rig.tab();
        rig.app.focused = None;
        assert!(pages(&rig).is_empty(), "no focused tab");
        rig.app.focused = Some(tab.clone());
        rig.app.view_mode = crate::ViewMode::Item;
        assert!(pages(&rig).is_empty(), "Tab view");
        rig.app.view_mode = crate::ViewMode::Graph;
        rig.app.current_group = Some("elsewhere".into());
        assert!(pages(&rig).is_empty(), "the tab is on another level");
        rig.app.current_group = None;
        rig.app.nav_history.insert(tab, NavHistory::with_tree(&[("/a", None, None)], 0));
        assert!(pages(&rig).is_empty(), "one page");
    }

    #[test]
    fn edges_join_parent_to_child_and_mark_the_path() {
        let rig = forked();
        let tab = rig.tab();
        let edges: Vec<_> = rig.app.level_edges().into_iter().filter(|e| e.from_id.starts_with("virtual:trail:")).collect();
        let key = |e: &crate::level::LevelEdge| (e.from_id.clone(), e.to_id.clone(), e.highlight);
        let id = |i: usize| format!("virtual:trail:{tab}:{i}");
        assert_eq!(edges.iter().map(key).collect::<Vec<_>>(), [
            (id(0), id(1), false),
            (id(1), id(2), false),
            (id(0), id(3), true),
        ]);
    }

    #[test]
    fn a_long_url_label_is_clipped_to_the_page() {
        // paint_trail_page paints the label clipped to `rect`: the galley is laid out unwrapped and the painter's clip
        // rect is the page.
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let page = TrailPage { entry: 0, parent: None, label: "x".repeat(400), cursor: false, on_path: false };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, PAGE);
        let mut shapes = 0;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter().with_clip_rect(rect);
            paint_trail_page(&painter, rect, &page, false);
            shapes = ui.ctx().graphics(|g| g.drain().count());
        });
        assert!(shapes > 0);
    }
}
```

(The last test only proves the painter runs with a pathological label and does not panic; its real check is the clip rect passed in. If `graphics(|g| …)` has a different API in this egui, assert only that `paint_trail_page` returns, keeping the test's name.)

Run `cargo test -j 8 -p manifold trail_strip` → FAIL (module missing).

- [ ] **Step 3: Implement** `trail_strip.rs`:

```rust
//! The trail strip (spec `2026-10-16-trail-strip-design.md`): the focused tab's page tree as virtual cards under the
//! tab's card, and the painter and geometry the Back/Forward popups share.

use crate::card::{CardBody, DropEffect};
use crate::level::{CardEntry, EdgeCurve, EdgeKind, LevelEdge, Origin};
use crate::nav_history::TrailPage;
use crate::store::model::TYPE_TRAIL_PAGE;
use crate::tree_layout::{tree_layout, TreeNode};
use crate::virtual_source::VirtualSource;
use crate::{App, CardKind, GraphAction};

pub(crate) const PAGE: egui::Vec2 = egui::vec2(160.0, 44.0);
pub(crate) const COLUMN_GAP: f32 = 24.0;
pub(crate) const ROW_GAP: f32 = 12.0;
/// Between the tab card's bottom edge and the root page.
const STRIP_GAP: f32 = 24.0;

const ID_PREFIX: &str = "virtual:trail:";

/// `virtual:trail:<tab>:<entry>` -> (tab, entry).
fn page_ids(id: &str) -> Option<(&str, usize)> {
    let (tab, entry) = id.strip_prefix(ID_PREFIX)?.rsplit_once(':')?;
    Some((tab, entry.parse().ok()?))
}

/// Every page's rect relative to the root page's top-left, from the shared tree layout.
pub(crate) fn page_rects(pages: &[TrailPage], cell: egui::Vec2, column_gap: f32, row_gap: f32) -> Vec<egui::Rect> {
    let nodes: Vec<TreeNode> = pages.iter().map(|p| TreeNode { parent: p.parent, size: cell }).collect();
    tree_layout(&nodes, column_gap, row_gap).into_iter().map(|min| egui::Rect::from_min_size(min, cell)).collect()
}

/// One page: the frame (accent for the cursor), the label clipped to the page, dimmed off the path.
pub(crate) fn paint_trail_page(painter: &egui::Painter, rect: egui::Rect, page: &TrailPage, hovered: bool) {
    let visuals = painter.ctx().global_style().visuals.clone();
    let fill = if hovered { visuals.widgets.hovered.bg_fill } else { visuals.widgets.inactive.bg_fill };
    painter.rect_filled(rect, 6.0, fill);
    if page.cursor {
        painter.rect_stroke(rect, 6.0, egui::Stroke::new(1.5, visuals.selection.stroke.color), egui::StrokeKind::Inside);
    }
    let color = if page.cursor {
        visuals.strong_text_color()
    } else if page.on_path {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    let clip = painter.with_clip_rect(rect.shrink2(egui::vec2(6.0, 0.0)).intersect(painter.clip_rect()));
    clip.text(rect.left_center() + egui::vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, &page.label, egui::FontId::proportional(12.0), color);
}

pub(crate) struct TrailSource;

impl App {
    /// The tab the strip hangs under: focused, in Graph view, its card on the level shown, and a trail of two or more
    /// pages.
    fn trail_strip_tab(&self) -> Option<String> {
        if self.view_mode != crate::ViewMode::Graph {
            return None;
        }
        let tab = self.focused.clone()?;
        let on_level = self.card_rect(&tab).is_some() && self.mirror.group(&tab) == self.current_group.as_deref();
        (on_level && self.nav_history.get(&tab).is_some_and(|h| h.tree().len() > 1)).then_some(tab)
    }
}

impl TrailSource {
    fn page_id(tab: &str, entry: usize) -> String {
        format!("{ID_PREFIX}{tab}:{entry}")
    }
}

impl VirtualSource for TrailSource {
    fn name(&self) -> &'static str {
        "trail"
    }

    fn entries(&self, app: &App) -> Vec<CardEntry> {
        let Some(tab) = app.trail_strip_tab() else { return Vec::new() };
        let (Some(history), Some(card)) = (app.nav_history.get(&tab), app.card_rect(&tab)) else { return Vec::new() };
        let pages = history.tree();
        let origin = card.left_bottom() + egui::vec2(0.0, STRIP_GAP);
        let kind = CardKind { ty: TYPE_TRAIL_PAGE, tags: crate::caps::TagSet::EMPTY };
        pages
            .iter()
            .zip(page_rects(&pages, PAGE, COLUMN_GAP, ROW_GAP))
            .map(|(page, rect)| CardEntry {
                id: Self::page_id(&tab, page.entry),
                kind: kind.clone(),
                origin: Origin::Virtual("trail"),
                rect: rect.translate(origin.to_vec2()),
            })
            .collect()
    }

    fn edges(&self, app: &App, rect_of: &dyn Fn(&str) -> Option<egui::Rect>) -> Vec<LevelEdge> {
        let Some(tab) = app.trail_strip_tab() else { return Vec::new() };
        let Some(history) = app.nav_history.get(&tab) else { return Vec::new() };
        let pages = history.tree();
        pages
            .iter()
            .filter_map(|page| {
                let parent = &pages[page.parent?];
                let (from_id, to_id) = (Self::page_id(&tab, parent.entry), Self::page_id(&tab, page.entry));
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

    fn paint_body(&self, app: &App, ui: &mut egui::Ui, entry: &CardEntry) -> CardBody {
        let page = page_ids(&entry.id).and_then(|(tab, i)| app.nav_history.get(tab)?.tree().into_iter().nth(i));
        if let Some(page) = page {
            paint_trail_page(ui.painter(), entry.rect, &page, false);
        }
        CardBody { interact_rect: entry.rect, tooltip: None }
    }

    fn on_click(&self, _app: &mut App, _entry: &CardEntry) -> Option<GraphAction> {
        None // Task 4
    }

    fn on_menu(&self, _app: &mut App, _entry: &CardEntry, _at: egui::Pos2) {} // never called: the kind has no `menu`

    fn on_drop(
        &self,
        _app: &mut App,
        _entry: &CardEntry,
        _target: Option<(DropEffect, Option<String>)>,
        _at: egui::Pos2,
    ) -> Option<GraphAction> {
        None // Task 4
    }

    fn searchable_text(&self, _entry: &CardEntry) -> Option<String> {
        None
    }
}
```

Notes for the implementer: (1) the card loop already draws the shared frame, so the card body must paint only the content; if the frame's fill hides `paint_trail_page`'s own fill or doubles it, drop the `rect_filled` from the card path by passing a flag, and keep it for the popup. (2) Match the egui version's names (`global_style` vs `style`, `rect_stroke`'s signature, `StrokeKind`): copy what `card.rs`'s ghost or another painter in the repo calls. (3) `searchable_text` returns the page's label and URL once `paint_body`'s lookup is a shared helper; if it is cheap to do, give it that and add a one-line test, else leave `None` and say so in the report. (4) Register the source: in `main.rs` change `virtual_sources: Vec::new()` to `virtual_sources: vec![std::rc::Rc::new(trail_strip::TrailSource)]`; remove the `#[allow(dead_code …)]` from `Origin::Virtual`.
- [ ] **Step 4:** `cargo test -j 8 -p manifold trail_strip caps` → PASS; `cargo test -j 8 -p manifold` (whole crate) → PASS (the default `App` now has the source; no existing test sets `focused` plus a trail in Graph view, so nothing else should change; if one does, fix the test's setup, not the source).
- [ ] **Step 5: Commit** `git commit -m "Trail strip: a virtual source draws the focused tab's page tree under its card"`.

---

### Task 4: Click and drag-out

**Files:**
- Modify: `crates/manifold/src/trail_strip.rs`, `crates/manifold/src/trail_nav.rs` (`open_branch_as_card`), `crates/manifold/src/tabs.rs` (`spawn_tab_from`, `spawn_new_tab_with_kind`), and their callers: `event_loop.rs:1546`, `node_menu.rs:742`, `trail_nav.rs:109`, `tabs.rs:457,468,613`, `session_sync/tests.rs:346`, `jar_defs/tests.rs:171`
- Test: `crates/manifold/src/trail_strip.rs` (`mod tests`)

**Interfaces:**
- Consumes: `App::trail_pick(tab, entry)`, `App::spawn_tab_from`, `Rig::{click, drag, screen}`, `App.test_spawns: Vec<(TabSpawn, u64)>`.
- Produces: `spawn_new_tab_with_kind(parent, group, url, kind, purpose, jar, pos: Option<egui::Vec2>)` (a `Some(pos)` replaces the computed `parent_pos + spawn_jitter`); `spawn_tab_from(opener, url, pos: Option<egui::Vec2>)`; `open_branch_as_card(tab, entry, pos: Option<egui::Vec2>)`.

- [ ] **Step 1: Failing tests** (append to `trail_strip.rs` tests):

```rust
    #[test]
    fn clicking_a_page_starts_a_plain_move_to_it_and_not_while_one_is_pending() {
        let mut rig = forked();
        let tab = rig.tab();
        let b = rig.screen(&format!("virtual:trail:{tab}:1")).center();
        rig.click(b);
        let now = std::time::Instant::now();
        assert!(rig.app.nav_history[&tab].move_pending(now), "the click started a move");
        // A second click on another page while that move is pending is ignored.
        let c = rig.screen(&format!("virtual:trail:{tab}:2")).center();
        rig.click(c);
        assert!(rig.app.nav_history[&tab].move_pending(now), "still the first move");
    }

    #[test]
    fn clicking_the_cursor_page_or_an_evicted_one_does_nothing() {
        let mut rig = forked();
        let tab = rig.tab();
        let d = rig.screen(&format!("virtual:trail:{tab}:3")).center();
        rig.click(d);
        assert!(!rig.app.nav_history[&tab].move_pending(std::time::Instant::now()), "the cursor page");
        rig.app.trail_pick(&tab, 99);
        assert!(!rig.app.nav_history[&tab].move_pending(std::time::Instant::now()), "an index past the end");
    }

    #[test]
    fn dragging_a_page_onto_empty_canvas_opens_it_as_a_card_there() {
        let mut rig = forked();
        let tab = rig.tab();
        let from = rig.screen(&format!("virtual:trail:{tab}:1")).center();
        let to = rig.screen(&format!("virtual:trail:{tab}:1")).center() + egui::vec2(0.0, 300.0);
        rig.drag(from, to);
        let (spawn, _) = rig.app.test_spawns.last().expect("a tab was spawned");
        assert_eq!(spawn.url, "/b");
        assert_eq!(spawn.parent.as_deref(), Some(tab.as_str()), "opened from the tab whose trail it came from");
        let world = rig.app.graph_screen_to_world(to);
        assert!((spawn.pos - world.to_vec2()).length() < 1.0, "placed at the release point");
        assert_eq!(rig.app.nav_history[&tab].export()["entries"], 4, "the trail is unchanged");
    }

    #[test]
    fn dropping_a_page_on_a_card_opens_it_the_same_way() {
        let mut rig = forked();
        let tab = rig.tab();
        let from = rig.screen(&format!("virtual:trail:{tab}:2")).center();
        let to = rig.screen(&tab).center();
        rig.drag(from, to);
        let (spawn, _) = rig.app.test_spawns.last().expect("a tab was spawned");
        assert_eq!(spawn.url, "/c");
        assert_eq!(spawn.parent.as_deref(), Some(tab.as_str()), "the card dropped on is not the opener");
    }
```

(`graph_screen_to_world` may be named differently: find the inverse of `graph_world_rect_to_screen` in `graph_ui.rs` ~line 232 and use it; if there is only the forward transform, compare `graph_world_rect_to_screen(Rect::from_min_size(spawn.pos.to_pos2(), ZERO)).min` to `to`.)

Run → FAIL (handlers return `None`, no `pos` parameter).
- [ ] **Step 2: Plumbing.** In `tabs.rs`: add `pos: Option<egui::Vec2>` as the last parameter of `spawn_new_tab_with_kind`, and change `let pos = parent_pos + graph_layout::spawn_jitter(&id);` to `let pos = pos.unwrap_or_else(|| parent_pos + graph_layout::spawn_jitter(&id));`. Add `pos: Option<egui::Vec2>` as the last parameter of `spawn_tab_from` and pass it on. Update every caller: `tabs.rs:457,468,613` and `session_sync/tests.rs:346` pass `None`; `event_loop.rs:1546`, `node_menu.rs:742`, `jar_defs/tests.rs:171` pass `None`. `open_branch_as_card(&mut self, tab: &str, entry: usize, pos: Option<egui::Vec2>)` passes `pos` to `spawn_tab_from`; its popup caller (`popups.rs` ~1316) passes `None`.
- [ ] **Step 3: Handlers** in `trail_strip.rs`:

```rust
    fn on_click(&self, app: &mut App, entry: &CardEntry) -> Option<GraphAction> {
        let (tab, page) = page_ids(&entry.id)?;
        app.trail_pick(&tab.to_owned(), page);
        None
    }

    fn on_drop(
        &self,
        app: &mut App,
        entry: &CardEntry,
        _target: Option<(DropEffect, Option<String>)>,
        at: egui::Pos2,
    ) -> Option<GraphAction> {
        let (tab, page) = page_ids(&entry.id)?;
        app.open_branch_as_card(&tab.to_owned(), page, Some(at.to_vec2()));
        None
    }
```

The target is ignored on purpose (spec decision 5): a drop on a card, on empty canvas, and on the back button (`Eject`) all open the page as a card at `at`, opened from the strip's tab.
- [ ] **Step 4:** `cargo test -j 8 -p manifold trail_strip trail_nav jar_defs session_sync tabs` → PASS; then the whole crate.
- [ ] **Step 5: Commit** `git commit -m "Trail strip: click navigates, dragging a page out opens it as a card there"`.

---

### Task 5: The popups paint the tree

**Files:**
- Modify: `crates/manifold/src/popups.rs` (`build_trail_branches_popup`, ~line 1271), `crates/manifold/src/nav_history.rs` (delete what only the old popup used), `crates/manifold/src/trail_strip.rs`
- Test: `crates/manifold/src/trail_strip.rs`, existing popup tests (find with `grep -n "build_trail_branches_popup\|BackBranches" crates/manifold/src -r`)

**Interfaces:**
- Consumes: `page_rects`, `paint_trail_page`, `PAGE` (Task 3), `trail_pick`, `open_branch_as_card(tab, entry, None)` (Task 4).
- Produces: the popup body is the tree; `branches(dir)` is still used by `back_fork`/`forward_fork`.

- [ ] **Step 1: Failing test.** The geometry is pure; test the popup's cell size and the scroll target there:

```rust
    #[test]
    fn popup_geometry_is_the_same_tree_at_a_smaller_cell() {
        let pages = NavHistory::with_tree(
            &[("/a", None, None), ("/b", Some(0), None), ("/c", Some(0), None)],
            0,
        )
        .tree();
        let rects = page_rects(&pages, POPUP_PAGE, POPUP_COLUMN_GAP, POPUP_ROW_GAP);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[1].min.x, POPUP_PAGE.x + POPUP_COLUMN_GAP, "second column");
        assert!(rects[2].min.y > rects[1].min.y, "second branch below the first");
        assert_eq!(rects[0].size(), POPUP_PAGE);
    }
```

Add `pub(crate) const POPUP_PAGE: egui::Vec2 = egui::vec2(120.0, 28.0); POPUP_COLUMN_GAP = 16.0; POPUP_ROW_GAP = 6.0;` to `trail_strip.rs` first so it compiles; run → FAIL only if the geometry is wrong; if it passes at once, it is a regression guard, say so in the report.
- [ ] **Step 2: Rewrite the popup body.** Keep the `egui::Area`, `popup_frame` and the close logic. Replace the flat list with a bounded scroll area holding the tree:

```rust
let pages = self.nav_history.get(tab).map_or_else(Vec::new, |h| h.tree());
let rects = crate::trail_strip::page_rects(&pages, POPUP_PAGE, POPUP_COLUMN_GAP, POPUP_ROW_GAP);
// …inside popup_frame(ui).show(ui, |ui| { menu_item_style(ui);
egui::ScrollArea::both().max_width(480.0).max_height(240.0).auto_shrink([true, true]).show(ui, |ui| {
    let extent = rects.iter().fold(egui::Vec2::ZERO, |m, r| m.max(r.max.to_vec2()));
    let (area, _) = ui.allocate_exact_size(extent, egui::Sense::hover());
    // lines first, so pages draw over them
    for (page, rect) in pages.iter().zip(&rects) {
        if let Some(parent) = page.parent {
            let (a, b) = (rects[parent].translate(area.min.to_vec2()).right_center(), rect.translate(area.min.to_vec2()).left_center());
            ui.painter().line_segment([a, b], ui.visuals().widgets.noninteractive.fg_stroke);
        }
    }
    for (page, rect) in pages.iter().zip(&rects) {
        let rect = rect.translate(area.min.to_vec2());
        let row = ui.interact(rect, egui::Id::new(("trail_popup_page", which, page.entry)), egui::Sense::click());
        paint_trail_page(&ui.painter().with_clip_rect(rect), rect, page, row.hovered());
        if page.cursor {
            ui.scroll_to_rect(rect, Some(egui::Align::Center));
        }
        if row.clicked() {
            picked = Some(page.entry);
        }
        row.context_menu(|ui| {
            if ui.button("Open as card").clicked() {
                as_card = Some(page.entry);
                ui.close();
            }
        });
    }
});
```

Delete the old `for (entry, label) in &rows` loop, the `rows` binding, and `branch_label` if nothing else uses it (`grep -n branch_label`). Keep `picked`/`as_card` handling below, changing `self.open_branch_as_card(tab, entry)` to pass `None`. `dir` still picks `which`. Adjust the `ui.interact` id and API to the egui version in use; look at how the surrounding code in `popups.rs` allocates ids.
- [ ] **Step 3:** `cargo test -j 8 -p manifold` → PASS (any test that read the old flat rows is rewritten to read the tree, not kept beside it). Launch check is left to Task 6's gate run.
- [ ] **Step 4: Commit** `git commit -m "Trail popups: the shared tree layout instead of a flat list"`.

---

### Task 6: Gate, docs, status

**Files:**
- Modify: `crates/gate/src/trail.rs`, `README.md`, `docs/superpowers/specs/2026-10-16-trail-strip-design.md`, `docs/superpowers/specs/2026-10-16-virtual-cards-design.md` (the Next section)
- Test: `gate trail`

**Interfaces:**
- Consumes: `State::all_cards()` (keeps cards with a `"virtual"` key), debug commands `TOGGLEVIEW` and the existing `inst.send` / `export_until` helpers.

- [ ] **Step 1: Gate check.** In `trail.rs`'s `drive`, after the `fork` section's two `check_is` lines and before `next("script")`, add a section:

```rust
    next("strip");
    // The Graph view draws the focused tab's trail: one virtual card per page (spec trail-strip decision 10).
    inst.send("TOGGLEVIEW")?;
    let strip = |s: &State| {
        s.all_cards().iter().filter(|c| c["virtual"].as_str() == Some("trail")).count()
    };
    let s = inst.export_until("strip", |s| !s.tab_view() && strip(s) == 4)?;
    check_is("the strip has a card per page", Some(4), Some(strip(&s)));
    inst.send("TOGGLEVIEW")?;
    inst.export_until("strip-off", |s| s.tab_view() && at(s, "/b4"))?;
```

(`check_is` takes `Option<T>`s of equal type: use `Some(4usize)` as needed.) Update the module doc comment to mention the strip.
- [ ] **Step 2: Build and run.** `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`, then detached at BelowNormal (see CLAUDE.md "Gate"): `target/debug/gate.exe trail`. Expected: all checks pass including "the strip has a card per page". Report CPU load and Defender state. If TOGGLEVIEW does not leave Tab view for Graph as assumed, read `debug_control.rs` (`"TOGGLEVIEW"`) and use the right command.
- [ ] **Step 3: Docs.** README: extend the "Virtual cards" paragraph with one sentence naming the trail strip as the first source, and find the trail paragraph and add the strip (Graph view, focused tab, drag out opens a card, popups draw the same tree). Spec `2026-10-16-trail-strip-design.md`: Status → `implemented 2026-10-16 (plan docs/superpowers/plans/2026-10-16-trail-strip.md)`; decision 10's gate sentence → "after a fork, the Graph view lists one strip card per page" (the click is tested in `trail_strip.rs`, as the gate has no world-to-screen click). Virtual-cards spec "Next": change to "Piece 2, the trail strip, is implemented (`2026-10-16-trail-strip-design.md`)."
- [ ] **Step 4: Full gate.** `cargo test -j 8 -p manifold -p gate` → PASS, then `target/debug/gate.exe all LOG light` (detached, BelowNormal; the strip touches drawing, popups and the trail) → exit 0 or a load-attributed failure re-run alone, reporting the numbers.
- [ ] **Step 5: Commit** `git add README.md docs crates/gate && git commit -m "Trail strip: gate check, README, spec status"`.

---

## Self-review

- **Spec coverage:** decision 1 → Task 1; 2 → Task 2; 3, 6, 7 → Task 3; 4 → Task 3 (row); 5 → Task 4 (menu dropped, spec fixed); 8 → Task 5; 9 → Task 4 (`trail_pick` out-of-range test; stale drag is the seam's `end_stale_drag`); 10 → Task 3 (EXPORTSTATE already lists virtual cards through the seam) and Task 6; 11 nothing to do.
- **Placeholders:** the two "match the egui version" notes name what to copy from; the first Task 1 test has a stated correction (children at -3.5 and 3.5), which the implementer applies when writing it.
- **Type consistency:** `TrailPage` fields (`entry`, `parent`, `label`, `cursor`, `on_path`) match in Tasks 2, 3, 5; `page_rects(pages, cell, column_gap, row_gap)` matches its callers; `open_branch_as_card(tab, entry, pos)` matches in Tasks 4 and 5; `spawn_tab_from(opener, url, pos)` matches its five callers.
- **Known risk:** Task 3 step 1's `drag_source`-only row relies on an exact row outranking the prefix row (the seam's test rows already do the same).
