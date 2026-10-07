# Link gestures Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A click on a link line deletes that link, and a drag from a card's edge handle onto another card links the two. Both gestures act on `link` edges only.

**Architecture:** `LevelEdge` gains its end ids and its Link node id. A new module, `link_gestures.rs`, holds the pure hit-test and handle geometry plus the `App` methods both gestures use. The line click adds no widget. It reads the click from the Scene's own background response, which egui gives a click only when no card, control or popup is on top, and never for a pan. Handles are drag-only egui widgets registered after a card and its controls. A press on one starts a connect-drag. The drag reuses pick-target mode (`connecting`) for its rings and ends at release on the card under the pointer.

**Tech Stack:** Rust, egui 0.36.2 (`Scene`, hit-testing), the existing `connect` / `delete_link` / `finish_connect` path (`links.rs`).

**Spec:** `docs/superpowers/specs/2026-10-07-link-gestures-design.md`. The egui facts below were read from `egui-0.36.2/src/{containers/scene.rs, hit_test.rs, response.rs, ui.rs}` in the cargo registry, and the app facts from the files named per task (line numbers as of 46dd947).

## Global Constraints

- Work on `main` (CLAUDE.md "Branches"), with no worktree. Commit once per task. Each implementer commit message ends with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Run `cargo fmt --all` before each commit. `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass. `.githooks/pre-commit` runs both, and takes 2+ minutes, so commit in the background. Never use `--no-verify`. Do not silence a lint without a comment saying why.
- Use `-j 8` on cargo. Run builds, tests and probes at BelowNormal where practical. C: is nearly full, so check `df -h /c` before a full build.
- No test may touch the real `%APPDATA%\manifold`. `App::default()` is safe: `profile::dir()` is `None` under `cfg(test)`.
- Python runs only from a script file, as `python -P f.py < /dev/null`, or as `python -P -c '...' args </dev/null` as the probe scripts already do. Never use `python -`, heredoc python, or a no-op python call. Prefer the Edit tool. Subagents must never dispatch subagents.
- Edition 2021: no let-chains. `Option::is_none_or` is fine (already used).
- Spec values:
  - Hit tolerance is `max(6 screen px, half the line width) * inv_zoom`.
  - Handles are 12 screen px, one per side, centred on each edge midpoint.
  - Handles are hidden when an edge is under the minimum on-screen length (Decision 4) or when `graph_lod_simplified` is set.
  - Handles show on hover only.
  - Opened-from lines never react.

## Decisions resolved by reading the code

1. **Where the line click is detected: after the Scene, from the Scene's background response, not from a region per edge.**
   - `Scene::show_global_transform` (scene.rs 195-226) creates `local_ui` with `Sense::click_and_drag()` before the contents and hands that same `Ui` to our closure. It returns `InnerResponse { response: pan_response, inner }`, where `pan_response` is the background response.
   - Inside the closure, `ui.response()` (ui.rs 944) is that same background widget.
   - egui hit-testing (hit_test.rs 206-400) gives a click to the last-registered widget at distance 0. Cards (`Sense::CLICK | DRAG`) and their controls (`Sense::CLICK`) are registered later, inside the closure, so they win. Popups and menus are other layers and hide this one.
   - So `background.hovered()` is true only over bare canvas, and `scene_background.clicked()` only for a click there. egui never reports a drag (the pan) as a click.
   - Detecting the line this way adds no widget, so the primary-drag pan cannot be stolen.
   - A `Sense::click` region per edge was rejected. egui interact rects are axis-aligned, so a diagonal line's region would be its bounding box, which covers canvas and cards. On top of the background, that region would also take the pan's drag.
   - The minimap is painted, not a widget (`handle_minimap_click` reads raw input), so the background is hovered over it. It is excluded explicitly.
   - The hover (for the highlight and cursor) is computed inside the closure, before the edges are drawn. The click is applied after the closure, before the `actions` loop.
2. **Coordinate spaces.**
   - `Response::hover_pos` (response.rs 573) and `Response::drag_delta` / `interact_pointer_pos` are layer-local, which is world space in the Scene.
   - `ui.input(|i| i.pointer.*)` is always global screen points. Existing code compares those raw points against screen rects: `drop_target` against `leave_group_button_rect`, and `handle_minimap_click` against the minimap rect.
   - Inside the closure, screen to world is `ui.ctx().layer_transform_from_global(ui.layer_id())`. This is this frame's transform, because `set_transform_layer` runs before `add_contents`.
   - After the closure, screen to world is `(p - translation) / zoom_scale`, using the transform recomputed at graph_ui.rs 594. Outside a frame (EXPORTSTATE, tests) use `graph_world_rect_to_screen` / `graph_screen_point_to_world`.
   - In this plan:
     - The line hover uses `background.hover_pos()` (world).
     - The minimap test uses the raw pointer (screen).
     - The connect target inside the closure uses the layer transform.
     - The release target after the closure uses the recomputed transform.
3. **"Ties go to the most recently created" becomes "the last in `link_ids()` order".**
   - `Mirror::link_ids` iterates a `BTreeSet<NodeId>`. `NodeId` orders by actor, then counter. There is no creation time on a Link node.
   - Within one launch, a later link has a greater counter, so the rule is exactly "newest". Across replicas or launches it is a deterministic stand-in. Record this as a deviation in the Outcome.
4. **Handle minimum edge length is 44 screen px, not "~40".**
   - Each handle is 12 px, centred on its edge's midpoint.
   - The Audio control is 16 px at `right_top + (-8, 8)` (`CardControl::rect`, card.rs), so it spans 16 px from the top-right corner along both the top and the right edge.
   - A top or right handle clears it only when half the edge is at least 16 + 6 px, so the edge must be at least 44 px. Close and Eject (18 px centred on a corner) need only 30.
5. **A press on a handle never clicks or drags the card.**
   - hit_test.rs (`(Some(hit_click), Some(hit_drag))` branch): when the topmost widget senses drag only, egui returns `click: None, drag: Some(handle)`.
   - So the card under the handle's inner half gets neither the click (no note editing, no tab open) nor the drag (no move, no `Drop`).
   - A drag-only widget is `dragged()` from the press frame.
6. **The release is detected from raw input, not from egui's drag end.** After the Scene, the check is `connect_drag.is_some() && !primary_down`. It works even if the source card is culled mid-drag, and it does not depend on which widget egui thinks it is dragging.

---

### Task 1: Click a link line to disconnect

**Files:**
- Modify: `crates/manifold/src/level.rs`
  - `LevelEdge`: drop `Copy`; add `from_id`, `to_id`, `link`.
  - `level_edges` (119-166) fills the new fields.
  - Test at 273 gains an id assertion.
- Create: `crates/manifold/src/link_gestures.rs` (constants, hit-test helpers, `App` methods, tests).
- Modify: `crates/manifold/src/main.rs`
  - Add `mod link_gestures;` after `mod links;` (line 36).
  - Add `LevelEdge` to the `use level::{…}` import (line 92).
- Modify: `crates/manifold/src/graph_ui.rs`
  - Scene closure (554-584).
  - After the closure (585-611).
  - `draw_graph_edges` (1496).
- Modify: `crates/manifold/src/debug_control.rs`
  - ExportState doc (236-252).
  - `export_state` (1320-1424): add `edges`.
  - New test.
- Modify: `crates/manifold/src/json_level.rs:498` and `crates/manifold/src/session_sync/tests.rs:4968` (assertions on the new fields).

**Interfaces:**
- Produces:
  - `LevelEdge { from, to, from_id: String, to_id: String, curve, highlight, kind, link: Option<String> }`. It is `Debug, Clone, PartialEq` (not `Copy`).
  - `link_gestures::{LINK_STROKE_PX, LINK_HOVER_STROKE_PX, LINK_HIT_PX}`.
  - `link_gestures::distance_to_segment(p, a, b) -> f32`.
  - `link_gestures::link_hit_tolerance(inv_zoom) -> f32`.
  - `link_gestures::nearest_link(&[LevelEdge], at: Pos2, tolerance) -> Option<&LevelEdge>`.
  - `App::line_click_allowed(&self) -> bool`.
  - `App::hovered_link_line(&self, ui, &[LevelEdge], inv_zoom, minimap: Option<Rect>) -> Option<String>`.
  - `App::click_link_line(&mut self, link: &str)`.
  - `App::draw_graph_edges(&self, ui, edges: &[LevelEdge], inv_zoom)`.
  - EXPORTSTATE `edges`.
- Consumes: `App::delete_link`, `App::save_session`, `Mirror::is_link`, `Mirror::link_ids` / `link_ends`.

- [ ] **Step 1: Write the failing tests.**

Create `crates/manifold/src/link_gestures.rs` with only the module doc and this test module. Add `mod link_gestures;` to main.rs so it compiles. The helpers come in Step 3.

```rust
//! Link gestures (spec `2026-10-07-link-gestures-design`): a click on a link line deletes that link, and a drag from a
//! card's edge handle links it to the card it is released on. Both act on `link` edges only; opened-from stays
//! menu-only.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{EdgeCurve, EdgeKind, LevelEdge};
    use crate::node::{NotePayload, Runtime};
    use crate::store::model::{Kind, NodeId};
    use crate::App;

    #[test]
    fn distance_to_a_segment_clamps_to_its_ends() {
        let (a, b) = (egui::pos2(0.0, 0.0), egui::pos2(10.0, 0.0));
        assert_eq!(distance_to_segment(egui::pos2(5.0, 3.0), a, b), 3.0);
        assert_eq!(distance_to_segment(egui::pos2(-4.0, 3.0), a, b), 5.0, "before the start: to the start");
        assert_eq!(distance_to_segment(egui::pos2(13.0, 4.0), a, b), 5.0, "past the end: to the end");
        assert_eq!(distance_to_segment(egui::pos2(3.0, 4.0), a, a), 5.0, "a point segment");
    }

    #[test]
    fn the_hit_tolerance_is_six_screen_points_at_any_zoom() {
        assert_eq!(link_hit_tolerance(1.0), 6.0);
        assert_eq!(link_hit_tolerance(2.0), 12.0, "zoomed out (inv_zoom 2): 12 world units, 6 on screen");
    }

    fn edge(link: Option<&str>, y: f32) -> LevelEdge {
        LevelEdge {
            from: egui::pos2(0.0, y),
            to: egui::pos2(100.0, y),
            from_id: "x".into(),
            to_id: "y".into(),
            curve: if link.is_some() { EdgeCurve::Straight } else { EdgeCurve::Bow },
            highlight: false,
            kind: if link.is_some() { EdgeKind::Link } else { EdgeKind::Opened },
            link: link.map(str::to_string),
        }
    }

    #[test]
    fn nearest_link_takes_the_closest_link_line_within_tolerance() {
        let edges = [edge(None, 0.0), edge(Some("far"), 5.0), edge(Some("near"), 2.0)];
        let at = egui::pos2(50.0, 0.0);
        assert_eq!(nearest_link(&edges, at, 6.0).and_then(|e| e.link.as_deref()), Some("near"), "opened-from never");
        assert!(nearest_link(&edges, egui::pos2(50.0, 20.0), 6.0).is_none(), "nothing within 6");
        let twins = [edge(Some("older"), 1.0), edge(Some("newer"), 1.0)];
        assert_eq!(nearest_link(&twins, at, 6.0).and_then(|e| e.link.as_deref()), Some("newer"), "last of equals");
    }

    /// Drives `build_graph_ui` headless, as `a_click_finishes_or_cancels_pick_target_mode` (graph_ui.rs) does.
    struct Harness {
        ctx: egui::Context,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::apply_fonts(&ctx);
            Self { ctx }
        }

        /// One frame at the size `graph_world_rect_to_screen` assumes with no window. Returns the cursor it asked for.
        fn frame(&self, app: &mut App, events: Vec<egui::Event>) -> egui::CursorIcon {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
            output.platform_output.cursor_icon
        }

        fn click(&self, app: &mut App, at: egui::Pos2) {
            self.frame(app, vec![egui::Event::PointerMoved(at), press(at, true)]);
            self.frame(app, vec![press(at, false)]);
            self.frame(app, vec![]);
        }

        /// Hovers `from` for two frames (so a hover-only widget there is registered and hit-testable), presses,
        /// moves to `to` in eight steps (sending `midway` with the fourth), and releases there.
        fn drag_with(&self, app: &mut App, from: egui::Pos2, to: egui::Pos2, midway: Vec<egui::Event>) {
            self.frame(app, vec![egui::Event::PointerMoved(from)]);
            self.frame(app, vec![egui::Event::PointerMoved(from)]);
            self.frame(app, vec![press(from, true)]);
            for step in 1..=8 {
                let mut events = vec![egui::Event::PointerMoved(from + (to - from) * (step as f32 / 8.0))];
                if step == 4 {
                    events.extend(midway.iter().cloned());
                }
                self.frame(app, events);
            }
            self.frame(app, vec![press(to, false)]);
            self.frame(app, vec![]);
        }

        fn drag(&self, app: &mut App, from: egui::Pos2, to: egui::Pos2) {
            self.drag_with(app, from, to, Vec::new());
        }
    }

    fn press(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    /// Three notes, a and b side by side and c below a, warmed up and fitted in view.
    fn three_notes() -> (Harness, App, [String; 3]) {
        let h = Harness::new();
        let mut app = App::default();
        let ids = [app.alloc_node_id(), app.alloc_node_id(), app.alloc_node_id()];
        for (id, at) in ids.iter().zip([egui::vec2(0.0, 0.0), egui::vec2(600.0, 0.0), egui::vec2(0.0, 400.0)]) {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id.clone(), at, Kind::Note, note);
        }
        for _ in 0..3 {
            h.frame(&mut app, vec![]);
        }
        app.camera.jump_to(app.fit_all_nodes_scene_rect());
        h.frame(&mut app, vec![]);
        (h, app, ids)
    }

    fn screen_rect(app: &App, id: &str) -> egui::Rect {
        app.graph_world_rect_to_screen(app.card_rect(id).expect("a card"))
    }

    /// The middle of `edge`'s straight segment, on screen.
    fn middle(app: &App, edge: &LevelEdge) -> egui::Pos2 {
        app.graph_world_rect_to_screen(egui::Rect::from_two_pos(edge.from, edge.to)).center()
    }

    fn line_middle(app: &App, link: &str) -> egui::Pos2 {
        let edges = app.level_edges();
        middle(app, edges.iter().find(|e| e.link.as_deref() == Some(link)).expect("drawn"))
    }

    #[test]
    fn a_click_on_a_link_line_deletes_that_link() {
        let (h, mut app, [a, b, c]) = three_notes();
        let ab = app.create_link(&a, &b);
        let ac = app.create_link(&a, &c);
        h.frame(&mut app, vec![]);
        h.click(&mut app, line_middle(&app, &ab));
        assert_eq!(app.links_naming(&a), [ac], "exactly the clicked link went");
    }

    #[test]
    fn hovering_a_link_line_shows_the_pointing_hand() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        let on = line_middle(&app, &ab);
        h.frame(&mut app, vec![egui::Event::PointerMoved(on)]);
        assert_eq!(h.frame(&mut app, vec![]), egui::CursorIcon::PointingHand);
        h.frame(&mut app, vec![egui::Event::PointerMoved(on + egui::vec2(0.0, 30.0))]);
        assert_ne!(h.frame(&mut app, vec![]), egui::CursorIcon::PointingHand, "30 points off the line");
    }

    #[test]
    fn a_click_beside_a_line_on_an_opened_from_line_or_on_a_card_deletes_nothing() {
        let (h, mut app, [a, b, c]) = three_notes();
        let ab = app.create_link(&a, &b);
        app.edit_parent(&c, Some(a.clone()));
        h.frame(&mut app, vec![]);
        h.click(&mut app, line_middle(&app, &ab) + egui::vec2(0.0, 30.0));
        let opened = app.level_edges().into_iter().find(|e| e.kind == EdgeKind::Opened).expect("c opened from a");
        h.click(&mut app, middle(&app, &opened));
        // b's centre is the link line's end: the card is on top and takes the click.
        h.click(&mut app, screen_rect(&app, &b).center());
        assert_eq!(app.links_naming(&a), [ab]);
        assert_eq!(app.mirror.parent(&c), Some(a.as_str()), "an opened-from line is not clickable");
    }

    #[test]
    fn a_line_click_is_ignored_in_pick_mode_and_with_a_menu_open() {
        let (h, mut app, [a, b, c]) = three_notes();
        let ab = app.create_link(&a, &b);
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &ab);
        app.connecting = Some((c.clone(), crate::caps::LINK));
        h.click(&mut app, on);
        assert!(app.connecting.is_none(), "the click ended pick mode, as any click off a target does");
        assert_eq!(app.links_naming(&a), [ab.clone()], "and deleted nothing");
        app.node_menu = Some((c.clone(), egui::pos2(5.0, 60.0)));
        h.frame(&mut app, vec![]);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), [ab]);
    }

    #[test]
    fn two_links_on_one_pair_go_one_click_at_a_time_newest_first() {
        let (h, mut app, [a, b, _]) = three_notes();
        let older = app.create_link(&a, &b);
        let newer = app.create_link(&a, &b);
        assert!(newer.parse::<NodeId>().unwrap() > older.parse::<NodeId>().unwrap(), "one launch: ids grow");
        h.frame(&mut app, vec![]);
        let on = line_middle(&app, &older);
        h.click(&mut app, on);
        assert_eq!(app.links_naming(&a), [older]);
        h.click(&mut app, on);
        assert!(app.links_naming(&a).is_empty());
    }

    #[test]
    fn a_drag_from_a_link_line_pans_and_deletes_nothing() {
        let (h, mut app, [a, b, _]) = three_notes();
        let ab = app.create_link(&a, &b);
        h.frame(&mut app, vec![]);
        let before = app.camera.rect;
        let on = line_middle(&app, &ab);
        h.drag(&mut app, on, on + egui::vec2(120.0, 80.0));
        assert_ne!(app.camera.rect.center(), before.center(), "the drag panned");
        assert_eq!(app.links_naming(&a), [ab]);
    }
}
```

In `crates/manifold/src/level.rs`, test `level_edges_join_parent_and_child_centers_on_this_level_only`, after the `curve` assertion:

```rust
        assert_eq!(
            (edges[0].from_id.as_str(), edges[0].to_id.as_str(), edges[0].link.as_deref()),
            ("p", "c", None),
            "an opened-from line names its ends and no Link node"
        );
