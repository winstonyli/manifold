# Node model 2b-4: pos, text, view and name in the mirror Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A node's document `pos`, a note's `text`, a JSON node's `view` and a group's `name` become mirror state written
through `edit_*` writers; the layout `pos`/`vel` become a runtime side-table; `explicit` loses its last user.

**Architecture:** Same shape as 2b-1..2b-3. `MIRRORED` grows by `POS`, `TEXT`, `VIEW`, `NAME`; writers queue into
`App::pending`; `mirror_remote` applies another replica's values except where a local edit is pending (or, for text,
being typed). Document `pos` stops being the layout position: physics and drags move a per-id layout table that the
document never reads, and only a drag release (and placement at create) writes the document.

**Tech Stack:** Rust, egui, the in-repo `store` journal.

**Spec:** `docs/superpowers/specs/2026-10-04-node-model-design.md` (2b "Read model"; section 6 items 2, 3, 10).
Facts about today's flow: `docs/superpowers/specs/2026-10-05-sync-flow-map.md`. Follows
`2026-10-05-node-model-2b-3-url-engine-jar.md` (read its Rulings and its Outcome "Parked" list; they all still hold).
Survey this plan was written from (git-ignored, may be gone): `.superpowers/scratch-2b4-map.md`.

## Global Constraints

- Work on `dev`, never on `main` (`CLAUDE.md`). Commit with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Run `cargo fmt --all` before each commit. `cargo fmt --all --check` and
  `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass.
  Never bypass `.githooks/pre-commit`. Don't silence a lint to pass: fix it, or `#[allow]` it with a comment saying why.
- No test may read or write the real `%APPDATA%\manifold`. Tests that need files use `store::testutil::temp_dir`.
- Test baseline before this plan: `cargo test -j 8 -p manifold --bins -p ipc-protocol`: 820 passed, 4 ignored (manifold
  --bins) and 43 (ipc-protocol); re-measure at Task 1 start and record it in the report.
- Builds and tests: `-j 8`, BelowNormal priority for long runs (`LONG_RUNS.md`). C: is tight: check `df` before big
  builds. The repo is LF; run Python edit scripts as `python -P script.py < /dev/null` (never a heredoc plus
  `< /dev/null`, never bare `python`). Foreground tool calls time out at 120 s: background long commands and check them.
  The probe scripts run the *existing* exe: `cargo build` first.
- Hand-built test ids ("g", "p", "c") are not store ids. Writers update the mirror only and queue nothing for them.
- `names.rs` has a guard that counts any `.name` token followed by a non-identifier character; a `mirror.name(id)`
  accessor counts as a read and needs `// name rule:` marks or an ALLOWED change (see names.rs).
- Behavior stays as today except where a Ruling says otherwise.

## Rulings

