# Wave 1: correctness and data integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the ten verified wave-1 integrity items, each as its own tested commit:
- L4: a corrupted doc comment
- L1: Group Selected's centroid
- P1: a group close that commits once per tab
- C1: five drifting per-id cleanup lists, plus a GPU texture leak on session replace
- H1: a selection that outlives a remote move
- M5: a Json view that loses what it can't read
- H4: all-or-nothing settings
- M3: a silent cookie-dump parse failure
- M4: unchecked arguments
- G2: a `Carried` drift guard

**Architecture:** The smallest changes, in place:
- P1 reuses the non-saving `remove_tab`.
- C1 adds one `App::forget_id`, which every permanent removal calls, guarded by a source-scan test.
- H1 adds a per-frame `end_stale_selection` beside `end_stale_drag`.
- M5 adds an `Unread` part to `JsonView` that is written back untouched.
- H4 adds per-field lenient deserializers.
- M3 changes `jar::from_json` to return `Result` and logs the failure.
- M4 validates the arguments before anything runs.

No store format, sync protocol or IPC change.

**Tech Stack:** Rust (edition 2021), egui 0.36.2, serde / serde_json 1.0.151 (`preserve_order`), the session store journal (`store::journal::inspect`).

**Spec:** `docs/superpowers/specs/2026-10-07-wave-1-integrity-design.md`. The scope and evidence come from `.superpowers/survey/verified-wave-1.md`. Every line number and symbol below was re-read at `c8b7485`. Paths are under `crates/manifold/src/` unless they start with `docs/`, `scripts/` or `README.md`. Find code by the quoted text: the numbers drift as earlier tasks land.

## Global Constraints

**Branch and commits**
- Work on branch `wave-1-integrity`, created from `main` before Task 1 (`git switch -c wave-1-integrity main`), with no worktree.
- The controller's first commit on the branch adds this plan and its spec: `git add docs/superpowers/specs/2026-10-07-wave-1-integrity-design.md docs/superpowers/plans/2026-10-07-wave-1-integrity.md`, then `git commit -m "Wave 1: spec and plan"` with the controller's own `Co-Authored-By:` line. Add no other untracked file.
- Then commit once per task (ten), and once more in Task 11 for the Outcome and the spec's Status. Merge into `main` only after the user's go-ahead (Task 11).
- Each implementer's commit message ends with that implementer's own `Co-Authored-By:` line, taken from its system reminder.

**Formatting and lints**
- Run `cargo fmt --all` before each commit.
- `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass.
- `.githooks/pre-commit` runs both and takes 2+ minutes, so commit in the background. Never use `--no-verify`.
- Don't silence a lint: fix it, or `#[allow]` it with a comment saying why.

**Builds and the machine**
- Use `-j 8` on every cargo command. Another session may be building: if cargo waits on the target-dir lock, wait for it rather than killing anything.
- C: is nearly full, so check `df -h /c` before a full build.
- Before the gate and after the wave, check for leftovers with `Get-Process python, manifold -ErrorAction SilentlyContinue`.

**Tests**
- No test may read or write the real `%APPDATA%\manifold`.
  - `App::default()` is safe: `profile::dir()` is `None` under `cfg(test)`.
  - A test that needs files uses `crate::store::testutil::temp_dir(name)`, with a name unique to that test.
- Write each test first and see it fail (each task says how it fails) before the change.

**Python**
- Never run `python` in any way that can read stdin: no `python -`, no heredoc python, no python call tacked on as a no-op.
- Write the script to a file and run `python -P f.py < /dev/null`. Prefer the Edit tool. Subagents never dispatch subagents.

**Rust**
- Edition 2021, so no let-chains. `Option::is_none_or` is fine.

**Spec values**
- `ZOOM_RANGE` is `0.5..=2.0`.
- M3's error text is `"{:?} error at line {} column {}"` (category, line, column) and never contains input.
- M5's refusal toast is exactly "This JSON view was saved by a newer manifold; changes to it aren't saved".
- M4's refusal prints `manifold: <why>` and exits 2.

## Decisions resolved by reading the code

1. **`struct App {` is not `pub(crate)`.** The C1 scan matches the line `struct App {` exactly (`main.rs:822`). Every field is on one line, and the struct ends at the first line that is exactly `}` (`main.rs:1507`).
2. **Which App fields the C1 scan sees: 49 fields at `c8b7485`.**
   - The scan takes every field whose type mentions `HashMap<String`, `HashSet<String`, `Option<String`, `Option<(String` or `Vec<String`.
   - 14 are named in `forget_id`'s body:
     - `skip_convert`, `pending_sleeps`, `saved_scroll`, `nav_history`, `navigated_from`, `tab_crashes`, `selected_tabs`
     - `naming_jar`, `renaming_group`, `node_menu`, `pending_respawn_focus`, `peek_suppressed`, `dragging`, `hovered`
   - The other 35 go in `NOT_FORGOTTEN`, with a reason each (Task 4).
3. **`forget_removed_group` is deleted, not wrapped.**
   - Its four steps (dragging, `forget_connect_source`, hovered, `renaming_group`) are a subset of `forget_id`. The extra steps are no-ops for a group id.
   - Its callers are `dissolve_members` (`main.rs:2447`) and `merge_groups` (`graph_nav.rs:167`).
4. **The P1 frame count is reliable.**
   - `inspect(&bytes).frames` lists the journal's frames.
   - Each `save_session` that has ops commits one frame (`txn.commit`).
   - The test file stays far below `COMPACT_MIN` (64 KiB), so no compaction rewrites it mid-test.
5. **The H1 behavior test drives one real frame in `ViewMode::Settings`.** It uses the harness of `popups.rs:2030-2050` (`theme::apply_fonts`, `theme::apply_style`, `ctx.run_ui`, `output.textures_delta.clear()`). `build_ui_inner` calls `end_stale_drag` and `end_stale_connect` before any view is drawn (`main.rs:2859-2861`), so the new call there runs in every view.
6. **M5 writes overrides through `#[serde(flatten)]`, never through `serde_json::to_value`.**
   - `to_value` turns an `f32` into an `f64` number, so `0.1` would be written as `0.10000000149011612`.
   - Serialising `CardOverride` itself keeps today's text byte-for-byte. The existing test `view_round_trips_and_stores_nothing_when_empty` pins that text.
7. **M5's toast needs no state.**
   - `edit_json_view` checks `is_opaque()` after cloning the view and before `edit`.
   - On that path the immutable `level` borrow is not used again, because it returns, so `self.json_level.as_mut()` and `self.queue_toast(..)` borrow cleanly.
   - It relays the level out from the unchanged view before returning: a drag moves the card's rect live (`json_level_ui.rs`, `moves`), so without it a refused drag would leave the card where it was dropped until the next relayout.
   - It toasts on every refused edit, with no once-per-view state. This departs from `verified-wave-1.md`, which suggests a one-time toast: each refused edit is a separate user action that silently did nothing, so each gets its answer, and a flag would need clearing when the view changes. The wording names the cause (a newer build) and the effect (not saved).
8. **M3 does not skip bad cookies.** A cookie missing from a pull is a logout and deletes that cookie from the jar (`main.rs` cookie tests: "its absence is a logout"). The parse stays all or nothing; the only change is the log.
9. **M4's flag list comes from every reader:**
   - `cli::run`: `--dump`, `--check`, `--import-json`, `--out`
   - `main`: `--headless` (2983), `--debug-commands` (2994)
   - `profile::dir_from`: `--profile-dir`
   - `debug_control.rs:823`: `--script`

   Scripts pass `--headless`, `--import-json`, `--out`, `--debug-commands` (`drive.sh` with `VISIBLE=1`, `foreground-watch.ps1 -Visible`), and `--profile-dir` / `--script` (`json-pan-profile.sh`, comma-separated in `-ExeArgs`). `--pid` appears in `scripts/`, but it is `tail`'s flag, not manifold's.
10. **Task order and file overlap.**
    - P1 (3) and C1 (4) both edit `main.rs`'s removal code (`destroy_group_members`; `drop_node`, `dissolve_members`), a few dozen lines apart, so C1 comes after P1.
    - H1 (5) comes after C1, which owns `selected_tabs` removal. It is a soft dependency.
    - M5 (6) and G2 (10) both edit `mirror.rs`, in different places. Run them in order, not in parallel.
    - C1 (4) and M3 (8) both edit `tabs.rs` in different functions (`drop_tab`, `apply_pulled_cookies`).
    - Run the tasks one at a time, in order.

## Per-task summary

| # | Item | Implementer | Gate if landed alone | Depends on | Proof |
|---|---|---|---|---|---|
| 1 | L4 doc | haiku | light | none | `git diff` touches only the comment (`6 +++---`); clippy clean |
| 2 | L1 centroid | haiku | light | none | `group_selected_centres_on_the_tabs_still_shown` fails (divides by 3), then passes |
| 3 | P1 group close | opus | full | none | `closing_a_group_commits_one_frame`: 4 frames before, 1 after |
| 4 | C1 `forget_id` | opus | full | 3 | the scan test, table test and teardown test pass; the teardown test fails first |
| 5 | H1 selection | sonnet | light | 4 (soft) | `a_tab_another_replica_moved_away_leaves_the_selection` fails (closes `b`), then passes |
| 6 | M5 Json view | opus | full | none | two `json_view` tests and one `json_level` test fail first, then pass; the keys guard passes; the existing text test is unchanged |
| 7 | H4 settings | haiku | full | none | `an_unknown_value_resets_only_its_own_setting` fails (`from_str` errs), then passes |
| 8 | M3 cookie log | haiku | full | none | `a_bad_dump_reports_where_without_echoing_a_value` fails to compile on `Option`, then passes |
| 9 | M4 arguments | haiku | full | none | `an_unknown_flag_or_a_missing_value_refuses_to_start` fails (`None`), then passes; each launch form run by hand (Step 8) |
| 10 | G2 guard | haiku | full (letter only: test-only) | 6 (same file) | `carried_holds_every_tab_field` passes |
| 11 | Wave gate and merge | controller | **full** | 1-10 | full `cargo test`, `scripts/gate.sh LOG full` all pass, CPU and Defender reported |

The gate column follows CLAUDE.md "Gate": `light` only for UI code (drawing, gestures, menus), scripts or docs; `full` for anything else, which here means sync, the store, the mirror, session save, an engine, settings or startup. It matters only for a task landed on its own later: the wave itself runs one full gate (Task 11).

The haiku tasks are mechanical, with the complete code given here. Each gets a coverage review: the reviewer checks that every step's code landed, that the test failed first, and that nothing else changed.

---

### Task 1: L4. Restore `end_stale_drag`'s doc comment

**Model:** haiku. **Gate alone:** light (a comment only). **Proof:** the diff touches the comment only.

**Files:**
- Modify: `crates/manifold/src/graph_nav.rs:503-506`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing (a doc comment only).

- [ ] **Step 1: Replace the corrupted comment.** The four lines directly above `pub(crate) fn end_stale_drag(&mut self, graph_shown: bool) {` are now:

```rust
    /// false), with no drop: nothing is saved. The layout keeps the dragged positions (remote snaps were skipped
    /// while it ran); a later remote pos snaps again. Otherwise the
    /// false), with no drop: nothing is saved, and the cards it moved take remote positions again. Otherwise the
    /// card's next release, whenever it is drawn again, would save the stale layout pos over a remote one.
```

