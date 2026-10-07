# Node model 2b-3: url, engine, jar and jar_set in the mirror Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A tab's shared `url`, `engine`, `jar` and `jar_set` become mirror state written through `edit_field`-style
writers, so `diff` stops comparing them for existing nodes and `explicit` keeps only `POS`.

**Architecture:** Same shape as 2b-1 and 2b-2. `MIRRORED` grows by `URL`, `ENGINE`, `JAR`, `JAR_SET`; the writers queue
into `App::pending`; `save_session` commits them after `diff`, resolving `Id` targets (now including `JAR`) against the
document and this commit; `mirror_remote` applies another replica's values except where a local edit is pending. The
tab payloads keep their *runtime* url and engine (live url, running engine, `swap_target`); only the shared value moves.
`JarChoice` leaves the payloads and `TabCarry` and lives in `Carried`.

**Tech Stack:** Rust, egui, the in-repo `store` journal.

**Spec:** `docs/superpowers/specs/2026-10-04-node-model-design.md` (section 6 items 4, 6, 10; 2b "Read model").
Facts about today's flow: `docs/superpowers/specs/2026-10-05-sync-flow-map.md`. Follows
`2026-10-05-node-model-2b-2-links.md` (read its Rulings and its Outcome "Parked" list; they all still hold).
Survey this plan was written from (git-ignored, may be gone): `.superpowers/scratch-2b3-map.md`.

## Global Constraints

- Work on `dev`, never on `main` (`CLAUDE.md`). Commit with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Run `cargo fmt --all` before each commit. `cargo fmt --all --check` and
  `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass.
  Never bypass `.githooks/pre-commit`. Don't silence a lint to pass: fix it, or `#[allow]` it with a comment saying why.
- No test may read or write the real `%APPDATA%\manifold`. Tests that need files use `store::testutil::temp_dir`.
- Test baseline before this plan: `cargo test -j 8 -p manifold --bins -p ipc-protocol`: 783 passed, 4 ignored (manifold
  --bins) and 43 (ipc-protocol); re-measure at Task 1 start and record it in the report.
- Builds and tests: `-j 8`, BelowNormal priority for long runs (`LONG_RUNS.md`). C: is nearly full: check `df` before big
  builds. The repo is LF; run Python edit scripts as `python -P script.py < /dev/null` (never a heredoc plus
  `< /dev/null`). Foreground tool calls time out at 120 s: background long commands and watch the log. The probe scripts
  run the *existing* exe: `cargo build` first (`cargo check`/`test` do not rebuild it).
- Hand-built test ids ("g", "p", "c") are not store ids. Writers update the mirror only for them and queue nothing.
- Jars are not graph nodes except as document nodes of kind Jar; `jar_defs` is untouched except where Task 2 says.
- Behavior stays as today except where a Ruling says otherwise.

## Rulings

1. **The mirror's `URL` is the shared (document) url.** `TabPayload.url` / `UnloadedTabPayload.url` stay as the *live*
   url (updated on every engine report); almost every read wants the live one, so reads barely move. Only the write side
   (`mark(URL)`), `shared_nodes`, `mirror_remote` and `LiveUrl.from` change.
