# Failed spawns become cards; the "No tabs." screen goes: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A failed Root or New tab spawn leaves a persisted failed card instead of an error line; the empty Graph view renders its canvas with a faint hint instead of the "No tabs." screen; failure toasts coalesce by cause.

**Architecture:** `handle_tab_spawn_result`'s existing failed-card branch (today for Respawn, Restore and Wake) takes Root and New too; only Split still toasts. `tab_spawn_error` and its toolbar display are deleted. Same-intent failed cards (same url, engine, group) are deduplicated, and a successful spawn of a url clears that url's failed cards through `destroy_node`. Failure toasts go through a small pure buffer (`FailureNotices`) that merges same-cause notices inside a window. The empty branch in `build_graph_ui` becomes a painted hint.

**Tech Stack:** Rust, egui; tests are unit tests in the `manifold` bin (`cargo test -j 8 -p manifold --bins`), live checks are bash probes under `scripts/`.

**Spec:** `docs/superpowers/specs/2026-10-06-failed-spawn-cards-design.md` (decisions 1-7; the parked list there stays parked).

## Global Constraints

- Work on `dev`, never `main`; do not push (CLAUDE.md "Branches").
- `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass; the `.githooks/pre-commit` hook runs both (can take over 10 minutes: commit in the background and poll). Never `--no-verify`. Do not silence a lint to pass it.
- No test may read or write the real `%APPDATA%\manifold`; use `profile::dir()` (None under `cfg(test)`) and `store::testutil::temp_dir`.
- Nodes enter and leave the graph only through the 2b-5 funnels: `queue_create`/`queue_delete`, `place_node`, `spawn_tab_async`. `check_graph_matches_document` (debug) asserts it after every save.
- New code reads `Mirror::ty` / `card_kind`, never a new `match` on store `Kind` (2b-6 Ruling 1).
- Python only via a script file: `python -P script.py < /dev/null`. Quote paths with spaces. Commands over 120 s run in the background and are polled. Run `cargo build -j 8 -p manifold` before any probe script (they run the existing exe; cargo test and check do not rebuild it).
- Every commit message ends with the trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` (use the implementer's own model name if different).

## File structure

- `crates/manifold/src/tabs.rs`: `handle_tab_spawn_result` (the failed-card branch, dedup, auto-clear), `spawn_root_tab`, `spawn_new_tab_with_kind`, the group-spawn path (the three `tab_spawn_error = None` clears).
- `crates/manifold/src/failure_toasts.rs` (new): `FailureNotices`, the pure coalescing buffer and its tests.
- `crates/manifold/src/main.rs`: delete the `tab_spawn_error` field and its init; add the `failure_notices` field; `queue_failure_toast`, the flush call in `build_save_toast`.
- `crates/manifold/src/graph_ui.rs`: the empty branch becomes the hint; delete `toolbar_spawn_error` and its call.
- `crates/manifold/src/session_sync/tests.rs`: rewrite the two `tab_spawn_error` assertions and the 2b-5 test around them; new tests.
- `scripts/`, `README.md`: probe and doc updates (Task 4).

## Facts the tasks rely on (verified 2026-10-06)

- A new tab is created in the document at `spawn_tab_async` time (`queue_create`, plus a `spawning` snapshot). Today a failed Root/New/Split spawn then `queue_delete`s it (`tabs.rs` after the `tab_spawn_error` set, "Ruling 4"); the 2b-5 test `a_failed_new_spawn_is_deleted_if_its_create_committed` pins that. This plan reverses it for Root and New (spec decision 2); Split keeps it.
- The failed-card branch (`tabs.rs`, `if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Restore | SpawnPurpose::Wake)`) inserts `Runtime::Unloaded(UnloadedTabPayload { url, title, muted, zoom_factor, state: UnloadedState::Failed(String) })`, calls `seed_landing`, pushes `node_order`, `apply_pending_local`, `land_remote_in_flight`, `save_session`, and queues a toast.
- `SpawnPurpose::Root` success sets `focused`, and `view_mode = ViewMode::Graph` (the app starts in `ViewMode::Item`). A failed Root card must therefore set `view_mode = ViewMode::Graph` itself.
- `destroy_node(id)` (`main.rs`) removes a node with full cleanup and records nothing in Recently Closed; `close_node` records first. Auto-clear uses `destroy_node`.
- `Mirror::engine(id) -> EngineKind`, `Mirror::group(id) -> Option<&str>`; `GraphNode::as_unloaded() -> Option<&UnloadedTabPayload>`.
- Tests inject a spawn failure with `fail_spawns(&mut app)` (`session_sync/tests.rs`, spawns parked in `test_spawns` fail with `"no engine"`); `app_on(path, n)` makes an app on a journal; `live_in_doc(&app, &id)`; `spawn_tab_at(url, pos, group, parent)` starts a New spawn.
- `queue_toast(message)` pushes onto `save_toast_queue: VecDeque<String>`; `build_save_toast(ui)` pops one when none is showing (called every frame in every view).

---

### Task 1: Failed Root/New spawns become cards; `tab_spawn_error` goes

**Files:**
- Modify: `crates/manifold/src/tabs.rs` (`handle_tab_spawn_result`, `spawn_root_tab`, `spawn_new_tab_with_kind`, the group-spawn path)
- Modify: `crates/manifold/src/main.rs` (field `tab_spawn_error` and its init)
- Modify: `crates/manifold/src/graph_ui.rs` (`toolbar_spawn_error`, its call site, the error arm of the empty branch)
- Test: `crates/manifold/src/session_sync/tests.rs`

**Interfaces:**
- Produces: for `SpawnPurpose::Root | New`, a failure leaves one failed card (`Runtime::Unloaded` in `UnloadedState::Failed`), type tab, `loaded` false, in the document; `SpawnPurpose::Split` failure toasts `"Couldn't start split view: {err}"`, deletes its node as today, and makes no card. `App::tab_spawn_error` no longer exists. A failed Root also sets `view_mode = ViewMode::Graph`.
- Consumes: nothing from other tasks.

- [ ] **Step 1: Rewrite the pinning test (it becomes the failing test)**

In `session_sync/tests.rs` replace `a_failed_new_spawn_is_deleted_if_its_create_committed` (the doc comment too) with:

```rust
/// A Root/New spawn that fails keeps its place as a failed card, in the document whether or not a save had created
/// it while in flight; Split, whose pane is not a card, is deleted and only toasts.
#[test]
fn a_failed_new_spawn_leaves_a_failed_card() {
    for saved_first in [true, false] {
        let mut app = app_on(&temp_dir("failed-new-spawn").join("session.mnf"), 1);
        let id = app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
        if saved_first {
            assert!(app.save_session());
            assert!(live_in_doc(&app, &id), "created while in flight");
        }
        fail_spawns(&mut app);
        let tab = app.nodes[&id].as_unloaded().expect("a failed card");
        assert!(matches!(tab.state, UnloadedState::Failed(_)), "saved_first={saved_first}");
        assert_eq!(tab.url, "https://a/");
        assert_eq!(app.card_kind(&id).map(|kind| kind.ty), Some(crate::store::model::TYPE_TAB));
        assert!(live_in_doc(&app, &id), "persisted, saved_first={saved_first}");
        assert!(app.pending_creates.is_empty() && app.pending_deletes.is_empty());
    }
}

/// A failed Root also leaves the app in Graph view (success does), so the card is seen.
#[test]
fn a_failed_root_spawn_shows_its_card_in_graph_view() {
    let mut app = app_on(&temp_dir("failed-root-spawn").join("session.mnf"), 1);
    assert_eq!(app.view_mode, ViewMode::Item, "the app starts in Item view");
    app.spawn_root_tab();
    fail_spawns(&mut app);
    assert_eq!(app.nodes.len(), 1);
    assert_eq!(app.view_mode, ViewMode::Graph);
}

/// A failed Split makes no card: its node is deleted, and the failure only toasts.
#[test]
fn a_failed_split_spawn_toasts_and_makes_no_card() {
    let mut app = app_on(&temp_dir("failed-split-spawn").join("session.mnf"), 1);
    let id = app.spawn_new_tab_with_kind(None, None, "https://s/", crate::EngineKind::Servo, SpawnPurpose::Split, None);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(!app.nodes.contains_key(&id), "no card for a split pane");
    assert!(!live_in_doc(&app, &id), "deleted by the failure's save");
    assert!(app.save_toast_queue.iter().any(|toast| toast.contains("Couldn't start split view")));
}
```

(Adjust imports in the test module for `UnloadedState`, `ViewMode`, `SpawnPurpose` and `EngineKind` as the file already does for neighbours; the engine used for the split spawn is any variant the test build accepts.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j 8 -p manifold --bins failed_ 2>&1 | tail -20`
Expected: the three new tests FAIL (no card; `app.nodes.len()` is 0; the split test may fail on the toast text).

- [ ] **Step 3: Implement**

In `tabs.rs` `handle_tab_spawn_result`, change the failed-card condition from `matches!(purpose, Respawn | Restore | Wake)` to everything but Split:

```rust
                // A tab keeps its place as a failed card rather than disappearing (see `UnloadedTabPayload`):
                // one that already existed (a crash restart, a restored session or group, a retry) and one just
                // asked for (Root, New). A split's secondary pane is not a card (`App::split_tab`).
                if !matches!(purpose, SpawnPurpose::Split) {
```

Inside that branch, after the node and `seed_landing` are in place, add for Root only:

```rust
                    if matches!(purpose, SpawnPurpose::Root) {
                        // Success switches to Graph view; the failed card must be seen too.
                        self.view_mode = ViewMode::Graph;
                    }
```

Replace the whole `self.tab_spawn_error = Some(match purpose { ... });` block (now reached by Split only) with:

```rust
                self.queue_toast(format!("Couldn't start split view: {err}"));
```

keeping the `self.queue_delete(&id); self.save_session();` that follow it. Delete the field `tab_spawn_error` and its doc comment and its `None` init in `main.rs`; delete the three `self.tab_spawn_error = None;` lines in `tabs.rs` (`spawn_root_tab`, `spawn_new_tab_with_kind`, the group-spawn path) and their "see `tab_spawn_error`'s doc" comments; in `graph_ui.rs` delete `toolbar_spawn_error` and its call in the toolbar, and in the empty branch drop the `match &self.tab_spawn_error` (leave the plain label for now; Task 4 replaces the branch). Delete the stale mention at `process_manager.rs` near line 1323 ("a `tab_spawn_error` the user..."): reword to "a failed card the user ...".

Update the two old `assert!(app.tab_spawn_error.is_some())` lines: they are gone with the replaced test.

- [ ] **Step 4: Run to verify**

Run: `cargo test -j 8 -p manifold --bins 2>&1 | tail -8`
Expected: PASS, no failures; `grep -rn tab_spawn_error crates/` prints nothing.

- [ ] **Step 5: Commit** `Failed Root and New spawns leave a failed card; tab_spawn_error is gone`

---

### Task 2: One failed card per intent; success clears it

**Files:**
- Modify: `crates/manifold/src/tabs.rs`
- Test: `crates/manifold/src/session_sync/tests.rs`

**Interfaces:**
- Consumes: Task 1's failed-card branch for Root/New.
- Produces: `App::failed_twin(&self, id: &str, url: &str, kind: EngineKind, group: Option<&str>) -> Option<String>` (a failed card with the same url, engine and group and a different id); `App::clear_failed_cards(&mut self, url: &str, except: &str)` (removes failed tab cards with that url through `destroy_node`).

- [ ] **Step 1: Write the failing tests**

```rust
fn failed_cards(app: &App) -> Vec<String> {
    let mut ids: Vec<String> = app
        .nodes
        .iter()
        .filter(|(_, node)| node.as_unloaded().is_some_and(|tab| matches!(tab.state, UnloadedState::Failed(_))))
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// The same intent failing again (same url, engine, group) refreshes the one card instead of adding another; a
/// different url is a different intent.
#[test]
fn a_repeat_failure_refreshes_one_card_and_a_new_url_adds_one() {
    let mut app = app_on(&temp_dir("failed-twin").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1, "the same intent is one card");
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.live().count(), 1, "and one document node");
    app.spawn_tab_at("https://b/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2, "another url is another intent");
}

/// A group is part of the intent: the same url failing in two groups is two cards.
#[test]
fn a_failed_twin_in_another_group_is_not_a_twin() {
    let mut app = app_on(&temp_dir("failed-twin-group").join("session.mnf"), 1);
    let group = add_group(&mut app, "G");
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, Some(group), None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2);
}

/// A tab that lands clears the failed cards for its url, and only those.
#[test]
fn a_successful_spawn_clears_failed_cards_for_its_url() {
    let mut app = app_on(&temp_dir("failed-cleared").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    app.spawn_tab_at("https://b/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2);
    let landed = succeed_spawn(&mut app, "https://a/");
    let left = failed_cards(&app);
    assert_eq!(left.len(), 1, "a/'s failed card is gone, b/'s stays");
    assert!(app.nodes[&left[0]].as_unloaded().unwrap().url == "https://b/");
    assert!(app.nodes.contains_key(&landed));
    assert!(app.closed.is_empty(), "a clear is not a close: nothing goes to Recently Closed");
}
```

`succeed_spawn(app, url)` is a new test helper: it starts a New spawn for `url` and completes it with a fake live tab. Look at how `session_sync/tests.rs` and `card_kind.rs` tests build a live tab (`EngineTab::fake`, used in `export_state_names_cards_by_type_loaded_and_json_card`) and complete a parked spawn with `handle_tab_spawn_result(TabSpawnOutcome { result: Ok(tab), .. })`; write the helper beside `fail_spawns` on the same pattern. If a fake tab cannot be produced for a spawn result, name the obstacle in the report and assert the same behaviour by calling `clear_failed_cards` directly.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j 8 -p manifold --bins failed_twin successful_spawn_clears repeat_failure 2>&1 | tail -20` (run each filter separately)
Expected: FAIL.

- [ ] **Step 3: Implement**

In `tabs.rs`:

```rust
impl App {
    /// A failed card already standing for the intent `(url, kind, group)`, other than `id`: a repeat failure refreshes
    /// it instead of adding a second card.
    fn failed_twin(&self, id: &str, url: &str, kind: EngineKind, group: Option<&str>) -> Option<String> {
        self.nodes.iter().find_map(|(other, node)| {
            let tab = node.as_unloaded()?;
            let same = other != id
                && matches!(tab.state, UnloadedState::Failed(_))
                && tab.url == url
                && self.mirror.engine(other) == kind
                && self.mirror.group(other) == group;
            same.then(|| other.clone())
        })
    }

    /// A tab for `url` landed: failed cards for the same url, other than `except`, are resolved. Removed like a
    /// delete that is not a close, so Recently Closed does not collect them. Another url's card stays.
    fn clear_failed_cards(&mut self, url: &str, except: &str) {
        let stale: Vec<String> = self
            .nodes
            .iter()
            .filter(|(id, node)| {
                id.as_str() != except
                    && node.as_unloaded().is_some_and(|tab| matches!(tab.state, UnloadedState::Failed(_)) && tab.url == url)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.destroy_node(&id);
        }
    }
}
```

(`destroy_node` is private to `main.rs`'s impl; if `tabs.rs` cannot call it, make it `pub(crate)`.) Use `crate::EngineKind` or the file's existing import. In `handle_tab_spawn_result`, at the start of the failed-card branch for `Root | New` (a new id; Respawn, Restore and Wake already have their card):

```rust
                    if matches!(purpose, SpawnPurpose::Root | SpawnPurpose::New) {
                        if let Some(twin) = self.failed_twin(&id, &url, kind, group.as_deref()) {
                            if let Some(Runtime::Unloaded(tab)) = self.nodes.get_mut(&twin).map(|node| &mut node.runtime) {
                                tab.state = UnloadedState::Failed(err.to_string());
                            }
                            self.queue_toast(/* the same toast text the branch builds */);
                            self.queue_delete(&id);
                            self.save_session();
                            return;
                        }
                    }
```

(Place it after the toast text is built so the two share it; do not duplicate the text: build it once into a local before this `if`.) In the `Ok(mut tab)` arm, after the live node is inserted and before the per-purpose `match`, call `self.clear_failed_cards(&url, &id);` (use the url as the landed tab holds it; clone before it moves).

- [ ] **Step 4: Run to verify**

Run: `cargo test -j 8 -p manifold --bins 2>&1 | tail -8`
Expected: PASS.

- [ ] **Step 5: Commit** `One failed card per intent; a landing tab clears its url's failed cards`

---

### Task 3: Coalesce failure toasts by cause

**Files:**
- Create: `crates/manifold/src/failure_toasts.rs`
- Modify: `crates/manifold/src/main.rs` (`mod failure_toasts;`, field `failure_notices`, `queue_failure_toast`, flush in `build_save_toast`)
- Modify: `crates/manifold/src/tabs.rs` (the failure toasts call `queue_failure_toast`)
- Test: in `failure_toasts.rs`, plus one in `session_sync/tests.rs`

**Interfaces:**
- Produces: `FailureNotices::push(&mut self, now: Instant, cause: &str, message: String)`, `FailureNotices::flush(&mut self, now: Instant) -> Vec<String>`, `FailureNotices::is_pending(&self) -> bool`, `const WINDOW: Duration = Duration::from_millis(1500)`. `App::queue_failure_toast(&mut self, cause: &str, message: String)`.
- Rule (spec decision 6): notices are held until `WINDOW` after the first one; then each cause yields one message: the notice's own message when it was alone, else `"{n} tabs couldn't start: {cause}"`. Order is first-seen.

- [ ] **Step 1: Write the failing tests (in `failure_toasts.rs`)**

```rust
//! Failure toasts that share a cause read as one: ten tabs failing because an engine is missing is one message with a
//! count, not ten toasts. Pure and clock-free (`now` is passed in) so the window is testable.

use std::time::{Duration, Instant};

/// How long after the first notice a burst is held before it is shown.
pub(crate) const WINDOW: Duration = Duration::from_millis(1500);

#[derive(Default)]
pub(crate) struct FailureNotices {
    first_at: Option<Instant>,
    /// `(cause, how many, the message when there was exactly one)`, first-seen order.
    held: Vec<(String, usize, String)>,
}

impl FailureNotices {
    pub(crate) fn push(&mut self, now: Instant, cause: &str, message: String) {
        self.first_at.get_or_insert(now);
        match self.held.iter_mut().find(|(c, ..)| c == cause) {
            Some((_, count, _)) => *count += 1,
            None => self.held.push((cause.to_owned(), 1, message)),
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.first_at.is_some()
    }

    /// The messages to show now: empty until `WINDOW` has passed since the first held notice.
    pub(crate) fn flush(&mut self, now: Instant) -> Vec<String> {
        let Some(first) = self.first_at else { return Vec::new() };
        if now.duration_since(first) < WINDOW {
            return Vec::new();
        }
        self.first_at = None;
        std::mem::take(&mut self.held)
            .into_iter()
            .map(|(cause, count, message)| if count == 1 { message } else { format!("{count} tabs couldn't start: {cause}") })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_failure_reads_as_its_own_message_after_the_window() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "no engine", "Couldn't start Servo for a: no engine".into());
        assert!(notices.flush(t0 + WINDOW / 2).is_empty(), "held inside the window");
        assert_eq!(notices.flush(t0 + WINDOW), ["Couldn't start Servo for a: no engine"]);
        assert!(!notices.is_pending());
    }

    #[test]
    fn same_cause_failures_are_one_message_with_a_count() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        for i in 0..10 {
            notices.push(t0 + Duration::from_millis(i * 50), "Blink (CEF) is not installed", format!("m{i}"));
        }
        assert_eq!(notices.flush(t0 + WINDOW), ["10 tabs couldn't start: Blink (CEF) is not installed"]);
    }

    #[test]
    fn two_causes_are_two_messages_in_first_seen_order() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "a", "ma".into());
        notices.push(t0, "b", "mb".into());
        notices.push(t0, "a", "ma2".into());
        assert_eq!(notices.flush(t0 + WINDOW), ["2 tabs couldn't start: a", "mb"]);
    }

    #[test]
    fn a_failure_after_a_flush_starts_a_new_window() {
        let t0 = Instant::now();
        let mut notices = FailureNotices::default();
        notices.push(t0, "a", "ma".into());
        assert_eq!(notices.flush(t0 + WINDOW).len(), 1);
        let later = t0 + WINDOW * 3;
        notices.push(later, "a", "again".into());
        assert!(notices.flush(later).is_empty());
        assert_eq!(notices.flush(later + WINDOW), ["again"]);
    }
}
```

Add `mod failure_toasts;` to `main.rs`. Write the file and its module line, run: `cargo test -j 8 -p manifold --bins failure_toasts 2>&1 | tail -12`. Expected at this point: PASS (the implementation is in the same file; the failing-first rule is met by writing the tests before filling `push` and `flush`: write the empty-bodied stubs first, run once to see FAIL, then fill them).

- [ ] **Step 2: Wire it into the app**

In `main.rs`: field `failure_notices: failure_toasts::FailureNotices` (doc: "Spawn-failure toasts held to merge by cause (`failure_toasts`)") with `Default` in the constructor; method

```rust
    /// A spawn-failure toast: held a moment so failures sharing `cause` read as one message (`failure_toasts`).
    pub(crate) fn queue_failure_toast(&mut self, cause: &str, message: String) {
        self.failure_notices.push(std::time::Instant::now(), cause, message);
    }
```

At the top of `build_save_toast`, before the pop:

```rust
        for message in self.failure_notices.flush(std::time::Instant::now()) {
            self.queue_toast(message);
        }
        if self.failure_notices.is_pending() {
            ui.ctx().request_repaint_after(failure_toasts::WINDOW);
        }
```

In `tabs.rs`, the failed-card branch's toast (and the twin path's) becomes `self.queue_failure_toast(&err.to_string(), toast)`; the Split toast (Task 1) and the engine-switch "Couldn't switch to" toast stay plain `queue_toast` (spec: coalescing covers the Restore, Respawn, Wake and new-spawn paths).

- [ ] **Step 3: Add the integration test (in `session_sync/tests.rs`)**

```rust
/// Spawn failures that share a cause reach the toast queue as one message once the window has passed.
#[test]
fn spawn_failures_of_one_cause_toast_once_with_a_count() {
    let mut app = app_on(&temp_dir("failure-toasts").join("session.mnf"), 1);
    for i in 0..3 {
        app.spawn_tab_at(&format!("https://t{i}/"), egui::Vec2::ZERO, None, None);
    }
    fail_spawns(&mut app);
    assert!(app.save_toast_queue.is_empty(), "held inside the window");
    for message in app.failure_notices.flush(std::time::Instant::now() + crate::failure_toasts::WINDOW) {
        app.queue_toast(message);
    }
    assert_eq!(app.save_toast_queue.iter().collect::<Vec<_>>(), ["3 tabs couldn't start: no engine"]);
}
```

- [ ] **Step 4: Run to verify**

Run: `cargo test -j 8 -p manifold --bins 2>&1 | tail -8`
Expected: PASS. If an older test asserted an immediate failure toast in `save_toast_queue`, update it to flush first and name the change in the report.

- [ ] **Step 5: Commit** `Spawn-failure toasts that share a cause read as one`

---

### Task 4: The empty canvas, probes, docs

**Files:**
- Modify: `crates/manifold/src/graph_ui.rs` (the empty branch)
- Modify: `scripts/` (a failure probe, and any script quoting "No tabs")
- Modify: `README.md`, `docs/superpowers/specs/2026-10-06-failed-spawn-cards-design.md` (status line), this plan's Outcome
- Test: `crates/manifold/src/graph_ui.rs` tests or the smoke probe

**Interfaces:**
- Consumes: Task 1 (no `tab_spawn_error`).
- Produces: `build_graph_ui` always renders the canvas; with no nodes at the level it paints the hint `"Ctrl+T for a new tab · drop a file or URL · F1 for shortcuts"` centred in the canvas, in `weak_text_color`, taking no input.

- [ ] **Step 1: Check that the canvas tolerates an empty level**

Before editing, run `grep -n "nodes.is_empty\|items.is_empty\|\.len() - 1\|\[0\]" crates/manifold/src/graph_ui.rs crates/manifold/src/graph_nav.rs crates/manifold/src/graph_layout.rs` and read each hit that runs in `build_graph_ui` after the empty branch (settle, minimap, fit-to-view, camera tween) for a divide by zero, an index into an empty list, or a NaN rect on zero items. Record what you found in the report. Fix a real hazard minimally (an early `if items.is_empty()` guard at that spot), do not restructure.

- [ ] **Step 2: Write the failing test**

Add a test that builds the Graph UI over an empty `App` the way existing graph UI tests do (search `graph_ui.rs`/`card.rs` tests for a headless `egui::Context` run, e.g. `ctx.run(..., |ctx| app.build_graph_ui(ui))`), then assert it does not panic and that the frame's painted text includes the hint (collect `ctx` output shapes and find a `Shape::Text` whose galley text equals the hint constant). If no such harness exists, test the pure part instead: expose `const EMPTY_HINT: &str` and assert the empty-level condition is the only place it is painted, and rely on Step 4's live check for the rest; say so in the report.

- [ ] **Step 3: Implement**

Replace the empty branch (the early return after `apply_camera_requests`) with a fall-through; the actions loop that the branch ran (`for action in actions { self.apply_graph_action(..) }`) is already run at the end of the function by the normal path, so the branch is simply deleted. Then, after the toolbar and before the scene, when `self.nodes.is_empty()` paint:

```rust
        if self.nodes.is_empty() {
            let rect = ui.max_rect();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                EMPTY_HINT,
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().weak_text_color(),
            );
        }
```

with `const EMPTY_HINT: &str = "Ctrl+T for a new tab \u{b7} drop a file or URL \u{b7} F1 for shortcuts";` near the function. Painting with `ui.painter()` takes no input, so panning and drops still work. Confirm `F1` and `Ctrl+T` are the current bindings (`shortcuts.rs`, `debug_control.rs` `NEWTAB`) and adjust the text if not.

- [ ] **Step 4: Live checks (after `cargo build -j 8 -p manifold`)**

1. A probe for the failure card: model it on the missing-engine case in `scripts/page-kind-probe.sh` (read how it forces an engine failure and reads EXPORTSTATE, using the `scripts/export_state.py` helper). Add `scripts/failed-spawn-probe.sh`: force a New-tab spawn failure, then assert via EXPORTSTATE that one card has `type` `application/vnd.manifold.tab` and `loaded` false, that its Retry (`RetryMissingFile` through the debug command the sleep probe uses) keeps the same id, and that a second failure for the same url leaves still one card. If a clean way to force the failure does not exist, say so in the report and cover it by the unit tests only.
2. `headless-smoke-test.sh` (expect 48/0), `sleep-probe.sh` (8/8), `page-kind-probe.sh` (6/6) still pass.
3. A screenshot of an empty session (use the smoke test's screenshot helper or `drive.sh`) shows the hint and no "No tabs." label; look at it.

- [ ] **Step 5: Docs**

`grep -n "No tabs" README.md scripts/*` and update every mention (README lines near 157 and 628: the first is feature text, the second a history entry; leave history entries intact and add a dated line saying the screen was replaced). Add a README current-behaviour line: a failed new tab leaves a failed card, deduplicated by url, engine and group, cleared when that url lands; failure toasts merge by cause. Spec: set a status line "Implemented 2026-10-06, plan `2026-10-06-failed-spawn-cards.md`, commits <hashes>". Plan: append an Outcome section (commits, deviations, the Step 1 canvas findings, the probe results, parked items).

- [ ] **Step 6: Gate, then commit** `The empty graph keeps its canvas and shows a hint; failed-spawn probe and docs`

Run `cargo fmt --all`, the full `cargo test -j 8 -p manifold --bins`, clippy via the hook. The controller runs the live two-instance gate (three runs) and the smoke test after the last task.

---

## Self-review (run 2026-10-06)

- **Spec coverage:** decision 1 (no screen, hint, no error variant) is Task 4; 2 (Root/New cards, Split toast) and 3 (field and clears deleted) are Task 1; 4 (dedup) and 5 (auto-clear through the delete queue) are Task 2, which uses `destroy_node` (queues the delete via `remove_tab`/`remove_node`); 6 (toast coalescing) is Task 3; 7 (Retry unchanged) needs no code and is checked live in Task 4. Risks: Root failure view is Task 1's `a_failed_root_spawn_shows_its_card_in_graph_view`; the auto-clear funnel is covered by the debug invariant running in every test save; the window mechanism is the pure `FailureNotices`.
- **Reverses a 2b-5 decision:** the 2b-5 test `a_failed_new_spawn_is_deleted_if_its_create_committed` (and its "failed card branch is only for a tab that exists" comment) encoded the opposite choice for Root/New. Task 1 rewrites it on purpose; the reviewer should expect that diff.
- **Placeholders:** the `succeed_spawn` helper and the Task 4 UI test harness are described by the pattern to follow rather than complete code, because they depend on test scaffolding I did not read in full (`EngineTab::fake`, an egui test context). The implementer reports the obstacle if a pattern cannot be followed; nothing else is open.
- **Type consistency:** `failed_twin`, `clear_failed_cards`, `FailureNotices::{push, flush, is_pending}`, `WINDOW`, `queue_failure_toast`, `EMPTY_HINT` are defined once and used with the same signatures.
- **Unverified at writing:** that `destroy_node` is callable from `tabs.rs` (private in `main.rs`'s impl; Task 2 says to widen it); that a group-spawn path has its own `tab_spawn_error = None` (three clears were seen: `spawn_root_tab`, `spawn_new_tab_with_kind`, and one near `tabs.rs:1230`); the exact test names for engine kind and `add_group`'s signature.

---

## Outcome (2026-10-06)

Commits on `dev`: 86913df (Task 1: failed Root/New cards, no `tab_spawn_error`), 8e4f9a6 and 2729ba1 (Task 2: one failed card per intent, a fresh Root/New landing clears its url's failed cards, after the landing), 8970810 (Task 3: failure toasts that share a cause merge), and Task 4's commit (`The empty graph keeps its canvas and shows a hint; failed-spawn probe and docs`).

**Deviations**
- Spec decision 5 narrowed (ruling during Task 2): only a fresh Root or New success clears failed cards; Respawn, Restore and Wake landings do not.
- Task 1 reversed the 2b-5 rule that a failed New spawn is deleted, as the spec required.
- Task 4's test is a real `build_graph_ui` run over an empty `App` in a headless `egui::Context` (the harness existed), asserting the hint text is painted once.
- `draw_graph_minimap` got an early `return None` for an empty level (see below).

**Step 1: canvas on an empty level.** The grep hit only `graph_nav.rs:605` (a bench helper, unrelated). Reading the rest of `build_graph_ui` after the old early return: `settle_level_if_changed(0)` lays out an empty level (no division); `bounds_for_rects` folds to `Rect::NOTHING` and falls back to `default_graph_scene_rect()`, so fit-to-view, `apply_camera_requests` and the camera tween get a finite rect; `update_graph_lod` counts zero; `handle_graph_keys`/`move_graph_focus` guard with `Option`. One real oddity: the minimap compared the default fallback rect against the camera and could paint an empty minimap box; fixed with `if items.is_empty() { return None; }`.

**Probes**
- `scripts/failed-spawn-probe.sh` (new): no clean in-app switch forces a New spawn failure, and a CEF or WebKit default engine does not fail (CEF starts; WebKit crashes the host after the spawn, which is a restart path, not a spawn failure). It runs a copy of the exe with no engine host beside it, so every spawn fails ("cannot find the file specified"). 6/6: one card after the startup Root failure (the probe proves the failed Root card, not a New card: the empty session spawns `home_page` as Root), a tab with `loaded` false, still one card with the same id after a New spawn of the same intent (refreshed through the twin path), and Retry (`GRAPHRIGHT GRAPHOPEN`, which runs `retry_missing_file`) keeps the card and its id.
- Screenshot of an empty session (`CLOSEALLLEVEL` on the bootstrapped session, headless `SCREENSHOT`): the hint is centred, no "No tabs." label, the toolbar and dot grid are drawn.
- `page-kind-probe.sh`: 6/6. `headless-smoke-test.sh` and `sleep-probe.sh` are left to the controller's gate.

**Parked**
- `leave_crashed_card`'s cannot-start toast is not coalesced (Task 3).
- A failure queued after `build_save_toast` in the same frame with no later repaint flushes late (Task 3).
- No test asserts the landing tab's mirror/layout entries survive the clear's save (Task 2).
- A comment near `tabs.rs` ~597 should name Split (Task 1).

**Final review and gate (2026-10-06)**
- Final whole-branch review (opus) found: an uncommitted duplicate-test write by a finished agent (restored, never committed); `clear_failed_cards` saving mid-landing through `destroy_node` (fixed: `remove_tab`, the landing's own save commits the delete; test with a parked remote delete panics at `session_sync.rs:1362` without it); and a url-only clear that also removed Restore/Wake/crash cards of arranged tabs (ruling: clear only failed cards in the landing tab's group, still any engine; fix commit 9c993c5; spec decision 5 and README updated). Open design question for the user: a same-group card left by a restored, arranged tab is still removed when the same url is opened fresh; alternatives are recording it to Recently Closed or clearing only cards a Root/New failure created.
- Gate on 9c993c5 (cargo build -j 8; two-instance x3 ok; headless-smoke 48/0; Defender real-time off; CPU 14-22% before each run, machine otherwise quiet; run 13:06-13:18): passed. The same gate on d3f9d30 also passed. Unit suite 896 passed, 0 failed, 5 ignored; fmt and clippy by the pre-commit hook.
- Parked from the final review: M2 the coalesced toast counts failures not tabs, and its key is the error text alone (spec says engine plus error text); M3 the hint tests the whole graph, not the level; M4 a failed startup Root no longer retries next launch (the card persists); M5 a card whose retry is in flight is not a twin.