Replace them with:

```rust
    /// Ends a drag that is no longer live (`live_drag`), or any drag when Graph view isn't shown (`graph_shown`
    /// false), with no drop: nothing is saved. The layout keeps the dragged positions (remote snaps were skipped
    /// while it ran); a later remote pos snaps again. Otherwise the card's next release, whenever it is drawn again,
    /// would save the stale layout pos over a remote one.
```

- [ ] **Step 2: Check.** Run `git diff --stat`. Expect `graph_nav.rs | 6 +++---`: the old first line is the new second line word for word, so git keeps it and counts three lines changed, not four. Then run `cargo fmt --all` and the clippy line. Expect no change and no warning.

- [ ] **Step 3: Commit.**

```bash
git add crates/manifold/src/graph_nav.rs
git commit -m "Wave 1 (L4): restore end_stale_drag's doc comment" -m "cc0441f replaced the comment's first line instead of its second sentence." -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 2: L1. Group Selected centres on the tabs still shown

**Model:** haiku. **Gate alone:** light (UI code: a graph menu action). **Proof:** the new test fails first (the sum is divided by 3), then passes.

**Files:**
- Modify: `crates/manifold/src/graph_ui.rs:315-340` (`GraphAction::GroupSelected` in `apply_graph_action`)
- Test: `crates/manifold/src/graph_ui.rs` tests module (after `use super::*;` at about 2103)

**Interfaces:**
- Consumes:
  - `App::apply_graph_action(&mut self, ctx: &egui::Context, action: GraphAction)`
  - `App::insert_placeholder_node(&mut self, id: String, pos: egui::Vec2, kind: Kind, runtime: Runtime)`
  - `crate::session_sync::tests::sleeping_tab(url: &str, title: &str) -> Runtime`
  - `Mirror::group(&self, id) -> Option<&str>` and `Mirror::pos(&self, id) -> Option<egui::Vec2>`
  - `App::layout_pos(&self, id) -> egui::Vec2`
- Produces: nothing new.

- [ ] **Step 1: Write the failing test.** Add it to the `graph_ui.rs` tests module, after `keys_typed_into_a_text_field_stay_out_of_graph_navigation`:

```rust
    /// L1: a selected id with no card (its tab mid-crash-respawn or mid-engine-swap) is left out of the centroid,
    /// not counted in it.
    #[test]
    fn group_selected_centres_on_the_tabs_still_shown() {
        use crate::session_sync::tests::sleeping_tab;
        use crate::store::model::Kind;
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_placeholder_node(a.clone(), egui::vec2(100.0, 0.0), Kind::Tab, sleeping_tab("https://a.test/", "a"));
        app.insert_placeholder_node(b.clone(), egui::vec2(300.0, 0.0), Kind::Tab, sleeping_tab("https://b.test/", "b"));
        let expected = (app.layout_pos(&a) + app.layout_pos(&b)) / 2.0;
        app.selected_tabs.extend([a.clone(), b.clone(), "999".to_string()]);
        app.apply_graph_action(&egui::Context::default(), GraphAction::GroupSelected);
        let group = app.mirror.group(&a).expect("a was grouped").to_string();
        assert_eq!(app.mirror.group(&b), Some(group.as_str()));
        assert_eq!(app.mirror.pos(&group), Some(expected), "the mean of the two cards, not divided by three");
        assert!(app.selected_tabs.is_empty());
    }
```

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo test -j 8 -p manifold group_selected_centres_on_the_tabs_still_shown`
Expected: FAIL at the `mirror.pos` assertion. The left side is two thirds of `expected`: the sum is divided by 3.

- [ ] **Step 3: Fix it.** Replace the whole `GraphAction::GroupSelected => { … }` arm with:

```rust
            GraphAction::GroupSelected => {
                // Only the selected cards still shown: a tab mid-crash-respawn or mid-engine-swap has no card, so it
                // is neither counted in the centroid nor moved (its selection is consumed with the rest).
                let live: Vec<String> =
                    self.selected_tabs.iter().filter(|id| self.nodes.contains_key(*id)).cloned().collect();
                self.selected_tabs.clear();
                if live.is_empty() {
                    return;
                }
                let id = self.alloc_node_id();
                // Centroid of the selected cards, rather than the usual
                // world-center-plus-jitter spawn point: these tabs already have
                // real positions the user just drew a box around, so the new
                // group card should appear where they actually are, not at the
                // origin of a possibly-distant part of the graph.
                let pos = live.iter().fold(egui::Vec2::ZERO, |acc, t| acc + self.layout_pos(t)) / live.len() as f32;
                let name = self.default_group_name();
                let at = self.placement(&id, Some(pos));
                self.place_group(Some(id.clone()), &name, at);
                for tab_id in live {
                    self.edit_group(&tab_id, Some(id.clone()));
                }
            }
```

`return` here is what the old arm did for an empty selection: it skips the `save_session()` after the match. With nothing live there is nothing to save.

- [ ] **Step 4: Run it and see it pass.**

Run: `cargo test -j 8 -p manifold group_selected_centres_on_the_tabs_still_shown`
Expected: PASS.

Then run `cargo test -j 8 -p manifold graph_ui`. Expected: all pass.

- [ ] **Step 5: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/graph_ui.rs
git commit -m "Wave 1 (L1): Group Selected centres on the selected cards still shown" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 3: P1. Closing a group commits once

**Model:** opus. **Gate alone:** full (session save behavior). **Proof:** the frame count changes by 4 before the fix and by 1 after.

**Files:**
- Modify: `crates/manifold/src/main.rs:2650-2680` (`destroy_group`, `destroy_group_members`)
- Test: `crates/manifold/src/session_sync/tests.rs` (append)

**Interfaces:**
- Consumes:
  - `App::remove_tab(&mut self, id: &str)`: `drop_tab` plus `queue_delete`, with no save (`tabs.rs:1197`).
  - `App::card_kind(&self, id) -> Option<CardKind>`, where `CardKind.ty: &'static str`.
  - `store::model::TYPE_TAB`.
  - Test helpers in `session_sync/tests.rs`:
    - `app_on(path: &Path, actor: u64) -> App`
    - `add(app, Kind, Runtime) -> String`
    - `add_group(app, name) -> String`
    - `sleeping_tab(url, title) -> Runtime`
    - `live_in_doc(app, id) -> bool`
    - `temp_dir`
- Produces: `destroy_group_members` no longer saves per tab. Task 4 edits these same functions' neighbours.

- [ ] **Step 1: Write the failing test.** Append it to `session_sync/tests.rs`. `Kind` and `temp_dir` are already imported there:

```rust
/// P1: closing a group commits once, so another replica never reads it half deleted and a crash can't leave part of
/// it.
#[test]
fn closing_a_group_commits_one_frame() {
    let path = temp_dir("group-close-once").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    for url in ["https://a.test/", "https://b.test/", "https://c.test/"] {
        let t = add(&mut app, Kind::Tab, sleeping_tab(url, "t"));
        app.edit_group(&t, Some(g.clone()));
    }
    assert!(app.save_session());
    let frames = || crate::store::journal::inspect(&std::fs::read(&path).unwrap()).unwrap().frames.len();
    let before = frames();
    app.close_node(&g);
    assert_eq!(frames() - before, 1, "one commit for the group and its tabs");
    assert!(!live_in_doc(&app, &g));
}
```

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo test -j 8 -p manifold closing_a_group_commits_one_frame`
Expected: FAIL with `left: 4, right: 1`.

- [ ] **Step 3: Fix it.** Replace `destroy_group` and `destroy_group_members` (from the doc comment "Destroys a group and everything in it" through the end of `destroy_group_members`) with:

```rust
    /// Destroys a group and everything in it, at any depth, then the group itself, and saves once. Records nothing:
    /// `close_node` has already recorded the whole subtree as one entry.
    fn destroy_group(&mut self, id: &str) {
        self.destroy_group_members(id, &mut HashSet::new());
        // The one save for the whole subtree: no member's removal saves (`destroy_group_members`), so the group and
        // everything in it go in one commit, and another replica never reads it half deleted.
        self.save_session();
    }

    /// `destroy_group`'s walk, with no save: a tab member is `remove_tab`d (not `destroy_tab`, which saves), every
    /// other member `destroy_node`d, each group once, so a group cycle in the mirror (two replicas' moves raced; the
    /// next save shows the document's groups) can't recurse forever. A group already being destroyed further up is
    /// left to it.
    fn destroy_group_members(&mut self, id: &str, visited: &mut HashSet<String>) {
        visited.insert(id.to_owned());
        let member_ids: Vec<String> = self.members_of(id).map(str::to_owned).collect();
        for tid in member_ids {
            if self.is_container(&tid) {
                if !visited.contains(&tid) {
                    self.destroy_group_members(&tid, visited);
                }
            } else if self.card_kind(&tid).is_some_and(|k| k.ty == store::model::TYPE_TAB) {
                // No save per tab: `destroy_group` commits the whole subtree once.
                self.remove_tab(&tid);
            } else {
                self.destroy_node(&tid);
            }
        }
        self.dissolve_group(id);
    }
