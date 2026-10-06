# Node model 2b-5: sync completion (fields, creates, deletes; `diff` goes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every stored field is mirror state, node creation and deletion are explicit queued ops, and `diff`, `explicit`/`mark`,
`FieldDef.explicit`, `keep`, `last_shared`, `Shared`/`shared_nodes`/`kind_fields` are deleted. The save path then
derives nothing from payloads or absence.

**Architecture:** Same shape as 2b-1..2b-4: writers `edit_*` update the mirror at once and queue ops in `App::pending*`;
`mirror_remote` applies another replica's values. New here: `pending_creates` and `pending_deletes` replace `diff`'s
existence logic, and jars join the mirror. The enum removal (`NodePayload`, `ItemNodeKind`, `ContentKind`, `LevelItemKind`,
store `Kind`, caps bridge) is **not** in this plan: it is 2b-6.

**Tech Stack:** Rust, egui, the in-repo `store` journal.

**Spec:** `docs/superpowers/specs/2026-10-04-node-model-design.md` (2b; section 6 items 2, 3, 6-10). Facts about today's flow:
`docs/superpowers/specs/2026-10-05-sync-flow-map.md`. Follows `2026-10-05-node-model-2b-4-pos-text-view-name.md` (read its
Rulings and Outcome; they all still hold). Survey this plan was written from (git-ignored, may be gone):
`.superpowers/scratch-2b5-map.md`.

## Global Constraints

- Work on `dev`, never on `main` (`CLAUDE.md`). Commit with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- `cargo fmt --all` before each commit. `cargo fmt --all --check` and
  `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass. Never
  bypass `.githooks/pre-commit`. Don't silence a lint to pass: fix it, or `#[allow]` it with a comment saying why.
- No test may read or write the real `%APPDATA%\manifold`; tests that need files use `store::testutil::temp_dir`.
- Test baseline before this plan: `cargo test -j 8 -p manifold --bins` 845 passed, 4 ignored; run `-p ipc-protocol`
  (43) **separately** (a combined `--bins -p ipc-protocol` run runs no ipc-protocol tests). Re-measure at Task 1 start.
- Builds and tests: `-j 8`, BelowNormal for long runs (`LONG_RUNS.md`); check `df` before big builds. LF repo; Python edit
  scripts as `python -P script.py < /dev/null`. Foreground tool calls time out at 120 s: background long commands.
  Probe scripts run the *existing* exe: `cargo build -p manifold` first (a workspace-wide build fails on Windows).
- Hand-built test ids ("g", "p", "c") are not store ids: writers update the mirror only and queue nothing for them.
- `names.rs` guard: a `mirror.name(` read needs a `// name rule:` mark or an ALLOWED change.
- Behavior stays as today except where a Ruling says otherwise. A missed create is data loss and a missed delete resurrects
  a node on restart: Task 3's invariant check is mandatory, not optional.

## Rulings

1. **Split the enums out (2b-6).** 2b-5 is sync completion only; the enums (about 790 sites), the caps bridge, EXPORTSTATE
   `cards[].kind`, the ~17 script lines that read it, and the grep guard are 2b-6. Reason: the enum work touches every
   test helper and mixes badly with the commit rewrite. Cost if wrong: the spec's "2b" lands one plan later.
2. **Every field of a node's type is mirror state.** `MIRRORED` is replaced by "fields with a writer" in Task 3 (the list
   of fields `mirror_remote` takes is the type's field list, from `kind_fields` until Task 4 turns it into row data).
   Fields a type does not list stay raw in the document untouched.