2. **`URL` joins `MIRRORED` for every kind** (so `diff`'s skip is unconditional), but only tabs seed and read it.
   Content kinds keep reading their payload `url` until 2b-5; `mirror_remote` writing it for them is harmless dead state.
3. **Local pending beats remote for url, engine, jar, jar_set** (as 2b-1/2b-2). This is a small behavior change for url,
   where a remote value beats an unsaved local mark today. Live effects (`Navigate`, `start_swap_to`, jar switch) follow
   what `mirror_remote` accepted, not `last_shared`.
4. **`edit_url` is called with the value at mark time**: `navigate_tab` (typed) and an accepted `shares_url_change`
   report. A later redirect therefore cannot become the saved value (closes the hazard that `navigate_tab`'s immediate
   save works around today, which stays).
5. **Engine is written when the switch starts, not at landing:** `start_swap_to` and `wake_tab(id, kind)` with a new kind
   call `edit_engine`. A failed swap with `fallback_kind` calls `edit_engine(old)`: today's "failed swap reverts" result.
   A wake into another engine is therefore saved at wake start (a quit mid-wake keeps the new engine).
6. **Engine stays a raw `FieldValue::Engine(code)` in the mirror;** `Mirror::engine(id)` decodes (absent or 0 = Servo,
   via `engine_of`). An unknown code from a newer build is no longer overwritten by the next save (test it).
7. **One writer for both jar fields:** `App::edit_jar(id, JarChoice)` queues only the field(s) that changed.
8. **Pending `Id` Sets are resolved at commit by Ruling 5 of 2b-2** (live in the doc and not Deleted by this commit, or
   Created by it, else none), generalized from `PARENT` to every `Id` field, so `JAR` goes through it. A dangling JAR (its
   jar was deleted remotely) is no longer cleared by every save; the chip still shows "missing". `Create` also resolves
   `JAR` (it does not today); test a Create naming a jar the same commit deletes.
9. **Cascades and `refollow` iterate `linked_ids`, not `self.nodes`,** so a respawning tab follows a local dissolve or
   merge (resolves the 2b-2 parked "respawning tab keeps its old jar snapshot").
10. **Seeding:** `Mirror::seed` stays "only if no entry". `place_node` seeds url, engine and jar from a tab payload (tests
    build tabs with `add(sleeping_tab(..))`); landing seeds from `TabSpawn` through `Carried`; remote/import/restore nodes
    already go through `materialize` -> `mirror_remote` first.
11. **Stay runtime:** `swap_target` (the one-swap lock), `EngineTab.kind` (the running engine), `nav_history`,
    `LiveUrl`. **Stay diffed:** `DEFAULT_JAR` (group field; retires in C), `POS`, `NAME`, text/view fields (2b-4).
12. **`explicit` keeps only `POS`** afterwards; `FieldDef.explicit` for `URL` stays until 2b-5.

## File Structure

- `crates/manifold/src/mirror.rs`: `MIRRORED`, accessors `url`, `engine`, `jar_choice`, `Carried` fields, `seed`.
- `crates/manifold/src/session_sync.rs`: `edit_url`, `edit_engine`, `edit_jar`, `shared_nodes`, `spawn_shared`, `diff`,
  `mirror_remote`, `save_session` commit (Id resolution), `update_live_node`, `materialize`, `last_shared` use.
- `crates/manifold/src/main.rs`, `event_loop.rs`, `tabs.rs`, `view_modes.rs`, `jar_defs.rs`, `node_menu.rs`,
  `graph_nav.rs`, `fetched.rs`, `node.rs`: write sites (see each task).
- Tests: `crates/manifold/src/session_sync/tests.rs`, `jar_defs.rs`, `node_menu.rs`, `tabs.rs`.
- Docs: spec 2b note, `docs/superpowers/specs/2026-10-05-sync-flow-map.md`.

---

### Task 1: url

**Files:** `mirror.rs`, `session_sync.rs`, `main.rs` (`navigate_tab`, ~1850), `event_loop.rs` (~1013), `fetched.rs`
(`place_node`), `tabs.rs` (landing seed), tests.

**Interfaces:**
- Produces: `Mirror::url(&self, id) -> String`; `App::edit_url(&mut self, id: &str, url: &str)` (wraps `edit_field`
  with `FieldValue::Str`); `Carried.url: String`.

- [ ] **Step 1: Failing tests** (`session_sync/tests.rs`, copy the 2b-1 pattern at the tests near 2220-2448):
  - `a_navigation_saves_the_url_it_started_with_not_the_redirect`: `navigate_tab`-style `edit_url(id, "https://a")`, then
    the live `tab.url` changes to `"https://b"`, save: the stored URL is `a`.
  - `a_local_url_edit_wins_over_a_remote_one`: `edit_url`, then a remote `Set URL` arrives before the save: mirror and
    stored value are the local one, the live tab is not sent `Navigate`.
  - `a_remote_url_change_navigates_a_live_tab_and_moves_a_sleeping_one`: existing behavior, now through the mirror (keep
    `a_remote_url_change_moves_a_sleeping_tab_without_waking_it`).
  - `an_unmarked_live_url_is_never_saved` (the existing `a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another`
    must still pass unchanged).
- [ ] **Step 2: Run them to see the right failures** (`cargo test -j 8 -p manifold --bins <name>`).
- [ ] **Step 3: Implement.** `MIRRORED` gains `field::URL`; `Mirror::url` decodes the `Str`; `Carried` grows `url` and
  `Mirror::seed` seeds it; `place_node` seeds a tab's url from its payload and `shared_nodes`/`spawn_shared` read the
  tab's `url` from the mirror (content kinds still read their payload). `diff` stops treating `URL` specially (the
  `explicit` check for URL goes; `MIRRORED` skip covers it); the two `mark(URL)` sites become
  `edit_url(id, <url at that moment>)`. `mirror_remote` applies `URL` (skipping a field with a pending edit);
  `update_live_node` sends `Navigate` and calls `note_host_load`/`skip_convert` only when `mirror_remote` accepted the
  url. `LiveUrl.from` keeps reading `last_shared` unless the compiler shows it must move.