1. **Document pos is mirror state; layout pos/vel is a runtime side-table** (the user's design, spec 2b). `Mirror::pos(id)
   -> Option<egui::Vec2>` is the document value (`None` = absent). `App::layout: HashMap<String, Layout>` with
   `struct Layout { pos: egui::Vec2, vel: egui::Vec2 }` holds what the graph draws; `GraphNode.pos`/`vel` go. The table
   is keyed by id and is not cleared by a respawn, so `TabSpawn`/`TabCarry` stop carrying pos and vel.
2. **Physics never writes the document** (as today). Only a drag release (`edit_pos` where `mark(POS)` is today,
   card.rs ~1162) and placement at create write it.
3. **Create stores the placement position, not the post-settle one.** `placement` (fetched.rs ~56) is deterministic; the
   mirror's POS is seeded with it at insert, and the layout starts there too. This changes today's "layout pos at the
   first save that sees the node": the fixture that expects the layout (1,1) (session_sync/tests.rs ~662) is rewritten.
4. **Remote pos:** `mirror_remote` sets the document pos unless a local pos is pending; the layout entry snaps to it and
   its vel zeroes, except for a node being dragged right now (skip the snap; the release then wins). Restore: the
   mirror gets the *document* pos, the layout gets the personal `LocalNode.pos` if there is one, else the document pos.
5. **Pos (0,0) stays "no pos"** in the encoding; documented, not changed.
6. **Text: the payload stays the live edit buffer while the box has focus.** `edit_text(id, text)` runs on blur and, for
   each node being edited, at `save_session` time (so a periodic save still commits what was typed). No per-keystroke
   mirror write: a note can reach 1 MB. Per-frame readers use a borrowing accessor `Mirror::text(&self, id) -> &str`
   (no clone); `Mirror::get`/`edit_field` clones are kept off the per-frame path.
7. **A remote text change while the note is being typed in is held:** `mirror_remote` returns TEXT as held back (as for a
   pending local edit); the held value is remembered in `App::held_text`. At blur: if the buffer is unchanged from focus,
   the held remote value is applied; if the user changed it, `edit_text(local)` wins (local pending wins, as 2b-3
   Ruling 3) and the held value is dropped. Today's remote-overwrites-typing is deliberately removed.
8. **View: the mirror holds the `Str`, plus a decoded `JsonView` cache** refreshed whenever the mirror's view changes
   (local `edit_view`, remote). The per-frame readers (json_level_ui.rs ~533, ~682) read the cache. A reload's
   remove-and-reinsert (fetched.rs ~301, ~341) must keep the view (mirror entries survive until `retain`; test it).
9. **Name:** group names only (`edit_name` at `CommitRename` and via `mirror_remote`); jar names stay in `jar_defs`
   and stay diffed (2b-5/C).
10. **Recently Closed keeps recording the layout pos** (what the user saw), as for the live url in 2b-3.
11. **`explicit`/`mark`/`FieldDef.explicit` stay as dead machinery** until 2b-5 deletes them; Task 1 adds a test that
    `explicit` is never populated. `diff` skips POS, TEXT, VIEW, NAME for existing nodes via `MIRRORED`.
12. **Left for 2b-5** (not touched here): `FROM_JSON`, `DEFAULT_JAR`, `PATH`, `KEY`, `VALUE`, `TYPE`, jar `NAME` and
    `EPHEMERAL`, content-kind `url`, the caps-bridge blank tag, `diff`, `explicit`, `keep`, the enums.

## File Structure

- `crates/manifold/src/mirror.rs`: `MIRRORED`, accessors `pos`, `text`, `view`, `name`, `Carried` fields, `seed`.
- `crates/manifold/src/session_sync.rs`: `edit_pos`, `edit_text`, `edit_view`, `edit_name`, `shared_nodes`, `diff`,
  `mirror_remote`, `update_live_node`, `materialize`/restore seeding, `held_text`.
- `crates/manifold/src/graph_nav.rs`, `graph_ui.rs`, `card.rs`, `fetched.rs`, `tabs.rs`, `main.rs`, `closed.rs`: pos
  read/write sites (layout side-table, drag, placement, respawn carry, Recently Closed).
- `crates/manifold/src/node.rs`, `json_level.rs`, `json_level_ui.rs`, `node_menu.rs`, `names.rs`: text/view/name sites.
- Tests: `crates/manifold/src/session_sync/tests.rs` and the modules above. Docs: spec 2b note, sync-flow map.

---

### Task 1: pos

**Files:** `mirror.rs`, `session_sync.rs`, `main.rs` (`App::layout`, `TabSpawn` carry), `graph_nav.rs` (settle, join/eject),
`graph_ui.rs` (drag), `card.rs` (~1162 drag release), `fetched.rs` (`placement`, import), `tabs.rs` (spawn near parent,
landing), `closed.rs`, tests.

**Interfaces:**
- Produces: `Mirror::pos(&self, id) -> Option<egui::Vec2>`; `App::edit_pos(&mut self, id: &str, pos: Option<egui::Vec2>)`
  (wraps `edit_field` with `FieldValue::Pos`); `App::layout: HashMap<String, Layout>`; `Carried.pos`.

- [ ] **Step 1: Failing tests** (`session_sync/tests.rs`; copy the 2b-3 test patterns):
  - `a_drag_release_saves_the_dropped_position`: `edit_pos`, save, the stored POS is that value; settle moves the layout
    entry and the stored POS does not change.
  - `a_node_is_created_at_its_placement_not_its_settled_position`: replaces the (1,1) fixture.
  - `a_local_pos_edit_wins_over_a_remote_one` and `a_remote_pos_snaps_the_layout_and_zeroes_its_velocity`.
  - `a_remote_pos_does_not_snap_a_node_being_dragged`.
  - `a_respawn_keeps_the_layout_position` (the table survives remove-and-reinsert without `TabSpawn` carrying it).
  - `a_restart_places_a_node_at_its_personal_position_while_the_mirror_holds_the_document_one`.
  - `explicit_is_never_populated` (nothing marks any field).
  - Recently Closed records the layout pos (existing test kept or added).
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement.** `MIRRORED` gains `field::POS`; `Mirror::pos` decodes `FieldValue::Pos`; `Carried` grows `pos`
  and `seed` seeds it. Introduce `App::layout`; replace every `GraphNode.pos`/`vel` read and write with the table
  (compiler-led; `settle_current_level`, drag, join/eject, spawn placement, `TabSpawn`'s pos/vel carry removed).
  `placement` seeds both the mirror POS (via `edit_pos` or seeding for a not-yet-created node) and the layout.
  `shared_nodes`/`spawn_shared` read POS from the mirror; `diff` stops treating POS specially. The drag release calls
  `edit_pos`; `mark(POS)` goes. `mirror_remote` applies POS (skipping a pending one); `update_live_node` snaps the layout
  unless the node is being dragged. `closed_entry` records the layout pos.
- [ ] **Step 4: Full suite, fmt, clippy.**
- [ ] **Step 5: Gate:** `cargo build`, then `scripts/two-instance-test.sh` (all scenarios) and
  `scripts/headless-smoke-test.sh` (about 3 minutes, background); record results and machine load.
- [ ] **Step 6: Commit** `Pos lives in the mirror; layout is a side-table`.

### Task 2: text

**Files:** `mirror.rs`, `session_sync.rs`, `node.rs`, `card.rs` (text box, blur), the caps-bridge reader if any, tests.

**Interfaces:**
- Consumes: Task 1's `mirror_remote` held-back return and seeding shape.
- Produces: `Mirror::text(&self, id) -> &str`; `App::edit_text(&mut self, id: &str, text: &str)`; `App::held_text:
  HashMap<String, String>`; an `editing` notion for a note (the focused text box's node id).

- [ ] **Step 1: Failing tests:**
  - `typing_commits_on_blur_and_at_save`: text typed, a save while editing commits it; blur commits it.
  - `a_remote_text_change_is_held_while_the_note_is_typed_in` and its two outcomes: user changed text -> local wins at
    blur and the held value is dropped; user did not change -> the held remote value applies at blur.
  - `a_large_note_edit_does_not_clone_per_frame` (assert the accessor borrows: a compile-level check via `&str`, plus a
    test that `Mirror::text` is not `Mirror::get`-based).
  - `a_remote_text_change_updates_an_idle_note` (existing behaviour).
  - The existing 538 test (`a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another`) must pass unchanged.
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement.** `MIRRORED` gains `field::TEXT`; `Mirror::text` borrows from the stored `Str`; the per-frame
  readers (card.rs ~870/1452/1512, node.rs ~442) use it. The text box keeps editing the payload buffer; blur and
  `save_session` (for nodes being edited) call `edit_text`; `mirror_remote` holds TEXT for an edited node and
  `update_live_node` respects the held-back set; `held_text` resolves at blur per Ruling 7. `shared_nodes` reads TEXT
  from the mirror. `diff` skips TEXT via `MIRRORED`.
- [ ] **Step 4: Full suite, fmt, clippy.**
- [ ] **Step 5: Commit** `Text lives in the mirror`.

### Task 3: view and name

**Files:** `mirror.rs`, `session_sync.rs`, `json_level.rs` (`edit_json_view`), `json_level_ui.rs`, `fetched.rs`
(reload), `node_menu.rs` (`CommitRename`), `names.rs`, tests.

**Interfaces:**
- Consumes: Tasks 1-2 patterns.
- Produces: `Mirror::view(&self, id) -> Option<&JsonView>` (the decoded cache; refreshed on every mirror view change);
  `App::edit_view(id, &str)`; `Mirror::name(&self, id) -> &str`; `App::edit_name(id, &str)`.

- [ ] **Step 1: Failing tests:** a rename saves and a remote rename reaches the mirror; a pending local rename wins over
  a remote one; a JSON view edit saves and a remote one refreshes the decoded cache; a reload keeps the view;
  `names.rs`'s guard test still passes with the new accessor marked.
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement.** `MIRRORED` gains `field::VIEW`, `field::NAME`; the decoded-view cache lives beside the
  stored string and is refreshed in the one place the mirror's VIEW is written; `edit_json_view` becomes `edit_view`;
  `CommitRename` calls `edit_name`; group-name reads use `Mirror::name` with the `names.rs` marks. Jar names are
  untouched.
- [ ] **Step 4: Full suite, fmt, clippy.**
- [ ] **Step 5: Commit** `View and name live in the mirror`.

### Task 4: Docs and final verification

**Files:** `docs/superpowers/specs/2026-10-04-node-model-design.md`, `docs/superpowers/specs/2026-10-05-sync-flow-map.md`;
`crates/manifold/src/debug_control.rs` and `scripts/two-instance-test.sh` if extending the scenarios (below).

- [ ] **Step 1:** Spec 2b paragraph: "2b-4 done: <hashes>. pos (document pos in the mirror; layout pos/vel in
  `App::layout`), text, view and name live in the mirror; `explicit` has no users. Next: 2b-5 (content kinds, then
  delete `diff`, `explicit`, `keep`, the enums and the caps bridge, rewrite fixtures, add the grep guard)."
- [ ] **Step 2:** Sync map "After 2b-4": document pos vs layout pos, placement at create, remote pos snap rule, text held
  while typed and resolved at blur, view cache, name; and the stale text that still says POS is explicit-only.
- [ ] **Step 3:** Extend `EXPORTSTATE` with name, text and pos, and add two-instance scenarios 11-12: A renames a group
  and edits a note's text, B follows; both edit the same note's text (local wins, converge). Skip this step and say so
  if the debug commands cannot drive a rename or text edit headlessly without a larger change.
- [ ] **Step 4:** `cargo build`, `scripts/two-instance-test.sh`, `scripts/headless-smoke-test.sh` (background), full suite,
  fmt, clippy. Record machine load and Defender state.
- [ ] **Step 5: Commit** `Docs: 2b-4 note and sync map` (and the script change as its own commit).

---

## Self-review (run 2026-10-05)

- **Spec coverage:** the user's design for pos (document in the mirror; layout side-table; remote placement snaps the
  layout) is Rulings 1-4 and Task 1; section 6 item 10 (direct ops) advances for four fields; items 2-3 and 6-9 belong to
  later plans. The 2b-5 leftovers are listed in Ruling 12.
- **Placeholders:** none intended; compiler-led read-site conversions are listed by file in the survey, not enumerated.
- **Type consistency:** `Carried` gains `pos` in Task 1; `held_text` and `editing` are defined in Task 2 and not used
  earlier. `Mirror::view` returns a decoded cache, so Task 3 owns its refresh invariant.
- **Risks:** Task 1 is widest (about 30 `GraphNode.pos`/`vel` sites, drag, physics, respawn) and may need two review
  rounds; the Create-position change is deliberate and tested. Task 2's held-remote rule is the one new user-visible
  behaviour (typing is no longer overwritten by a remote edit); the cost is a held value possibly lost if the app quits
  mid-edit (the document keeps the remote value, the local buffer is saved by the periodic save). A node mid-drag when its
  document pos changes remotely is not snapped; the release then wins.
- **Unverified at writing:** `FieldValue::Pos` details and the exact `placement` seeding path; whether the headless debug
  commands can drive a rename or text edit (Task 4 Step 3 checks).

## Outcome (2026-10-05)

Done on `dev`: a9c0149, 3b0cdc8 (pos; the second clears a stale drag); 51edf26, 35138e4 (text; the second adds the
held-text test); ceefbe2 (view, name); plus this docs commit. 843 tests pass (4 ignored) plus 43 in ipc-protocol (note:
`cargo test -p manifold --bins -p ipc-protocol` runs no ipc-protocol tests, since `--bins` skips its lib; run it
separately); fmt and clippy clean. `two-instance-test.sh` scenarios 1-10 and `headless-smoke-test.sh` (48 checks)
passed at 85-100% CPU from other sessions' jobs, Defender real-time protection off.

### Rulings made

- **Task 2 deviated from Ruling 7: a held remote text is resolved against "unchanged since the last commit", not "since
  focus".** Every save commits any note whose box differs from the mirror (`commit_note_buffers`), so a remote text
  that arrives after a local commit is held while the note is typed in and applies at blur, unless the user typed or
  committed more (`end_note_edit`: buffer equal to the mirror and no TEXT pending). A commit carrying a pending TEXT
  drops a held value older than it. Read literally, "since focus" would leave the mirror showing the local text while
  the document held the remote one. Cost: a user who typed, got saved, then stopped typing sees a later-arriving remote
  text replace theirs at blur.
- **`is_typed_in` is the `NotePayload::editing` flag, not egui focus** (Task 2 review M1, accepted). A restored note
  whose personal state says `editing`, or a new note that never gets focus, holds remote text with no time limit, until
  its box blurs or `end_note_edits` runs.
- **The note payload stays a full copy of the text** (Task 2), because the 538 test must pass unchanged: a 1 MB note is
  held twice, and a commit of a large note copies it a few times.
- **Task 3 deleted `GroupPayload::name` and `JsonPayload::view`:** the mirror is the only copy. A new group is placed with
  `App::place_group(id, name, at)`.
- **A reload's view prune is now a pending local edit** (Task 3 review M1, accepted): a remote view edit arriving before
  the next save loses to it (last writer wins per field, as 2b-3 Ruling 3).
- **Task 1's review fix** (3b0cdc8) ended a drag whose grabbed card left the level or whose Graph view was left
  (`live_drag`, `end_stale_drag`), so a stale release can't overwrite a remote pos.
- **Task 4 Step 3 skipped** (`EXPORTSTATE` name/text/pos and two-instance scenarios 11-12). The debug commands can't
  drive either edit headlessly without new commands. A rename (`RENAMEGROUP`, then `EGUITEXT`, `ENTER`) renames only
  the *current* group, and no command enters a group by id: `GRAPHOPEN` opens the focused card, and focus order among
  the earlier scenarios' cards isn't predictable. A new note starts `editing` but its `TextEdit` never requests focus,
  so `EGUITEXT` types nowhere until a `CLICK` at a screen point computed from `EXPORTSTATE`'s `rect`, and the blur
  needs another click on empty canvas. A follow-up would add by-id commands (enter a group; focus, type into and blur
  a note) and then the scenarios. **Done in a follow-up** (commit "Debug commands and two-instance scenarios for name,
  text and pos"): by-id `RENAMEGROUP id name`, `SETNOTETEXT`, `MOVENODE`, `EXPORTSTATE` `mirror_names`/`mirror_texts`/
  `mirror_pos`, and scenarios 11-12. `SETNOTETEXT` types and blurs in one command, so the back-to-back text edits
  never exercise a held remote text; the unit tests cover that path.

### Parked (not fixed)

- A node in neither `nodes` nor `spawning` at a successful save loses its `App::layout` entry. None was found, but such
  a node reinserted later lands at its document pos, not where it was shown.
- A remote pos for a card in `drag_moved` is ignored until release, while the drag is live; a drag that stopped being
  live is cleared by `end_stale_drag`. Neither is covered by the two-instance test.
- `wake_tab` and `TabSpawn::respawn` compute a pos nobody uses: the layout entry persists.
- Restore edge: a node whose stored fields include no MIRRORED field gets its personal pos seeded into the mirror as its
  document pos (the same edge as colour).
- The card.rs drag body has no unit test (`release_drag` is tested).
- VIEW is no longer canonicalized at Create: the mirror's raw string is written, not re-encoded through `JsonView`.
- `App::own_text` returns a group's raw name. A caller that draws it must apply a name rule; the names.rs guard sees
  only the one mark inside `own_text`.
- `place_group` and `place_node` decide "fresh" differently: `place_group` checks `mirror.has` (any field or link
  entry), and `place_node` seeds through `Mirror::init` (no field entry).
- No two-instance scenario covers a rename, a text edit or a view edit (Step 3 above).
- 2b-3's parked items still stand.
- The commit trailers of Tasks 1-3 and their fixes say "Claude Opus 5.5", not Sonnet 5.5: the implementers were Opus.