```

In `crates/manifold/src/json_level.rs`, after `assert_eq!(app.level_edges().len(), 2);` (498):

```rust
        assert!(app.level_edges().iter().all(|e| e.link.is_none()), "no JSON-level line is a link");
```

In `crates/manifold/src/session_sync/tests.rs`, `a_link_is_a_level_edge_only_while_both_ends_are_live_at_this_level`, after the `curve` assertion (4970):

```rust
    assert!(edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Link)
        .all(|e| (e.from_id.as_str(), e.to_id.as_str()) == (a.as_str(), b.as_str()) && e.link.is_some()));
```

In `crates/manifold/src/debug_control.rs` `mod tests`:

```rust
    /// EXPORTSTATE's `edges`: each line at this level with its kind, Link node, ends and on-screen middle.
    #[test]
    fn export_state_lists_the_level_edges_with_their_link() {
        use crate::node::NotePayload;
        use crate::store::model::Kind;
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        for (id, x) in [(&a, 0.0), (&b, 400.0)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id.clone(), egui::vec2(x, 0.0), Kind::Note, note);
        }
        let link = app.create_link(&a, &b);
        let state = app.export_state();
        let edges = state["edges"].as_array().expect("an array");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["kind"], "link");
        assert_eq!(edges[0]["link"], link.as_str());
        assert_eq!((edges[0]["from"].as_str(), edges[0]["to"].as_str()), (Some(a.as_str()), Some(b.as_str())));
        let mid = app.graph_world_rect_to_screen(egui::Rect::from_two_pos(egui::pos2(0.0, 0.0), egui::pos2(400.0, 0.0)));
        let screen: Vec<f64> = edges[0]["screen"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!((screen[0] as f32 - mid.center().x).abs() < 0.01 && (screen[1] as f32 - mid.center().y).abs() < 0.01);
    }