```

- [ ] **Step 4: Run it and see it pass.**

Run: `cargo test -j 8 -p manifold closing_a_group_commits_one_frame`
Expected: PASS.

Then run `cargo test -j 8 -p manifold session_sync` and `cargo test -j 8 -p manifold close`. Expected: all pass. The existing group-close and Recently Closed tests must not change.

- [ ] **Step 5: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/main.rs crates/manifold/src/session_sync/tests.rs
git commit -m "Wave 1 (P1): closing a group commits once" -m "A tab member goes through the non-saving remove_tab; destroy_group's one save commits the subtree in one frame." -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 4: C1. `forget_id`: one cleanup for a removed id

**Model:** opus. **Gate alone:** full (session teardown, remote delete). **Proof:**
- `teardown_forgets_the_old_tabs_per_id_state` fails first.
- The table test and the scan test fail first because `forget_id` doesn't exist.
- All three then pass, with every existing test unchanged.

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - `drop_node` (2248-2275)
  - add `forget_id` after it
  - delete `forget_removed_group` (2451-2465) and change its call in `dissolve_members` (2447)
  - tests module (3027+)
- Modify: `crates/manifold/src/tabs.rs:1216-1247` (`drop_tab`'s hand list)
- Modify: `crates/manifold/src/graph_nav.rs:167` (`merge_groups`)
- Modify: `crates/manifold/src/session_sync.rs`:
  - `remove_remotely_deleted`'s `self.selected_tabs.remove(id);` (786)
  - `teardown_running_session` (2017-2060)
- Test: `crates/manifold/src/main.rs` tests module, `crates/manifold/src/session_sync/tests.rs`

**Interfaces:**
- Consumes:
  - `App::clear_cookie_state(&mut self, id: &str)` (`tabs.rs:247`)
  - `App::forget_connect_source(&mut self, id: &str)` (`links.rs:162`)
  - `PopupGate::forget(&mut self, source: &str)`
  - `GpuState::{free_tab_thumbnail, free_tab_texture, free_image}(&mut self, id: &str)`, each a no-op for an id it lacks
- Produces: `fn forget_id(&mut self, id: &str)`, a private method on `App` in `main.rs`. Task 5 relies on `selected_tabs` being cleared here for every permanent removal.

- [ ] **Step 1: Write the failing tests in `main.rs`.** Add them to `main.rs`'s tests module (after `use super::*;` and `const SID`):

```rust
    /// C1: everything `forget_id` owns goes for the forgotten id, and only for it.
    #[test]
    fn forget_id_clears_every_id_keyed_table() {
        let mut app = App::default();
        let now = std::time::Instant::now();
        let t = || "t".to_string();
        app.skip_convert.insert(t(), true);
        app.pending_sleeps.insert(t(), sleep_state::PendingSleep { why: sleep_state::SleepWhy::Manual, sent: now });
        app.jar_tabs.insert(t(), (jar::JarKey::Default, jar::TabCookies::default()));
        app.jar_reseed.insert(t());
        app.cookie_holds.insert(t(), (now, tabs::Held::Snapshot(EngineKind::Webview2, String::new())));
        app.jar_pulls.insert(t(), now);
        app.jar_silent.insert(t(), 1);
        app.saved_scroll.insert(t(), sleep_state::SavedScroll::new(0.0, 1.0));
        app.saved_scroll.insert("u".into(), sleep_state::SavedScroll::new(0.0, 1.0));
        app.nav_history.insert(t(), nav_history::NavHistory::default());
        app.navigated_from.insert(t(), ("https://a.test/".into(), now, false));
        for _ in 0..popup_gate::POPUP_BURST {
            app.popup_gate.admit("t", true, now);
        }
        app.tab_crashes.insert(t(), vec![now]);
        app.selected_tabs.extend([t(), "u".to_string()]);
        app.naming_jar = Some((t(), String::new()));
        app.renaming_group = Some((t(), String::new()));
        app.node_menu = Some((t(), egui::Pos2::ZERO));
        app.pending_respawn_focus = Some(t());
        app.peek_suppressed = Some(t());
        app.dragging = Some(t());
        app.connecting = Some((t(), crate::caps::LINK));
        app.connect_drag = Some(t());
        app.hovered = Some(t());

        app.forget_id("t");

        assert!(app.skip_convert.is_empty() && app.pending_sleeps.is_empty());
        assert!(app.jar_tabs.is_empty() && app.jar_reseed.is_empty() && app.cookie_holds.is_empty());
        assert!(app.jar_pulls.is_empty() && app.jar_silent.is_empty());
        assert!(!app.saved_scroll.contains_key("t") && app.saved_scroll.contains_key("u"), "only t's entry goes");
        assert!(app.nav_history.is_empty() && app.navigated_from.is_empty() && app.tab_crashes.is_empty());
        assert_eq!(app.popup_gate.admit("t", true, now), popup_gate::Admit::Open, "t's popup burst count went");
        assert_eq!(app.selected_tabs, HashSet::from(["u".to_string()]));
        assert!(app.naming_jar.is_none() && app.renaming_group.is_none() && app.node_menu.is_none());
        assert!(app.pending_respawn_focus.is_none() && app.peek_suppressed.is_none());
        assert!(app.dragging.is_none() && app.connecting.is_none() && app.connect_drag.is_none());
        assert!(app.hovered.is_none());
    }

    /// C1: App fields that can hold a node id, but that `forget_id` deliberately leaves alone, each with why. A
    /// field of an id-holding type that is in neither place fails `every_id_keyed_field_is_forgotten_or_excused`.
    const NOT_FORGOTTEN: &[(&str, &str)] = &[
        ("node_order", "removed with `nodes` by each caller (kind-specific order)"),
        ("layout", "pruned to live nodes at every save (`layout.retain`)"),
        ("held_text", "pruned to live nodes at every save (`held_text.retain`)"),
        ("current_group", "kind-specific: `drop_node` and `dissolve_members` switch level"),
        ("focused", "`drop_tab`'s focus, split and view-mode logic"),
        ("split_tab", "`drop_tab`'s focus, split and view-mode logic"),
        ("viewed_image", "`remove_remotely_deleted`; `image_view` resets it when the image is gone"),
        ("jar_tabs", "via `clear_cookie_state`"),
        ("jar_reseed", "via `clear_cookie_state`"),
        ("cookie_holds", "via `clear_cookie_state`"),
        ("jar_pulls", "via `clear_cookie_state`"),
        ("jar_silent", "via `clear_cookie_state`"),
        ("connecting", "via `forget_connect_source`"),
        ("connect_drag", "via `forget_connect_source`"),
        ("wakes_in_flight", "spawn lifecycle: a tab closed while waking needs it so the landing commits its delete"),
        ("spawning", "spawn lifecycle: the landing or teardown removes it"),
        ("remote_in_flight", "spawn lifecycle: replayed after the landing"),
        ("host_loads", "spawn lifecycle: expired by `shares_url_change`, cleared at teardown"),
        ("drag_moved", "ends with the drag; a gone id is skipped at release"),
        ("line_press", "a link id, taken at release only if still hovered"),
        ("native_drag_tab", "taken at the button-up"),
        ("hidden_for_minimize", "drained at restore"),
        ("node_menu_cache", "the closing menu's fade copy; `node_menu` is the live one"),
        ("pending_session_delete", "a session name, not a node id"),
        ("favicon_resolved", "origins, not node ids"),
        ("favicon_origin_cache", "urls, not node ids"),
        ("graph_settled_key", "a level fingerprint, rebuilt when it differs"),
        ("debug_address_bar_typing", "typed text, not a node id"),
        ("pending_screenshot", "a file path, not a node id"),
        ("pending_print_to_pdf", "a file path, not a node id"),
        ("save_toast", "a message, not a node id"),
        ("cef_dir_draft", "a path draft, not a node id"),
        ("home_page_draft", "a url draft, not a node id"),
        ("renaming_bookmark_folder", "a bookmark folder, not a node id"),
        ("history_matches_cache", "a history query, not a node id"),
    ];

    /// C1: a new App table that can hold a node id must be decided: forgotten by `forget_id` (permanent removal)
    /// or excused in `NOT_FORGOTTEN` with why. The drift this replaces was five hand-written lists.
    #[test]
    fn every_id_keyed_field_is_forgotten_or_excused() {
        let source = include_str!("main.rs");
        let fields: Vec<(&str, &str)> = source
            .lines()
            .skip_while(|l| *l != "struct App {")
            .skip(1)
            .take_while(|l| *l != "}")
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && !l.starts_with('#'))
            .filter_map(|l| l.split_once(": "))
            .collect();
        assert!(fields.len() > 100, "App's fields were found ({})", fields.len());
        let body = source
            .lines()
            .skip_while(|l| !l.contains("fn forget_id("))
            .take_while(|l| *l != "    }")
            .collect::<Vec<_>>()
            .join("\n");
        assert!(body.contains("self.skip_convert"), "forget_id's body was found");
        let mentions = |name: &str| {
            let needle = format!("self.{name}");
            body.match_indices(&needle).any(|(at, _)| {
                !body[at + needle.len()..].starts_with(|c: char| c == '_' || c.is_ascii_alphanumeric())
            })
        };
        const ID_SHAPES: [&str; 5] = ["HashMap<String", "HashSet<String", "Option<String", "Option<(String", "Vec<String"];
        let mut undecided = Vec::new();
        for &(name, ty) in &fields {
            if !ID_SHAPES.iter().any(|shape| ty.contains(shape)) {
                continue;
            }
            let excused = NOT_FORGOTTEN.iter().any(|&(n, _)| n == name);
            if mentions(name) == excused {
                let why = if excused { "both forgotten and excused" } else { "neither forgotten nor excused" };
                undecided.push(format!("{name}: {why}"));
            }
        }
        assert!(undecided.is_empty(), "decide each App field: {undecided:?}");
        for &(name, _) in NOT_FORGOTTEN {
            assert!(fields.iter().any(|&(n, _)| n == name), "{name} is excused but is no longer an App field");
        }
    }
```

- [ ] **Step 2: Write the failing teardown test.** Append it to `session_sync/tests.rs`:

```rust
/// C1: replacing the running session forgets the old tabs' per-id state (and, with a GPU, frees their textures).
#[test]
fn teardown_forgets_the_old_tabs_per_id_state() {
    let mut app = App::default();
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://a.test/", "a"));
    let now = std::time::Instant::now();
    app.saved_scroll.insert(t.clone(), crate::sleep_state::SavedScroll::new(0.0, 1.0));
    app.tab_crashes.insert(t.clone(), vec![now]);
    app.skip_convert.insert(t.clone(), true);
    app.jar_pulls.insert(t.clone(), now);
    app.teardown_running_session();
    assert!(app.nodes.is_empty());
    assert!(app.saved_scroll.is_empty(), "saved_scroll");
    assert!(app.tab_crashes.is_empty(), "tab_crashes");
    assert!(app.skip_convert.is_empty(), "skip_convert");
    assert!(app.jar_pulls.is_empty(), "jar_pulls");
}
```

- [ ] **Step 3: Run the tests and see them fail.**

Run: `cargo test -j 8 -p manifold teardown_forgets_the_old_tabs_per_id_state`
Expected: a compile error. `forget_id` doesn't exist yet, so the `main.rs` tests stop the build.

To see the teardown test fail on behavior, temporarily comment out the two new `main.rs` tests, run it again (expect FAIL at `saved_scroll`), then restore them.

- [ ] **Step 4: Add `forget_id`.** In `main.rs`, directly after `drop_node`'s closing `}`:

```rust
    /// Everything a permanently removed node leaves behind that carries no order: per-id tables, the gestures and
    /// latches naming it, its GPU textures. Called by every permanent removal (`drop_tab`, `drop_node`,
    /// `dissolve_members`, `merge_groups`, `teardown_running_session`); `main.rs`'s
    /// `every_id_keyed_field_is_forgotten_or_excused` fails if an id-holding App field is in neither this nor its
    /// `NOT_FORGOTTEN` list. Deliberately not here:
    /// - `wakes_in_flight`: a tab closed while waking needs it, so the landing commits its delete;
    /// - `spawning`, `remote_in_flight`, `host_loads`: the spawn lifecycle owns them;
    /// - the `pending*` save queues: the save's `editable` filter drops a deleted node's ops;
    /// - `layout`, `held_text`: pruned to live nodes at every save.
    ///
    /// Never call it for a crash respawn or an engine swap: those are temporary, and keep `tab_crashes`,
    /// `nav_history` and `saved_scroll`.
    fn forget_id(&mut self, id: &str) {
        self.skip_convert.remove(id);
        self.pending_sleeps.remove(id);
        self.clear_cookie_state(id);
        self.saved_scroll.remove(id);
        self.nav_history.remove(id);
        self.navigated_from.remove(id);
        self.popup_gate.forget(id);
        self.tab_crashes.remove(id);
        self.selected_tabs.remove(id);
        if self.naming_jar.as_ref().is_some_and(|(t, _)| t == id) {
            self.naming_jar = None;
        }
        if self.renaming_group.as_ref().is_some_and(|(g, _)| g == id) {
            self.renaming_group = None;
        }
        if self.node_menu.as_ref().is_some_and(|(n, _)| n == id) {
            self.node_menu = None;
        }
        if self.pending_respawn_focus.as_deref() == Some(id) {
            self.pending_respawn_focus = None;
        }
        if self.peek_suppressed.as_deref() == Some(id) {
            self.peek_suppressed = None;
        }
        if self.dragging.as_deref() == Some(id) {
            self.dragging = None;
        }
        self.forget_connect_source(id);
        if self.hovered.as_deref() == Some(id) {
            self.hovered = None;
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
            gpu.free_image(id);
        }
    }