- [ ] **Step 4: Run the full suite, fmt, clippy.** Fix compiler-led fallout (about 40 tab-url reads should not change).
- [ ] **Step 5: Gate:** `cargo build`, then `scripts/two-instance-test.sh` scenarios 3, 4, 5, 7 (url sharing, nonce
  redirect, live_url); record results.
- [ ] **Step 6: Commit** `Url lives in the mirror`.

### Task 2: jar and jar_set

**Files:** `mirror.rs`, `session_sync.rs`, `jar_defs.rs` (`put_tab_jar`, `resnapshot`, `refollow`, `delete_jar`,
`set_group_default_jar` cascade, `tab_jar`), `node_menu.rs`, `graph_nav.rs`, `main.rs` (`TabCarry`, wake, local dissolve),
`tabs.rs`, `node.rs` (payload literals), tests.

**Interfaces:**
- Consumes: Task 1's `Carried` shape and `place_node` seeding.
- Produces: `Mirror::jar_choice(&self, id) -> JarChoice`; `App::edit_jar(&mut self, id: &str, choice: JarChoice)`
  queueing `JAR` and/or `JAR_SET` through `edit_field`; `Carried.jar: JarChoice`; `JarChoice` removed from
  `TabPayload`, `UnloadedTabPayload` and `TabCarry`.

- [ ] **Step 1: Failing tests:**
  - `a_jar_pick_saves_both_fields_and_only_the_changed_one` (jar and set; re-picking the same jar queues nothing).
  - `a_pending_jar_naming_a_jar_deleted_in_this_commit_is_written_as_none`, and the Create variant (Ruling 8).
  - `a_dangling_remote_deleted_jar_survives_a_save` (the accepted behavior change; chip logic unchanged).
  - `a_local_jar_pick_wins_over_a_remote_one`.
  - `a_respawning_tab_follows_a_local_dissolve_s_jar_snapshot` (the 2b-2 parked item, Ruling 9).
  - Existing `a_tabs_jar_and_a_groups_default_project_to_fields_and_back`, `a_remote_jar_edit_updates_a_sleeping_tab_and_a_group`,
    `deleting_a_jar_clears_the_tabs_and_groups_that_named_it` and the `jar_defs` tests must pass (edit them only to read
    through the mirror).
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement.** `MIRRORED` gains `JAR`, `JAR_SET`; generalize the commit-time target resolution in
  `save_session` from `PARENT` to every `Id` field in `pending`; `Create` resolves `JAR` too. Route `put_tab_jar`,
  `resnapshot`, `refollow`, the default-jar cascade and `delete_jar` through `edit_jar` (`delete_jar` stops writing
  payloads directly); `refollow` and the cascades iterate `linked_ids`. `tab_jar` and the direct payload reads
  (`will_switch`, `sweep_jar_switches`, wake/respawn carry, `resolved_jar(&spawn.carry.jar)` at spawn) read the mirror.
  `mirror_remote` applies `JAR`/`JAR_SET`; the jar switch (`sweep_jar_switches`) still runs after `apply_remote`.
  `shared_nodes` and `spawn_shared` read the jar from the mirror. Fix the literals the compiler lists
  (`UnloadedTabPayload {` about 9, `TabPayload {` 2, `TabCarry {` 5).