```

- [ ] **Step 2: Run them to see them fail.**
  - Run `cargo test -j 8 -p manifold link_gestures export_state_lists_the_level_edges level_edges_join a_link_is_a_level_edge entering_builds_the_level`.
  - Expect compile errors: there is no `from_id` / `to_id` / `link` on `LevelEdge`, and no `distance_to_segment` / `link_hit_tolerance` / `nearest_link`.

- [ ] **Step 3: `LevelEdge` and `level_edges`.**

In `crates/manifold/src/level.rs`, replace the struct (lines 46-54):

```rust
/// One edge on the current level, in world space.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LevelEdge {
    pub(crate) from: egui::Pos2,
    pub(crate) to: egui::Pos2,
    /// The cards at each end (a JSON level's card ids there).
    pub(crate) from_id: String,
    pub(crate) to_id: String,
    pub(crate) curve: EdgeCurve,
    pub(crate) highlight: bool,
    pub(crate) kind: EdgeKind,
    /// The Link node this line draws. `None` for an opened-from line and every JSON-level line, which no click takes.
    pub(crate) link: Option<String>,
}
```

In `level_edges`:
- The JSON branch becomes `.iter().zip(&level.rects).enumerate().filter_map(|(i, (card, rect))| { … })`. Its `LevelEdge` adds `from_id: level.card_id(parent), to_id: level.card_id(i)` and `link: None`.
- The `opened` edge adds `from_id: parent_id.to_string(), to_id: id.clone()` and `link: None`.
- The `links` closure becomes:

```rust
        let links = self.mirror.link_ids().filter_map(|link| {
            let (from, to) = self.mirror.link_ends(&link.to_string());
            let (from, to) = (from?.to_string(), to?.to_string());
            let both_here = [&from, &to].iter().all(|end| self.nodes.contains_key(*end) && self.in_level(end));
            if !both_here {
                return None;
            }
            Some(LevelEdge {
                from: self.layout_pos(&from).to_pos2(),
                to: self.layout_pos(&to).to_pos2(),
                from_id: from,
                to_id: to,
                curve: EdgeCurve::Straight,
                highlight: false,
                kind: EdgeKind::Link,
                link: Some(link.to_string()),
            })
        });
```

Struct fields are evaluated in the order written, so `from` is read by `layout_pos` before it moves into `from_id`. Update the doc comment of `level_edges` to say: "Link lines come in `link_ids` order, so the last of two on one pair is the greater id (`nearest_link`'s tie rule)."

In `crates/manifold/src/main.rs`, line 92: `use level::{EdgeCurve, EdgeKind, LevelEdge, LevelItem, LevelItemKind, LevelScan};`. Drop `LevelEdge` again if nothing in graph_ui uses it unqualified, since clippy flags an unused import.

- [ ] **Step 4: The hit-test helpers and `App` methods.**

Above the test module in `crates/manifold/src/link_gestures.rs`:

```rust
use crate::level::LevelEdge;
use crate::store::model::NodeId;
use crate::App;

/// A link line's stroke width in screen points (times `inv_zoom` in world units), and while the pointer is on it.
pub(crate) const LINK_STROKE_PX: f32 = 1.5;
pub(crate) const LINK_HOVER_STROKE_PX: f32 = 3.0;
/// How far from a link line, in screen points, a click still takes it.
pub(crate) const LINK_HIT_PX: f32 = 6.0;

/// The distance from `p` to the segment `a`-`b`; to `a` when the segment is a point.
pub(crate) fn distance_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The hit tolerance in world units at `inv_zoom`: 6 screen points, or half the hovered line if that were wider.
pub(crate) fn link_hit_tolerance(inv_zoom: f32) -> f32 {
    LINK_HIT_PX.max(LINK_HOVER_STROKE_PX / 2.0) * inv_zoom
}

/// The link line nearest `at` (world) within `tolerance`, or none. Opened-from lines are never taken. Of equals the
/// last wins: `level_edges` lists links in id order, so on one pair that is the newest within a launch.
pub(crate) fn nearest_link(edges: &[LevelEdge], at: egui::Pos2, tolerance: f32) -> Option<&LevelEdge> {
    let mut best: Option<(f32, &LevelEdge)> = None;
    for edge in edges.iter().filter(|e| e.link.is_some()) {
        let d = distance_to_segment(at, edge.from, edge.to);
        if d <= tolerance && best.is_none_or(|(b, _)| d <= b) {
            best = Some((d, edge));
        }
    }
    best.map(|(_, edge)| edge)
}

impl App {
    /// Whether a click on a link line may delete it now. Not in a JSON level, and not while a card drag, pick-target
    /// mode, a card or new-node menu, or the right-drag marquee owns the pointer.
    pub(crate) fn line_click_allowed(&self) -> bool {
        self.json_level.is_none()
            && self.dragging.is_none()
            && self.connecting.is_none()
            && self.node_menu.is_none()
            && self.pending_new_node.is_none()
            && self.rect_select_start.is_none()
    }

    /// The link line under the pointer. Called inside `build_graph_ui`'s Scene, where `ui.response()` is the Scene's
    /// own background: hovered only when no card, control or popup is on top and nothing is being dragged. Off the
    /// minimap (`minimap`, screen points), which is painted, not a widget. Sets the pointing-hand cursor when there is
    /// a line, so a click is never a surprise.
    pub(crate) fn hovered_link_line(
        &self,
        ui: &egui::Ui,
        edges: &[LevelEdge],
        inv_zoom: f32,
        minimap: Option<egui::Rect>,
    ) -> Option<String> {
        if !self.line_click_allowed() {
            return None;
        }
        let background = ui.response();
        if !background.hovered() || background.dragged() {
            return None;
        }
        let pointer = ui.input(|i| i.pointer.hover_pos())?;
        if minimap.is_some_and(|rect| rect.contains(pointer)) {
            return None;
        }
        // World space: `Response::hover_pos` applies the Scene layer's transform.
        let at = background.hover_pos()?;
        let link = nearest_link(edges, at, link_hit_tolerance(inv_zoom))?.link.clone();
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        link
    }

    /// A click on link line `link`: deletes exactly that Link node, as the node menu's Disconnect row does, and saves.
    pub(crate) fn click_link_line(&mut self, link: &str) {
        if link.parse::<NodeId>().is_ok_and(|id| self.mirror.is_link(id)) {
            self.delete_link(link);
            self.save_session();
        }
    }
}
```

- [ ] **Step 5: Wire it into `build_graph_ui` and `draw_graph_edges`.**

In `crates/manifold/src/graph_ui.rs`, inside the Scene closure, replace `self.draw_graph_edges(ui, inv_zoom);` (566) with:

```rust
                // The link line under the pointer (link-gestures spec B) is drawn highlighted; a click on it is
                // applied after the Scene, from the Scene's own response.
                let mut edges = self.level_edges();
                let line_hover = self.hovered_link_line(ui, &edges, inv_zoom, minimap.map(|(rect, _, _)| rect));
                for edge in &mut edges {
                    edge.highlight |= edge.link.is_some() && edge.link == line_hover;
                }
                self.draw_graph_edges(ui, &edges, inv_zoom);
```

and make the closure return `(deferred_overlays, line_hover)` instead of `deferred_overlays`. After the closure, destructure:

```rust
        let egui::InnerResponse { inner: (deferred_overlays, line_hover), response: scene_background } = scene_response;