```

The scan reads the body from the line containing `fn forget_id(` to the first line that is exactly `    }`. Keep every inner block indented deeper, as `cargo fmt` leaves it.

- [ ] **Step 5: Use it in `drop_node`.** Replace its body from `let removed = self.nodes.remove(id);` to the end with:

```rust
        let removed = self.nodes.remove(id);
        if removed.as_ref().is_some_and(|n| n.as_pdf().is_some()) {
            self.pdf_renderer.cancel(id);
        }
        if self.current_group.as_deref() == Some(id) {
            self.switch_group_level(self.mirror.group(id).filter(|_| removed.is_some()).map(str::to_owned));
        }
        self.node_order.retain(|d| d != id);
        // The texture of an image or PDF (`GpuState::images`) goes here too.
        self.forget_id(id);
    }
```

- [ ] **Step 6: Use it in `drop_tab` (`tabs.rs`).**
  - Replace every line from `self.skip_convert.remove(id);` through the `hovered` block (the line after `self.forget_connect_source(id);` that sets `self.hovered = None` and its closing `}`) with `self.forget_id(id);`.
  - That removes the hand list together with its `selected_tabs` comment and its GPU block.
  - The function then reads, from `self.nodes.remove(id);`:

```rust
        self.nodes.remove(id);
        self.node_order.retain(|t| t != id);
        self.forget_id(id);
        // A split only ever makes sense paired with a specific focused tab
```

  - Everything from that split comment to `true` stays as it is.

- [ ] **Step 7: Delete `forget_removed_group` and point both callers at `forget_id`.**
  - In `main.rs`, delete the doc comment "Drops the per-card state still naming a group card that's gone…" and the whole `fn forget_removed_group`.
  - In `dissolve_members`, change `self.forget_removed_group(id);` to `self.forget_id(id);`.
  - In `graph_nav.rs` (`merge_groups`), change `self.forget_removed_group(source);` to `self.forget_id(source);`.
  - Then `grep -rn forget_removed_group crates/` must find nothing.

- [ ] **Step 8: Drop the redundant line in `remove_remotely_deleted` (`session_sync.rs`).**
  - Delete `self.selected_tabs.remove(id);` just above `self.wakes_in_flight.remove(id);`. All three branches above it (`dissolve_group_quietly`, `drop_tab`, `drop_node`) now reach `forget_id`.
  - Keep `self.wakes_in_flight.remove(id);`. A remote delete of a waking tab is parked in `remote_in_flight` and replayed only after the landing.

- [ ] **Step 9: Forget every old id at teardown (`session_sync.rs`).** In `teardown_running_session`, directly above `self.nodes.clear();`, insert:

```rust
        // Each old node's per-id state and GPU textures (`forget_id`), including tabs whose spawn is still in flight
        // (no `nodes` entry yet, but cookie and popup state already). The session-wide clears below stay.
        let old_ids: Vec<String> = self.nodes.keys().chain(self.spawning.keys()).cloned().collect();
        for id in old_ids {
            self.forget_id(&id);
        }
```

- [ ] **Step 10: Run the three tests.**

Run: `cargo test -j 8 -p manifold forget_id_clears_every_id_keyed_table`, then `cargo test -j 8 -p manifold every_id_keyed_field_is_forgotten_or_excused`, then `cargo test -j 8 -p manifold teardown_forgets_the_old_tabs_per_id_state`.
Expected: all PASS.

If the scan test names a field:
- Decide it: add it to `forget_id` only if it is a permanent-removal table, otherwise to `NOT_FORGOTTEN` with a reason.
- Never add a spawn-lifecycle table to `forget_id`.
- Record the decision in the task report.

- [ ] **Step 11: Run the wider suites.**

Run: `cargo test -j 8 -p manifold`
Expected: all pass. The tests most exposed to this change are:
- `session_sync` (remote delete, wake landing)
- `graph_nav` (merge)
- the `tabs` close and cookie tests
- `links` (connect source)

- [ ] **Step 12: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/main.rs crates/manifold/src/tabs.rs crates/manifold/src/graph_nav.rs crates/manifold/src/session_sync.rs crates/manifold/src/session_sync/tests.rs
git commit -m "Wave 1 (C1): forget_id, one cleanup for a removed id; teardown frees the old session's textures" -m "drop_tab, drop_node, dissolve_members, merge_groups and teardown_running_session call it. A source-scan test makes every id-holding App field forgotten or excused." -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 5: H1. The rectangle selection drops tabs no longer on the level

**Model:** sonnet. **Gate alone:** light (UI code: the graph's selection state, pruned per frame). **Proof:** the test fails first on behavior (Close Selected closes `b`), then passes.

**Files:**
- Modify: `crates/manifold/src/graph_nav.rs` (add `end_stale_selection` after `end_stale_drag`, about line 512)
- Modify: `crates/manifold/src/main.rs:2861` (`build_ui_inner`, after `self.end_stale_connect(..)`)
- Test: `crates/manifold/src/graph_nav.rs` tests module (append)

**Interfaces:**
- Consumes:
  - `App::in_level(&self, id: &str) -> bool` (`session_sync.rs:1046`)
  - `App::follow_remote_group(&mut self, id: &str, group: Option<String>)`
  - `App::build_ui_inner(&mut self, ui: &mut egui::Ui)`
  - `App::apply_graph_action(&mut self, ctx, GraphAction::CloseSelected)`
  - from `session_sync::tests`: `add`, `add_group`, `sleeping_tab`
  - from Task 4: `forget_id` already removes closed ids from `selected_tabs`, so this net only covers ids that are still live but off the level.
- Produces: `pub(crate) fn end_stale_selection(&mut self)`.

- [ ] **Step 1: Write the failing test.** Append it inside `graph_nav.rs`'s `mod tests`:

```rust
    /// H1: another replica moves a selected tab into a group; the next frame drops it from the selection, so Close
    /// Selected closes only what is still on screen.
    #[test]
    fn a_tab_another_replica_moved_away_leaves_the_selection() {
        use crate::session_sync::tests::{add, add_group, sleeping_tab};
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App { view_mode: crate::view_modes::ViewMode::Settings, ..App::default() };
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a.test/", "a"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b.test/", "b"));
        let h = add_group(&mut app, "H");
        app.selected_tabs.extend([a.clone(), b.clone()]);
        app.follow_remote_group(&b, Some(h)); // another replica's move
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1000.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_ui_inner(ui));
        output.textures_delta.clear();
        assert_eq!(app.selected_tabs, std::collections::HashSet::from([a.clone()]));
        app.apply_graph_action(&ctx, crate::graph_ui::GraphAction::CloseSelected);
        assert!(!app.nodes.contains_key(&a), "the tab still shown is closed");
        assert!(app.nodes.contains_key(&b), "the tab moved away is not");
    }
```

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo test -j 8 -p manifold a_tab_another_replica_moved_away_leaves_the_selection`
Expected: FAIL at the first `assert_eq!`. The selection still holds both ids.

- [ ] **Step 3: Add `end_stale_selection`.** In `graph_nav.rs`, inside the same `impl App` block, directly after `end_stale_drag`:

```rust
    /// Drops from the rectangle selection every tab no longer drawn on this level: closed, mid-respawn, or moved by
    /// another replica. It only ever holds this level's tabs (`eject`, `switch_group_level`); otherwise Close Selected
    /// or Group Selected would act on a tab the user can't see.
    pub(crate) fn end_stale_selection(&mut self) {
        let mut selected = std::mem::take(&mut self.selected_tabs);
        selected.retain(|id| self.nodes.contains_key(id) && self.in_level(id));
        self.selected_tabs = selected;
    }
```

- [ ] **Step 4: Call it every frame.** In `main.rs` `build_ui_inner`, directly after `self.end_stale_connect(self.view_mode == ViewMode::Graph);`, add:

```rust
        // And the rectangle selection, which only ever holds the tabs drawn on this level.
        self.end_stale_selection();
```

- [ ] **Step 5: Run it and see it pass.**

Run: `cargo test -j 8 -p manifold a_tab_another_replica_moved_away_leaves_the_selection`
Expected: PASS.

Then run `cargo test -j 8 -p manifold graph_nav` and `cargo test -j 8 -p manifold graph_ui`. Expected: all pass.

- [ ] **Step 6: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/graph_nav.rs crates/manifold/src/main.rs
git commit -m "Wave 1 (H1): the rectangle selection drops tabs no longer on the level" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 6: M5. A Json view keeps what this build can't read

**Model:** opus. **Gate alone:** full (mirror decode of a synced field). **Proof:**
- The new tests fail first.
- `view_round_trips_and_stores_nothing_when_empty` keeps its exact expected text.
- Every `json_view` and `json_level` test passes.

**Files:**
- Modify: `crates/manifold/src/json_view.rs`:
  - imports (7)
  - `JsonView` and `NO_VIEW` (66-71)
  - `from_field`, `to_field`, `entry`, `retain_resolvable` (73-108)
  - tests (543+, including line 551)
- Modify: `crates/manifold/src/mirror.rs` (`Mirror::set`, the line `if view.0.is_empty() {`)
- Modify: `crates/manifold/src/json_level.rs:231-245` (`edit_json_view`), tests (461+)
- Modify: `docs/superpowers/specs/2026-09-27-json-node-rework-design.md` ("Saved view state", one bullet)

**Interfaces:**
- Consumes: `App::edit_view(&mut self, id: &str, view: &str)`, `App::queue_toast(&mut self, message: String)`, and the `app_with_json(name, text) -> (App, String)` fixture in `json_level.rs` tests.
- Produces:
  - `pub(crate) struct Unread` (private fields)
  - `JsonView(pub(crate) BTreeMap<String, CardOverride>, pub(crate) Unread)`
  - `JsonView::is_empty(&self) -> bool`
  - `JsonView::is_opaque(&self) -> bool`
  - `from_field`, `to_field`, `entry`, `get`, `pages`, `retain_resolvable` and `clear_positions` keep their signatures.

- [ ] **Step 1: Write the failing `json_view` tests.** In `json_view.rs` tests, change line 551 from `assert_eq!(JsonView::from_field("not json"), JsonView::default());` to:

```rust
        assert!(JsonView::from_field("not json").is_opaque(), "kept, not read as no overrides");
```

Then add:

```rust
    /// M5: one entry this build can't read (a newer build's shape) costs neither the others nor itself.
    #[test]
    fn an_entry_this_build_cannot_read_survives_an_edit() {
        let mut view = JsonView::from_field(r#"{"/a":{"pages":{"offset":0,"pages":2}},"/b":{"collapsed":true}}"#);
        assert_eq!(view.get("/b").and_then(|o| o.collapsed), Some(true), "the readable entry still loads");
        view.entry("/c").collapsed = Some(true);
        let back: Value = serde_json::from_str(&view.to_field()).unwrap();
        assert_eq!(back["/a"]["pages"]["offset"], 0, "written back as it was");
        assert_eq!(back["/b"]["collapsed"], true);
        assert_eq!(back["/c"]["collapsed"], true);
    }

    /// M5: an override's unknown keys, a non-object entry and text that isn't an object of overrides are all
    /// written back; positions keep their short f32 form.
    #[test]
    fn unknown_keys_and_unreadable_text_are_written_back() {
        let mut view = JsonView::from_field(r#"{"/a":{"collapsed":true,"width":3},"/x":[]}"#);
        assert_eq!(view.get("/a").and_then(|o| o.collapsed), Some(true));
        view.entry("/a").collapsed = None;
        view.entry("/b").pos = Some([0.1, 0.2]);
        assert_eq!(view.to_field(), r#"{"/a":{"width":3},"/b":{"pos":[0.1,0.2]},"/x":[]}"#);
        let opaque = JsonView::from_field("[1]");
        assert!(opaque.is_opaque() && !opaque.is_empty());
        assert_eq!(opaque.to_field(), "[1]");
        assert!(JsonView::from_field("").is_empty());
    }

    /// The keys `from_field` treats as known are exactly the ones `CardOverride` writes.
    #[test]
    fn card_override_keys_are_the_fields_it_reads() {
        let full = CardOverride { pos: Some([1.0, 2.0]), collapsed: Some(true), color: Some(1), pages: Some(2) };
        let Value::Object(object) = serde_json::to_value(&full).unwrap() else { panic!("an object") };
        assert_eq!(object.keys().map(String::as_str).collect::<Vec<_>>(), CARD_OVERRIDE_KEYS);
    }
```

- [ ] **Step 2: Write the failing `json_level` test.** Append it inside `json_level.rs`'s `mod tests`:

```rust
    /// M5: a view in a format this build can't read (a newer build's) is never overwritten by an edit here, which
    /// would replace it on every replica.
    #[test]
    fn an_unreadable_view_is_never_overwritten() {
        use crate::store::model::{field, FieldValue};
        let (mut app, j) = app_with_json("opaque-view", r#"{"a": {"b": 1}, "c": [1, 2]}"#);
        app.edit_view(&j, "[1]");
        app.switch_group_level(Some(j.clone()));
        app.apply_json_edit(JsonEdit::Toggle("/a".into()));
        // A live drag moves the card's rect frame by frame (`json_level_ui`); the refused drop must put it back.
        let a = app.json_level.as_ref().unwrap().index["/a"];
        let laid = app.json_level.as_ref().unwrap().rects[a];
        let dropped = laid.translate(egui::vec2(300.0, 0.0));
        app.json_level.as_mut().unwrap().rects[a] = dropped;
        app.apply_json_edit(JsonEdit::Moved("/a".into(), dropped.min));
        assert_eq!(app.mirror.get(&j, field::VIEW), FieldValue::Str("[1]".into()));
        assert!(app.mirror.view(&j).is_some_and(|view| view.is_opaque()));
        assert_eq!(app.json_level.as_ref().unwrap().rects[a], laid, "the refused drag snaps back to the layout");
    }
```

- [ ] **Step 3: Run the tests and see them fail.**

Run: `cargo test -j 8 -p manifold json_view`
Expected: a compile error, because `is_opaque`, `is_empty` and `CARD_OVERRIDE_KEYS` don't exist yet.

To see the behavior failures first: temporarily comment out every line that uses `is_opaque`, `is_empty` or `CARD_OVERRIDE_KEYS` (and the whole `card_override_keys_are_the_fields_it_reads` test), run once, and expect:
- `an_entry_this_build_cannot_read_survives_an_edit` to FAIL (`/b` is lost);
- `an_unreadable_view_is_never_overwritten` to FAIL (VIEW becomes `{"/a":…}`, and the card stays at the drop).

Then restore the lines.

- [ ] **Step 4: Implement `Unread` in `json_view.rs`.**
  - Change the import line to `use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};`.
  - Replace everything from `/// A Json node's view overrides by JSON Pointer` through the end of `retain_resolvable` with the code below. `clear_positions` and everything after it stay as they are.

```rust
/// The keys `CardOverride` reads; any other key of a stored override is kept as it was (`Unread::extra`).
const CARD_OVERRIDE_KEYS: [&str; 4] = ["pos", "collapsed", "color", "pages"];

/// What a view's stored text held that this build can't read (M5), kept so an edit here writes it back untouched
/// (`JsonView::to_field`) rather than wiping it on every replica: whole entries that don't read as a `CardOverride`
/// (a newer build changed a field's shape), the unknown keys of entries that do, and text that isn't an object of
/// overrides at all, which no edit may overwrite (`JsonView::is_opaque`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Unread {
    entries: BTreeMap<String, Value>,
    extra: BTreeMap<String, serde_json::Map<String, Value>>,
    opaque: Option<String>,
}

/// A Json node's view overrides by JSON Pointer: the `VIEW` field (14), and what this build couldn't read of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct JsonView(pub(crate) BTreeMap<String, CardOverride>, pub(crate) Unread);

/// No overrides: what a node without a view (`Mirror::view`'s `None`) lays out with.
pub(crate) static NO_VIEW: JsonView =
    JsonView(BTreeMap::new(), Unread { entries: BTreeMap::new(), extra: BTreeMap::new(), opaque: None });

/// One pointer's entry as `to_field` writes it: an unreadable one as it was, or an override with its unknown keys
/// after its own. Serialising `CardOverride` itself (not through `serde_json::to_value`, which widens an `f32` to
/// `f64`) keeps today's text.
#[derive(Serialize)]
#[serde(untagged)]
enum Written<'a> {
    Unread(&'a Value),
    Card(WrittenCard<'a>),
}

/// An override and its unknown keys, flattened into one object. (Not `Card`: that is this file's laid-out card.)
#[derive(Serialize)]
struct WrittenCard<'a> {
    #[serde(flatten)]
    known: Option<&'a CardOverride>,
    #[serde(flatten)]
    extra: Option<&'a serde_json::Map<String, Value>>,
}

impl JsonView {
    /// The stored field's text; empty text is no overrides. Each entry is read on its own, and what this build
    /// can't read is kept (`Unread`).
    pub(crate) fn from_field(text: &str) -> Self {
        if text.is_empty() {
            return Self::default();
        }
        let Ok(raw) = serde_json::from_str::<BTreeMap<String, Value>>(text) else {
            eprintln!("json view: the stored view is not an object of overrides; kept as is, edits to it refused");
            return JsonView(BTreeMap::new(), Unread { opaque: Some(text.to_owned()), ..Unread::default() });
        };
        let mut view = Self::default();
        for (pointer, value) in raw {
            let read = match &value {
                Value::Object(object) => CardOverride::deserialize(&value).ok().map(|known| {
                    let extra: serde_json::Map<String, Value> = object
                        .iter()
                        .filter(|(key, _)| !CARD_OVERRIDE_KEYS.contains(&key.as_str()))
                        .map(|(key, v)| (key.clone(), v.clone()))
                        .collect();
                    (known, extra)
                }),
                _ => None,
            };
            match read {
                Some((known, extra)) => {
                    if !known.is_empty() {
                        view.0.insert(pointer.clone(), known);
                    }
                    if !extra.is_empty() {
                        view.1.extra.insert(pointer, extra);
                    }
                }
                None => {
                    view.1.entries.insert(pointer, value);
                }
            }
        }
        view
    }

    /// Compact JSON of the non-empty overrides and of what this build couldn't read, written back as it was, or `""`
    /// when there is nothing. Opaque text comes back unchanged.
    pub(crate) fn to_field(&self) -> String {
        if let Some(text) = &self.1.opaque {
            return text.clone();
        }
        let mut out: BTreeMap<&str, Written> =
            self.1.entries.iter().map(|(pointer, value)| (pointer.as_str(), Written::Unread(value))).collect();
        let pointers: BTreeSet<&str> = self
            .0
            .iter()
            .filter(|(_, o)| !o.is_empty())
            .map(|(pointer, _)| pointer.as_str())
            .chain(self.1.extra.keys().map(String::as_str))
            .collect();
        for pointer in pointers {
            let known = self.0.get(pointer).filter(|o| !o.is_empty());
            out.insert(pointer, Written::Card(WrittenCard { known, extra: self.1.extra.get(pointer) }));
        }
        if out.is_empty() {
            return String::new();
        }
        serde_json::to_string(&out).unwrap_or_default()
    }

    /// No overrides and nothing unread: `Mirror::set` stores no view for it.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty() && self.1 == Unread::default()
    }

    /// Text that isn't an object of overrides (a newer format): laid out as no overrides, written back unchanged,
    /// and never edited here (`edit_json_view`).
    pub(crate) fn is_opaque(&self) -> bool {
        self.1.opaque.is_some()
    }

    pub(crate) fn get(&self, pointer: &str) -> Option<&CardOverride> {
        self.0.get(pointer)
    }

    /// The override to edit at `pointer`. An unreadable entry there goes: the user's edit replaces that card's state.
    pub(crate) fn entry(&mut self, pointer: &str) -> &mut CardOverride {
        self.1.entries.remove(pointer);
        self.0.entry(pointer.to_string()).or_default()
    }

    /// Pages revealed for the card at `pointer`, at least one.
    pub(crate) fn pages(&self, pointer: &str) -> usize {
        self.get(pointer).and_then(|o| o.pages).map_or(1, |pages| (pages as usize).max(1))
    }

    /// Drops overrides whose pointer no longer resolves in `root` (after a Refresh), and empty ones; unread entries
    /// and keys go by the same rule. Opaque text is left alone.
    pub(crate) fn retain_resolvable(&mut self, root: &Value) {
        self.0.retain(|pointer, o| !o.is_empty() && root.pointer(pointer).is_some());
        self.1.entries.retain(|pointer, _| root.pointer(pointer).is_some());
        self.1.extra.retain(|pointer, _| root.pointer(pointer).is_some());
    }
```

