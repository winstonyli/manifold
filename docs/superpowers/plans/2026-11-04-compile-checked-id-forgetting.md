---
status: done
---

# Compile-checked node-id forgetting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline; the tasks share files, so no per-task subagents). Steps use checkbox syntax.

**Goal:** Every struct that owns node-keyed state forgets a removed node through a `forget` that destructures `Self` without `..`, so a new field fails to compile until handled; six loose `App` fields move into `NodeScoped`; the 114-line source-text scanner becomes a ~20-line check of `App`'s direct fields. Behavior unchanged.

**Spec:** `docs/superpowers/specs/2026-11-04-compile-checked-id-forgetting-design.md` (approved 2026-11-04). Parent plan item ARC1: `docs/superpowers/plans/2026-11-04-restructuring-cleanup.md`.

## Global Constraints

- Work on `main`; small commits; never push, tag or bump. `cargo fmt --all` before each commit (run it twice if the hook still rejects); the pre-commit hook runs fmt and `cargo lint`. Never `--no-verify`.
- No behavior change: the same removals in the same order, except as the spec's "Deliberate changes" says.
- No shims: the old scanner, `GROUPS`, `NOT_FORGOTTEN` and `App::forget_connect_source` are deleted, callers edited.
- Excuses are `field: _, // why`, and the why is a real reason (carry the text from `NOT_FORGOTTEN` and the `forget_id` doc; fix the ones the spec lists as wrong).
- Python runs as `python -P file.py < /dev/null`, never on stdin. Cargo uses `-j 8`.
- Test-id check: `node <scratchpad>/testlist.js OUT` writes the sorted manifold and gate test list. BEFORE is taken at the start of Task 1; the only expected difference at the end is the scanner test's id (Task 1 renames it to the slim check's id).

## Review Focus

- A destructure that lists a field with `_` but the old code removed it: a silent behavior loss. Each `forget` is checked against the old body line by line (Task 2, Step 9).
- A renamed `scoped.*` field in a place that is a different struct's field of the same name (`Loaded.nav_history`, `local_state` fields): wrong receiver compiles only if the types agree; the regex in Task 3 is restricted to `self.`/`app.` receivers and the compiler finds the rest.
- The slim check silently passing: it must fail when a direct `App` field with an id is neither in `forget_id`'s body nor excused (Task 1, Step 4 seeds one).
- `forget_id` order: `pending_sleeps` is removed before the cookie forget (the held answer is dropped, not cancelled), and the GPU frees stay thumbnail, texture, image.

---

### Task 1: Slim check replaces the scanner

**Files:** `crates/manifold/src/main/tests.rs`.

The group destructures of Task 2 stop naming `self.<field>` for the fields they excuse, which the old scanner reads as "forgotten", so the scanner must go first.

- [x] **Step 1: Baseline.** `git status --short` shows only `docs/runs.md` (the spec and plan files are committed first); `node testlist.js BEFORE` (2029+ tests); note the scanner's id exactly as BEFORE prints it (`tests::every_id_keyed_field_is_forgotten_or_excused`, perhaps `main::`-prefixed).
- [x] **Step 2: Replace the test.** In `main/tests.rs` delete `NOT_FORGOTTEN` and `every_id_keyed_field_is_forgotten_or_excused` (lines 61 to the end of that test, ~114 lines) and add:

```rust
/// App fields whose type names a node or card id that `forget_id` does not name, each with why. Groups that
/// own id-keyed state are not here: their `forget` destructures `Self` exhaustively (`SpawnTracker::forget`, ...).
const EXCUSED: &[(&str, &str)] = &[
    ("node_order", "removed with `nodes` by each caller (kind-specific order); `unshow` takes it"),
    ("layout", "pruned to live nodes at every save (`layout.retain`)"),
    ("current_group", "`take_out` switches level"),
    ("focused", "`unshow`'s focus, split and view-mode logic"),
    ("split_tab", "`unshow`'s focus, split and view-mode logic"),
    ("viewed_image", "`unshow` clears it with the Item view; `image_view` resets when the image is gone"),
    ("jars", "removed by `forget_jar_memory`, which `forget_id` calls"),
];

/// C1: a new `App` field whose type mentions a node or card id must be decided: named in `forget_id`'s body (a
/// group's `forget` call counts: the group destructures itself) or excused in `EXCUSED` with why. The scan sees
/// only `App`'s direct fields; a new group struct whose type hides its ids is the destructures' job, and stays
/// until ARC7 groups the loose item-view fields.
#[test]
fn every_id_field_of_app_is_forgotten_or_excused() {
    fn block<'a>(source: &'a str, header: &str, end: &str) -> Vec<&'a str> {
        source.lines().skip_while(|l| !l.trim_start().starts_with(header)).skip(1).take_while(|l| *l != end).collect()
    }
    let names_an_id = |ty: &str| {
        ty.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|word| word == "NodeId" || word == "CardId")
    };
    let app = block(include_str!("../main.rs"), "struct App {", "}");
    let fields: Vec<(&str, &str)> = app
        .iter()
        .map(|l| l.trim().trim_start_matches("pub(crate) "))
        .filter(|l| !l.starts_with("//") && !l.starts_with('#'))
        .filter_map(|l| l.split_once(": "))
        .collect();
    assert!(fields.len() > 50, "App's fields were found ({})", fields.len());
    let body = block(include_str!("../node_store.rs"), "pub(crate) fn forget_id(", "    }").join("\n");
    assert!(body.contains("self.pending_sleeps") || body.contains("self.scoped"), "forget_id's body was found");
    let named = |name: &str| {
        let needle = format!("self.{name}");
        body.match_indices(&needle)
            .any(|(at, _)| !body[at + needle.len()..].chars().next().is_some_and(|c| c == '_' || c.is_ascii_alphanumeric()))
    };
    let undecided: Vec<String> = fields
        .iter()
        .filter(|(_, ty)| names_an_id(ty))
        .filter_map(|(name, _)| {
            let (forgotten, excused) = (named(name), EXCUSED.iter().any(|&(n, _)| n == *name));
            (forgotten == excused).then(|| format!("{name}: {}", if excused { "both forgotten and excused" } else { "neither" }))
        })
        .collect();
    assert!(undecided.is_empty(), "decide each App field: {undecided:?}");
    for &(name, _) in EXCUSED {
        assert!(fields.iter().any(|(n, ty)| *n == name && names_an_id(ty)), "EXCUSED lists {name}, which is not an id field");
    }
}
```

  The `jars` row is real: `forget_id` calls `forget_jar_memory` (which removes `self.jars`), and does not name `self.jars` itself. If the first run reports another undecided field, decide it (name or excuse with a real reason) and note it in the outcome.
- [x] **Step 3: Run it.** `cargo test -j 8 -p manifold every_id_field_of_app` (Expected: PASS; fix `EXCUSED` per Step 2's note if not). If `pending_sleeps` etc. are still direct fields (they are until Task 3) the `body.contains` anchor holds.
- [x] **Step 4: Seed.** Temporarily add `probe: Option<NodeId>,` to `struct App` (and `probe: None,` where `App` is built if the compiler asks); re-run the test (Expected: FAIL naming `probe: neither`); add `self.probe = None;` to `forget_id` (Expected: PASS); revert both.
- [x] **Step 5: Verify and commit.** `cargo fmt --all` (twice), `cargo lint -j 8 -- -D warnings`, `cargo check -j 8 -p manifold --all-targets`; `node testlist.js AFTER`; `diff` BEFORE AFTER shows exactly one line replaced (old scanner id for new id). Commit `ARC1: slim App-field check replaces the id-forgetting scanner`. This unblocks LAY6 (no more `include_str!("../debug_control.rs")` in `main/tests.rs`; check `git grep -n "include_str" crates/manifold/src/main/tests.rs` shows only `main.rs` and `node_store.rs`).

### Task 2: Destructuring `forget` per group

**Files:** `tabs.rs`, `jar.rs`, `graph_nav.rs`, `popups/menus.rs`, `page_tab_gate.rs`, `links.rs`, `doc.rs`, `gpu.rs`, `node_store.rs`, `tabs/jar_pull.rs`, `link_gestures/tests.rs`.

Each `forget` starts `let Self { a, b, c } = self;` with no `..`; an excused field is `field: _, // why`. The reasons come from the old `NOT_FORGOTTEN` and the `forget_id` doc (the git history has them: `git show HEAD~1:crates/manifold/src/main/tests.rs`).

- [x] **Step 1: `SpawnTracker::forget`** (`tabs.rs`). Destructure all 12: used `skip_convert`, `navigated_from`, `tab_crashes`, `pending_respawn_focus`, `pending_trail_focus`; excused `spawning`, `remote_in_flight`, `wakes_in_flight`, `host_loads` (spawn lifecycle: the landing or teardown owns them; `wakes_in_flight` is read by a tab closed while waking so the landing commits its delete; `host_loads` expires by `shares_url_change`), and `tab_spawns_in_flight`, `tabs_spawned_waiters`, `session_generation` (not per node). Body logic unchanged:

```rust
let Self { skip_convert, navigated_from, tab_crashes, pending_respawn_focus, pending_trail_focus,
    spawning: _, remote_in_flight: _, wakes_in_flight: _, host_loads: _, // spawn lifecycle ...
    tab_spawns_in_flight: _, tabs_spawned_waiters: _, session_generation: _ } = self;
skip_convert.remove(&id);
navigated_from.remove(&id);
tab_crashes.remove(&id);
if *pending_respawn_focus == Some(id) { *pending_respawn_focus = None; }
if *pending_trail_focus == Some(id) { *pending_trail_focus = None; }
```

  Write the comments on their own lines for readability (rustfmt decides the layout).
- [x] **Step 2: `CookieState::forget`** (`jar.rs`). Destructure all 8: used `jar_tabs`, `jar_reseed`, `jar_pulls`, `jar_silent`, `cookie_holds` (returned, unchanged); excused `jar_snapshots`, `jar_dirty` ("forgotten by `App::forget_jar_memory`: it saves a dirty jar, then drops the snapshot", file I/O so it stays on `App`) and `jar_last_pull` ("a round timestamp, not per node"). Fix its doc line.
- [x] **Step 3: `GraphInteraction::forget`** (`graph_nav.rs`). Destructure all 11: used `dragging`, `hovered`, `peek_suppressed`, `focus_new_note`, `selected`, `pane_card`; excused `drag_moved` (ends with the drag; a gone id is skipped at release), `drag_origin` (taken by `apply_drop`; a drop with no effect discards it), `rect_select_start` (a screen point), `label_section` (reset whenever the pane shows no cards), `pending_new_node` (names no node).
- [x] **Step 4: `NodeMenu::forget`** (`popups/menus.rs`): `let Self { open, page, fade: _ } = self;` with `fade`'s reason (the closing menu's frozen copy fades out). `PageTabGate::forget` (`page_tab_gate.rs`): `let Self { by_source } = self;`. `MenuPopups` gets none (`trail_menu` holds a virtual card id; `build_trail_menu` closes it when its page is gone) and the reason goes in `MenuPopups`' doc where `trail_menu` is.
- [x] **Step 5: `LinkGesture::forget`** (`links.rs`), replacing `App::forget_connect_source`:

```rust
/// Ends pick-target mode when `id` is its source or the card a connect-drag started on.
pub(crate) fn forget(&mut self, id: NodeId) {
    let Self { connecting, connect_drag, retarget: _, retarget_preview: _, line_press: _ } = self;
    // retarget: cleared by `end`; retarget_preview: set each frame by the graph pass;
    // line_press: a link id, taken at release only if still hovered.
    if connecting.as_ref().is_some_and(|&(from, _)| from == id) || *connect_drag == Some(id) {
        self.end();
    }
}
```

  Delete `App::forget_connect_source` (`links.rs:385`); the test at `link_gestures/tests.rs:1101` calls `app.link.forget(a)` instead.
- [x] **Step 6: `Doc::forget`** (`doc.rs`): `let Self { hidden_local, journal: _, replica: _, fallback_ids: _, mirror: _, writes: _, local_persist: _, store_recovered: _, label_ms: _ } = self; hidden_local.remove(&id);` with a real reason per excused field (`mirror`: each caller removes the node itself; `writes`: the `pending*` queues are dropped by the save's `editable` filter and `held_text` is pruned for non-editing notes at save, teardown clears it; the rest are not per node). Keep the "A hide re-inserts its entry after this; every permanent removal drops it" comment.
- [x] **Step 7: `GpuState::forget`** (`gpu.rs`): a plain wrapper, no destructure (`GpuState` owns the device and queue):

```rust
/// Frees everything held for node `id`. The order matters: `free_tab_texture` reads `thumbs_dirty`, which the
/// thumbnail free removes first.
pub(crate) fn forget(&mut self, id: NodeId) {
    self.free_tab_thumbnail(id);
    self.free_tab_texture(id);
    self.free_image(id);
}
```

- [x] **Step 8: `forget_id`** (`node_store.rs`) becomes (order kept; `pending_sleeps` before the cookie forget):

```rust
self.spawns.forget(id);
self.doc.forget(id);
self.level_views.remove(&Some(id));
self.pending_sleeps.remove(&id);
self.cookies.forget(id); // the held answer is dropped: `pending_sleeps` is already gone, nothing awaits it
self.saved_scroll.remove(&id);
self.nav_history.remove(&id);
self.page_tab_gate.forget(id);
self.forget_jar_memory(id);
if self.naming_jar ... (unchanged) ; if self.renaming_group ... (unchanged)
self.menus.node_menu.forget(id);
self.interaction.forget(CardId::from(id));
self.link.forget(id);
if let Some(gpu) = &mut self.gpu { gpu.forget(id); }
```

  `clear_cookie_state` stays (used by `note_spawned_jar`); if `cookies.forget`'s result is `#[must_use]` or clippy objects, bind it with `let _held = ...` and a comment. Rewrite `forget_id`'s doc: callers are `unshow` (node_store.rs:167), `teardown_running_session` (session_files.rs:517), `forget_deleted` (session_sync/remote.rs:486); `hide_node` runs it via `unshow` and then re-inserts `hidden_local`; `undo` (`UndoHistory`) deliberately keeps ids so Reopen works; drop the "`main.rs`'s scanner" sentence (now: each group's `forget` destructures itself, and `every_id_field_of_app_is_forgotten_or_excused` covers `App`'s own fields). Do not claim `merge_groups` is a caller.
- [x] **Step 9: Compare against the old body.** `git diff HEAD -- crates/manifold/src/node_store.rs` next to `git show 175f9486:crates/manifold/src/node_store.rs` (the pre-ARC1 body, whatever is committed since) and tick every old removal: `skip_convert`, `navigated_from`, `tab_crashes`, `pending_respawn_focus`, `pending_trail_focus`, `hidden_local`, `level_views`, `pending_sleeps`, `jar_tabs`, `jar_reseed`, `jar_pulls`, `jar_silent`, `cookie_holds`, `saved_scroll`, `nav_history`, page-tab gate, jar memory, `naming_jar`, `renaming_group`, node menu, `dragging`, `hovered`, `peek_suppressed`, `focus_new_note`, `selected`, `pane_card`, connect source, three GPU frees.
- [x] **Step 10: Seed.** Add `probe: Option<NodeId>` to `SpawnTracker` (default derive covers it); `cargo check -j 8 -p manifold` (Expected: FAIL "pattern does not mention field `probe`"); revert. Same on `CookieState` and `Doc`. Record the three results in the outcome.
- [x] **Step 11: Verify and commit.** `cargo fmt --all` (twice); `cargo lint -j 8 -- -D warnings`; `cargo test -j 8 -p manifold forget` and `-p manifold link_gestures`; `cargo check -j 8 -p manifold --all-targets`. Commit `ARC1: id-forgetting groups destructure Self exhaustively`.

### Task 3: `NodeScoped`

**Files:** `node_store.rs` (new struct), `main.rs` (6 declarations, 6 initialisers, `Loaded` stays), and every file naming the six fields (about 166 lines in 35 files).

- [x] **Step 1: The struct** in `node_store.rs`, fields and doc comments moved verbatim from `main.rs`:

```rust
/// App state keyed by a node id that goes when the node does (`NodeScoped::forget`, from `App::forget_id`).
#[derive(Default)]
pub(crate) struct NodeScoped {
    pub(crate) pending_sleeps: HashMap<NodeId, sleep_state::PendingSleep>,
    pub(crate) saved_scroll: HashMap<NodeId, sleep_state::SavedScroll>,
    pub(crate) nav_history: HashMap<NodeId, trail::history::NavHistory>,
    pub(crate) level_views: BTreeMap<Option<NodeId>, local_state::LevelView>,
    pub(crate) naming_jar: Option<(NodeId, String)>,
    pub(crate) renaming_group: Option<(NodeId, String)>,
}

impl NodeScoped {
    pub(crate) fn forget(&mut self, id: NodeId) {
        let Self { pending_sleeps, saved_scroll, nav_history, level_views, naming_jar, renaming_group } = self;
        level_views.remove(&Some(id));
        pending_sleeps.remove(&id);
        saved_scroll.remove(&id);
        nav_history.remove(&id);
        if naming_jar.as_ref().is_some_and(|(t, _)| *t == id) { *naming_jar = None; }
        if renaming_group.as_ref().is_some_and(|(g, _)| *g == id) { *renaming_group = None; }
    }
}
```

  `pending_sleeps` must still be removed before `cookies.forget`: in `forget_id` call `self.scoped.forget(id)` before `self.cookies.forget(id)` and keep the comment. The old order put `level_views` first and `naming_jar` after `forget_jar_memory`; none of those depend on each other, so the group order is irrelevant except `pending_sleeps` before `cookies`.
- [x] **Step 2: `main.rs`.** Delete the six `App` fields (lines near 218, 275, 278, 310, 321, 323) with their doc comments (moved into the struct), add `scoped: node_store::NodeScoped` with a one-line doc, and in the initialiser (near 523-542) build `scoped: NodeScoped { nav_history, ..Default::default() }` from the `Loaded` destructure (`Loaded.nav_history` at 459 stays) and delete the other five initialisers. `forget_id` calls `self.scoped.forget(id)` in place of the six old lines.
- [x] **Step 3: Rename the uses.** Mechanical, receiver-restricted: `perl -pi -e 's/(?<![\w.])(self|app)\.(pending_sleeps|saved_scroll|nav_history|level_views|naming_jar|renaming_group)\b/$1.scoped.$2/g'` over `git grep -l` of the six names in `crates/manifold/src` (exclude `Loaded` and `local_state`). Then `cargo check -j 8 -p manifold --all-targets` and fix the remaining receivers the compiler reports. The line-based regex misses three multi-line `self\n.field` chains: `local_state.rs:429` (`.level_views`), `popups/dropdowns.rs:714` (`.nav_history`), `tabs/sleep.rs:84` (`.pending_sleeps`); edit them by hand. Only `App` and `Loaded.nav_history` (`main.rs`) declare these names, and `main.rs:656` is a struct update (`..App::load()`) needing no edit (`state.`, `a.`, `this.`, a `&mut App` named differently): a different struct's same-named field must NOT be renamed, so edit only what the compiler flags.
- [x] **Step 4: Update the extended test.** `forget_id_clears_every_id_keyed_table` (`main/tests.rs`) uses the six fields: they become `app.scoped.*`. Add `app.doc.hidden_local.insert(t, ...)` (use the `LocalNode` default or an existing test helper; `grep -n "hidden_local.insert" crates/manifold/src`) and assert it is removed and `u`'s kept.
- [x] **Step 5: Slim check.** The six names are no longer direct fields: `cargo test -j 8 -p manifold every_id_field_of_app` (Expected: PASS; its `body.contains` anchor accepts `self.scoped`). Delete from `EXCUSED` nothing (none of the six was excused).
- [x] **Step 6: Seed.** Add `probe: HashMap<NodeId, u8>` to `NodeScoped`: `cargo check` must fail on `forget` (Expected: "pattern does not mention field `probe`"); revert.
- [x] **Step 7: Verify and commit.** `cargo fmt --all` (twice); `cargo lint -j 8 -- -D warnings`; `cargo check -j 8 -p manifold --all-targets`; `node testlist.js AFTER`; the diff against BEFORE (Task 1) is the one renamed test. `git diff --stat HEAD` stays near the spec's 166 lines. Commit `ARC1: per-node App state moves into NodeScoped`.

### Task 4: Docs, parked entry, tests through the wrapper

**Files:** `PARKED.md`, `docs/superpowers/plans/2026-11-04-restructuring-cleanup.md`, this plan, the spec, `docs/devlog.md` if it mentions the scanner.

- [x] **Step 1: Old-name grep.** `git grep -n "NOT_FORGOTTEN\|every_id_keyed_field\|forget_connect_source\|FORGET-ID-TYPED\|const GROUPS" -- . ':!docs/superpowers' ':!.superpowers' ':!docs/runs.md'` (plain `GROUPS` also hits `MAX_CLOSED_GROUPS` and `SUGGEST_GROUPS`): fix each live hit (expected: `PARKED.md:68` and `:72`; any comment naming the scanner). Delete the `FORGET-ID-TYPED` entry from `PARKED.md` and its name from the line-68 intro.
- [x] **Step 2: Outcome.** Append to ARC1 in the restructuring plan: Done 2026-11-04 (commits), actual size against the estimate (~166 edited, ~114 test lines deleted), the seeded compile results (SpawnTracker, CookieState, Doc, NodeScoped fail as expected), the slim test's seed, dead excuses removed (15), wrong reasons rewritten, and `Not done:` the egui-memory inference and the ARC7 follow-up. Mark this plan and the spec `status: done`. Also fix the plan's LAY2/LAY6 decisions line ("q1 debug split three ways" -> q3, pointing at the trail/debug spec) while there.
- [x] **Step 3: Unit tests through the wrapper.** `bash`: `powershell -NoProfile -File "$env:USERPROFILE\.claude\runs\run.ps1" -Kind probe -Name arc1-unit -Detach -Command "cargo unit -j 8"`, then arm a `run_in_background` `until` loop on `run.json`'s `"end"` (RUNS.md) so the run is not bound by the 10-minute tool limit; read `run.json` for exit and `peak_commit_mb` (raise `-MaxMemGB` if it died at the 12 GB cap). Expected: exit 0. Gate tier `full` is on request only (session-removal paths).
- [x] **Step 4: Commit** `ARC1: docs, PARKED entry removed, outcome recorded`.