```

and change `for overlay in scene_response.inner {` to `for overlay in deferred_overlays {`. After the `handle_minimap_click` block (605-607) and before `for action in actions` (609), add:

```rust
        // A click on a link line deletes that link (link-gestures spec B). The Scene's background took the click, so
        // no card, control or popup was on top, and egui never reports a pan (a drag) as a click.
        if let Some(link) = line_hover.filter(|_| scene_background.clicked()) {
            self.click_link_line(&link);
        }
```

`draw_graph_edges` (1496) becomes `pub(crate) fn draw_graph_edges(&self, ui: &egui::Ui, edges: &[LevelEdge], inv_zoom: f32)`. The loop becomes `for edge in edges {`, and the Link stroke becomes:

```rust
                EdgeKind::Link => {
                    let width = if edge.highlight {
                        crate::link_gestures::LINK_HOVER_STROKE_PX
                    } else {
                        crate::link_gestures::LINK_STROKE_PX
                    };
                    egui::Stroke::new(width * inv_zoom, ACCENT)
                }
```

Extend its doc comment: "A hovered link line (`highlight`) is drawn twice as thick." It has one caller, graph_ui.rs:566; confirm with `grep -rn draw_graph_edges crates/`.

- [ ] **Step 6: EXPORTSTATE `edges`.**

In `export_state` (debug_control.rs), before the `serde_json::json!({` (about line 1399):

```rust
        // The lines drawn at this level, with the middle of each straight segment on screen (where a probe clicks a
        // link line). A bowed opened-from line passes elsewhere; its middle is listed for symmetry only.
        let edges: Vec<serde_json::Value> = self
            .level_edges()
            .iter()
            .map(|edge| {
                let mid = self.graph_world_rect_to_screen(egui::Rect::from_two_pos(edge.from, edge.to)).center();
                let kind = match edge.kind {
                    crate::level::EdgeKind::Link => "link",
                    crate::level::EdgeKind::Opened => "opened",
                };
                serde_json::json!({
                    "kind": kind, "link": edge.link, "from": edge.from_id, "to": edge.to_id, "screen": [mid.x, mid.y],
                })
            })
            .collect();
```

Add `"edges": edges,` after `"links": links,`. In the ExportState doc (236-252), after the `links` sentence, add: "`edges` lists the lines drawn at the current level as `{kind: link|opened, link (the Link node id, or null), from, to, screen: [x, y]}`, where `screen` is the middle of the straight segment between the two card centres."

- [ ] **Step 7: Run the tests.**
  - Run `cargo test -j 8 -p manifold link_gestures export_state level_edges a_link_is_a_level_edge entering_builds_the_level pick_target`. Expect all to pass.
  - Run the whole suite, `cargo test -j 8 -p manifold`, and expect no new failures.
  - If `hovering_a_link_line_shows_the_pointing_hand` fails, check that nothing later in the frame overrides the cursor (`set_cursor_icon` is last-writer-wins). Do not weaken the test.

- [ ] **Step 8: fmt, clippy, commit.**
  - Run `cargo fmt --all` and the clippy command from Global Constraints.
  - Commit "Link gestures: click a link line to disconnect", with the body naming the decision ("click read from the Scene's background response; no per-edge region") and the `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` trailer.

---

### Task 2: Edge handles and connect-drag

**Files:**
- Modify: `crates/manifold/src/link_gestures.rs` (handle geometry, painting, `App` methods, tests).
- Modify: `crates/manifold/src/main.rs`: the field `connect_drag` after `connecting` (1176), and its initialiser after `connecting: None,` (1601).
- Modify: `crates/manifold/src/links.rs`:
  - `finish_connect`, `cancel_connect` and `forget_connect_source` also end the connect-drag.
  - `end_stale_connect` ends it through `cancel_connect` and also treats a connect-drag without its mode as stale.
- Modify: `crates/manifold/src/session_sync.rs`, the session-replace reset (~2046): add `self.connect_drag = None;`.
- Modify: `crates/manifold/src/card.rs`:
  - `CardPass` (613) gains `connect_target`.
  - In `draw_graph_card`, the pick-ring alpha (1111), handles after the controls loop (1152), and `still_hovered` (1157).
  - The test `CardPass` literal (~2542) gains `connect_target: None`.
- Modify: `crates/manifold/src/graph_ui.rs`:
  - `draw_graph_cards`: `connect_target` and the rubber band.
  - `build_graph_ui`: the release, placed after the closure.
  - `track_pan_momentum` (1371).
- Modify: `crates/manifold/src/debug_control.rs` (EXPORTSTATE `link_handles`, `connect_drag`; doc; test).

**Interfaces:**
- Consumes (Task 1): `LINK_STROKE_PX`, `App::line_click_allowed`.
- Produces:
  - `link_gestures::HandleSide { Top, Right, Bottom, Left }`, with `ALL` and `name()`.
  - `HANDLE_PX`, `HANDLE_MIN_EDGE_PX`.
  - `link_handle_rects(card: Rect, zoom_scale: f32, simplified: bool) -> Vec<(HandleSide, Rect)>`.
  - `paint_link_handle(ui, rect, active, inv_zoom)`.
  - `App.connect_drag: Option<String>`.
  - `App::shows_link_handles(&self, id) -> bool`.
  - `App::start_connect_drag(&mut self, id)`.
  - `App::card_at(&self, world: Pos2) -> Option<String>`.
  - `App::connect_drag_target(&self, ui) -> Option<String>`.
  - `App::draw_connect_rubber_band(&self, ui, inv_zoom)`.
  - `App::end_connect_drag(&mut self, target: Option<String>)`.
  - `CardPass.connect_target: Option<&'a str>`.
  - EXPORTSTATE `link_handles`, `connect_drag`.

- [ ] **Step 1: Write the failing tests** (append to `link_gestures::tests`).

```rust
    #[test]
    fn handles_sit_on_each_edge_midpoint_at_a_constant_screen_size() {
        let card = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(180.0, 140.0));
        let rects = link_handle_rects(card, 2.0, false);
        assert_eq!(rects.iter().map(|(side, _)| *side).collect::<Vec<_>>(), HandleSide::ALL);
        assert!(rects.iter().all(|(_, r)| r.width() == HANDLE_PX / 2.0), "12 screen points at zoom 2");
        assert_eq!(rects[0].1.center(), card.center_top());
        assert_eq!(rects[1].1.center(), card.right_center());
        assert_eq!(rects[2].1.center(), card.center_bottom());
        assert_eq!(rects[3].1.center(), card.left_center());
    }

    #[test]
    fn handles_hide_on_short_edges_and_when_simplified() {
        let thin = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(180.0, 30.0));
        let sides: Vec<HandleSide> = link_handle_rects(thin, 1.0, false).into_iter().map(|(s, _)| s).collect();
        assert_eq!(sides, [HandleSide::Top, HandleSide::Bottom], "30 points tall: no side handles");
        assert!(link_handle_rects(thin, 0.2, false).is_empty(), "180 * 0.2 = 36 points wide: none");
        assert!(link_handle_rects(thin, 3.0, true).is_empty(), "a simplified level shows none");
    }

    /// Two notes at the top level, not yet drawn.
    fn two_notes() -> (App, String, String) {
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        for (id, x) in [(&a, 0.0), (&b, 600.0)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id.clone(), egui::vec2(x, 0.0), Kind::Note, note);
        }
        (app, a, b)
    }

    #[test]
    fn handles_show_only_while_nothing_else_owns_the_pointer() {
        let (mut app, a, b) = two_notes();
        assert!(app.shows_link_handles(&a));
        app.dragging = Some(b.clone());
        assert!(!app.shows_link_handles(&a), "a card drag");
        app.dragging = None;
        app.node_menu = Some((b.clone(), egui::Pos2::ZERO));
        assert!(!app.shows_link_handles(&a), "a menu");
        app.node_menu = None;
        app.start_connect_drag(&a);
        assert!(app.shows_link_handles(&a) && !app.shows_link_handles(&b), "during a connect-drag only its source");
        app.cancel_connect();
        app.create_link(&a, &b);
        assert!(!app.shows_link_handles(&a), "linked to the only other card: no target left");
    }

    #[test]
    fn a_connect_drag_ends_with_its_mode_and_with_its_source() {
        let (mut app, a, b) = two_notes();
        app.start_connect_drag(&a);
        assert_eq!(app.connecting, Some((a.clone(), crate::caps::LINK)), "it runs as pick-target mode for link");
        app.cancel_connect();
        assert!(app.connect_drag.is_none());
        app.start_connect_drag(&a);
        app.forget_connect_source(&a);
        assert!(app.connect_drag.is_none() && app.connecting.is_none(), "its source removed");
        app.start_connect_drag(&a);
        app.end_stale_connect(false);
        assert!(app.connect_drag.is_none(), "Graph view left");
        app.start_connect_drag(&a);
        app.connecting = None;
        app.end_stale_connect(true);
        assert!(app.connect_drag.is_none(), "a connect-drag without its mode is stale");
        app.start_connect_drag(&a);
        app.end_connect_drag(None);
        assert!(app.connect_drag.is_none() && app.connecting.is_none() && app.links_naming(&a).is_empty());
        app.start_connect_drag(&a);
        app.end_connect_drag(Some(b.clone()));
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
        assert_eq!(app.links_naming(&a).len(), 1, "released on b");
    }

    fn handle_on_screen(app: &App, id: &str, side: HandleSide) -> egui::Pos2 {
        let (zoom, _) = app.graph_world_to_screen_transform(app.graph_scene_outer_rect());
        let rects = link_handle_rects(app.card_rect(id).expect("a card"), zoom, false);
        let (_, rect) = rects.into_iter().find(|(s, _)| *s == side).expect("shown at this zoom");
        app.graph_world_rect_to_screen(rect).center()
    }

    /// Hovers card `id` (so its handles show), then drags from its `side` handle to `to`.
    fn drag_from_handle(h: &Harness, app: &mut App, id: &str, side: HandleSide, to: egui::Pos2, midway: Vec<egui::Event>) {
        h.frame(app, vec![egui::Event::PointerMoved(screen_rect(app, id).center())]);
        let from = handle_on_screen(app, id, side);
        h.drag_with(app, from, to, midway);
    }

    #[test]
    fn dragging_a_handle_onto_a_card_links_them_and_moves_nothing() {
        let (h, mut app, [a, b, _]) = three_notes();
        let before = app.layout_pos(&a);
        let onto = screen_rect(&app, &b).center();
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, onto, vec![]);
        let links = app.links_naming(&a);
        assert_eq!(links.len(), 1);
        assert_eq!(app.mirror.link_ends(&links[0]), (a.parse().ok(), b.parse().ok()), "a -> b");
        assert_eq!(app.layout_pos(&a), before, "the card did not move");
        assert!(app.dragging.is_none() && app.connect_drag.is_none() && app.connecting.is_none());
    }

    #[test]
    fn a_handle_drag_released_on_canvas_or_on_its_source_links_nothing_silently() {
        let (h, mut app, [a, b, _]) = three_notes();
        let canvas = screen_rect(&app, &a).center().lerp(screen_rect(&app, &b).center(), 0.5);
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, canvas, vec![]);
        let own = screen_rect(&app, &a).center();
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, own, vec![]);
        assert!(app.links_naming(&a).is_empty());
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
        assert!(app.save_toast.is_none() && app.save_toast_queue.is_empty(), "no toast");
    }

    #[test]
    fn a_handle_drag_onto_a_card_already_linked_says_so() {
        let (h, mut app, [a, b, _]) = three_notes();
        app.create_link(&a, &b);
        let onto = screen_rect(&app, &b).center();
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, onto, vec![]);
        assert_eq!(app.links_naming(&a).len(), 1);
        let toasts: Vec<&String> = app.save_toast.iter().map(|(m, _)| m).chain(&app.save_toast_queue).collect();
        assert!(toasts.iter().any(|m| m.as_str() == "Can't connect these cards"), "{toasts:?}");
    }

    #[test]
    fn escape_cancels_a_handle_drag() {
        let (h, mut app, [a, b, _]) = three_notes();
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let onto = screen_rect(&app, &b).center();
        drag_from_handle(&h, &mut app, &a, HandleSide::Right, onto, vec![escape]);
        assert!(app.links_naming(&a).is_empty());
        assert!(app.connect_drag.is_none() && app.connecting.is_none());
    }

    #[test]
    fn pressing_a_handle_without_moving_ends_the_gesture_and_clicks_nothing() {
        let (h, mut app, [a, _, _]) = three_notes();
        let handle = handle_on_screen(&app, &a, HandleSide::Bottom);
        h.frame(&mut app, vec![egui::Event::PointerMoved(screen_rect(&app, &a).center())]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![egui::Event::PointerMoved(handle)]);
        h.frame(&mut app, vec![press(handle, true)]);
        assert_eq!(app.connect_drag.as_deref(), Some(a.as_str()), "the press started it");
        h.frame(&mut app, vec![press(handle, false)]);
        h.frame(&mut app, vec![]);
        assert!(app.connect_drag.is_none() && app.connecting.is_none() && app.links_naming(&a).is_empty());
        assert!(!app.nodes[&a].as_note().unwrap().editing, "the press was the handle's, not a click on the note");
    }

    #[test]
    fn a_drag_on_the_card_body_still_moves_the_card() {
        let (h, mut app, [a, _, _]) = three_notes();
        let before = app.layout_pos(&a);
        let from = screen_rect(&app, &a).center();
        h.drag(&mut app, from, from + egui::vec2(-60.0, 0.0));
        assert_ne!(app.layout_pos(&a), before, "the card moved");
        assert!(app.connect_drag.is_none() && app.links_naming(&a).is_empty());
    }
```

In `debug_control.rs` `mod tests`:

```rust
    /// EXPORTSTATE's `link_handles`: each card that would show connect handles, each side's on-screen rect.
    #[test]
    fn export_state_gives_each_linkable_card_its_handle_rects() {
        use crate::node::NotePayload;
        use crate::store::model::Kind;
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        for (id, x) in [(&a, 0.0), (&b, 400.0)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id.clone(), egui::vec2(x, 0.0), Kind::Note, note);
        }
        let state = app.export_state();
        let handles = state["link_handles"][a.as_str()].as_object().expect("a has handles");
        assert_eq!(handles.keys().map(String::as_str).collect::<Vec<_>>(), ["top", "right", "bottom", "left"]);
        let r: Vec<f32> = handles["right"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let card = app.graph_world_rect_to_screen(app.card_rect(&a).unwrap());
        assert!((egui::pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0) - card.right_center()).length() < 0.01);
        assert!((r[2] - r[0] - crate::link_gestures::HANDLE_PX).abs() < 0.01, "12 points wide on screen");
        assert!(state["connect_drag"].is_null());
        app.start_connect_drag(&a);
        let state = app.export_state();
        assert_eq!(state["connect_drag"], a.as_str());
        assert!(state["link_handles"].get(b.as_str()).is_none(), "during a connect-drag only the source");
    }
```

- [ ] **Step 2: Run them to see them fail.** Run `cargo test -j 8 -p manifold link_gestures export_state_gives`. Expect compile errors: there is no `HandleSide`, `link_handle_rects` or `connect_drag`.

- [ ] **Step 3: State and lifecycle.**

In `crates/manifold/src/main.rs`, after `connecting` (1176):

```rust
    /// A connect-drag (link-gestures spec A): the card whose edge handle the primary button went down on. While it
    /// runs, `connecting` is `(source, LINK)`, so pick-target mode's rings show the targets. The release ends it on
    /// the card under the pointer (`end_connect_drag`). It ends with pick-target mode (`cancel_connect`,
    /// `finish_connect`), so every path that ends that mode ends this too.
    connect_drag: Option<String>,
```

And add `connect_drag: None,` after `connecting: None,` (1601).

In `crates/manifold/src/links.rs`:
- `finish_connect`: add `self.connect_drag = None;` as its first line.
- `cancel_connect`: add `self.connect_drag = None;` and change its doc to "Ends pick-target mode, and a connect-drag running as it, without drawing anything."
- `forget_connect_source`:

```rust
    /// Ends pick-target mode, and a connect-drag, when `id`, a card being removed, is its source.
    pub(crate) fn forget_connect_source(&mut self, id: &str) {
        if self.connecting.as_ref().is_some_and(|(from, _)| from == id) || self.connect_drag.as_deref() == Some(id) {
            self.cancel_connect();
        }
    }
```

- `end_stale_connect`:

```rust
    /// Ends pick-target mode once its targets are no longer drawn: the Graph view not `graph_shown`, or the source
    /// gone or moved off the level shown (another replica's edit). A connect-drag left without its mode (a path that
    /// cleared `connecting` alone) ends too.
    pub(crate) fn end_stale_connect(&mut self, graph_shown: bool) {
        let stale = |(from, _): &(String, &str)| !graph_shown || !self.nodes.contains_key(from) || !self.in_level(from);
        if self.connecting.as_ref().is_some_and(stale) || (self.connecting.is_none() && self.connect_drag.is_some()) {
            self.cancel_connect();
        }
    }
```

In `crates/manifold/src/session_sync.rs`, the session-replace reset (~2046), after `self.connecting = None;`, add `self.connect_drag = None;`.

Every other reset path already goes through these:
- `drop_node` and `forget_removed_group` call `forget_connect_source`.
- `switch_group_level`, `apply_graph_action`, `apply_node_action` and the Escape in `handle_graph_keys` call `cancel_connect`.
- `build_ui_inner` calls `end_stale_connect` every frame. This covers `event_loop.rs:1313` and `view_modes.rs:306`, which clear `dragging` only.

Confirm with `grep -rn "connecting = " crates/manifold/src`: the only direct writes left should be in `links.rs`, the node menu's Connect (`node_menu.rs:999`, which starts a mode) and the session reset.

In `line_click_allowed` (Task 1), add `&& self.connect_drag.is_none()`. It is implied by `connecting`, but it makes the gate read as the spec's list.

- [ ] **Step 4: Handle geometry, painting and the `App` methods** (in `link_gestures.rs`, after Task 1's code):

```rust
/// A side of a card, where a connect-drag handle sits (centred on that edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HandleSide {
    Top,
    Right,
    Bottom,
    Left,
}

impl HandleSide {
    pub(crate) const ALL: [HandleSide; 4] = [HandleSide::Top, HandleSide::Right, HandleSide::Bottom, HandleSide::Left];

    /// The key EXPORTSTATE's `link_handles` uses.
    pub(crate) fn name(self) -> &'static str {
        match self {
            HandleSide::Top => "top",
            HandleSide::Right => "right",
            HandleSide::Bottom => "bottom",
            HandleSide::Left => "left",
        }
    }
}

/// A handle's diameter in screen points.
pub(crate) const HANDLE_PX: f32 = 12.0;
/// The shortest on-screen card edge that shows its handle. A top or right handle clears the 16-point Audio control
/// in the top-right corner (`CardControl::rect`) only while half the edge is at least 16 + 6 points.
pub(crate) const HANDLE_MIN_EDGE_PX: f32 = 44.0;

/// Card `card`'s handles (world space) at `zoom_scale`, each `HANDLE_PX` on screen, in `HandleSide::ALL` order. A side
/// whose edge is under `HANDLE_MIN_EDGE_PX` on screen has none, and a `simplified` level (`graph_lod_simplified`) has
/// none at all.
pub(crate) fn link_handle_rects(card: egui::Rect, zoom_scale: f32, simplified: bool) -> Vec<(HandleSide, egui::Rect)> {
    if simplified {
        return Vec::new();
    }
    let size = egui::Vec2::splat(HANDLE_PX / zoom_scale);
    HandleSide::ALL
        .into_iter()
        .filter_map(|side| {
            let (centre, edge) = match side {
                HandleSide::Top => (card.center_top(), card.width()),
                HandleSide::Right => (card.right_center(), card.height()),
                HandleSide::Bottom => (card.center_bottom(), card.width()),
                HandleSide::Left => (card.left_center(), card.height()),
            };
            (edge * zoom_scale >= HANDLE_MIN_EDGE_PX).then(|| (side, egui::Rect::from_center_size(centre, size)))
        })
        .collect()
}

/// A handle: an accent ring, filled while hovered or dragged from.
pub(crate) fn paint_link_handle(ui: &egui::Ui, handle: egui::Rect, active: bool, inv_zoom: f32) {
    let fill = if active { crate::theme::ACCENT } else { crate::theme::BG };
    let stroke = egui::Stroke::new(1.5 * inv_zoom, crate::theme::ACCENT);
    ui.painter().circle(handle.center(), handle.width() / 2.0, fill, stroke);
}
```

Inside `impl App`:

```rust
    /// Whether card `id` shows its connect-drag handles while hovered. During a connect-drag only its source does.
    /// Otherwise only when nothing else owns the pointer (as `line_click_allowed`) and the card has a valid link
    /// target at this level, which also makes it a valid link end.
    pub(crate) fn shows_link_handles(&self, id: &str) -> bool {
        if let Some(source) = &self.connect_drag {
            return source == id;
        }
        self.line_click_allowed() && !self.valid_targets(crate::caps::LINK, id).is_empty()
    }

    /// A press on one of card `id`'s handles: pick-target mode for `link` from it, which rings the targets, run as a
    /// drag.
    pub(crate) fn start_connect_drag(&mut self, id: &str) {
        self.connecting = Some((id.to_string(), crate::caps::LINK));
        self.connect_drag = Some(id.to_string());
    }

    /// The topmost card under world point `at`. `level_items` is bottom-first, as right-click's hit-test reads it.
    pub(crate) fn card_at(&self, at: egui::Pos2) -> Option<String> {
        self.level_items()
            .into_iter()
            .rev()
            .find(|item| matches!(item.kind, crate::level::LevelItemKind::Node(_)) && item.rect.contains(at))
            .map(|item| item.id)
    }

    /// The pointer in world space, inside `build_graph_ui`'s Scene (this frame's layer transform).
    fn scene_pointer(ui: &egui::Ui) -> Option<egui::Pos2> {
        let pointer = ui.input(|i| i.pointer.interact_pos())?;
        Some(ui.ctx().layer_transform_from_global(ui.layer_id()).map_or(pointer, |t| t * pointer))
    }

    /// During a connect-drag, the card other than the source under the pointer, whose target ring brightens. egui
    /// hovers only the dragged handle meanwhile, so the card's own hover cannot say so.
    pub(crate) fn connect_drag_target(&self, ui: &egui::Ui) -> Option<String> {
        let source = self.connect_drag.as_ref()?;
        self.card_at(Self::scene_pointer(ui)?).filter(|target| target != source)
    }

    /// The rubber band: a dashed accent line from the connect-drag's source centre to the pointer, over the cards.
    pub(crate) fn draw_connect_rubber_band(&self, ui: &egui::Ui, inv_zoom: f32) {
        let (Some(source), Some(pointer)) = (&self.connect_drag, Self::scene_pointer(ui)) else { return };
        let stroke = egui::Stroke::new(LINK_STROKE_PX * inv_zoom, crate::theme::ACCENT);
        let line = [self.layout_pos(source).to_pos2(), pointer];
        ui.painter().add(egui::Shape::dashed_line(&line, stroke, 6.0 * inv_zoom, 4.0 * inv_zoom));
    }

    /// Ends a connect-drag at `target`, the card under the pointer at release (or none). Any card other than the
    /// source finishes the connect (`finish_connect`, whose toast covers a refused pair). Empty canvas or the source
    /// cancels silently.
    pub(crate) fn end_connect_drag(&mut self, target: Option<String>) {
        let Some(source) = self.connect_drag.take() else { return };
        let owns_mode = self.connecting.as_ref().is_some_and(|(from, edge)| *from == source && *edge == crate::caps::LINK);
        match target.filter(|t| *t != source) {
            Some(target) if owns_mode => self.finish_connect(&target),
            _ => self.cancel_connect(),
        }
    }
```

- [ ] **Step 5: Handles on the card** (`crates/manifold/src/card.rs`).

Add to `CardPass` (after `pick_targets`):

```rust
    /// During a connect-drag, the card under the pointer (`App::connect_drag_target`): its target ring is drawn bright,
    /// as a hovered one is in pick-target mode.
    pub(crate) connect_target: Option<&'a str>,
```

Pick ring (1111): `let alpha = if pointer_on_card || pass.connect_target == Some(id) { 255 } else { 160 };`.

After the controls loop (after line 1152), replacing `let still_hovered = pointer_on_card || control_hovered;` (1157):

```rust
        // Connect-drag handles (link-gestures spec A), on a hovered card or on the source while its drag runs.
        // Registered after the card and its controls: egui gives a press on a drag-only widget on top to that widget
        // alone (no click below), so the card neither moves nor opens.
        let mut handle_hovered = false;
        let is_source = self.connect_drag.as_deref() == Some(id);
        if (is_hovered || is_source) && self.shows_link_handles(id) {
            let handles = crate::link_gestures::link_handle_rects(rect, pass.zoom_scale, self.graph_lod_simplified);
            for (side, handle) in handles {
                let handle_response = ui
                    .interact(handle, egui::Id::new(("link_handle", side, id)), egui::Sense::DRAG)
                    .on_hover_cursor(egui::CursorIcon::Crosshair);
                handle_hovered |= handle_response.hovered();
                let active = handle_response.hovered() || handle_response.dragged();
                crate::link_gestures::paint_link_handle(ui, handle, active, inv_zoom);
                if handle_response.drag_started() {
                    self.start_connect_drag(id);
                }
            }
        }

        // This frame's hover only: passing `is_hovered` (which includes
        // `self.hovered`) made a card stay hovered, ✕ and all, after the
        // pointer left it, until another card was hovered. Never during a
        // connect-drag, whose peek would cover the targets.
        let still_hovered = self.connect_drag.is_none() && (pointer_on_card || control_hovered || handle_hovered);
```

The existing comment above `still_hovered` moves with it, as shown. `pass.deferred_overlays` is already reborrowed as `deferred_overlays`; reading `pass.zoom_scale` is a disjoint field, so it compiles. In the test literal at ~2542, add `connect_target: None,`.

- [ ] **Step 6: Scene wiring** (`crates/manifold/src/graph_ui.rs`).
  - In `draw_graph_cards`, before `let mut pass = CardPass {`, add `let connect_target = self.connect_drag_target(ui);`. Add the field `connect_target: connect_target.as_deref(),`.
  - After `self.card_nav_active = pass.any_focused;` (the end of the function), add `self.draw_connect_rubber_band(ui, inv_zoom);`.
  - In `build_graph_ui`, directly after `self.track_pan_momentum(ui, scene_rect);` (603), add:

```rust
        // A connect-drag ends when the button comes up (link-gestures spec A). Its target is the card under the
        // pointer, unlike a card drop's (the dragged card's centre), in the scene and off the minimap.
        if self.connect_drag.is_some() && !ui.input(|i| i.pointer.primary_down()) {
            let at = ui.input(|i| i.pointer.interact_pos()).filter(|p| {
                scene_outer_rect.contains(*p) && !minimap.is_some_and(|(rect, _, _)| rect.contains(*p))
            });
            let target = at.and_then(|p| self.card_at(((p.to_vec2() - translation) / zoom_scale).to_pos2()));
            self.end_connect_drag(target);
        }
```

The release block sits before the line click, before the `actions` loop, and before the pick-target cancel, so a press and release on a handle ends the gesture before that rule sees `connecting`. `zoom_scale` and `translation` are the ones recomputed at 594.

`track_pan_momentum` (1371) becomes:

```rust
        let actively_panning =
            pointer_down && self.dragging.is_none() && self.connect_drag.is_none() && self.camera.tween.is_none();
```

That stops a connect-drag from recording pan samples that would start a glide on release.

- [ ] **Step 7: EXPORTSTATE `link_handles` and `connect_drag`.**

In `export_state`, before the `json!`:

```rust
        // Where each card that would show connect handles has them (shown on hover only), on screen, so a probe can
        // press one: `{card id: {top|right|bottom|left: [left, top, right, bottom]}}`.
        let (zoom_scale, _) = self.graph_world_to_screen_transform(self.graph_scene_outer_rect());
        let link_handles: serde_json::Map<String, serde_json::Value> = self
            .level_items()
            .into_iter()
            .filter(|item| matches!(item.kind, LevelItemKind::Node(_)) && self.shows_link_handles(&item.id))
            .map(|item| {
                let sides: serde_json::Map<String, serde_json::Value> =
                    crate::link_gestures::link_handle_rects(item.rect, zoom_scale, self.graph_lod_simplified)
                        .into_iter()
                        .map(|(side, rect)| {
                            let r = self.graph_world_rect_to_screen(rect);
                            (side.name().to_string(), serde_json::json!([r.left(), r.top(), r.right(), r.bottom()]))
                        })
                        .collect();
                (item.id, serde_json::Value::Object(sides))
            })
            .collect();
```

Add `"link_handles": link_handles,` and `"connect_drag": self.connect_drag,` after `"connecting"`. `serde_json` has `preserve_order`, so the sides keep ALL order. Use `crate::level::LevelItemKind` if it is not in scope there.

In the ExportState doc, after the `connecting` sentence, add: "`link_handles` maps each card that would show connect-drag handles when hovered to `{top, right, bottom, left}` (those shown at this zoom), each `[left, top, right, bottom]` on screen. `connect_drag` is the source card of a running connect-drag, or null."

- [ ] **Step 8: Run the tests.**
  - Run `cargo test -j 8 -p manifold link_gestures export_state pick_target escape_ends card`, then the whole suite with `cargo test -j 8 -p manifold`.
  - `a_drag_on_the_card_body_still_moves_the_card` and the existing drag and drop tests must pass unchanged, which proves the handles did not take the card drag.

- [ ] **Step 9: fmt, clippy, commit.** Run `cargo fmt --all` and clippy. Commit "Link gestures: drag from an edge handle to connect" with the `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` trailer.

---

### Task 3: Probe, screenshots, docs and the gate

**Files:**
- Modify: `scripts/link-probe.sh`: header comment; run 3 gains the line click and the handle drag; new run 4.
- Modify: `scripts/chrome-shots.sh`:
  - Header list (lines 13-30) and `ALL` (49).
  - Hoist `cardid()` out of the link block.
  - New views `graph-link-hover`, `graph-link-handles`, `graph-link-drag`.
- Modify: `README.md`: the Links paragraph (161), the `LINK` bullet (380), the chrome-shots paragraph (400) and the link-probe paragraph (406).
- Modify: `docs/superpowers/specs/2026-10-07-link-gestures-design.md` (Status) and this plan (Outcome).
- Build first: `cargo build -j 8 -p manifold -p manifold-engine-webview2`. The probe and shots run the existing exe, and `require_fresh_exe` refuses a stale one.

- [ ] **Step 1: Extend the probe.**

Replace `start log3 … stop` with the block below. Ids and coordinates go into the python as literals, and nothing reads stdin.

```bash
start log3
send "$(x s4)" "LINK $a $b" "SLEEP 1" "$(x s5)"
wait_file "$out/s5.json" && sleep 0.2
# The link line's middle on screen (EXPORTSTATE edges), clear of both cards: a click there deletes it.
read -r lx ly <<<"$(py 'import json,sys; e=[e for e in json.load(open(sys.argv[1], encoding="utf-8"))["edges"] if e["kind"]=="link"][0]["screen"]; print(round(e[0]), round(e[1]))' s5)"
send "MOUSEMOVE $lx $ly" "CLICK $lx $ly" "SLEEP 1" "$(x s6)"
wait_file "$out/s6.json" && sleep 0.2
# a's centre, a's right handle and b's centre, then a drag from the handle onto b in six steps.
read -r ax ay hx hy bx by <<<"$(py "import json,sys; d=json.load(open(sys.argv[1], encoding='utf-8')); c={x['id']: x['screen'] for x in d['cards']}; r=d['link_handles']['$a']['right']; print(*[round(v) for v in (*c['$a'], (r[0]+r[2])/2, (r[1]+r[3])/2, *c['$b'])])" s6)"
send "MOUSEMOVE $ax $ay" "MOUSEMOVE $hx $hy" "MOUSEMOVE $hx $hy" "MOUSEDOWN $hx $hy"
for i in 1 2 3 4 5 6; do send "MOUSEMOVE $((hx + (bx - hx) * i / 6)) $((hy + (by - hy) * i / 6))"; done
send "MOUSEUP $bx $by" "SLEEP 1" "$(x s7)"
stop
start log4
send "$(x s8)"
stop
```

Move `read -r n4 _ <<<"$(summary s4)"` and the checks so they read the new states. Add these helpers and checks:

```bash
# a's rect, rounded, for "the card did not move"; and connect_drag.
rect_of() { py "import json,sys; print(*[round(v) for v in [c for c in json.load(open(sys.argv[1], encoding='utf-8'))['cards'] if c['id']=='$a'][0]['rect']])" "$1"; }
field() { py "import json,sys; print(json.load(open(sys.argv[1], encoding='utf-8'))['$2'])" "$1"; }
read -r n5 _ <<<"$(summary s5)"
read -r n6 _ <<<"$(summary s6)"
read -r n7 id7 from7 to7 <<<"$(summary s7)"
read -r n8 id8 _ <<<"$(summary s8)"
check "LINK again" "1" "$n5"
check "a click on the link line removes it" "0" "$n6"
check "a drag from a's handle onto b links a -> b" "1 $a $b" "$n7 $from7 $to7"
check "the card dragged from did not move" "$(rect_of s6)" "$(rect_of s7)"
check "the connect-drag ended" "None" "$(field s7 connect_drag)"
check "the dragged link persists across a restart" "1 $id7" "$n8 $id8"
```

Add `log4` to the failure tail loop. Update the header comment:
- 4. In run 3, LINK again, then a CLICK on the line's middle (from EXPORTSTATE `edges`) deletes it.
- 5. A MOUSEDOWN on a's right handle (`link_handles`), six MOUSEMOVEs and a MOUSEUP on b links a to b, without moving a.
- 6. Run 4 still has that link.

Run `bash scripts/link-probe.sh` and expect 13 PASS lines (7 old, 6 new). If the drag check fails, rerun with `KEEP=1` and read `s6.json` / `s7.json` before changing anything. `debug_control.rs` lines 41-58 say a drag needs several MOUSEMOVEs, and each pointer command draws a frame first.

- [ ] **Step 2: chrome-shots views.**
  - Move the `cardid()` function out of the `if has graph-link || has graph-menu-links` block, so it sits just above it with its comment.
  - Change that block's condition to `if has graph-link || has graph-link-hover || has graph-menu-links; then`.
  - After `if has graph-link; then shot graph-link; fi`, add:

```bash
    # The pointer on the link line: the line drawn thicker (the cursor, a pointing hand, is not in a screenshot).
    if has graph-link-hover; then
        state
        read -r lx ly < <(python -P -c 'import json,sys; e=[e for e in json.load(open(sys.argv[1], encoding="utf-8"))["edges"] if e["kind"]=="link"][0]["screen"]; print(round(e[0]), round(e[1]))' "$(w "$work/state.json")" </dev/null | tr -d '\r')
        send "MOUSEMOVE $lx $ly" "SLEEP 1"; shot graph-link-hover; reset
    fi
```

After that block (the link is gone after its UNLINK), add:

```bash
# Connect-drag: the top-level tab hovered shows its four edge handles; then a drag from its left handle 4/5 of the
# way to the note's centre (the rubber band, and the note's target ring bright under the pointer), shot mid-drag.
# Esc cancels it before the button comes up, so nothing is linked.
if has graph-link-handles || has graph-link-drag; then
    tab_id=$(cardid "$TAB")
    read -r tx ty < <(center "$TAB" top loaded)
    send "MOUSEMOVE $tx $ty" "SLEEP 1"
    state
    read -r hx hy < <(python -P -c 'import json,sys; r=json.load(open(sys.argv[1], encoding="utf-8"))["link_handles"][sys.argv[2]]["left"]; print(round((r[0]+r[2])/2), round((r[1]+r[3])/2))' "$(w "$work/state.json")" "$tab_id" </dev/null | tr -d '\r')
    if has graph-link-handles; then send "MOUSEMOVE $hx $hy" "SLEEP 1"; shot graph-link-handles; fi
    if has graph-link-drag; then
        read -r nx ny < <(center "$NOTE" top)
        send "MOUSEMOVE $tx $ty" "MOUSEMOVE $hx $hy" "MOUSEMOVE $hx $hy" "MOUSEDOWN $hx $hy"
        for i in 1 2 3 4 5 6 7 8; do send "MOUSEMOVE $((hx + (nx - hx) * i * 4 / 40)) $((hy + (ny - hy) * i * 4 / 40))"; done
        send "SLEEP 1"; shot graph-link-drag
        send ESCAPE "MOUSEUP $((hx + (nx - hx) * 4 / 5)) $((hy + (ny - hy) * 4 / 5))" "SLEEP 1"
    fi
    reset
fi
```

  - Add `graph-link-hover graph-link-handles graph-link-drag` to `ALL` after `graph-menu-links`, and to the header's `#   graph-link graph-menu-links` line.
  - Run `VIEWS='graph-link graph-link-hover graph-link-handles graph-link-drag graph-menu-links' bash scripts/chrome-shots.sh`, then **look at each PNG** in `target/shots/`:
    - graph-link-hover: the dashed accent line visibly thicker than in graph-link.
    - graph-link-handles: four small accent rings on the tab's edge midpoints, one filled (under the pointer), and none colliding with the Close/Eject/Audio corner controls.
    - graph-link-drag: a dashed line from the tab's centre to the pointer over the note, the note's ring bright, and the other valid cards' rings dim.
  - Also run `VIEWS=mouse-regress bash scripts/chrome-shots.sh` and expect all PASS. It checks that card drag, the swatch click and middle-click close still work.
  - If a picture is wrong, fix the code (Task 2's files) and say so in the Outcome.

- [ ] **Step 3: Docs.**
  - **README line 161 (Links paragraph).** Replace "Not yet: drag to connect, click a line to disconnect, directed or named links, links across levels." with:
    - "Gestures (2026-10-07, spec `docs/superpowers/specs/2026-10-07-link-gestures-design.md`): a click on a link line deletes that link. The line under the pointer draws thicker, with a pointing-hand cursor. A card on top takes the click, a drag pans as before, and it is off in pick-target mode, with a menu open and in a JSON level. Of two links on one pair, the greater id goes first."
    - "Hovering a card that has a valid link target shows a small handle on each edge's midpoint. It is hidden on an edge under 44 screen points and on a simplified level. Dragging from a handle draws a dashed rubber band and rings the valid targets. Releasing on one links the two, releasing anywhere else does nothing, and Esc cancels. Neither gesture applies to opened-from lines."
    - "Not yet: directed or named links, links across levels."
    - In its Debug sentence, add `edges`, `link_handles` and `connect_drag`.
  - **README line 380 (`LINK` bullet).** Append: "`EXPORTSTATE`'s `edges` lists the lines drawn at this level as `{kind, link, from, to, screen}` (`screen`: the straight segment's middle, where a probe clicks a link line). `link_handles` maps each card that would show connect handles to its `{top, right, bottom, left}` screen rects. `connect_drag` is a running connect-drag's source or null."
  - **README line 400.** Change "about 5 min for all 46" to "all 49". To "a link and the tab menu's Links section", add "a hovered link line, a card's connect handles and a connect-drag mid-way".
  - **README line 406 (link-probe).** Change the run count to four. Add: "the third run links them again, deletes the link by clicking its line, and links them by dragging from the first tab's right handle onto the second, which must not move the first. The fourth run must still have that link."
  - **Spec Status.** Change it to "Implemented 2026-10-0x (commits …); see the plan's Outcome for deviations."
  - **This plan.** Write an `## Outcome (date)` section in `2026-10-06-links.md`'s style:
    - A table of tasks, commits and review findings.
    - The test count.
    - The probe PASS count.
    - What the shots show.
    - The gate result with CPU load and Defender state.
    - **Deviations** (at least Decision 3's tie rule and Decision 4's 44 px).
    - Parked items.

- [ ] **Step 4: The gate.**
  - Prechecks:
    - `(Get-MpComputerStatus).RealTimeProtectionEnabled`.
    - CPU: `(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue`.
    - `Get-Process python, manifold -ErrorAction SilentlyContinue` (look for leftovers).
    - `df -h /c`.
  - Write `<scratchpad>/gate.sh`:

```bash
#!/usr/bin/env bash
# gate.sh <log>: two-instance-test x3, then headless-smoke-test x1, each exit code logged.
log="$1"
cd "/c/Users/Winston Li/Documents/GitHub/manifold" || exit 1
for i in 1 2 3; do
    echo "== two-instance $i $(date +%T)" >>"$log"
    bash scripts/two-instance-test.sh >>"$log" 2>&1 </dev/null
    echo "exit $?" >>"$log"
done
echo "== smoke $(date +%T)" >>"$log"
bash scripts/headless-smoke-test.sh >>"$log" 2>&1 </dev/null
echo "exit $?" >>"$log"
echo "== done $(date +%T)" >>"$log"
```

  - Launch it detached at BelowNormal: `cmd //c start "" //b //belownormal bash "<scratchpad>/gate.sh" "<scratchpad>/gate.log"`.
  - `start //b` returns at once, so watch the log for `== done` (Monitor with an until-loop), not the launcher. It takes about 12 minutes.
  - Expect `exit 0` four times, and the smoke test's pass count with 0 fail.
  - Record the result, CPU load and Defender state in the Outcome and in `.superpowers/sdd/2026-10-07-link-gestures/progress.md`.
  - Afterwards check `Get-Process manifold, python` for leftovers.

- [ ] **Step 5: fmt, clippy, commit.**
  - Run `cargo fmt --all` and clippy.
  - Commit "Link gestures: probe, screenshots, docs" with the `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` trailer.
  - The commit holds the scripts, README, spec and plan Outcome. Screenshots stay in `target/` and are not committed.

---

## Self-review against the spec

**B. Click a link line to disconnect**

| Spec item | Where it is covered |
|---|---|
| `LevelEdge` gains `from_id`, `to_id`, `link` | Task 1 Step 3 |
| Hit-test with `max(6, half width) * inv_zoom`, point-to-segment | `link_hit_tolerance`, `distance_to_segment`, unit tests |
| Click, not drag | `scene_background.clicked()`; `a_drag_from_a_link_line_pans_and_deletes_nothing` |
| No card, control, menu, minimap or pick mode claims it | egui hit-test (Decision 1), minimap check, `line_click_allowed`; tests for a card, pick mode and a menu |
| Not while dragging, connecting or connect-dragging; not in a JSON level | `line_click_allowed` |
| Effect: `delete_link`, then save | `click_link_line` |
| Two links on one pair go one per click; ties to the newest | `nearest_link`, test, Decision 3 |
| Highlight and pointer cursor | `hovered_link_line`, `draw_graph_edges`, cursor test, `graph-link-hover` shot |
| Plan decision on region versus after the Scene | Decision 1 |

**A. Drag from an edge handle to connect**

| Spec item | Where it is covered |
|---|---|
| Four handles of ~12 px on edge midpoints, hover only | `link_handle_rects`, `draw_graph_card` |
| Only on valid link ends | `shows_link_handles` |
| Never in a JSON level or during another gesture | `line_click_allowed` inside `shows_link_handles` |
| Hidden on a short edge or when simplified | Decision 4, unit test |
| A press does not move or select the card, and no `Drop` | Decision 5, tests |
| `connect_drag` beside `connecting = (source, LINK)` | `start_connect_drag` |
| Rings, with the bright one under the pointer | `connect_target` |
| Rubber band | `draw_connect_rubber_band`, `graph-link-drag` shot |
| Release target is the card under the pointer, by `level_items` reversed | `card_at` |
| Valid target finishes, canvas or source cancels silently, refused pair toasts, Esc cancels | `end_connect_drag`, tests |
| Lifecycle resets | Task 2 Step 3, test |
| Press and release ends the gesture | test |

**Testing and risks**

| Spec item | Where it is covered |
|---|---|
| Spec Testing list | Every case is in Tasks 1-2 except "line click in a JSON level", which is covered by `line_click_allowed`'s `json_level` gate plus the JSON-level `link.is_none()` assertion. No JSON-level line is a link, so nothing could be hit. |
| Risks: pointer ownership | Decisions 1 and 5; the card-drag test, the pan test, mouse-regress |
| Risks: coordinates | Decision 2 |
| Risks: thin cards | Decision 4 |
| Risks: probes | EXPORTSTATE `edges` and `link_handles`; probe steps |

---

## Outcome (2026-10-07)

Executed subagent-driven, one commit per task plus review fix rounds:

| Task | Commits | Review |
|---|---|---|
| 1 Click a link line to disconnect | f0fa529, 9dbeece | 0 crit, 2 important (popup-dismissal click, glide tap; fixed red-first), 5 minor (M2, M3 fixed; M1, M4, M5 below), 5 nit |
| 2 Edge handles and connect-drag | dfc01f8, add755e | 0 crit, 1 important (Esc with a focused text field; fixed), 6 minor (M1, M4, M5, M6 fixed; M2, M3 parked), 5 nit |
| 3 Probe, screenshots, docs | "Link gestures: probe, screenshots, docs" | |
| Final whole-branch review | "Link gestures: final-review fixes" | 0 crit, 2 important (I1 a click ending a text edit took a line; I2 a lost button-up let the next click link; both fixed red-first), 5 minor (M2, M3, M5 fixed; M1, M4 parked), 4 nit (N1 fixed; N2-N4 parked) |

`cargo test -p manifold`: 983 passed, 0 failed, 5 ignored after Task 2's fix round (Task 3 changed no Rust); 989
passed, 0 failed, 5 ignored after the final-review fixes.
`scripts/link-probe.sh`: 13 PASS (the 7 from links, plus: LINK again; a click on the line's middle deletes it; a
drag from a's right handle onto b links a -> b; a's rect unchanged; `connect_drag` null after; the link persists).
It passed on the first run in the real app; the state files showed a new link id after the drag and identical
rects. `VIEWS=mouse-regress`: 3 PASS (card drag, swatch click, middle-click close).

Shots (chrome-shots, 1280x800, zoom 144%), looked at whole and zoomed:

- `graph-link-hover`: the dashed accent line is visibly thicker and brighter than in `graph-link`.
- `graph-link-handles`: four 12-point accent rings on the tab's edge midpoints, centred on the card's outer border,
  the left one filled under the pointer; none touch Close (top right), the pin (bottom left) or the resize corner. The
  tab's hover peek opens to the right as on any tab hover.
- `graph-link-drag`: a dashed band from the tab's centre to the pointer over the note, drawn over both cards (tan on
  the note's yellow is low contrast but visible); the note's ring bright, the image, JSON and group rings dim; the
  source keeps its handles, the pressed one filled.
- Extra checks (scratch, not in the script): handles on the note, image, group and JSON cards sit on the frame,
  clear of the note's text area and the JSON rows; a note being edited looks the same at rest. Zoomed out, a 118x80
  tab has four handles, a 60x42 one only top and bottom, a 30x22 one none, as Decision 4 says. At 60x42 the bottom
  handle covers the end of the title's ellipsis, which sits on the bottom edge at that size (minor, not fixed).
- Geometry: a handle spans edge/2 +- 6 and Audio reaches 16 from the top-right corner, so at exactly 44 they touch
  and never overlap; no tab played audio in the shots, so Audio itself was not pictured.

Gate (two-instance-test x3, headless-smoke-test x1) on ddf2704, 2026-10-07: two-instance ok x3, smoke 48 pass / 0 fail. Rerun on 688e391 (final-review fixes): same result (CPU 22-100% from other sessions, Defender real-time off).

**Deviations from the plan text:**

- Decision 3's tie rule as built: of two links on one pair the greatest id wins (`nearest_link`: last of equals in
  `level_edges` order). `NodeId` orders by (actor, counter) and each launch has a fresh actor, so that is the newest
  only within one launch; across launches or replicas it is a deterministic stand-in. Both lines stay clickable one at
  a time. The spec now says so.
- Decision 4: the minimum edge is 44 screen points, not the spec's ~40; the spec now says 44 and why.
- `HandleSide::name()` is `key()` (`names::tests` reads every `.name(` as a stored name).
- The connect-drag release block runs after `track_pan_momentum`, which since 9dbeece runs after `line_click`.
- `connect_drag_target` is one function for both the bright ring (pre-Scene transform) and the release (post-Scene),
  so they agree off the Scene and over the minimap (Task 2 review M1).
- A handle press stops a glide (`start_connect_drag` clears `camera.momentum`); `track_pan_momentum` still ignores a
  connect-drag so a wheel zoom mid-drag starts no glide (Task 2 review M4).
- The spec's "line click in a JSON level" case has no harness test: a JSON level draws no link lines, so nothing could
  be hit; the `json_level` gate is covered by the JSON-level `link.is_none()` assertion.
- Task 3: `chrome-shots.sh`'s `center()` now strips python's `\r`, so its y can go into shell arithmetic (the
  `graph-link-drag` view does; before this, every caller only pasted it into a command). README's chrome-shots count
  says 46 (the size of `ALL`; it said 46 before with 43 names, so the brief's 49 would have been off by three).

**Fixed in the final-review round:**

- I1: no line hover, so no line press or click, while a text field has the keys (`typing` at frame start, a group
  rename, the quick switcher). The click that ends a note edit keeps the line. The gate is in `hovered_link_line`, not
  `line_click_allowed`, so the connect handles stay usable while a note is being edited.
- I2 (and Task 2 M3 for the connect-drag): `on_window_focus(false)` cancels a connect-drag and drops `line_press`. The
  release block cancels, instead of linking, when the button is up with no release this frame (a lost button-up).
  Focus-loss verified live (`scripts/focus-loss-probe.sh`, 2026-10-07): 9 PASS on each of 3 runs (and on
  one earlier run without the late button-up check). A real foreground change (the headless window given the
  foreground, then a throwaway form taking it) delivered winit's `Focused(false)`; `connect_drag` was null, and
  neither a late button-up over b nor a click on b linked. The control (`NO_STEAL=1`) keeps the drag alive and the late button-up links a -> b, so the checks
  discriminate. No headless short-circuit stands in the way: real `Focused` events reach `on_window_focus` in
  headless mode too (only egui's `raw_input.focused` is forced on).
- M2: a release that egui counts as a click (no movement) cancels, so a click on a handle's outer half over a
  neighbouring card links nothing. A press held past egui's `max_click_duration` without moving is no longer a click
  and links to whatever is under the handle's outer half; that remainder is accepted.
- M3: `connect_drag_target` refuses a pointer whose topmost interactable egui Area is above the Background order
  (`Context::layer_id_at`, the layer test egui's hit-test uses), so a release over a popup or menu links nothing. The
  save toast is non-interactable, so a release over it still links, as a click there still reaches the card.
- M5: the comment in `handle_graph_keys` now says egui's own Esc focus drop also ends a note edit, and the test
  asserts it.
- N1: `line_press` is cleared on session replace and on a level change (`switch_group_level`).

**Fixed after the branch (2026-10-07):**

- Task 2 M3, card-drag half: `on_window_focus(false)` ends a card drag as a release where the card is (`release_drag`
  saves its pos; no `Drop`, so no join, group-with or reparent) and calls egui's `stop_dragging`, which also clears a
  press not yet a drag. Without that, egui (button still down) kept the card following the pointer and the next real
  release dropped it. Test `losing_window_focus_ends_a_card_drag_in_place` (red first, and red again with only the
  `stop_dragging` removed). `scripts/focus-loss-probe.sh` has a `card` case (`CASES=card`): 22 PASS for both cases;
  the `NO_STEAL=1` control fails 5 card checks (the drag survives, a follows the pointer, the late release groups a
  and b). `EXPORTSTATE` gained `dragging`.
- Final review M1 ("Links: double-click, right-press during drag, merge retargets links"): `line_click` takes no line
  on a release that egui counts as a double- or triple-click (`button_double_clicked` / `button_triple_clicked`), so a
  double-click on a pair's line deletes one link. Any click within egui's 0.3 s double-click window and 6 points of
  the last one counts, so a click right after one that dismissed a popup or ended a note edit takes nothing either
  (the tests that click twice in a row now let time pass; the harness feeds egui a clock). Test
  `a_double_or_triple_click_on_a_line_deletes_one_link` (red first).
- Task 2 M2 (same commit): `handle_rect_select` ignores a right-press while `dragging` or `connect_drag` is set, so
  it opens no node menu and starts no marquee during a card drag or a connect-drag. Tests
  `a_right_press_during_a_connect_drag_opens_no_menu_and_starts_no_marquee` and
  `a_right_press_during_a_card_drag_opens_no_menu_and_starts_no_marquee` (both halves of each red first).

**Parked:**

- Final review M4: the top handle's inner half sits over the header's centre, where cards are usually grabbed to
  drag. Look at it live; the handle could sit on the border's outside instead.
- Final review N2: the spec's Lifecycle says `end_stale_drag` where the code uses `end_stale_connect`.
- Final review N3: `level_edges()` clones two `String`s per edge per frame.
- Final review N4: no harness tests for a source culled or simplified mid-drag, or for no handles in a JSON level.
- Task 1 M5: a painted, non-widget overlay over a line (a label past its card's rect, the marquee) lets a click through
  to the line. Not checked live.
- Task 1 N3: the EXPORTSTATE doc calls `edges`' `screen` the middle "between the two card centres"; it is the straight
  segment's middle and may be covered (README now says the former).
- Task 2 N2: at low zoom the inner half of a handle can sit over an editing note's text field edge and take its
  tooltip; not seen at 144%.
- Task 2 N3: the tab peek can overlap the right handle by about a point at zoom 0.4-0.5.
- The bottom handle over the title ellipsis on a short card (above).
- Not in scope, per the spec: opened-from by gesture or line click, lines anchored on the side handles, directed or
  named links, and an undo for a line click (the node menu can re-link).