Notes for the implementer:
- `CardOverride::deserialize(&value)` uses serde_json's `Deserializer` impl for `&Value`. `Deserialize` is already imported.
- The `Value::Object` guard keeps a non-object entry (such as `[]`, which serde's derive would read as an all-default struct) in `entries`, rather than reading it as an empty override and losing it.
- `extra` never holds a known key, so the flattened map has no duplicate keys.
- A flattened `None` writes nothing, so a `WrittenCard` with no known part and no extra would be `{}`. `to_field` never builds one: a pointer is listed only when its override is non-empty or it has extras.
- Never fall back to `to_value`: the `[0.1,0.2]` assertion exists to catch it. If the flattened `Option` fields don't compile, report it rather than switching to `to_value`.

- [ ] **Step 5: `Mirror::set` keeps a view that has only unread state.** In `mirror.rs`, inside `if no == field::VIEW {`, change `if view.0.is_empty() {` to `if view.is_empty() {`.

- [ ] **Step 6: `edit_json_view` refuses an opaque view.** In `json_level.rs`, directly after `let mut view = self.mirror.view(&id).cloned().unwrap_or_default();`, insert:

```rust
        if view.is_opaque() {
            // M5: a newer build's format, written back untouched; an edit here would replace it on every replica.
            // Lay out from the unchanged view so a refused drag doesn't leave its card where it was dropped.
            if let Some(level) = self.json_level.as_mut() {
                level.relayout(&view);
            }
            self.queue_toast("This JSON view was saved by a newer manifold; changes to it aren't saved".into());
            return;
        }
```

- [ ] **Step 7: Run the tests and see them pass.**

Run: `cargo test -j 8 -p manifold json_view`, `cargo test -j 8 -p manifold json_level` and `cargo test -j 8 -p manifold mirror`.
Expected: all PASS, including `view_round_trips_and_stores_nothing_when_empty` with its expected text unchanged:

```text
{"/users":{"pages":2},"/users/0":{"pos":[520.0,20.0],"collapsed":true}}
```

Then run `cargo test -j 8 -p manifold fetched` (the Refresh prune path).

- [ ] **Step 8: The older spec.** In `docs/superpowers/specs/2026-09-27-json-node-rework-design.md`, section "Saved view state", directly after the bullet that begins "Entries whose pointer no longer resolves (after a Refresh) are dropped", add the bullet:

  "- What this build can't read is kept and written back as it was: an entry that isn't an override, an override's unknown keys, and text that isn't an object of overrides at all (which is then never edited here; an edit is refused with a toast). Wave 1, M5."

- [ ] **Step 9: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/json_view.rs crates/manifold/src/json_level.rs crates/manifold/src/mirror.rs docs/superpowers/specs/2026-09-27-json-node-rework-design.md
git commit -m "Wave 1 (M5): a Json view keeps what this build can't read" -m "Unreadable entries and unknown keys are written back as they were; text that is not an object of overrides is kept and never edited." -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 7: H4. One unreadable setting resets only itself; zoom is clamped

**Model:** haiku. **Gate alone:** full (settings load and save, read at every start; not UI code). **Proof:** `an_unknown_value_resets_only_its_own_setting` fails first (`from_str` errs on `"Kagi"`), then passes.

**Files:**
- Modify: `crates/manifold/src/settings.rs`:
  - imports (line 3)
  - three field attributes in `Settings` (about 57-65)
  - new functions after `impl Default for Settings`
  - tests (230+)
- Modify: `crates/manifold/src/shortcuts.rs:299` and `:310`
- Modify: `crates/manifold/src/settings_page.rs:156` and `:160`
- Modify: `README.md:275` (the Settings paragraph)

**Interfaces:**
- Consumes:
  - `EngineKind::from_name(&str) -> Option<EngineKind>` (case-insensitive)
  - `crate::process_manager::engine_kind_serde::serialize`
- Produces: `pub(crate) const ZOOM_RANGE: std::ops::RangeInclusive<f64>` in `settings.rs`.

- [ ] **Step 1: Write the failing tests.** Add them to `settings.rs`'s tests module:

```rust
    /// H4: a value this build can't read resets that setting alone, not the whole file.
    #[test]
    fn an_unknown_value_resets_only_its_own_setting() {
        let s: Settings = serde_json::from_str(
            r#"{"home_page":"https://a.test","default_search_engine":"Kagi",
                "default_starting_engine":"chrome","zoom_factor":0,"cef_directory":"D:/cef"}"#,
        )
        .unwrap();
        assert_eq!(s.home_page, "https://a.test");
        assert_eq!(s.cef_directory.as_deref(), Some("D:/cef"));
        assert_eq!(s.default_search_engine, SearchEngine::DuckDuckGo);
        assert_eq!(s.default_starting_engine, Settings::default().default_starting_engine);
        assert_eq!(s.zoom_factor, 0.5, "clamped into the range the UI steps through");
    }

    /// H4: zoom loads clamped, a wrongly typed zoom is the default, and the defaults round-trip.
    #[test]
    fn zoom_is_clamped_and_a_wrong_type_is_its_default() {
        let zoom = |text: &str| serde_json::from_str::<Settings>(text).unwrap().zoom_factor;
        assert_eq!(zoom(r#"{"zoom_factor":9}"#), 2.0);
        assert_eq!(zoom(r#"{"zoom_factor":"1.2"}"#), 1.0);
        assert_eq!(zoom(r#"{"zoom_factor":1.3}"#), 1.3);
        let defaults = Settings::default();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&defaults).unwrap()).unwrap();
        assert_eq!(back.default_search_engine, defaults.default_search_engine);
        assert_eq!(back.default_starting_engine, defaults.default_starting_engine);
        assert_eq!(back.zoom_factor, defaults.zoom_factor);
    }
```

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo test -j 8 -p manifold an_unknown_value_resets_only_its_own_setting`
Expected: FAIL, panicking at `.unwrap()` with "unknown variant `Kagi`".

`zoom_is_clamped_and_a_wrong_type_is_its_default` also fails: `zoom_factor` 9 loads as 9.

- [ ] **Step 3: Add the lenient deserializers.** In `settings.rs`:
  - Under `use crate::process_manager::{engine_kind_serde, EngineKind};`, add `use serde::Deserialize as _;`.
  - After the closing `}` of `impl Default for Settings`, add:

```rust
/// The page zoom the UI steps through (`zoom_in_shortcut`, `zoom_out_shortcut`, the settings page's buttons); a
/// stored value outside it loads clamped into it.
pub(crate) const ZOOM_RANGE: std::ops::RangeInclusive<f64> = 0.5..=2.0;

/// A value this build can't read (a newer build's name, a hand edit) loads as that setting's default, logged, rather
/// than failing the whole file, which `load_or_move_aside` would set aside with every other setting (H4).
fn lenient<'de, D: serde::Deserializer<'de>, T>(
    d: D,
    what: &str,
    read: impl FnOnce(&serde_json::Value) -> Option<T>,
    default: impl FnOnce() -> T,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(read(&value).unwrap_or_else(|| {
        eprintln!("settings: unreadable {what} {value}; using the default");
        default()
    }))
}

fn search_engine<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SearchEngine, D::Error> {
    lenient(d, "default_search_engine", |v| SearchEngine::deserialize(v).ok(), || {
        Settings::default().default_search_engine
    })
}

fn starting_engine<'de, D: serde::Deserializer<'de>>(d: D) -> Result<EngineKind, D::Error> {
    lenient(d, "default_starting_engine", |v| v.as_str().and_then(EngineKind::from_name), || {
        Settings::default().default_starting_engine
    })
}

fn zoom<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    lenient(d, "zoom_factor", |v| v.as_f64().map(|z| z.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end())), || 1.0)
}
```

- [ ] **Step 4: Put them on the fields.** In `pub struct Settings`:
  - Change `pub default_search_engine: SearchEngine,` to:

```rust
    #[serde(deserialize_with = "search_engine")]
    pub default_search_engine: SearchEngine,
```

  - Change `#[serde(with = "engine_kind_serde")]` above `pub default_starting_engine: EngineKind,` to:

```rust
    #[serde(serialize_with = "engine_kind_serde::serialize", deserialize_with = "starting_engine")]
```

  - Above `pub zoom_factor: f64,` (after its doc comment), add `#[serde(deserialize_with = "zoom")]`.

- [ ] **Step 5: Use `ZOOM_RANGE` for the UI bounds.**
  - `shortcuts.rs` `zoom_in_shortcut`: `(self.focused_zoom_factor() + 0.1).min(2.0)` becomes `(self.focused_zoom_factor() + 0.1).min(*crate::settings::ZOOM_RANGE.end())`.
  - `shortcuts.rs` `zoom_out_shortcut`: `(self.focused_zoom_factor() - 0.1).max(0.5)` becomes `(self.focused_zoom_factor() - 0.1).max(*crate::settings::ZOOM_RANGE.start())`.
  - `settings_page.rs`, the MINUS button: `.max(0.5)` becomes `.max(*crate::settings::ZOOM_RANGE.start())`.
  - `settings_page.rs`, the PLUS button: `.min(2.0)` becomes `.min(*crate::settings::ZOOM_RANGE.end())`.
  - Then `grep -n "min(2.0)\|max(0.5)" crates/manifold/src/shortcuts.rs crates/manifold/src/settings_page.rs` must find nothing.

- [ ] **Step 6: README.** In `README.md`'s Settings paragraph (line 275), replace "a missing or corrupt file just falls back to defaults rather than failing to start." with:

  "a missing file falls back to defaults. A value this build can't read (an unknown engine name, a zoom that isn't a number) resets only that setting, with a log line, and a zoom outside 50–200% loads clamped into it. A file that can't be read at all (not JSON, or another field of the wrong type) is moved aside as `settings.unreadable-<secs>.json` with a toast, and defaults load rather than failing to start."

- [ ] **Step 7: Run the tests and see them pass.**

Run: `cargo test -j 8 -p manifold settings`
Expected: all PASS, including the existing conflict, `cef_directory` and starting-engine tests.

- [ ] **Step 8: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/settings.rs crates/manifold/src/shortcuts.rs crates/manifold/src/settings_page.rs README.md
git commit -m "Wave 1 (H4): one unreadable setting resets only itself; zoom is clamped" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 8: M3. A cookie dump that doesn't parse is logged, without values

**Model:** haiku. **Gate alone:** full (the jar store and the engine cookie pull). **Proof:** the new test fails first to compile (`unwrap_err` on an `Option`), then passes.

**Files:**
- Modify: `crates/manifold/src/jar.rs:122-125` (`from_json`), `:1046` (existing test), tests module (append)
- Modify: `crates/manifold/src/tabs.rs:178-187` (`apply_pulled_cookies` doc and its parse line)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn from_json(json: &str) -> Result<Vec<Cookie>, String>` in `jar.rs`. Callers:
  - `tabs.rs:187`, changed here.
  - `main.rs:3239` (`jar::from_json(SID).unwrap()`), which stays valid on `Result`.

- [ ] **Step 1: Write the failing test.** Add it to `jar.rs`'s tests module:

```rust
    /// M3: a dump that doesn't parse says where, never what. serde's own message would quote the input ("unknown
    /// variant `SECRET`"), and a cookie value is a credential. It stays all or nothing: a cookie skipped from a pull
    /// would read as a logout and be deleted from the jar.
    #[test]
    fn a_bad_dump_reports_where_without_echoing_a_value() {
        let good = r#"{"name":"a","value":"b","domain":"x.com","path":"/","expires":null,"secure":true,"http_only":false,"same_site":"Strict"}"#;
        let bad = r#"{"name":"sid","value":"SECRET","domain":"a","path":"/","expires":null,"secure":true,"http_only":true,"same_site":"SECRET"}"#;
        let err = from_json(&format!("[{bad}]")).unwrap_err();
        assert!(err.contains("line 1"), "{err}");
        assert!(!err.contains("SECRET"), "{err}");
        assert!(from_json(&format!("[{good},{bad}]")).is_err(), "all or nothing");
        assert_eq!(from_json(&format!("[{good}]")).map(|c| c.len()), Ok(1));
    }
```

Also change the existing `assert!(from_json("nope").is_none());` in `engine_json_parses_into_cookies` to `assert!(from_json("nope").is_err());`.

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo test -j 8 -p manifold a_bad_dump_reports_where_without_echoing_a_value`
Expected: a compile error, because `unwrap_err` and `is_err` don't exist on `Option<Vec<Cookie>>`.

- [ ] **Step 3: Change `from_json`.** Replace it, with its doc comment, in `jar.rs`:

```rust
/// An engine's cookie dump, as its host serialised it. The error gives the position only: serde's message can quote
/// the input, and a cookie value is a credential.
pub fn from_json(json: &str) -> Result<Vec<Cookie>, String> {
    serde_json::from_str(json).map_err(|e| format!("{:?} error at line {} column {}", e.classify(), e.line(), e.column()))
}
```

