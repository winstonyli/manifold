---
status: done
---

# Per-Level View State Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Each level (the top level, every group, every JSON level) remembers its own camera and Collapse Groups toggle, across level switches and restarts, instead of one global camera and one global toggle.

**Architecture:** `LocalState` (the `session.mnf.local` JSON) replaces its single `camera` and `groups_collapsed` with `levels: BTreeMap<Option<K>, LevelView>`. The app keeps the same map live as `App::level_views`; the level in view stays live in `camera.rect` (or the running ease's target) and `groups_collapsed`, and `switch_group_level` saves the level it leaves into the map, then `enter_level` restores the level it enters (falling back to fit-all). `remap`, `forget_id` and the save-time prune keep the map's ids honest.

**Tech Stack:** Rust 2021 workspace, egui (`egui::Rect`), serde / serde_json; tests are `#[test]` in the `manifold` binary crate.

**Spec:** `docs/superpowers/specs/2026-11-01-scopes-nested-layout-anchor-design.md`, section "Slice 2: per-level view state" (and its Tests and Docs lines for `LocalState`). Slice 1 (anchor) and Slice 3 (nested settle, `expanded`) are not in this plan; Slice 2 does not depend on Slice 1.

## Global Constraints

- Run `cargo fmt --all` before each commit; `cargo fmt --all --check` must pass.
- `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass. Never `--no-verify`. Don't silence a lint; an `#[allow(..., reason = "...")]` needs its reason.
- No test may read or write the real `%APPDATA%\manifold`: tests use `App::default()`, `app_on(&temp_dir(..).join("session.mnf"), actor)` or `crate::store::testutil::temp_dir`.
- No format migration and no compatibility shim: the `.local` wrapper's `FORMAT_VERSION = 1` (`store/local.rs`) is untouched; "every field is `#[serde(default)]` with lenient ids, so an old file loses its camera and collapsed bit once and reads without error".
- Delete what the new map supersedes: `LocalState::camera`, `LocalState::groups_collapsed`, `lenient_camera`, `all_finite`.
- Every commit message ends with the trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never push, tag or bump.
- Python, if ever needed: write a file and run `python -P f.py < /dev/null`, never `python -` or a heredoc.
- Gate tier: `local_state.rs` and `session_sync/` are core paths (`crates/gate/src/tier.rs` `CORE_MODULES`), so `gate tier` says `full`. The full gate runs only when the user asks (before the next tag).

## Review Focus

- Leaving a level while the camera is still easing (a second click inside `TRANSITION_DURATION`): the level's saved view should be where the ease was going, not the half-way rect of the last frame. Pinned in Task 1 (`personal_state_applies_the_level_in_views_camera_and_keeps_the_others`, the `ease_to` part) and Task 2 (`leaving_mid_ease_saves_where_the_ease_was_going`).
- Entering a JSON level that cannot be entered (a refetch placeholder, a missing file): `sync_json_level` bounces to its group inside the same switch; the bounced-from level must not be saved with the previous level's camera, and the level actually left must be saved once, under its own id. Pinned in Task 2 (`leaving_an_unenterable_json_level_saves_no_view_for_it`).
- Deleting the group in view (`take_out`, local or remote): the switch out of it must not re-create an entry for the deleted id after `forget_id` dropped it. Pinned in Task 2 (`deleting_the_level_in_view_leaves_no_view_for_it`).
- Opening or importing another session (`replace_running_session` -> `teardown_running_session`): the old session's level views and its Collapse Groups toggle must not leak into the new one. Pinned in Task 2 (`opening_a_session_drops_the_old_sessions_level_views`).
- A `.local` file with an unreadable level entry, a non-finite camera, or the old single `camera`/`groups_collapsed` fields: the rest of the file must still read, and a non-finite value must neither break the file nor rewrite it every 30 s. Pinned in Task 1 (`an_unreadable_id_costs_only_itself`, `a_non_finite_value_costs_only_itself`, `a_non_finite_camera_is_no_change`, `an_old_files_camera_and_collapsed_are_ignored`).

- Callers of `show_level_of` that select a card and rely on the camera showing it (`finish_trail_focus`, `open_ref`): fit-all used to guarantee it, a restored view does not, so they set `camera.reveal` as `omnibar.rs` already does. The other callers are covered: `omni_activate` and `omnibar.rs:351` set `reveal` or open the Item view, `shortcuts.rs:354` opens the Item view (`focus_tab`), `json_level.rs:297` is `open_ref`. Pinned in Task 2 (`opening_a_ref_reveals_its_card`).
- A JSON level that was shown and then becomes unenterable (a failed Refresh) bounces without saving, so it loses its saved view (R3). Deliberately untested: reaching the state needs a failed fetch in a test, and the cost is accepted in R3.

## Rulings on spec gaps (made by this plan)

- **R1 Ease target.** The view saved for a level (and the one personal state writes for the level in view) is the running ease's target when an ease is running (`camera.tween.to`), else `camera.rect`. The spec says only "saves the outgoing level's camera rect"; a mid-ease rect is a frame of animation, not a place the user chose. The rule lives in one accessor, `GraphCamera::target()`, which `live_level_view` and the JSON level's reveal check (`json_level.rs:562`, the same expression) share.
- **R2 One guard instead of two evictions.** The spec says the switches the user did not make (`remote.rs:476`, `node_store.rs:127`) "evict a deleted level's entry after the switch". This plan gets the same end state with one rule in `switch_group_level`: it saves the level it leaves only while that level is still shown (`current_group` is `None` or in `self.nodes`). `take_out` has already run `unshow` (so `forget_id`) before its switch, so nothing is re-created; `leave_hidden` (`remote.rs:469-477`) switches before `hide_node`, which then evicts through `unshow` -> `forget_id`. Neither call site changes.
- **R3 Non-saving entry.** `switch_group_level` becomes "save, then `enter_level`". `sync_json_level` (`json_level.rs:264-267`) bounces out of an unenterable JSON level through `enter_level`, so a level never shown gets no entry. Cost: a JSON level that becomes unenterable while in view (a failed Refresh) loses its saved view.
- **R4 Teardown clears.** `teardown_running_session` saves nothing (spec) and also clears `level_views` and resets `groups_collapsed`: both belong to the replaced session's levels.
- **R5 JSON shape.** `levels` is written as an object keyed by the level's id text, the top level under the key `"top"` (no node id text can be `"top"`; `NodeId::from_str` needs `<16 hex>-<digits>`). An entry whose key or value does not read is dropped alone (`lenient_levels`). `LevelView.camera` is `Option<[f32; 4]>` so a dropped non-finite camera keeps the entry's toggle.
- **R6 Replace on apply.** `apply_local` replaces `level_views` with the file's map, as it overwrote the one camera before.
- **R7 One finite pass.** `differs` and `to_bytes` share one `LocalState::finite`, replacing `all_finite` and the camera-only strip in `differs`. Side effect: a NaN `right_panel_width` no longer reads as a change on every 30 s check (it did before: `NaN != NaN`).
- **R8 Closed levels.** A closed (hidden) group's level loses its entry: `hide_node` -> `unshow` -> `forget_id`, which the spec says evicts. Reopening it fits all.
- **R9 Export Group.** Nothing to implement; the PARKED scope entry already says "Export Group carries no local state (`session_files.rs:250-261`), so it is unchanged" (checked 2026-11-01).
- **R10 README :217.** The Ungroup All entry has the same stale "top level only" as `:233` (the toolbar shows both at every level with groups, `graph_ui/toolbar.rs:244-264`); Task 2 fixes both.
- **R11 Reveal after a level show.** `enter_level` clears `camera.reveal` and eases to the saved view, so a caller that selects a card with `show_level_of` and wants it seen sets `camera.reveal` itself after the call (`finish_trail_focus`'s Graph branch, `open_ref`).
- **R12 Stale docs.** The 2026-09-25 session-op-log spec says "the app has one camera; per-level cameras are later" (`:167`): Task 2 annotates it. PARKED's "Nested bottom-up settle" trigger (`PARKED.md:52`, "the anchor register and per-level view state have landed") is unchanged: it also waits on Slice 1.

## File Structure

- `crates/manifold/src/local_state.rs` — `LevelView`, `LocalState::levels` and its serde, `remap`, `finite`, `differs`, `to_bytes`; `App::live_level_view`; `local_state()` (prune, current level) and `apply_local`. Unit tests in its `mod tests`.
- `crates/manifold/src/main.rs` — `App::level_views` field and its initializer; `groups_collapsed` doc comment.
- `crates/manifold/src/node_store.rs` — `forget_id` evicts `level_views`.
- `crates/manifold/src/graph_ui/scene.rs` — `switch_group_level` (save) and new `enter_level` (restore).
- `crates/manifold/src/json_level.rs` — `sync_json_level` bounces through `enter_level`.
- `crates/manifold/src/session_files.rs` — `teardown_running_session` clears the map.
- `crates/manifold/src/graph_ui/camera.rs` — `GraphCamera::target`, and a doc comment; `crates/manifold/src/view_modes.rs` — doc comment only.
- `crates/manifold/src/trail_nav.rs`, `crates/manifold/src/json_level.rs` — `finish_trail_focus` and `open_ref` set `camera.reveal` (Task 2).
- Tests: `crates/manifold/src/session_sync/tests/local_state.rs`, `crates/manifold/src/json_level/tests.rs`, `crates/manifold/src/main/tests.rs`.
- Docs: `README.md` (`:217`, `:233`, `:364`), `PARKED.md` (scope entry), `docs/superpowers/specs/2026-09-25-session-op-log-design.md` (`:167`).

---

### Task 1: `LocalState::levels` and the live map

**Files:**
- Modify: `crates/manifold/src/local_state.rs:5` (imports unchanged), `:21-52` (struct), `:83-114` (lenient helpers, `differs`), `:148-164` (`Default`), `:166-187` (`remap`), `:189-208` (`all_finite`, `to_bytes`), `:349-386` (`local_state`), `:394-459` (`apply_local`), `:528-632` (tests)
- Modify: `crates/manifold/src/main.rs:206-212` (doc), `:212` (new field after it), `:516` (initializer)
- Modify: `crates/manifold/src/node_store.rs:171-207` (`forget_id`)
- Modify: `crates/manifold/src/graph_ui/camera.rs:28` (`GraphCamera::target`, after `ease_to`), `crates/manifold/src/json_level.rs:562` (use it)
- Test: `crates/manifold/src/graph_ui/camera.rs` `mod tests`, `crates/manifold/src/local_state.rs` `mod tests`, `crates/manifold/src/session_sync/tests/local_state.rs`, `crates/manifold/src/main/tests.rs:7-56`
- Docs: `README.md:364`

**Interfaces:**
- Consumes: nothing from other tasks. Existing: `App::camera: GraphCamera` (`rect: egui::Rect`, `tween: Option<GraphCameraTween>` with `to: egui::Rect`, `frame_restored: bool`), `App::groups_collapsed: bool`, `App::current_group: Option<NodeId>`.
- Produces (Task 2 relies on these exact names):
  - `pub(crate) struct LevelView { pub camera: Option<[f32; 4]>, pub collapsed: bool }` deriving `Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize`, in `crate::local_state`.
  - `LevelView::new(camera: egui::Rect, collapsed: bool) -> LevelView`
  - `LevelView::camera_rect(&self) -> Option<egui::Rect>`
  - `LocalState<K>::levels: BTreeMap<Option<K>, LevelView>`
  - `App::level_views: BTreeMap<Option<NodeId>, local_state::LevelView>` (field in `main.rs`)
  - `GraphCamera::target(&self) -> egui::Rect` (the running ease's `to`, else `rect`)
  - `App::live_level_view(&self) -> LevelView`

- [ ] **Step 1: Mark the plan active**

In `docs/superpowers/plans/2026-11-01-per-level-view-state.md` change the front matter line `status: draft` to `status: active` (it is committed with Task 1).

- [ ] **Step 2: Write the failing unit tests in `local_state.rs`**

In `crates/manifold/src/local_state.rs` `mod tests`, add this helper after `with_title`:

```rust
    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
    }
```

Replace `remap_renames_ids_and_drops_unknown_ones` with:

```rust
    #[test]
    fn remap_renames_ids_and_drops_unknown_ones() {
        let entry = LocalNode { title: "t".into(), ..LocalNode::default() };
        let state = LocalState::<String> {
            focused: Some("a".into()),
            current_group: Some("gone".into()),
            levels: BTreeMap::from([
                (None, LevelView { camera: None, collapsed: true }),
                (Some("a".to_owned()), LevelView::default()),
                (Some("gone".to_owned()), LevelView::default()),
            ]),
            nodes: BTreeMap::from([("a".to_owned(), entry), ("gone".to_owned(), LocalNode::default())]),
            ..LocalState::default()
        };
        let out = state.remap(&HashMap::from([("a".to_owned(), node(2))]));
        assert_eq!(out.focused, Some(node(2)));
        assert_eq!(out.current_group, None);
        assert_eq!(out.nodes.keys().collect::<Vec<_>>(), [&node(2)]);
        assert_eq!(out.levels.keys().copied().collect::<Vec<_>>(), [None, Some(node(2))], "the top level stays");
    }
```

Replace `an_unreadable_id_costs_only_itself` with:

```rust
    #[test]
    fn an_unreadable_id_costs_only_itself() {
        let text = format!(
            r#"{{"focused":"not an id","split_tab":"{}","levels":{{"nope":{{"collapsed":true}},"{}":{{"camera":"junk"}},"top":{{"collapsed":true}}}},"nodes":{{"junk":{{"title":"j"}},"{}":{{"title":"k"}}}}}}"#,
            node(5),
            node(7),
            node(6)
        );
        let state = LocalState::from_bytes(text.as_bytes()).expect("the file still reads");
        assert_eq!((state.focused, state.split_tab), (None, Some(node(5))));
        assert_eq!(state.levels, BTreeMap::from([(None, LevelView { camera: None, collapsed: true })]));
        assert_eq!(state.nodes.keys().collect::<Vec<_>>(), [&node(6)]);
    }
```

Add after `ids_are_written_as_their_text`:

```rust
    #[test]
    fn levels_round_trip_with_the_top_level_under_its_own_key() {
        let state = LocalState {
            levels: BTreeMap::from([
                (None, LevelView::new(rect(0.0, 0.0, 10.0, 10.0), true)),
                (Some(node(3)), LevelView::new(rect(-5.0, -5.0, 5.0, 5.0), false)),
            ]),
            ..LocalState::default()
        };
        let json: serde_json::Value = serde_json::from_slice(&state.to_bytes()).unwrap();
        assert_eq!(json["levels"][TOP_LEVEL]["collapsed"], true);
        assert_eq!(json["levels"][node(3).to_string()]["camera"], serde_json::json!([-5.0, -5.0, 5.0, 5.0]));
        assert!(json["levels"][node(3).to_string()].get("collapsed").is_none(), "a false toggle is left out");
        assert_eq!(LocalState::from_bytes(&state.to_bytes()), Some(state));
    }

    /// Files written before per-level views hold one `camera` (with its level) and one `groups_collapsed`: they are
    /// ignored, once, and the rest of the file reads.
    #[test]
    fn an_old_files_camera_and_collapsed_are_ignored() {
        let text = format!(r#"{{"camera":[[0,0,1,1],"{}"],"groups_collapsed":true,"focused":"{}"}}"#, node(2), node(2));
        let state = LocalState::from_bytes(text.as_bytes()).expect("still readable");
        assert_eq!(state.focused, Some(node(2)));
        assert!(state.levels.is_empty());
    }
```

Replace `a_non_finite_value_costs_only_itself` and `a_non_finite_camera_is_no_change` with:

```rust
    #[test]
    fn a_non_finite_value_costs_only_itself() {
        let mut state = with_title("kept");
        state.levels.insert(None, LevelView { camera: Some([0.0, f32::INFINITY, 1.0, 1.0]), collapsed: true });
        state.right_panel_width = Some(f32::NAN);
        let back = LocalState::from_bytes(&state.to_bytes()).expect("still readable");
        assert_eq!(back.nodes[&node(1)].title, "kept");
        assert_eq!(back.levels[&None], LevelView { camera: None, collapsed: true }, "the toggle stays");
        assert_eq!(back.right_panel_width, None);
    }

    #[test]
    fn a_non_finite_camera_is_no_change() {
        let mut state = with_title("t");
        state.levels.insert(None, LevelView { camera: Some([0.0, f32::NAN, 1.0, 1.0]), collapsed: false });
        state.right_panel_width = Some(f32::NAN);
        assert!(!differs(&state, &state.clone()), "neither is written, so neither differs");
    }
```

In `nulls_written_by_older_builds_read_back`, replace the line `assert_eq!(state.camera, None);` with:

```rust
        assert!(state.levels.is_empty(), "an old file's camera is ignored");
```

In `crates/manifold/src/graph_ui/camera.rs` `mod tests`, add:

```rust
    #[test]
    fn the_target_is_where_an_ease_is_going() {
        let mut camera = GraphCamera::new();
        let to = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(50.0, 50.0));
        assert_eq!(camera.target(), camera.rect);
        camera.ease_to(to);
        assert_eq!(camera.target(), to);
    }
```

- [ ] **Step 3: Write the failing App-level tests**

In `crates/manifold/src/session_sync/tests/local_state.rs`, add after `use super::*;`:

```rust
use crate::local_state::LevelView;

fn world_rect(x0: f32, y0: f32, x1: f32, y1: f32) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
}
```

Append these tests to the same file:

```rust
/// Personal state's level views: the level in view takes its camera and Collapse Groups toggle at once, the others
/// wait in `level_views` for a switch, and all are written back, the level in view from the live camera (or where a
/// running ease is taking it).
#[test]
fn personal_state_applies_the_level_in_views_camera_and_keeps_the_others() {
    let mut app = app_on(&temp_dir("level-views-apply").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.groups_collapsed = true;
    let top = LevelView::new(world_rect(-500.0, -400.0, 500.0, 400.0), true);
    let inside = LevelView::new(world_rect(-300.0, -200.0, 300.0, 200.0), false);
    let levels = BTreeMap::from([(None, top), (Some(g), inside)]);
    app.apply_local(LocalState { current_group: Some(g), levels: levels.clone(), ..LocalState::default() });
    assert_eq!(app.current_group, Some(g));
    assert_eq!(Some(app.camera.rect), inside.camera_rect());
    assert!(!app.groups_collapsed, "g's toggle, not the top level's");
    assert_eq!(app.level_views[&None], top, "kept for the switch back");
    assert_eq!(app.local_state().levels, levels, "written back as read");
    let ahead = world_rect(0.0, 0.0, 50.0, 50.0);
    app.camera.ease_to(ahead);
    assert_eq!(app.local_state().levels[&Some(g)].camera_rect(), Some(ahead), "a running ease writes where it goes");
}

/// The written map keeps the level in view and every level the document still holds; an id it no longer holds is
/// pruned (as `hidden_local` is).
#[test]
fn personal_state_drops_the_views_of_levels_the_document_no_longer_holds() {
    let mut app = app_on(&temp_dir("level-views-prune").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    let gone = NodeId { actor: ActorId(7), counter: 1 };
    app.level_views.insert(Some(g), LevelView { camera: None, collapsed: true });
    app.level_views.insert(Some(gone), LevelView { camera: None, collapsed: true });
    let levels = app.local_state().levels;
    assert!(levels.contains_key(&Some(g)), "a shown group keeps its view");
    assert!(!levels.contains_key(&Some(gone)), "an id the document doesn't hold is dropped");
    assert!(levels.contains_key(&None), "the level in view is always written");
}
```

In `a_late_read_of_no_personal_state_keeps_the_live_view` (same file, `:236`), replace

```rust
    assert!(LocalState::from_bytes(&bytes).unwrap().groups_collapsed, "the next write saves the live view");
```

with

```rust
    assert!(LocalState::from_bytes(&bytes).unwrap().levels[&None].collapsed, "the next write saves the live view");
```

In `crates/manifold/src/main/tests.rs` `forget_id_clears_every_id_keyed_table`, add before `app.forget_id(t);`:

```rust
    app.level_views.insert(Some(t), crate::local_state::LevelView::default());
    app.level_views.insert(Some(u), crate::local_state::LevelView::default());
```

and after the last assert of that test:

```rust
    assert!(!app.level_views.contains_key(&Some(t)) && app.level_views.contains_key(&Some(u)), "only t's view goes");
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold --bins local_state`
Expected: FAIL to compile, with errors such as "cannot find type `LevelView` in this scope", "struct `LocalState` has no field named `levels`", "no field `level_views` on type `App`".

- [ ] **Step 5: Replace the struct fields and the lenient camera reader**

In `crates/manifold/src/local_state.rs`, replace the two fields `:33-38`

```rust
    #[serde(default)]
    pub groups_collapsed: bool,
    /// The camera rect `[min x, min y, max x, max y]` and the level it was
    /// saved at.
    #[serde(default, deserialize_with = "lenient_camera")]
    pub camera: Option<([f32; 4], Option<K>)>,
```

with

```rust
    /// Each level's view (`None` the top level; a JSON level under its Json node's id), written as an object keyed by
    /// the level's id text, the top level under `TOP_LEVEL`. An entry whose key or value does not read costs only
    /// itself (`lenient_levels`).
    #[serde(default, serialize_with = "levels_by_key", deserialize_with = "lenient_levels")]
    pub levels: BTreeMap<Option<K>, LevelView>,
```

Insert after the `LiveUrl` struct (after `:77`):

```rust
/// A level's view as it was left (`LocalState::levels`, `App::level_views`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LevelView {
    /// The camera rect `[min x, min y, max x, max y]`; `None` once a non-finite one was dropped (`LocalState::finite`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<[f32; 4]>,
    /// Collapse Groups: this level's group cards hide their member-thumbnail grid (`card/group_body.rs`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
}

impl LevelView {
    pub(crate) fn new(camera: egui::Rect, collapsed: bool) -> Self {
        LevelView { camera: Some([camera.min.x, camera.min.y, camera.max.x, camera.max.y]), collapsed }
    }

    pub(crate) fn camera_rect(&self) -> Option<egui::Rect> {
        self.camera.map(|[x0, y0, x1, y1]| egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1)))
    }
}

/// The key the top level's view is written under in `LocalState::levels`; a group's is its id text.
const TOP_LEVEL: &str = "top";

/// A `LocalState::levels` key as written: `TOP_LEVEL` for the top level, else the id's own text.
struct LevelKey<'a, K>(Option<&'a K>);

impl<K: Serialize> Serialize for LevelKey<'_, K> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            None => s.serialize_str(TOP_LEVEL),
            Some(id) => id.serialize(s),
        }
    }
}

fn levels_by_key<S: serde::Serializer, K: Serialize>(
    levels: &BTreeMap<Option<K>, LevelView>,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.collect_map(levels.iter().map(|(level, view)| (LevelKey(level.as_ref()), view)))
}

/// `LocalState::levels` as read: an entry whose key is neither `TOP_LEVEL` nor a `K`, or whose value is not a
/// `LevelView`, is dropped on its own.
fn lenient_levels<'de, D: serde::Deserializer<'de>, K: FromStr + Ord>(
    d: D,
) -> Result<BTreeMap<Option<K>, LevelView>, D::Error> {
    let raw = HashMap::<String, serde_json::Value>::deserialize(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|(key, value)| {
            let level = if key == TOP_LEVEL { None } else { Some(key.parse().ok()?) };
            Some((level, serde_json::from_value(value).ok()?))
        })
        .collect())
}
```

Delete `lenient_camera` and its `#[allow(clippy::type_complexity, ...)]` and doc comment (`:88-101`).

Replace `differs` (`:103-114`) with:

```rust
/// Whether `a` and `b` differ enough to write. Compared as written (`LocalState::finite`): a non-finite value is never
/// written, so it must not read as a change either.
pub(crate) fn differs(a: &LocalState, b: &LocalState) -> bool {
    a.finite() != b.finite()
}
```

- [ ] **Step 6: Update `Default`, `remap`, `to_bytes`**

In `impl<K> Default for LocalState<K>` replace the two lines `groups_collapsed: false,` and `camera: None,` with:

```rust
            levels: BTreeMap::new(),
```

In `remap`, replace

```rust
            camera: self
                .camera
                .filter(|(_, level)| level.is_none() || id(level).is_some())
                .map(|(r, level)| (r, id(&level))),
```

with

```rust
            levels: self
                .levels
                .into_iter()
                .filter_map(|(level, view)| {
                    let level = match level {
                        None => None,
                        Some(old) => Some(*map.get(&old)?),
                    };
                    Some((level, view))
                })
                .collect(),
```

and delete the line `groups_collapsed: self.groups_collapsed,`.

Replace `all_finite` and `to_bytes` (`:190-208`) with:

```rust
    /// This state as written: serde_json writes NaN and infinity as `null`, which a plain `f32` can't read back, so one
    /// bad value would make the whole file unreadable. A non-finite camera or panel width is dropped (a level keeps its
    /// toggle).
    fn finite(&self) -> LocalState {
        let mut clean = self.clone();
        for view in clean.levels.values_mut() {
            view.camera = view.camera.filter(|r| r.iter().all(|v| v.is_finite()));
        }
        clean.right_panel_width = clean.right_panel_width.filter(|w| w.is_finite());
        clean
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.finite()).expect("personal state serializes")
    }
```

- [ ] **Step 7: Add the App field, its initializer and the doc fix**

In `crates/manifold/src/main.rs`, replace the `groups_collapsed` doc comment and field (`:206-212`)

```rust
    /// Toggled by the Graph toolbar's "Collapse Groups"/"Expand Groups"
    /// button -- while `true`, every non-empty group card falls back to
    /// the plain centered name+count layout an empty group already uses,
    /// skipping the 2x2 member-thumbnail grid. A denser overview for a
    /// graph with many/large groups, purely a rendering toggle -- doesn't
    /// touch `groups`/membership at all. Session-only, resets on restart.
    groups_collapsed: bool,
```

with

```rust
    /// Toggled by the Graph toolbar's "Collapse Groups"/"Expand Groups"
    /// button -- while `true`, every non-empty group card at the level in
    /// view falls back to the plain centered name+count layout an empty
    /// group already uses, skipping the 2x2 member-thumbnail grid. A denser
    /// overview for a level with many/large groups, purely a rendering
    /// toggle -- doesn't touch membership at all. The level in view's own:
    /// each level keeps its toggle in `level_views` while out of view and in
    /// personal state across restarts (`LocalState::levels`).
    groups_collapsed: bool,
    /// The view each level was left at (`None` the top level), restored on the way back and written to personal
    /// state (`LocalState::levels`). The level in view lives in `camera` and `groups_collapsed` (`live_level_view`),
    /// so its entry here, if any, is stale until the next switch saves it.
    level_views: BTreeMap<Option<NodeId>, local_state::LevelView>,
```

In the constructor, after `groups_collapsed: false,` (`:516`) add:

```rust
            level_views: BTreeMap::new(),
```

- [ ] **Step 8: `forget_id` evicts the level**

In `crates/manifold/src/node_store.rs` `forget_id`, after `self.doc.hidden_local.remove(&id);` add:

```rust
        self.level_views.remove(&Some(id));
```

- [ ] **Step 9: `live_level_view`, `local_state()` and `apply_local`**

In `crates/manifold/src/graph_ui/camera.rs`, after `ease_to`, add:

```rust
    /// Where the camera is headed: the running ease's target, else where it is. A frame of a running ease is not a place
    /// the user chose (`App::live_level_view`, the JSON level's reveal check).
    pub(crate) fn target(&self) -> egui::Rect {
        self.tween.as_ref().map_or(self.rect, |t| t.to)
    }
```

In `crates/manifold/src/json_level.rs:562` replace

```rust
        let view = self.camera.tween.as_ref().map_or(self.camera.rect, |t| t.to);
```

with

```rust
        let view = self.camera.target();
```

In `crates/manifold/src/local_state.rs`, in `impl App`, add before `local_state`:

```rust
    /// The level in view's live view: the camera (or, while an ease runs, where it is going: the level change or fit
    /// the user asked for, not a frame of the animation) and its Collapse Groups toggle. `switch_group_level` saves it
    /// for the level it leaves; personal state writes it for the level in view.
    pub(crate) fn live_level_view(&self) -> LevelView {
        LevelView::new(self.camera.target(), self.groups_collapsed)
    }
```

In `local_state()`, replace

```rust
        // Hidden nodes keep theirs while the document still holds them (a Delete not yet seen is dropped).
        if let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) {
```

with

```rust
        let doc = self.doc.journal.as_ref().map(|j| j.doc());
        // Hidden nodes keep theirs while the document still holds them (a Delete not yet seen is dropped).
        if let Some(doc) = doc {
```

replace `let rect = self.camera.rect;` with

```rust
        // A level's view is kept while its group is shown or the document still holds it; the level in view's is live.
        let mut levels: BTreeMap<Option<NodeId>, LevelView> = self
            .level_views
            .iter()
            .filter(|(level, _)| {
                level.is_none_or(|g| {
                    self.nodes.contains_key(&g) || doc.is_some_and(|d| d.nodes.get(&g).is_some_and(Node::is_live))
                })
            })
            .map(|(&level, &view)| (level, view))
            .collect();
        levels.insert(self.current_group, self.live_level_view());
```

and in the returned `LocalState { .. }` replace the two lines

```rust
            groups_collapsed: self.groups_collapsed,
            camera: Some(([rect.min.x, rect.min.y, rect.max.x, rect.max.y], self.current_group)),
```

with

```rust
            levels,
```

In `apply_local`, delete the line `self.groups_collapsed = state.groups_collapsed;` (`:415`) and replace the camera block at its end (`:453-458`)

```rust
        if let Some((rect, level)) = state.camera {
            if level == self.current_group {
                self.camera.rect = egui::Rect::from_min_max(egui::pos2(rect[0], rect[1]), egui::pos2(rect[2], rect[3]));
                self.camera.frame_restored = false;
            }
        }
```

with

```rust
        // The level in view takes its view now; the others wait in `level_views` for a switch (`enter_level`).
        self.level_views = state.levels;
        let view = self.level_views.get(&self.current_group).copied().unwrap_or_default();
        self.groups_collapsed = view.collapsed;
        if let Some(rect) = view.camera_rect() {
            self.camera.rect = rect;
            self.camera.frame_restored = false;
        }
```

- [ ] **Step 10: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold --bins local_state`
Expected: PASS, including `levels_round_trip_with_the_top_level_under_its_own_key`, `an_old_files_camera_and_collapsed_are_ignored`, `a_non_finite_camera_is_no_change`, `personal_state_applies_the_level_in_views_camera_and_keeps_the_others`, `personal_state_drops_the_views_of_levels_the_document_no_longer_holds`, `a_late_read_of_no_personal_state_keeps_the_live_view`.

Run: `cargo test -j 8 -p manifold --bins -- the_target_is_where_an_ease_is_going forget_id_clears_every_id_keyed_table every_id_keyed_field_is_forgotten_or_excused`
Expected: PASS (the second test sees `level_views`' `Option<NodeId` type and finds `self.level_views` in `forget_id`).

- [ ] **Step 11: README `:364`**

In `README.md` (`:364`, the `session.mnf.local` bullet), replace

```text
focus, camera position, each tab's title
```

with

```text
focus, each level's camera position and Collapse Groups toggle (`LocalState::levels`), each tab's title
```

- [ ] **Step 12: Format, lint and commit**

Run: `cargo fmt --all` then the clippy command from Global Constraints.
Expected: clippy finishes with no warnings.

```bash
git add crates/manifold/src/local_state.rs crates/manifold/src/main.rs crates/manifold/src/node_store.rs crates/manifold/src/graph_ui/camera.rs crates/manifold/src/json_level.rs crates/manifold/src/session_sync/tests/local_state.rs crates/manifold/src/main/tests.rs README.md docs/superpowers/plans/2026-11-01-per-level-view-state.md
git diff --cached --stat
git commit -m "Personal state: a view per level (LocalState::levels) replaces the one camera and collapsed bit

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Save and restore on a level switch

**Files:**
- Modify: `crates/manifold/src/graph_ui/scene.rs:105-126` (`switch_group_level`, new `enter_level`)
- Modify: `crates/manifold/src/local_state.rs` (`LevelView::camera_or`, a unit test)
- Modify: `crates/manifold/src/json_level.rs:251-268` (`sync_json_level`)
- Modify: `crates/manifold/src/session_files.rs:527-540` (`teardown_running_session`)
- Modify: `crates/manifold/src/trail_nav.rs:201` (`finish_trail_focus`), `crates/manifold/src/json_level.rs:293-300` (`open_ref`)
- Modify (doc comments): `crates/manifold/src/graph_ui/camera.rs:234`, `crates/manifold/src/view_modes.rs:67-70`
- Test: `crates/manifold/src/session_sync/tests/local_state.rs`, `crates/manifold/src/json_level/tests.rs`, `crates/manifold/src/local_state.rs` `mod tests`
- Docs: `README.md:217`, `:233`; `PARKED.md:47`; `docs/superpowers/specs/2026-09-25-session-op-log-design.md:167`

**Interfaces:**
- Consumes (Task 1): `LevelView { camera, collapsed }`, `LevelView::new(egui::Rect, bool)`, `LevelView::camera_rect() -> Option<egui::Rect>`, `App::level_views: BTreeMap<Option<NodeId>, LevelView>`, `App::live_level_view() -> LevelView`, `GraphCamera::target() -> egui::Rect`. Existing: `App::fit_all_nodes_scene_rect() -> egui::Rect` (`graph_nav.rs:414`), `GraphCamera::ease_to(egui::Rect)`, `GraphCamera::jump_to(egui::Rect)`, `App::take_out(NodeId, node_store::Removal)`, `App::replace_running_session(&JsonDoc, Trust)`.
- Produces:
  - `LevelView::camera_or(&self, fit: egui::Rect) -> egui::Rect`
  - `App::enter_level(&mut self, group: Option<NodeId>)` (`pub(crate)`, in `graph_ui/scene.rs`)
  - `App::switch_group_level(&mut self, group: Option<NodeId>)` keeps its signature; it now saves, then calls `enter_level`.

- [ ] **Step 1: Write the failing unit test for `camera_or`**

In `crates/manifold/src/local_state.rs` `mod tests`, add:

```rust
    #[test]
    fn a_saved_camera_is_used_only_while_it_meets_the_level() {
        let fit = rect(0.0, 0.0, 100.0, 100.0);
        let near = rect(50.0, 50.0, 150.0, 150.0);
        assert_eq!(LevelView::new(near, false).camera_or(fit), near);
        let far = rect(1.0e6, 1.0e6, 1.0e6 + 1.0, 1.0e6 + 1.0);
        assert_eq!(LevelView::new(far, false).camera_or(fit), fit, "it misses the level's cards");
        assert_eq!(LevelView::default().camera_or(fit), fit, "none saved");
        assert_eq!(LevelView::new(rect(10.0, 10.0, 10.0, 20.0), false).camera_or(fit), fit, "an empty rect");
    }
```

- [ ] **Step 2: Write the failing switch tests**

Append to `crates/manifold/src/session_sync/tests/local_state.rs`:

```rust
/// Where the camera is easing to after a switch.
fn eased_to(app: &App) -> Option<egui::Rect> {
    app.camera.tween.map(|t| t.to)
}

/// A switch saves the view of the level it leaves and brings back the view of the one it enters: its camera (fit-all
/// the first time) and its own Collapse Groups toggle.
#[test]
fn switching_levels_returns_to_each_levels_view() {
    let mut app = app_on(&temp_dir("level-views-switch").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    let top = app.fit_all_nodes_scene_rect().translate(egui::vec2(25.0, 0.0));
    app.camera.jump_to(top);
    app.groups_collapsed = true;
    app.switch_group_level(Some(g));
    assert_eq!(eased_to(&app), Some(app.fit_all_nodes_scene_rect()), "a level never left before fits all");
    assert!(!app.groups_collapsed, "a level's toggle is its own");
    let inside = app.fit_all_nodes_scene_rect().translate(egui::vec2(0.0, 15.0));
    app.camera.jump_to(inside);
    app.switch_group_level(None);
    assert_eq!(eased_to(&app), Some(top));
    assert!(app.groups_collapsed);
    app.switch_group_level(Some(g));
    assert_eq!(eased_to(&app), Some(inside));
}

#[test]
fn leaving_mid_ease_saves_where_the_ease_was_going() {
    let mut app = app_on(&temp_dir("level-views-mid-ease").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.switch_group_level(Some(g));
    let fit = app.fit_all_nodes_scene_rect();
    assert!(app.camera.tween.is_some(), "still easing: no frame has run");
    app.switch_group_level(None);
    assert_eq!(app.level_views[&Some(g)].camera_rect(), Some(fit));
}

#[test]
fn a_saved_view_that_misses_the_level_falls_back_to_fit_all() {
    let mut app = app_on(&temp_dir("level-views-miss").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    let far = world_rect(1.0e6, 1.0e6, 1.0e6 + 100.0, 1.0e6 + 100.0);
    app.level_views.insert(Some(g), LevelView::new(far, true));
    app.switch_group_level(Some(g));
    assert_eq!(eased_to(&app), Some(app.fit_all_nodes_scene_rect()));
    assert!(app.groups_collapsed, "the toggle still comes back");
}

/// `take_out` removes the group, then switches out of it: the switch must not save a view for the deleted id.
#[test]
fn deleting_the_level_in_view_leaves_no_view_for_it() {
    let mut app = app_on(&temp_dir("level-views-delete").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.switch_group_level(Some(g));
    app.take_out(g, crate::node_store::Removal::Local);
    assert_eq!(app.current_group, None);
    assert!(!app.level_views.contains_key(&Some(g)));
    assert!(app.level_views.contains_key(&None), "the top level's view was saved on the way in");
}

/// Teardown saves nothing, and the replaced session's level views and toggle go with it.
#[test]
fn opening_a_session_drops_the_old_sessions_level_views() {
    let mut app = app_on(&temp_dir("level-views-teardown").join("session.mnf"), 1);
    let (g, _) = group_with_note(&mut app);
    app.camera.jump_to(world_rect(7.0, 7.0, 77.0, 77.0));
    app.switch_group_level(Some(g));
    app.groups_collapsed = true;
    assert!(app.level_views.contains_key(&None), "the top level's view was saved on the way in");
    let json =
        JsonDoc { nodes: vec![json_node("t", Kind::Tab, serde_json::json!({ "url": "https://new/" }))], local: None };
    app.replace_running_session(&json, crate::store::json::Trust::Own);
    assert!(app.level_views.is_empty(), "no level of the old session survives, the top level's included");
    assert!(!app.groups_collapsed);
    assert_eq!(app.local_state().levels.keys().copied().collect::<Vec<_>>(), [None], "only the level in view is written");
}

/// `show_level_of` no longer fits the level, so a caller that selects a card must bring it into view itself.
#[test]
fn opening_a_ref_reveals_its_card() {
    let mut app = app_on(&temp_dir("level-views-open-ref").join("session.mnf"), 1);
    let (g, note) = group_with_note(&mut app);
    app.switch_group_level(Some(g));
    app.switch_group_level(None);
    app.open_ref(note);
    assert_eq!(app.current_group, Some(g));
    assert_eq!(app.camera.reveal, Some(note));
}

/// Each level's view goes through `session.mnf.local` and comes back: the level in view at once, the others on the
/// switch back.
#[test]
fn each_levels_view_survives_a_restart() {
    let path = temp_dir("level-views-restart").join("session.mnf");
    let (g, top, inside) = {
        let mut app = app_on(&path, 1);
        let (g, _) = group_with_note(&mut app);
        let top = app.fit_all_nodes_scene_rect().translate(egui::vec2(25.0, 0.0));
        app.camera.jump_to(top);
        app.groups_collapsed = true;
        app.switch_group_level(Some(g));
        let inside = app.fit_all_nodes_scene_rect().translate(egui::vec2(0.0, 15.0));
        app.camera.jump_to(inside);
        app.close_store();
        (g, top, inside)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.current_group, Some(g));
    assert_eq!(app.camera.rect, inside);
    assert!(!app.groups_collapsed, "g's own toggle");
    app.switch_group_level(None);
    assert_eq!(eased_to(&app), Some(top), "the top level's view came back with the file");
    assert!(app.groups_collapsed);
}
```

Append to `crates/manifold/src/json_level/tests.rs`:

```rust
/// A JSON level keeps its own camera entry, keyed by its Json node's id, like a group's.
#[test]
fn a_json_level_keeps_its_own_camera() {
    let (mut app, j) = app_with_json("level-camera", r#"{"a": {"b": 1}, "c": 2}"#);
    app.switch_group_level(Some(OUTER));
    app.switch_group_level(Some(j));
    let view = app.fit_all_nodes_scene_rect().translate(egui::vec2(20.0, 10.0));
    app.camera.jump_to(view);
    app.switch_group_level(Some(OUTER));
    assert_eq!(app.local_state().levels[&Some(j)].camera_rect(), Some(view));
    app.switch_group_level(Some(j));
    assert_eq!(app.camera.tween.map(|t| t.to), Some(view));
}

/// Entering a JSON level that can't be entered bounces to its group inside the same switch: the level never shown
/// gets no view, and the level actually left is saved once, under its own id.
#[test]
fn leaving_an_unenterable_json_level_saves_no_view_for_it() {
    let dir = crate::store::testutil::temp_dir("json_level-unenterable-view");
    let mut app = App::default();
    app.place_group(Some(OUTER), "outer", Placement { group: None, pos: egui::Vec2::ZERO });
    let payload =
        JsonPayload::file(dir.join("not-fetched-yet.json").to_string_lossy().into_owned(), Default::default(), None);
    app.insert_placeholder_node(J, egui::Vec2::ZERO, Kind::Json, Runtime::Json(payload));
    app.edit_group(J, Some(OUTER));
    assert!(!app.nodes[&J].as_json().unwrap().enterable());
    app.switch_group_level(Some(OUTER));
    let outer = app.fit_all_nodes_scene_rect().translate(egui::vec2(20.0, 0.0));
    app.camera.jump_to(outer);
    app.switch_group_level(Some(J));
    assert_eq!(app.current_group, Some(OUTER), "bounced to the group");
    assert!(!app.level_views.contains_key(&Some(J)), "a level never shown keeps no view");
    assert_eq!(app.level_views[&Some(OUTER)].camera_rect(), Some(outer));
    assert_eq!(app.camera.tween.map(|t| t.to), Some(outer), "and the camera comes straight back");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold --bins local_state`
Expected: FAIL to compile with "no method named `camera_or` found for struct `LevelView`". (Once Step 4 adds it, the switch tests compile and fail on their `assert_eq!`s until Step 5, since the old switch always eases to fit-all and saves nothing.)

- [ ] **Step 4: Add `camera_or`**

In `crates/manifold/src/local_state.rs` `impl LevelView`, after `camera_rect`, add:

```rust
    /// Where the camera goes on entering this level: the saved rect while it is a usable rect that still meets `fit`
    /// (the level's fit-all rect, `App::fit_all_nodes_scene_rect`), else `fit`.
    pub(crate) fn camera_or(&self, fit: egui::Rect) -> egui::Rect {
        self.camera_rect().filter(|r| r.is_finite() && r.is_positive() && r.intersects(fit)).unwrap_or(fit)
    }
```

- [ ] **Step 5: Split `switch_group_level` into save and `enter_level`**

In `crates/manifold/src/graph_ui/scene.rs`, replace `switch_group_level` and its doc comment (`:105-126`) with:

```rust
    /// Shows `group`'s level (`None` for the top), first saving the view of the level being left (`App::level_views`,
    /// `live_level_view`) so coming back returns to it (`enter_level`). A level no longer shown saves nothing:
    /// `take_out` has already removed the group it switches out of, so no entry is made for a deleted id.
    pub(crate) fn switch_group_level(&mut self, group: Option<NodeId>) {
        if self.current_group.is_none_or(|g| self.nodes.contains_key(&g)) {
            let view = self.live_level_view();
            self.level_views.insert(self.current_group, view);
        }
        self.enter_level(group);
    }

    /// Shows `group`'s level without saving the one left (`switch_group_level` saves it): eases the camera to the view
    /// this level was left at, or fits it when none is saved or the saved one misses its cards (`LevelView::camera_or`),
    /// brings back its Collapse Groups toggle, and drops everything tied to the old level: the rectangle selection (the
    /// toolbar's "Group Selected"/"Close Selected" would otherwise act on tabs no longer visible), the empty-canvas
    /// card, any open node menu, a drag whose card is no longer shown, and pick-target mode (its targets were the old
    /// level's). `sync_json_level` leaves a JSON level it never showed (an unenterable one) through this, so that level
    /// gets no saved view.
    pub(crate) fn enter_level(&mut self, group: Option<NodeId>) {
        self.end_note_edits();
        self.current_group = group;
        self.camera.reveal = None;
        self.end_stale_drag(true);
        self.cancel_connect();
        self.link.line_press = None;
        self.sync_json_level();
        self.layout_current_level();
        self.graph.settled_key = Some(self.current_level_settle_key());
        // Read after `sync_json_level`, which may have left an unenterable JSON level for its group.
        let view = self.level_views.get(&self.current_group).copied().unwrap_or_default();
        self.groups_collapsed = view.collapsed;
        self.camera.ease_to(view.camera_or(self.fit_all_nodes_scene_rect()));
        self.interaction.selected.clear();
        self.interaction.rect_select_start = None;
        self.interaction.pending_new_node = None;
        self.menus.node_menu.close();
    }
```

- [ ] **Step 6: `sync_json_level` bounces without saving**

In `crates/manifold/src/json_level.rs`, in `sync_json_level`'s doc comment (`:253`) replace

```rust
    /// start of `build_graph_ui` and in `switch_group_level`.
```

with

```rust
    /// start of `build_graph_ui` and in `enter_level`.
```

and in its body replace

```rust
        if !json.enterable() {
            let group = self.group_of(self.current_group);
            self.switch_group_level(group);
            return;
        }
```

with

```rust
        if !json.enterable() {
            let group = self.group_of(self.current_group);
            // Not `switch_group_level`: this level was never shown (or can no longer be), so it saves no view.
            self.enter_level(group);
            return;
        }
```

- [ ] **Step 7: Teardown clears the map**

In `crates/manifold/src/session_files.rs` `teardown_running_session`, after `self.current_group = None;` add:

```rust
        // Not a switch: nothing is saved, and the replaced session's level views and toggle go with it.
        self.level_views.clear();
        self.groups_collapsed = false;
```

- [ ] **Step 8: Stale comments**

In `crates/manifold/src/graph_ui/camera.rs:234` replace

```rust
            // Settled first, as in `switch_group_level`: fitting the
```

with

```rust
            // Settled first, as in `enter_level`: fitting the
```

In `crates/manifold/src/view_modes.rs:67-70` replace

```rust
/// An in-progress eased pan/zoom from one `camera.rect` to another --
/// entering/leaving a group moves the camera to the new level's
/// `fit_all_nodes_scene_rect()` over
/// `TRANSITION_DURATION` rather than instantly, with the same smoothstep easing as `Transition`
```

with

```rust
/// An in-progress eased pan/zoom from one `camera.rect` to another --
/// entering/leaving a group moves the camera to the view the new level was
/// left at, or its `fit_all_nodes_scene_rect()` (`App::enter_level`), over
/// `TRANSITION_DURATION` rather than instantly, with the same smoothstep easing as `Transition`
```

- [ ] **Step 9: Callers that select a card after a level show**

In `crates/manifold/src/json_level.rs`, in `open_ref`, after `self.show_level_of(target);` add:

```rust
        self.camera.reveal = Some(target);
```

In `crates/manifold/src/trail_nav.rs`, in `finish_trail_focus`'s Graph branch (the `show_level_of(id)` at `:201`, after the `leave_to_graph` check), add after `self.show_level_of(id);`:

```rust
        self.camera.reveal = Some(id);
```

(The Item-view branch at `:194` opens the card in the Item view and needs none. `omnibar.rs:351` and `:363` already set `reveal` or use the Item view.)

- [ ] **Step 10: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold --bins local_state`
Expected: PASS, including `opening_a_ref_reveals_its_card`, `a_saved_camera_is_used_only_while_it_meets_the_level`, `switching_levels_returns_to_each_levels_view`, `leaving_mid_ease_saves_where_the_ease_was_going`, `a_saved_view_that_misses_the_level_falls_back_to_fit_all`, `deleting_the_level_in_view_leaves_no_view_for_it`, `opening_a_session_drops_the_old_sessions_level_views`, `each_levels_view_survives_a_restart`.

Run: `cargo test -j 8 -p manifold --bins json_level`
Expected: PASS, including `a_json_level_keeps_its_own_camera`, `leaving_an_unenterable_json_level_saves_no_view_for_it` and the existing `a_refetch_placeholder_is_not_entered_and_keeps_its_view`.

Run: `cargo test -j 8 -p manifold --bins` (the whole crate, once, before this task's commit: existing tests and headless paths that assumed fit-all after a switch, such as `graph_ui.rs:524` and `debug_export.rs:577`, `:595`, first meet the restore here).
Expected: PASS (the one `LOCK_WAIT` test takes about 5 s; `#[ignore]`d timing tests are skipped). A newly failing existing test is triaged, not patched around: if it wants fit-all after a switch, it drops the level's saved view first (`app.level_views.clear()`); if it was about restoring, it asserts the restored rect. A failure that is neither means the plan is wrong: stop and report.

- [ ] **Step 11: README `:217`, `:233`, PARKED and the op-log spec**

Use the Edit tool for each replacement (old text, then new text).

`README.md:217`:

```text
**Ungroup All**: a new Graph-view toolbar button, top level only, shown whenever at least one group exists
```

```text
**Ungroup All**: a new Graph-view toolbar button, shown at any level with at least one group (top level only when this entry was written)
```

`README.md:233`, first phrase:

```text
**"Collapse Groups"/"Expand Groups"**: a new Graph toolbar button, top level only, shown whenever at least one group exists
```

```text
**"Collapse Groups"/"Expand Groups"**: a new Graph toolbar button, shown at any level with at least one group (top level only when this entry was written)
```

`README.md:233`, second phrase:

```text
since a collapsed non-empty group now reaches that same branch too. Session-only, resets on restart.
```

```text
since a collapsed non-empty group now reaches that same branch too. Each level keeps its own toggle, across restarts too (per-level view state, 2026-11-01; it was session-only when this entry was written).
```

`PARKED.md:47` (the "Groups as scopes inside one document" entry), first the stale "today" clause:

```text
(`local_state.rs`: today one global `camera` and `groups_collapsed`; a map keyed by level, cached for levels not in view, `remap` renaming its keys)
```

```text
(`local_state.rs`, `LocalState::levels`: a map keyed by level, cached for levels not in view, `remap` renaming its keys; done by the per-level view state plan, 2026-11-01)
```

then the status (`*Status:* parked, direction decided.` alone also matches `:50`, so the old text includes the evidence clause before it, which only `:47` has):

```text
*Evidence:* surveys of 2026-10-31 (counts by grep, order of magnitude). *Status:* parked, direction decided.
```

```text
*Evidence:* surveys of 2026-10-31 (counts by grep, order of magnitude). *Status:* parked, direction decided; Slice 2 (per-level camera and collapsed state, `LocalState::levels`) done, plan `docs/superpowers/plans/2026-11-01-per-level-view-state.md`.
```

`docs/superpowers/specs/2026-09-25-session-op-log-design.md:166-167` (a done spec; annotate, don't rewrite):

```text
  was saved at (the app has one camera; per-level cameras are later);
```

```text
  was saved at (the app has one camera; per-level cameras are later) [superseded 2026-11-01: `LocalState::levels` keeps a camera and Collapse Groups toggle per level];
```

- [ ] **Step 12: Format, lint and commit**

Run: `cargo fmt --all` then the clippy command from Global Constraints.
Expected: clippy finishes with no warnings.

```bash
git add crates/manifold/src/graph_ui/scene.rs crates/manifold/src/local_state.rs crates/manifold/src/json_level.rs crates/manifold/src/session_files.rs crates/manifold/src/graph_ui/camera.rs crates/manifold/src/view_modes.rs crates/manifold/src/trail_nav.rs crates/manifold/src/session_sync/tests/local_state.rs crates/manifold/src/json_level/tests.rs README.md PARKED.md docs/superpowers/specs/2026-09-25-session-op-log-design.md
git diff --cached --stat
git commit -m "Switching levels returns to that level's camera and Collapse Groups toggle

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Final lint, gate tier and close-out

**Files:**
- Modify: `docs/superpowers/plans/2026-11-01-per-level-view-state.md` (front matter)

**Interfaces:**
- Consumes: Tasks 1 and 2 committed.
- Produces: nothing new.

- [ ] **Step 1: Leftover references**

Run: `git grep -n "groups_collapsed\b" -- crates` and `git grep -n "lenient_camera\|all_finite\|state\.camera\|\.camera ==" -- crates`
Expected: `groups_collapsed` only as the `App` field and its readers (`main.rs`, `card/group_body.rs`, `graph_ui/toolbar.rs`, `local_state.rs`, `scene.rs`, `session_files.rs`, tests); the second grep prints nothing.

- [ ] **Step 2: Format check, lint and gate tier**

Run: `cargo fmt --all --check` and the clippy command from Global Constraints.
Expected: both clean.

Run: `cargo build -p gate` then `target/debug/gate.exe tier HEAD~2`
Expected: last line `TIER full` (`local_state` and `session_sync` are core). Report it; the full gate (`target/debug/gate.exe all LOG full` through `run.ps1`, CLAUDE.md "Gate") runs only when the user asks.

- [ ] **Step 3: Mark the plan done and commit**

Change the front matter `status: active` to `status: done`.

```bash
git add docs/superpowers/plans/2026-11-01-per-level-view-state.md
git commit -m "Plan: per-level view state done

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```