- [ ] **Step 4: Full suite, fmt, clippy.**
- [ ] **Step 5: Gate:** `cargo build`, then `scripts/jar-probe.sh`; record results.
- [ ] **Step 6: Commit** `Jar and jar_set live in the mirror`.

### Task 3: engine

**Files:** `mirror.rs`, `session_sync.rs`, `view_modes.rs` (`start_swap_to`), `tabs.rs` (fallback respawn, failed card,
`wake_tab` callers), `main.rs` (`wake_tab`), `event_loop.rs`, tests.

**Interfaces:**
- Consumes: Tasks 1 and 2 (`Carried`, seeding, `Id`-resolution change).
- Produces: `Mirror::engine(&self, id) -> <the kind type `engine_of` returns>`; `App::edit_engine(&mut self, id: &str,
  kind)` queueing `FieldValue::Engine(code)`.

- [ ] **Step 1: Failing tests (the survey found none of these today):**
  - `a_failed_swap_writes_the_old_engine_back` (start swap to B, spawn fails with `fallback_kind` A: stored engine is A).
  - `a_remote_engine_change_on_a_live_tab_starts_a_swap` and `a_local_swap_in_flight_wins_over_a_remote_engine`.
  - `a_wake_into_another_engine_is_saved_at_wake_start`.
  - `an_unknown_engine_code_survives_a_save` (Ruling 6).
  - `a_swap_saves_its_target_before_the_state_snapshot` (today's `swap_target.unwrap_or(kind)` behavior, now via
    `edit_engine` at `start_swap_to`).
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement.** `MIRRORED` gains `field::ENGINE`; `shared_nodes`/`spawn_shared` read it from the mirror (no
  more `swap_target.unwrap_or(tab.kind)`); `start_swap_to`, `wake_tab` with a new kind and the fallback path call
  `edit_engine`; `UnloadedTabPayload.kind` reads become `Mirror::engine`; `EngineTab.kind` and `swap_target` stay as they
  are (Ruling 11). Remote unloaded: `mirror_remote` applies it, the payload `kind` field is removed if no reader remains.
  A remote change on a live tab calls `start_swap_to` only when `mirror_remote` accepted it and no swap is in flight.
  Verify (and record) whether sleeping mid-swap is reachable; if it is, `unloaded_from` must take the mirror's engine.
- [ ] **Step 4: Full suite, fmt, clippy.**
- [ ] **Step 5: Commit** `Engine lives in the mirror`.

### Task 4: Docs and final verification

**Files:** `docs/superpowers/specs/2026-10-04-node-model-design.md`, `docs/superpowers/specs/2026-10-05-sync-flow-map.md`.

- [ ] **Step 1:** Spec 2b paragraph: add "2b-3 done: <hashes>. url (shared url; the payload keeps the live url), engine,
  jar and jar_set live in the mirror; `explicit` keeps only POS. Next: 2b-4 (name, text, view, pos: the document `pos`
  goes in the mirror, the layout pos/vel become a runtime side-table); 2b-5 (content kinds, then delete `diff`,
  `explicit`, `keep`, the enums and the caps bridge, rewrite fixtures, add the grep guard)."
- [ ] **Step 2:** Sync map "After 2b-3": which fields are mirror-only, the generalized `Id` resolution, engine written at
  switch start, local pending wins for url.