- [ ] **Step 4: Log it in `apply_pulled_cookies` (`tabs.rs`).**
  - In the doc comment, change "`false` for a dump that does not parse," to "`false` for a dump that does not parse (logged),".
  - Replace `let Some(pulled) = crate::jar::from_json(json) else { return false };` with:

```rust
        let pulled = match crate::jar::from_json(json) {
            Ok(pulled) => pulled,
            Err(err) => {
                // All or nothing, never per cookie: a cookie missing from a pull is a logout (`jar::Jar::merge`), so
                // skipping one that doesn't parse would delete a live login. Logged, as the reason this tab's
                // logins stop reaching its jar.
                eprintln!("jar: tab {tab_id}'s cookie dump didn't parse ({err}); its jar is not updated");
                return false;
            }
        };
```

- [ ] **Step 5: Run the tests and see them pass.**

Run: `cargo test -j 8 -p manifold a_bad_dump_reports_where_without_echoing_a_value`, then `cargo test -j 8 -p manifold jar`, then `cargo test -j 8 -p manifold cookie`.
Expected: all PASS.

- [ ] **Step 6: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/jar.rs crates/manifold/src/tabs.rs
git commit -m "Wave 1 (M3): a cookie dump that doesn't parse is logged, without values" -m "The pull stays all or nothing: a skipped cookie would read as a logout." -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 9: M4. An unknown argument or a flag missing its value refuses to start

**Model:** haiku. **Gate alone:** full (startup, and the session tools `--dump` / `--check` / `--import-json`). The gate's probes launch the exe with `--headless` only, so Step 8 runs the other launch forms by hand. **Proof:** the new test fails first (every case returns `None`), then passes, and Step 8's launches exit as expected.

**Files:**
- Modify: `crates/manifold/src/cli.rs:14-35` (`FLAGS`, `validate`, first line of `run`), tests module (91+)
- Modify: `crates/manifold/src/profile.rs:111-115`, `:125` (comments only)
- Modify: `README.md:358` (the `--script` / `--profile-dir` paragraph)

**Interfaces:**
- Consumes: nothing.
- Produces: `fn validate(args: &[String]) -> Result<(), String>`, private in `cli.rs`. `cli::run`'s signature is unchanged.

- [ ] **Step 1: Write the failing test.** Add it to `cli.rs`'s tests module:

```rust
    /// M4: anything this build doesn't read refuses to start, rather than opening the GUI on the real profile.
    #[test]
    fn an_unknown_flag_or_a_missing_value_refuses_to_start() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(run(&args(&["--hedless"])), Some(2));
        assert_eq!(run(&args(&["--headless", "--profile-dir"])), Some(2));
        assert_eq!(run(&args(&["--profile-dir", "--headless"])), Some(2));
        assert_eq!(run(&args(&["--profile-directory", "X"])), Some(2));
        assert_eq!(run(&args(&["--profile-dir", ""])), Some(2));
        assert_eq!(run(&args(&["--profile-dir="])), Some(2));
        assert_eq!(run(&args(&["some-file.html"])), Some(2));
        assert_eq!(run(&args(&["--headless", "--profile-dir", "P", "--script", "s.txt"])), None);
        assert_eq!(run(&args(&["--profile-dir=P", "--debug-commands"])), None);
        assert_eq!(run(&args(&[])), None);
    }
```

The valid `--import-json … --out …` form is already covered by `import_json_builds_a_clean_file_that_dumps_back`, which must keep passing.

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo test -j 8 -p manifold an_unknown_flag_or_a_missing_value_refuses_to_start`
Expected: FAIL with `left: None, right: Some(2)` on the first assertion.

- [ ] **Step 3: Add `FLAGS` and `validate`, and call it first.** In `cli.rs`:
  - Above `/// Runs the tool the arguments name`, add:

```rust
/// Every flag this build reads, and whether it takes a value (`--profile-dir` also as `--profile-dir=<dir>`). The
/// readers: `run` below, `main` (`--headless`, `--debug-commands`), `profile::dir_from` and `debug_control`
/// (`--script`). A flag added to any of them goes here too.
const FLAGS: [(&str, bool); 8] = [
    ("--headless", false),
    ("--debug-commands", false),
    ("--profile-dir", true),
    ("--script", true),
    ("--dump", true),
    ("--check", true),
    ("--import-json", true),
    ("--out", true),
];

/// An unknown argument, or a flag missing its value (absent, empty, or another flag), is refused: run on, it would
/// open the GUI on the real profile (`--hedless`, `--profile-directory X`, a bare `--profile-dir`).
fn validate(args: &[String]) -> Result<(), String> {
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg.strip_prefix("--profile-dir=").is_some_and(|dir| !dir.is_empty()) {
            continue;
        }
        let Some(&(_, takes_value)) = FLAGS.iter().find(|(flag, _)| *flag == arg.as_str()) else {
            return Err(format!("unknown argument {arg:?}"));
        };
        if takes_value && !rest.next().is_some_and(|value| !value.is_empty() && !value.starts_with("--")) {
            return Err(format!("{arg} needs a value"));
        }
    }
    Ok(())
}
```

  - Make the first lines of `run`'s body:

```rust
    if let Err(err) = validate(args) {
        eprintln!("manifold: {err}");
        return Some(2);
    }
```

  - Then update `run`'s doc comment to: `/// Runs the tool the arguments name: `None` when they name none (the app runs), `Some(2)` when they can't be read.`

- [ ] **Step 4: Note it in `profile.rs`'s tests.**
  - In `profile_dir_accepts_the_equals_form`, above the `--profile-directory` assertion, add `// \`cli::validate\` refuses this before \`profile::dir\` is read (M4); \`dir_from\` alone still ignores it.`
  - In `profile_dir_flag_beats_appdata`, above the bare `--profile-dir` assertion, add `// \`cli::validate\` refuses a bare \`--profile-dir\` first (M4); this is \`dir_from\`'s own fallback.`
  - Change no assertion.

- [ ] **Step 5: README.** At the end of the paragraph that begins "`--script <file>` reads the commands from a file instead of stdin" (line 358), append:

  "Any other argument, or a flag missing its value (a bare `--profile-dir`, an empty one, or one followed by another flag), is refused: manifold prints `manifold: <why>` and exits 2, rather than running on the real profile."

- [ ] **Step 6: Check every script's flags are listed.**

Run: `grep -rhoE -- '--(headless|debug-commands|profile-dir|script|dump|check|import-json|out)\b' scripts | sort | uniq -c`, then `grep -rn 'manifold.exe\|"\$exe"' scripts | grep -oE -- ' --[a-z][a-z-]*' | sort -u`.
Expected: every flag the second command prints is in `FLAGS`, apart from `--pid`, which belongs to `tail`. If any other flag appears, add it to `FLAGS` and say so in the report.

The second grep misses argument lists that aren't space-separated after the exe. Run `grep -rn -- '-ExeArgs\|\.Arguments' scripts` and read each hit's flags by eye. At `c8b7485` there are two: `json-pan-profile.sh` (`--headless,--profile-dir,…,--script,…`, comma-separated) and `foreground-watch.ps1` (`--debug-commands` or `--headless`). Both use only listed flags.

- [ ] **Step 7: Run the tests and see them pass.**

Run: `cargo test -j 8 -p manifold cli`, then `cargo test -j 8 -p manifold profile`.
Expected: all PASS.

- [ ] **Step 8: Launch the exe once in each launcher's form.** The gate can't catch a launcher this change breaks: `two-instance-test.sh` and `headless-smoke-test.sh` start the exe with `--headless` alone. So run each other form by hand from the repo root, after `cargo build -j 8 -p manifold`, in a temp profile. Every run is headless, quits on its own (`QUIT`, or stdin EOF), and never touches the real `%APPDATA%\manifold`: `drive.sh` sets its own `APPDATA`, and the others pass `--profile-dir` or a file path. Each takes a few seconds.

```bash
T="$(mktemp -d)"; mkdir -p "$T/p" "$T/bad"
printf '%s' '{"nodes":[{"id":"g","type":"application/vnd.manifold.group","name":"G"}]}' > "$T/session.json"
printf 'QUIT\n' > "$T/quit.txt"
w() { cygpath -w "$1"; }
# drive.sh and the probes: --import-json … --out …, then --headless (drive.sh runs both with SEED).
SEED="$T" scripts/drive.sh "$T/quit.txt" </dev/null; echo "drive exit=$?"
# json-pan-profile.sh's argument list, on the debug exe (that script itself needs the release exe and the Samply task).
target/debug/manifold.exe --headless --profile-dir "$(w "$T/p")" --script "$(w "$T/quit.txt")" </dev/null; echo "script exit=$?"
# the session tools
target/debug/manifold.exe --import-json "$(w "$T/session.json")" --out "$(w "$T/s.mnf")"; echo "import exit=$?"
target/debug/manifold.exe --dump "$(w "$T/s.mnf")" >/dev/null; echo "dump exit=$?"
target/debug/manifold.exe --check "$(w "$T/s.mnf")"; echo "check exit=$?"
# one refused form: headless with its own profile, so a broken check would only run and quit on EOF, not open a window
target/debug/manifold.exe --headless --profile-dir "$(w "$T/bad")" --hedless </dev/null; echo "refused exit=$?"
rm -rf "$T"
```

Expected: `drive exit=0`, `script exit=0`, `import exit=0`, `dump exit=0`, `check exit=0`, and `refused exit=2` after the line `manifold: unknown argument "--hedless"`. Then check `Get-Process manifold -ErrorAction SilentlyContinue` lists nothing of yours.

`--debug-commands` is not launched here: it opens a real window over the user's work. The unit test covers it (`--profile-dir=P --debug-commands` passes), and Step 6 shows its launchers (`drive.sh` with `VISIBLE=1`, `foreground-watch.ps1 -Visible`) pass it alone.

- [ ] **Step 9: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/cli.rs crates/manifold/src/profile.rs README.md
git commit -m "Wave 1 (M4): an unknown argument or a flag missing its value refuses to start" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 10: G2. Guard test: `Carried` holds every tab field

**Model:** haiku. **Gate alone:** full by CLAUDE.md's letter (it edits `mirror.rs`), though it adds only a test, so the gate runs no changed code. **Proof:** the test passes today. It is a guard against a future tab field missing from `Carried`.

**Files:**
- Test: `crates/manifold/src/mirror.rs` tests module (append; Task 6 already edited `Mirror::set` in this file)

**Interfaces:**
- Consumes:
  - `Mirror::{set, get, set_parent, set_group, parent, group, carried, seed}`
  - `Kind::fields(self) -> &'static [u32]`
  - `field_def(no) -> Option<&'static FieldDef>`, where `FieldDef.ty: FieldType` (`Copy`)
  - `FieldValue`, `NodeId`, `ActorId`
- Produces: nothing.

- [ ] **Step 1: Write the guard test.** Append it inside `mirror.rs`'s `mod tests`:

```rust
    /// G2: every field a tab carries (`Kind::Tab.fields()`) survives `carried` → `seed` onto a fresh entry. A tab
    /// field added to `TYPES` but not to `Carried` fails here, rather than going unseeded on a new spawn's `Create`.
    #[test]
    fn carried_holds_every_tab_field() {
        use crate::store::model::{ActorId, FieldType};
        let sample = |ty: FieldType| match ty {
            FieldType::Pos => FieldValue::Pos(1.5, -2.0),
            FieldType::Color => FieldValue::Color(Some(0x123456)),
            FieldType::Id => FieldValue::Id(Some(NodeId { actor: ActorId(1), counter: 2 })),
            FieldType::Str => FieldValue::Str("s".into()),
            FieldType::Bool => FieldValue::Bool(true),
            FieldType::Engine => FieldValue::Engine(3),
        };
        let fields: Vec<u32> = Kind::Tab.fields().iter().copied().filter(|&no| no != field::PARENT).collect();
        assert!(!fields.is_empty());
        let mut a = Mirror::default();
        for &no in &fields {
            a.set("a", no, sample(field_def(no).unwrap().ty));
        }
        a.set_parent("a", Some("p".into()));
        a.set_group("a", Some("g".into()));
        let mut b = Mirror::default();
        b.seed("b", &a.carried("a"));
        for &no in &fields {
            assert_eq!(b.get("b", no), a.get("a", no), "field {no} is not carried");
        }
        assert_eq!((b.parent("b"), b.group("b")), (Some("p"), Some("g")));
    }
```

- [ ] **Step 2: Run it.**

Run: `cargo test -j 8 -p manifold carried_holds_every_tab_field`
Expected: PASS.

If it fails, do not change `Carried` or the test to make it pass. Report which field differs: that would be a real G2 drift, and the controller decides.

- [ ] **Step 3: fmt, clippy, commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
git add crates/manifold/src/mirror.rs
git commit -m "Wave 1 (G2): guard test, Carried holds every tab field" -m "Co-Authored-By: <implementer's own line>"
```

---

### Task 11: Wave gate, docs, and the merge question

**Model:** controller (the session running the plan). **Gate:** **full**, once for the whole wave. **Proof:**
- the full test suite passes;
- `scripts/gate.sh` reports every run `exit=0` and the smoke test `fail=0`;
- CPU load and Defender state are reported with it.

**Files:**
- Modify: `docs/superpowers/specs/2026-10-07-wave-1-integrity-design.md` (Status line)
- Modify: `docs/superpowers/plans/2026-10-07-wave-1-integrity.md` (fill in the `## Outcome` section)

**Interfaces:**
- Consumes: the commits of Tasks 1-10 on `wave-1-integrity`.
- Produces: a branch ready to merge, and an Outcome record.

- [ ] **Step 1: Run the whole suite.**

Run: `cargo test -j 8 -p manifold`
Expected: all pass. Record the count.

Run clippy once more, with the CLAUDE.md line.

- [ ] **Step 2: Prechecks.**
  - `df -h /c`
  - `Get-Process python, manifold -ErrorAction SilentlyContinue` (no leftovers)
  - `(Get-MpComputerStatus).RealTimeProtectionEnabled`
  - `(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue`

- [ ] **Step 3: Build, then run the full gate detached.**

```bash
cargo build -j 8 -p manifold
cmd //c start "" //b //belownormal bash scripts/gate.sh "<scratchpad>/wave1-gate.log" full
```

`start //b` returns at once. Watch `<scratchpad>/wave1-gate.log` for the `GATE done` line (Monitor with an until-loop), not the launcher. It takes about 17 minutes on a quiet machine, and up to 28 under load.

Expected:
- `GATE two-instance run 1..3 exit=0`;
- `GATE smoke exit=0 … fail=0`.

On a failure, read `<log>.two-N.log` or `<log>.smoke.log`. Under load, rerun the failed part once before calling it a regression, and name what else was running.

- [ ] **Step 4: Docs.**
  - Set the spec's Status to "Implemented 2026-10-0x on `wave-1-integrity` (commits …); see the plan's Outcome for deviations."
  - Fill in `## Outcome` below:
    - a table of tasks, commits and review findings;
    - the test count;
    - the gate result with CPU load and Defender state;
    - deviations from this plan;
    - what stays parked.
  - Commit "Wave 1: outcome and spec status" with the controller's own `Co-Authored-By:` line.

- [ ] **Step 5: Ask before merging.** Report the gate result and ask the user whether to merge `wave-1-integrity` into `main`. Don't merge or tag without that go-ahead. Afterwards, check `Get-Process python, manifold` for leftovers.

---

## Self-review against the spec

| Spec item | Where it is covered |
|---|---|
| Decision 1: ten item commits, in order | Tasks 1-10 |
| Decision 2: one branch, per-task tests and clippy, one full gate, merge on go-ahead | Global Constraints; each task's last step; Task 11 |
| Decision 2: M4's launch forms by hand (the gate uses `--headless` only) | Task 9 Step 8 |
| Decision 4: M3 stays all or nothing | Decision 8; Task 8's comment and its "all or nothing" assertion |
| Decision 5: tests fail first, no real profile | Each task's "see it fail" step; Global Constraints |
| L4 text | Task 1 |
| L1 live-only centroid and grouping | Task 2 |
| P1 `remove_tab` for tab members, one save, stale comment | Task 3 Step 3 |
| C1 `forget_id` contents and exclusions | Task 4 Step 4 (body and doc) |
| C1 callers: `drop_tab`, `drop_node`, `dissolve_members`, `merge_groups`, teardown with `spawning` | Task 4 Steps 5-9 |
| C1 `forget_removed_group` deleted (deviation) | Task 4 Step 7, Decision 3 |
| C1 `remove_remotely_deleted` keeps `wakes_in_flight`, drops redundant `selected_tabs` | Task 4 Step 8 |
| C1 table test, source scan (with `Vec<String`), teardown test | Task 4 Steps 1-2 |
| C1 GPU frees by review | Task 4 Step 4 doc; spec Testing |
| H1 `end_stale_selection` and its per-frame call | Task 5 Steps 3-4 |
| H1 behavior test through a real frame | Task 5 Step 1, Decision 5 |
| M5 `Unread` (entries, extra, opaque text) | Task 6 Step 4 |
| M5 `to_field` keeps today's text (no `to_value`) | Decision 6; Task 6 Step 4 notes; Step 7 |
| M5 `entry`, `retain_resolvable`, `is_empty`, `Mirror::set` | Task 6 Steps 4-5 |
| M5 opaque refused, relaid out, with a toast (deviation: every refused edit) | Task 6 Step 6, Decision 7 |
| M5 Relayout keeps positions in unread entries; log for opaque text only | Task 6 Step 4 code (`clear_positions` unchanged; one `eprintln!`) |
| M5 older spec updated | Task 6 Step 8 |
| C1 exclusions `idle_clock`, `pending_new_node`, `card_hint` | Nothing to do: the scan doesn't see them, and `forget_id` doesn't touch them |
| H1 and headless scripts | Nothing to do: no debug command selects by id (checked at `c8b7485`) |
| Decision 1: docs committed first, outcome committed last | Global Constraints "Branch and commits"; Task 11 Step 4 |
| M5 four tests, line 551 updated | Task 6 Steps 1-2 |
| H4 lenient fields, `ZOOM_RANGE`, UI literals, README | Task 7 Steps 3-6 |
| M3 `Result`, position-only error, log, callers | Task 8 Steps 3-4 |
| M4 `FLAGS`, `validate` (empty value too), `run` first, profile notes, README | Task 9 Steps 3-5 |
| M4 scripts' flags | Task 9 Step 6 (both greps and the `-ExeArgs` / `.Arguments` read), Decision 9 |
| G2 guard | Task 10 |
| Not in scope / parked | Spec section; Task 11 Outcome repeats what stays parked |
| Risks: C1 ordering and lifecycle tables | Task 4 Steps 5-6 keep the order; `NOT_FORGOTTEN` reasons; Step 10's rule |
| Risks: P1 waking tab | Task 3 Step 4 runs `session_sync` |
| Risks: M4 launchers | Task 9 Step 6 (flags) and Step 8 (each launch form run by hand); the gate doesn't cover them |
| Decision 2: per-task gate labels from CLAUDE.md | Per-task summary and each task's header |

Placeholder scan: every code step has its code. The only `<…>` left are the implementer's own `Co-Authored-By:` line, the scratchpad path and the commit list in Task 11's Status line, each filled in at run time.

Type consistency:
- `forget_id(&mut self, id: &str)`: Task 4, used in Tasks 4-5.
- `end_stale_selection(&mut self)`: Task 5.
- `JsonView::{is_empty, is_opaque}`: Task 6, used in `mirror.rs` and `json_level.rs`.
- `ZOOM_RANGE`: Task 7.
- `from_json -> Result<Vec<Cookie>, String>`: Task 8.
- `validate(&[String]) -> Result<(), String>`: Task 9.

---

## Outcome

Implemented 2026-10-08 on `wave-1-integrity`, base `99af4ec`.

| Task | Item | Commit |
|---|---|---|
| 1 | L4 `end_stale_drag` doc comment | `9f16f93` |
| 2 | L1 Group Selected centres on live cards | `bb270d7` |
| 3 | P1 closing a group commits once | `2308875` |
| 4 | C1 `forget_id`, texture leak on session replace | `b650ed9` |
| 5 | H1 `end_stale_selection` | `cd9e08d` |
| 6 | M5 Json `Unread` part | `b04003f` |
| 7 | H4 lenient settings, zoom clamp | `d82bed4`, `9ddbfed` |
| 8 | M3 `jar::from_json` returns `Result` | `b4a1712` |
| 9 | M4 CLI `validate()` | `f1ec3ed` |
| 10 | G2 `carried_holds_every_tab_field` | `714343b` |
| gate tooling | `WAITIDLE`; scripts wait on conditions | `8575d45`, `f8f5d8b` |

Each task passed a per-task review; the whole-branch Opus review said "Ready" with four Minors, applied in the docs commit.

**Tests:** 1013 passed, 5 ignored (`manifold`), 44 (`ipc-protocol`), 9 (engine-webview2).

**Gate (full, 2026-10-08 13:15-13:21):** three two-instance runs and the smoke test all passed (48 PASS, 0 FAIL), 5.5 minutes. CPU 16-30%, Defender real-time protection off; GPU users and leases not checked. An earlier gate on the same code (before the timing fix) failed scenarios 5 and 7 and lost the eGPU during the smoke test, under heavy load from other sessions; that is why the scripts now wait on conditions (`WAITIDLE`, the page server's request log) instead of fixed sleeps. Scenarios 3-5 keep fixed windows only where they assert that nothing happens.

**Deviations:**
- Task 7 also deleted the now-dead `engine_kind_serde::deserialize` (reviewer grepped all callers), and `9ddbfed` fixed its stale doc.
- Task 8's brief cited `main.rs:3239`; the real caller is `main.rs:3397`.
- Spec Decision 1 (spec and plan on `main` as `32ad186`) was already satisfied before the branch.
- Gate tooling (`WAITIDLE`) was added to the branch because the gate could not pass without it.

**Parked:**
- Task 4: `ID_SHAPES` misses `BTreeSet` and tuple-keyed maps; the scan checks fields, not call sites; the teardown test lacks spawning-id and GPU coverage; `forget_id` could note `cancel_held` ordering.
- Task 6: an entry that partly fails to read is dropped whole on edit; the toast repeats on refused drag edits.
- Task 9: `FLAGS` is kept in step by hand; a value starting with `--` is refused.
- Task 10: the G2 guard covers `Kind::Tab` only.
- `destroy_group_members` bypasses `destroy_tab` (note for the P1 audit).
- Hand-testing of the M4 launch forms.