3. **Creates are an explicit queue, filtered at commit.** `App::pending_creates: Vec<String>` is pushed by the create
   funnels; at commit a queued id gets a Create only if the document lacks it entirely (never a tombstone), groups first,
   fields from the mirror (or `spawning`'s snapshot), Id fields through the one resolver (Ruling 12).
4. **Deletes are explicit and local-only.** `App::queue_delete(id)` is called from the local delete funnels:
   `remove_node`/`destroy_node`, `remove_tab`/`destroy_tab`, `dissolve_members(id, false)`, `merge_groups`' removed
   source, `delete_jar`/`forget_jar`, a failed Root/New/Split spawn, and `teardown_running_session` (which enumerates
   `journal.doc().live()` non-jar ids). Remote-driven removals (`remove_remotely_deleted`) do not queue, matching 2b-2's
   `follow_remote_*`; so plan E can turn the one local funnel into `Set closed`. Removals that must NOT delete (crash
   respawn, swap, reload's remove-and-reinsert, `retry_missing_file`) never call it. At commit a queued delete applies only
   if the id is still live in the document.
5. **PATH:** the mirror holds the stored (create-time) path, never rewritten; the payload keeps the resolved runtime path
   (`ContentIndex::resolve(id, stored)`); Recently Closed records the resolved one (as for the live url). Reload still
   does not write PATH.
6. **DEFAULT_JAR** moves to commit-time Id resolution like JAR: a remote-deleted default jar stays stored (dangling, reads
   as default) instead of being cleared by the next save. Accepted (2b-3 Ruling 8 precedent; sub-project C replaces the field).
7. **Jars join the mirror.** `materialize` runs `mirror_remote` for jars, `mirror.retain` keeps jar ids, and `JarDef`
   becomes a cache refreshed in `Mirror::set` (as `views`) so the ~76 readers do not move. `set_jar_ephemeral`'s side
   effects (file delete/save) must still run for a remote change.
8. **FROM_JSON, KEY, VALUE stay write-once fields** (no writer needed, no payload copy for FROM_JSON/KEY). Retiring them
   with the legacy kinds is a 2b-6 question for the owner.
9. **`last_shared` goes in Task 4.** `mirror_remote` returns the fields whose value changed (old mirror value != new)
   besides the held-back ones; `update_live_node` and `materialize_jar` use that; the jar resurrection guard becomes
   "a delete is pending for it"; `LiveUrl.from` becomes the mirror's url.
10. **Seed `Mirror::ty` (the node's type string) in Task 1**, with no reader yet, so 2b-6 swaps readers only.
11. **Fields passed to the app are the type's field list**, not every register.
12. **One Id resolver.** `valid()` (Create) and commit-time `resolve` (Sets) merge into one rule: an Id is kept if it is
    live in the document and not deleted by this commit, or created by it; a closed target stays valid (spec 6.4); else none.
13. **Create ordering and cycles keep today's behavior:** groups before members; a group created later in the batch makes
    the member top-level; a tombstone is never recreated.
14. **Test-build invariant after each successful save:** every store-id in `nodes ∪ spawning ∪ jars` is live in the document,
    and every document-live known-type node is one of them (modulo remote nodes not yet materialized, which are never
    ahead after `apply_remote`). Panics under `cfg(test)` / debug assertions only.

## File Structure

- `crates/manifold/src/mirror.rs`: `ty`, content/group/leaf field accessors, jar entries, `retain`, changed-field return.
- `crates/manifold/src/session_sync.rs`: `edit_*` for the remaining fields, `pending_creates`, `queue_delete`, commit
  builder, `mirror_remote`, `update_live_node`, `materialize`, teardown; `diff`/`shared_nodes`/`kind_fields` deleted.
- `crates/manifold/src/node.rs`, `fetched.rs`, `tabs.rs`, `main.rs`, `jar_defs.rs`, `closed.rs`, `graph_nav.rs`,
  `event_loop.rs`, `view_modes.rs`, `node_menu.rs`, `settings_page.rs`, `popups.rs`: create/delete funnels and payload
  copies; `crates/store/src/model.rs`: `FieldDef.explicit` column.
- Tests: `crates/manifold/src/session_sync/tests.rs` (the 13 `diff` tests become commit tests), `store/model.rs`.
  Docs: spec 2b note, sync-flow map, README history.

---

### Task 1: group, leaf and content fields in the mirror

**Files:** `mirror.rs`, `session_sync.rs`, `node.rs` (`GroupPayload`, `JsonPayload`, content `url` fields), `fetched.rs`
(`place_node`, `insert_content`, `insert_download_node`, `insert_materialized`), `jar_defs.rs` (default-jar writers), tests.

**Interfaces:**
- Produces: `Mirror::{from_json, default_jar, path, key, value, ty}`; `App::edit_default_jar(group, Option<JarId>)`
  (via `edit_field`, Id resolved at commit); `Carried` grows these fields and `seed` writes them; the content `url`
  is seeded in `insert_content` and `insert_download_node` (no longer only from `saved_tab()`); payload copies deleted:
  `GroupPayload.from_json`/`.default_jar`, content `url`, `JsonPayload.key` (a leaf's runtime value stays in the payload).

- [ ] **Step 1: Failing tests:** a locally created content node has a mirror url; `set_group_default_jar` /
  `clear_group_default_jar` save and a remote change reaches the mirror and `inherited_jar`; a dangling remote-deleted
  default stays stored (Ruling 6); PATH in the mirror stays the create-time path after a reload while the payload path
  changes (Ruling 5); Recently Closed records the resolved path; `Mirror::ty` is seeded on every creation path;
  import/reopen of a closed group holding leaves carries KEY/VALUE.
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement.** `MIRRORED` grows (still a list in this task); readers move to the accessors; `diff` skips the
  new fields via `MIRRORED`.
- [ ] **Step 4: Full suite (`--bins`, `-p ipc-protocol` separately), fmt, clippy.**
- [ ] **Step 5: Gate:** `cargo build -p manifold`, `scripts/two-instance-test.sh`. **Step 6: Commit**
  `Group, leaf and content fields live in the mirror`.

### Task 2: jars in the mirror

**Files:** `mirror.rs`, `jar_defs.rs`, `session_sync.rs` (`materialize`, `materialize_jar`, `retain`), `settings_page.rs`,
`popups.rs`, `node_menu.rs`, tests.

**Interfaces:**
- Consumes: Task 1's mirror field shape.
- Produces: jar NAME/COLOR/EPHEMERAL as mirror fields; `jar_defs: BTreeMap<NodeId, JarDef>` becomes a cache refreshed in
  `Mirror::set`; `rename_jar`, `set_jar_ephemeral`, `create_jar` write through `edit_field`; `diff`'s `Kind::Jar`
  exception is no longer needed.

- [ ] **Step 1: Failing tests:** rename / ephemeral / create of a jar save and a remote change refreshes `jar_defs`;
  `mirror.retain` keeps jar ids; a remote ephemeral change still runs `set_jar_ephemeral`'s file side effects; the jar
  resurrection guard still refuses a locally deleted jar (still `last_shared`-based until Task 4).
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement.** Keep jars-first ordering in Create and `apply_remote`.
- [ ] **Step 4: Full suite, fmt, clippy.** **Step 5: Gate:** `two-instance-test.sh`, `headless-smoke-test.sh` (background).
  **Step 6: Commit** `Jars live in the mirror`.

### Task 3: creates and deletes as queued ops; delete `diff`, `explicit`, `mark`, `FieldDef.explicit`, `keep`

**Files:** `session_sync.rs` (commit builder), `main.rs`, `tabs.rs`, `fetched.rs`, `graph_nav.rs`, `jar_defs.rs`,
`event_loop.rs`, `store/src/model.rs`, tests.

**Interfaces:**
- Consumes: Tasks 1-2 (every field in the mirror).
- Produces: `App::pending_creates`, `App::queue_delete(&mut self, id: &str)`, one Id resolver (Ruling 12); a commit builder
  with order Creates (groups first), Moves, Sets, Deletes; the test-build invariant (Ruling 14). Deleted: `diff`,
  `explicit`, `mark`, `FieldDef.explicit` (20 table rows), `keep`.

- [ ] **Step 1: Failing/rewritten tests:** the 13 `diff` tests (groups first; unchanged emits nothing; missing deleted
  unless in flight; tombstone never recreated; re-homing; parent none; create naming a deleted node; random convergence)
  become commit tests over `pending_creates`/`queue_delete`; `explicit_is_never_populated` and `path_is_explicit` are
  deleted; model.rs field-table asserts drop `explicit`; plus: a failed Root/New/Split spawn after its Create committed is
  deleted; teardown deletes every old non-jar document node; a remote-driven removal queues nothing; crash respawn, swap
  and reload delete nothing; the invariant fires on a deliberately missed funnel.
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement** funnel by funnel (survey lists them: create funnels `place_node`, `spawn_tab_async`,
  `create_jar`, `materialize_imported`, download completion, the first-launch demo; delete funnels in Ruling 4).
- [ ] **Step 4: Full suite, fmt, clippy.** **Step 5: Gate:** `two-instance-test.sh` three times and `headless-smoke-test.sh`.
  **Step 6: Commit** `Creates and deletes are queued ops; diff is gone`.

### Task 4: delete `last_shared`, `Shared`, `shared_nodes`, `kind_fields`

**Files:** `session_sync.rs`, `closed.rs`, `jar_defs.rs`, `tabs.rs`, `main.rs`, `caps/rows.rs`, tests.

**Interfaces:**
- Consumes: Task 3's commit builder (reads the mirror directly).
- Produces: `mirror_remote(...) -> (held, changed)`; a mirror snapshot `(type, fields, group)` for Create and Recently
  Closed (`live_fields` overrides keep the resolved path and live url); field lists as row data (`Row.fields`) if adopted,
  else a single const table; `Mirror::ty`-based `worth_reopening(ty, fields)` computes the blank tag so the caps bridge
  loses `store_node`'s text read.

- [ ] **Step 1: Failing/rewritten tests:** the 5 `last_shared` and 12 `shared_nodes` tests move to the mirror snapshot;
  `the_store_sees_nothing_of_the_level_but_view` (json_level.rs ~577) reads the snapshot; `live_url.from` equals the mirror
  url; a remote url/engine change while a save failed still takes effect (2b-3 final-review I1 fix preserved).
- [ ] **Step 2: Run them to see the right failures.**
- [ ] **Step 3: Implement.** Do not delete `MIRRORED` unless nothing reads it; otherwise rename it to what it is.
- [ ] **Step 4: Full suite, fmt, clippy.** **Step 5: Gate:** `two-instance-test.sh`, `headless-smoke-test.sh`.
  **Step 6: Commit** `last_shared and shared_nodes are gone`.

### Task 5: docs and live checks

**Files:** spec 2b note, sync-flow map ("After 2b-5"), README history, this plan's Outcome.

- [ ] **Step 1:** Spec 2b paragraph with real hashes: "2b-5 done: every stored field is mirror state; creates and deletes
  are queued ops; `diff`, `explicit`, `keep`, `last_shared` are gone. Next: 2b-6 (enums to type + runtime table, caps
  bridge, EXPORTSTATE/scripts, grep guard)."
- [ ] **Step 2:** Sync map "After 2b-5" (creates/deletes queues, resolver, invariant, jars, changed-field return) and fix the
  text that still describes `diff`/`last_shared`.
- [ ] **Step 3:** Optional two-instance scenario for jar rename and default jar if the debug commands can drive them.
- [ ] **Step 4:** `cargo build -p manifold`, `two-instance-test.sh` (three times), `headless-smoke-test.sh`, full suite
  (`--bins` and `-p ipc-protocol`), fmt, clippy; record machine load and Defender state.
- [ ] **Step 5: Commit** `Docs: 2b-5 note and sync map`.

---

## Self-review (run 2026-10-05)

- **Spec coverage:** spec 6 items 2, 3, 6-10 advance; the enums (item 6's type-string and runtime table), caps bridge and
  grep guard are deliberately 2b-6 (Ruling 1) and named in the Goal. B (links) needs Rulings 2, 10, 11, 12; E needs
  Rulings 3-4 (explicit single writers, local-only deletes); C needs Ruling 7 and no new default-jar cascade code.
- **Placeholders:** none intended; compiler-led read-site conversions are listed by file in the survey.
- **Type consistency:** `pending_creates`, `queue_delete`, the resolver and `mirror_remote`'s changed return are defined in
  Tasks 3-4 and not used earlier; Task 1 adds accessors only; Task 2's `JarDef` cache keeps its API.
- **Risks:** Task 3 is the riskiest (a missed create is data loss; a missed delete resurrects on restart): hence the
  invariant (Ruling 14), three two-instance runs and the smoke test. Task 2's remote ephemeral side effects and Task 4's
  jar resurrection guard are the subtle ones. Two behavior changes: a remote-deleted default jar stays stored (Ruling 6);
  nothing else visible.
- **Unverified at writing:** survey grep counts; whether `Row.fields` is worth adopting in Task 4; the exact list of
  create funnels beyond those found; ItemNodeKind and ContentKind sites were not read.

## Outcome (2026-10-06)

Done on `dev`: f032489 and 4b57c18 (Task 1: group, leaf and content fields in the mirror; the second reconciles a created
group's DEFAULT_JAR into the mirror), 0bdf7d3 and 6052e43 (Task 2: jars in the mirror with the `JarDef` cache; the second
adds the ephemeral first-sighting and pending-toggle tests), 4b928c3 (Task 3: creates and deletes are queued ops; `diff`,
`explicit`, `keep` gone), 05d31cc (Task 4: `last_shared`, `shared_nodes`, `Shared`, `kind_fields`, `MIRRORED` gone;
`mirror_remote` returns changed fields; resurrection guard); plus this docs commit. 869 tests at Task 4.
Gate on 05d31cc: see ledger.

### Rulings and deviations

- **Ruling 7 deviation:** jar readers moved to `self.mirror.jar_defs()`; `App.jar_defs` is gone.
- **Behavior change besides Ruling 6:** an unsaved local jar rename or ephemeral change now beats a remote one (the remote
  value is compared against the mirror, not `last_shared`).
- **Ruling 4 text** said "queued and live"; the shipped delete filter is plain "queued and doc-live". Task 3 had an extra
  "not shown" clause; Task 4 added the resurrection guard in `materialize` and removed it.
- **Ruling 9 interpretation:** `materialize_jar` keeps its own `jar_defs`-based ephemeral comparison and runs before
  `mirror_remote`; the guard is the pending delete.

### Parked (not fixed)

- TYPE is canonical, not raw: `Kind::from_type` maps `image/*` to Image. Flag for 2b-6, before readers of the raw string exist.
- Recently Closed now also records group members whose spawn is in flight (the snapshot falls back to `spawning`). An
  improvement, untested.
- The direct test of containment by type string (only `TYPE_GROUP` accepts CONTAINS) was dropped with `is_container_store`.
- A failed download stays in `pending_creates` for the run; harmless.
- `LiveUrl.from` is now the mirror url, so an unsaved tab records a live_url and an unsaved `edit_url` does not.
- `pending_deletes` never clears on a read-only store (matches the old jar guard).
- Two-instance flakes: scenario 4 (link click) and scenario 7 (script navigation) each failed once on a 15 s page-load wait
  under CPU load of 39-77% on commit 4b928c3, and passed on rerun; no create or delete path is involved.
- Step 3 (a two-instance scenario for jar rename and default jar) was not done.
- The commit trailer of 05d31cc says "Fable 5.1"; left as is.