- [ ] **Step 3:** `cargo build` then `scripts/headless-smoke-test.sh` (about 3 minutes; background it), and the scale test
  if the project docs call for it after sync changes. Run fmt, clippy and the full suite. Record machine load.
- [ ] **Step 4: Commit** `Docs: 2b-3 note and sync map`.

---

## Self-review (run 2026-10-05)

- **Spec coverage:** section 6 item 4 (one validity rule) extends to `JAR` (Ruling 8, Task 2); item 10 (direct ops)
  advances for four field kinds; items 2, 3, 6-9 belong to later plans. The parked 2b-2 item about `refollow` and
  respawning tabs is Task 2.
- **Placeholders:** none intended; the compiler-led read-site conversions are listed by file in the survey, not
  enumerated, because the compiler enumerates them exactly.
- **Type consistency:** `Carried` gains `url` (Task 1) then `jar` (Task 2); the engine accessor's return type is whatever
  `engine_of` returns, named in Task 3.
- **Risks:** Task 2 is widest (about 20 reads, 8 write paths, cascades) and may need two review rounds. Task 1's
  remote-url precedence change and Task 3's raw-engine behavior change are deliberate and tested. The Task 1 gate needs a
  fresh `cargo build` or it probes the old exe. A respawning tab with no mirror entry must be excluded from
  `linked_ids` (2b-2 Task 3 fix) when cascades widen.
- **Unverified at writing:** whether sleeping mid-swap is reachable (Task 3 Step 3 checks); exact test line numbers.

---

## Outcome (2026-10-05)

Done on `dev`: aa95040, b51f345 (url); 02810e6 (jar, jar_set); f1626b8, 637d7c0 (engine); f92ae1f (docs); 58835f1, 0b01986 (final-review fixes). 816 tests pass (4 ignored) plus 43 in ipc-protocol; fmt and clippy clean; two-instance scenarios 3,4,5,7, `jar-probe.sh` and the headless smoke test passed (smoke ran at about 53% CPU from other sessions, Defender real-time protection off). `scale-test.sh` was not run.

### Rulings I made

- Recently Closed keeps recording a tab's live url (no ruling changes it; one extra live-url read in `closed_entry`).
- Task 2 got no fix round (only minors); minors 1, 4, 5 were folded into the final-review fix.
- A crash mid-swap finishes the swap, and duplicating a tab mid-swap uses the target engine (the mirror holds the target; reverting needs a second source of truth). Cost: a tab restarts on a target that may fail; the fallback covers it.
- After a swap's own edit is saved, a later remote engine change is overwritten by the target, also for swaps another replica started (documented in the sync map).
- One fix dispatch covered the final review's Important (a remote url/engine change parked during a spawn lost its live effect), the mid-spawn jar delete, the test gaps and the doc fixes.

### Parked (not fixed)

- `seed_landing` clears any *new* spawn whose jar is missing from `jar_defs`, not only one deleted mid-flight: a duplicate of a tab holding a dangling jar, or a new tab in a group whose `DEFAULT_JAR` dangles, lands on the default jar. The doc comment and sync map describe only the mid-flight case.
- Per-field last-writer-wins can mix a jar choice (jar and jar_set are separate fields); a pending Set equal to the stored value is still written; the post-commit mirror correction covers a created tab's JAR only.
- An engine pick during the cookie hold (up to 2 s) is dropped silently; the picker isn't greyed out. A crash card after a crash mid-swap names the target engine with the old engine's crash reason. Duplicating mid-swap has no fallback if the target fails to start.
- `NodePayload::saved_tab()` now returns only the live url, so its name is a slight misnomer.
- 2b-2's parked items still stand (cycle-refused Move leaves the mirror wrong, `merge_groups` loop guard, `children_of` cost, local cascade vs parked remote entry, `destroy_group` with a respawning member).
- Commit trailers for Tasks 2-3 and the fixes name "Claude Opus 5.5", not Sonnet 5.5 (the implementers were Opus).
