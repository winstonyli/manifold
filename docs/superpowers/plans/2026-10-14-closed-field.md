---
status: done
---

# Closed Field Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> **Review fixes (2026-10-14).** The code blocks below are as first implemented. Where they differ, the spec's "Review fixes" section governs: `closed` holds the closing replica's persisted id (not a Bool; `Ctrl+Shift+T` matches it, not the stamp's actor), `apply_visibility` compares with `App.nodes` only, `evict_closed` waits for pending `closed_flags`, and `close_node` aborts when its flush leaves a kept node uncreated.

**Goal:** Closing a node sets one document flag, `closed`, on the closed root. The node keeps its id, its group, its links and every field. Its subtree is hidden in place, and reopening clears the flag. Recently Closed becomes a query over the document. The per-replica snapshot list goes away: `LocalState.closed`, `ClosedEntry` history, `record_closed`, `restore_closed` and `touch_backing`. Duplicate keeps a pure snapshot path in a new `duplicate.rs`.

**Architecture:**
- **Store.** Add `field::CLOSED = 23`, a replica-id register (`FieldType::Replica`, 0 or absent = open) on every kind, and `Node::closed()`.
  - `Doc::placement` answers `Shown(group)` or `Hidden`. `Doc::is_shown`, `Doc::shown`, `Doc::shown_level` and `Doc::closed_roots` build on it.
  - `Node::is_live` stays the commit gate. Display and export use *shown*.
- **Sync.** `RemoteChange` becomes three-state: `Remote::{Shown, Hidden, Deleted}`.
  - A hide path (`hide_node`) is separate from the delete path (`remove_remotely_deleted`). It takes the node out of `App.nodes`, `node_order`, the layout and the mirror. It parks the node's personal state (`LocalNode`) in a new `App.hidden_local`. It never re-parents, deletes links or releases files.
  - Both paths share one primitive, `unshow`, which `drop_tab` and `drop_node` also use.
  - A shared `leave_hidden(root)` moves the current level out of a hidden subtree.
- **Verbs.** `close_node` flushes the drawn position and the typed text into the document. It deletes the members that cannot come back. It queues `closed = <this replica's id>` on the root and hides the subtree locally, then saves and evicts.
  - `reopen_node` queues `closed = 0`, plus a re-stamp of the closed roots nested directly under the node (each keeps its own value). After every successful commit, a *visibility pass* shows or hides what the commit's flags and moves changed. That is what materializes a reopen.
  - `Ctrl+Shift+T` reopens the newest root whose `closed` value is this replica's persisted id (`profile::replica_id`). The popup lists `closed_roots()`.
- **Eviction.** `evict_closed` deletes the whole subtree of every root past `MAX_CLOSED`. It runs after a local close, after remote ops are applied, and at load.
- **Edges.** `App::opened_from(id)` is the one reader of the `opened-from` edge. Links stay in the mirror and are drawn only when both ends are live cards. A Default jar never sits in a group: `ensure_default_jar` repairs it, hidden ones included.

**Tech Stack:** Rust 2021, egui, and the manifold store (`store::model`, `store::json`, `store::journal`). Also `session_sync`, `mirror`, `local_state`, and the `gate` crate (headless checks).

**Spec:** `docs/superpowers/specs/2026-10-14-closed-field-design.md` (14 decisions).

## Global Constraints

- **Branch and commits:** Work on `main`, one commit per task (or per batch, see "Task order and batching"). Every commit message ends with the line `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- **Every task leaves the tree green:**
  - `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate` succeeds.
  - `cargo test -j 8 -p manifold -p gate` passes.
  - `cargo fmt --all` has been run, and `cargo fmt --all --check` is clean.
  - `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` passes.
  - `manifold` is a binary crate, so an unused `pub(crate)` or `pub` item warns, and the warning fails clippy. Each task adds only what it uses and deletes what it leaves dead.
- **Hooks and lints:**
  - Never `--no-verify`. The pre-commit hook runs fmt and clippy; if it fails, fix the cause.
  - Silence no lint without `#[allow(..., reason = "...")]`.
- **No shims and no migration** (memory `no-migration-until-users`; spec decision 14).
  - `LocalState.closed` is deleted outright. serde ignores the unknown key in an old file, and those entries are lost.
  - No alias keeps `close_node_and_save`, `reopen_closed` or `record_closed`.
- **Replace, don't layer.** Each task deletes what it supersedes, with its tests and docs, in the same commit.
- **Profile isolation:** no test reads or writes the real `%APPDATA%\manifold`. Files go under `store::testutil::temp_dir(..)`, and a test with a journal uses `session_sync::tests::app_on(&path, actor)`.
- **Tests:** each piece of non-trivial logic gets one unit test. Run one with `cargo test -j 8 -p manifold <filter>`.
- **No python.** Never run `python` that reads stdin (no `python -`, no heredoc into python). This plan needs no python at all.
- **Reserved names:**
  - `field::CLOSED = 23`, named `"closed"`.
  - `store::model::Placement { Shown(Option<NodeId>), Hidden }`. Do not confuse it with `fetched::Placement`, a struct for where a new card goes; the two live in different modules.
  - `session_sync::Remote<T> { Shown(T), Hidden, Deleted }`.
  - `App.hidden_local: HashMap<String, LocalNode>`.
  - `PendingWrites.closed_flags: Vec<(NodeId, u64)>` (node, replica id; 0 reopens).
  - New `App` methods: `unshow`, `hide_node`, `leave_hidden`, `forget_deleted`, `local_node`, `keep_hidden_local`, `reopen_node`, `evict_closed` and `opened_from`.
  - The new gate stage is `gate closed`, in `crates/gate/src/closed.rs`.
  - The new debug verb is `REOPENNODE id`.
- **Line numbers** are as of commit `3d853d5` and were checked against the files. Earlier tasks shift later line numbers, so search for the quoted text, not the number.

## Deviations from the spec's wording (same behaviour, fewer concepts)

- **D1. `placement` counts any closed ancestor, group or not.** The spec says "a group that is closed". Only a group has members, and `with_descendants` (the remote change set) walks every raw `group` edge alike, so this is the same set. One walk serves both.
- **D2. A Link node is always shown when live.** `Doc::is_shown` skips `placement` for `Kind::Link`. The mirror must keep every live link (decision 9: `links_naming` must still find it for eviction), and a link stored with a group under a closed node would otherwise leave the mirror. Drawing already requires both ends to be live cards at the level (`level.rs:170-177`).
- **D3. The hide path of a waking tab parks `Remote::Hidden` and ends the wake (revised by review fix 2).** A waking tab is in `nodes` and `spawning`. `hide_node` drops the placeholder, parks `Remote::Hidden` and removes the id from `wakes_in_flight`, so the landing is an ordinary spawn that applies the parked change (hides it, or shows it when a reopen replaced the `Hidden` with a `Shown`). Originally the entry stayed and the landing, finding no node, only saved: a reopen before then left the tab invisible.
  - A tab mid-respawn has no `nodes` entry. It is parked the same way (decision 3's "third state"), and `land_remote_in_flight` hides it once it lands.
- **D4. `leave_hidden(root)` only moves the level.** Selection, hover, drag, a link in progress, `focused`, `viewed_image` and `split_tab` are per-id state. `unshow` clears them for each hidden id (it absorbs the `drop_tab` and `remove_remotely_deleted` resets), and `forget_id` clears the gestures. Putting them in `leave_hidden` as well would clear them twice.
  - The level falls to `Doc::shown_level`, the nearest shown ancestor by raw `group`. That one function also serves the restore fallback of `LocalState.current_group`.
- **D5. `hidden_local` is cleared by `forget_id`, not by a `NOT_FORGOTTEN` excuse.** `hide_node` inserts its entry after `unshow` (which runs `forget_id`), so a hide keeps it. Every permanent removal still drops it, because every permanent removal runs `forget_id`. `forget_deleted` is then just `release_backing` plus `forget_id`.
- **D6. The close flushes `pos` and the typed text, not `path`.** Decision 4 lists "a file's resolved path". `FIELDS` marks `path` machine-local (`model.rs:262-264`): writing this machine's resolved path would push it to every replica. The node keeps its id, so its own content copy `<id>.<ext>` still resolves on reopen. The typed text is committed by `commit_note_buffers`, which the close calls first.
- **D7. An in-progress download inside a closing group is removed.** It has no document node and is not restorable, so it goes through `remove_node`, like a blank note. Nothing in the engine protocol cancels a download, and `in_flight_download_id` stops showing its progress once the card is gone. "Cancelled" in decision 4 means exactly this.
- **D8. Reopen reveals with a toast, not a scroll.** `reveal.rs` reveals only in Explorer, and nothing in the app reveals a card at another level. The spec leaves "should navigate there" open. A reopened node that lands at another level shows a toast naming the level, and one still hidden under a closed group says so (decision 7).
- **D9. Duplicate stops calling `rebind_jar_files`.** That function copies a jar's file only when the old id is not live (`closed.rs:303`), and a duplicated jar is always live. The call was a no-op for Duplicate, so only session import keeps it (`session_sync.rs:2045`).
- **D10. EXPORTSTATE gains `closed` and per-link `shown`.** The gate stage needs to read the history order, and to tell a hidden link from a deleted one.
- **D11. The Default-jar invariant moves into Task 3, and `backing_keep` into Task 2.** The brief puts both in Task 5. Task 3's close is what can hide a group, so the eject branch it deletes must be replaced by the invariant in the same commit. Task 2's hide path is what takes a node out of `nodes`, and `backing_keep` reads `nodes`: without the change, a launch after Task 2 or 3 could prune a hidden node's files. Task 5 keeps the edges.
- **D12. Tasks 1-3 do not change teardown's node list.** `teardown_running_session` (`session_sync.rs:2092`) iterates `doc.live()`, and `live()` keeps meaning `is_live`, which includes closed nodes. It already deletes the closed nodes (decision 14). Task 2 only adds `hidden_local.clear()`, and a test pins it.

## Leftover-reference check

Each task's check step greps for what it deletes, over all of `crates/` (tests included), and expects no hits beyond those listed:

```bash
git grep -nE "<names>" -- crates
```

Each task lists its `<names>`. A hit is fixed before the commit, never left for a later task. After Task 6 the whole set must be empty:

```bash
git grep -nE 'record_closed|ClosedEntry|closed_changed_at|AFTER_CLOSED|restore_closed|touch_backing|close_node_and_save|reopen_closed\(|closed_entry\(|app\.closed\b|self\.closed\b|\.closed\.(push|pop|len|is_empty|iter|remove)|Option<Snapshot>|Option<Option<RemoteNode>>|eject' -- crates
git grep -nE 'Recently Closed (entry|list) (holds|keeps)|reopen(s|ed)? .*fresh ids|record(s|ed)? (it )?in Recently Closed' -- crates docs/superpowers/specs README.md
```

The second command may hit only spec text that a "Superseded by 2026-10-14-closed-field-design.md" note sits right beside.

Comments naming the old mechanism are not caught by the code greps. In Task 3 and Task 4 also read and fix these comment mentions of Recently Closed, `reopen_closed` and the snapshot list: `content.rs:200`, `popups.rs:1048`, `fetched.rs` ~525, ~571, ~586, `spawn_result.rs:43`, `session_sync.rs:1890` and `session_sync/tests.rs:738`. Run `git grep -nE "Recently Closed|reopen_closed|closed entry" -- crates` and settle each hit.

## Task order and batching

1. **Store: the field and the queries.** `CLOSED`, `placement`, `is_shown`, `shown`; display and export switch to shown. No writer yet, so behaviour is unchanged.
2. **Sync core: the hide path.** `Remote`, `unshow`, `hide_node`, `leave_hidden`, `hidden_local`, `forget_deleted`, `Mirror::forget_node`, `shown_level`, `backing_keep` and `backing_names`. A remote `closed` Set now hides.
3. **Close, reopen, evict.** `closed_roots`, `closed_flags`, the visibility pass, `close_node`, `reopen_node`, `evict_closed`, `Ctrl+Shift+T`, the popup, the toolbar and the Default-jar invariant. The old history is deleted, except the snapshot import that Duplicate still uses.
4. **Duplicate moves to `duplicate.rs`.** `ClosedEntry` and `reopen_closed` are deleted.
5. **Edges.** `opened_from` and its readers.
6. **Gate stage `closed`, the flipped gate expectations, EXPORTSTATE, `REOPENNODE`, docs, and the full gate.**

**Batching (WORKING_STYLE "Delegation and models"):**
- Tasks 1 and 2 go to one Sonnet implementer. Task 1 is ~150 lines, Task 2 consumes all of it, and both are store and sync plumbing with open borrow shapes.
- Task 3 goes alone to a Sonnet implementer. It is the largest task, and its behaviour changes need judgment on the flipped tests.
- Tasks 4 and 5 go to one Sonnet implementer. Task 4 is a move; Task 5 is ~60 lines of reader conversions.
- Task 6 goes to its own Sonnet implementer. The gate stage's waits are open details.
- Review:
  - one Sonnet reviewer after Tasks 1-2;
  - one after Task 3;
  - one after Tasks 4-6;
  - then one Opus final whole-branch review, since the branch is well over ~300 changed lines.
- No Haiku: every task has design or root-cause work left.
- Briefs say "change nothing beyond this; report every deviation", and repeat the python rule.

The closing gate run comes after Task 6 and belongs to the controller (see "Closing step").

---

### Task 1: Store — the `closed` field and the shown queries

Nothing writes `closed` yet, so no behaviour changes. Every display and export path reads *shown* from here on, so Task 2 only has to add the hide path.

**Files:**
- Modify `crates/manifold/src/store/model.rs`:
  - `TYPES` rows at 152-165 and `GENERIC` at 169;
  - `field` module at 221-250, `FIELDS` at 252-272;
  - `Node` at 428-510 (add `closed()`), `Doc` at 524-581;
  - tests: the generic-list test at ~721; new tests in `mod tests`.
- Modify `crates/manifold/src/store/json.rs`:
  - `doc_to_json_with` at 105-112, `is_live_card` at 120-122, `subtree_to_json` at 127-144;
  - `import` at 207-214 (skip `CLOSED`);
  - tests: new ones in `mod tests`.
- Modify `crates/manifold/src/store/mod.rs`: `testutil::shown` at 77-81.
- Modify `crates/manifold/src/session_sync.rs`:
  - `check_graph_matches_document`'s last loop at 1469;
  - `restore_session` at 1484;
  - `export_group_json` at 1871.
- Modify `crates/manifold/src/session_sync/tests.rs`:
  - `jar_fields_belong_to_tabs_groups_and_jars` at 11-19 (add `CLOSED` to each list);
  - the `shown` helper at 88-90 (`doc.live()` becomes `doc.shown()`).
- Modify `crates/manifold/src/mirror.rs`: the test at ~586 that compares a spawn's seeded fields. Filter `field::CLOSED` out as it filters `PARENT`, because a spawn is never closed.

**Interfaces:**
- Consumes: `Node::is_live`, `Node::is_group`, `Node::kind`, `Doc::effective_group`, `Reg.stamp`.
- Produces:
  - `field::CLOSED`, `Node::closed(&self) -> bool`;
  - `pub enum Placement { Shown(Option<NodeId>), Hidden }`, `Doc::placement(&self, NodeId) -> Placement`;
  - `Doc::is_shown(&self, NodeId) -> bool`;
  - `Doc::shown(&self) -> impl Iterator<Item = (NodeId, &Node)>`.

- [ ] **Step 1: Write the failing store tests** in `store/model.rs`'s `mod tests`:

```rust
    /// A doc of `(id, kind, group, closed)` rows, all live; ids are `NodeId { actor: ActorId(1), counter }`.
    fn doc_of(rows: &[(u64, Kind, Option<u64>, bool)]) -> (Doc, impl Fn(u64) -> NodeId) {
        let id = |c: u64| NodeId { actor: ActorId(1), counter: c };
        let mut doc = Doc::new(DocId([0; 16]));
        for &(c, kind, group, closed) in rows {
            let mut node = Node::typed(kind);
            node.group = group.map(id);
            if closed {
                let stamp = Stamp { hlc: Hlc { ms: c, logical: 0 }, actor: ActorId(1) };
                let enc = encode_field(field::CLOSED, &FieldValue::Bool(true));
                node.fields.insert(field::CLOSED, Reg { stamp, enc });
            }
            doc.nodes.insert(id(c), node);
        }
        (doc, id)
    }

    #[test]
    fn a_closed_group_hides_everything_under_it_and_a_deleted_one_passes_members_up() {
        let (mut doc, id) = doc_of(&[
            (1, Kind::Group, None, false),
            (2, Kind::Group, Some(1), true),
            (3, Kind::Tab, Some(2), false),
            (4, Kind::Group, Some(3), false), // under a tab: still hidden by 2 (D1)
            (5, Kind::Note, Some(1), false),
        ]);
        assert!(doc.nodes[&id(2)].closed());
        assert_eq!(doc.placement(id(2)), Placement::Shown(Some(id(1))), "a closed node's own place is its group's");
        assert!(!doc.is_shown(id(2)), "but it is not shown: it is closed");
        assert_eq!(doc.placement(id(3)), Placement::Hidden);
        assert_eq!(doc.placement(id(4)), Placement::Hidden);
        assert_eq!(doc.placement(id(5)), Placement::Shown(Some(id(1))));
        let shown: Vec<NodeId> = doc.shown().map(|(n, _)| n).collect();
        assert_eq!(shown, [id(1), id(5)]);
        // A deleted closed group hides nothing: its members pass up to the nearest shown group.
        doc.nodes.get_mut(&id(2)).unwrap().deleted = true;
        assert_eq!(doc.placement(id(3)), Placement::Shown(Some(id(1))));
        assert!(doc.is_shown(id(3)));
    }

    #[test]
    fn a_live_link_is_shown_whatever_its_group() {
        let (mut doc, id) = doc_of(&[(1, Kind::Group, None, true), (2, Kind::Link, Some(1), false)]);
        assert!(doc.is_shown(id(2)), "the mirror keeps every live link (D2)");
        doc.nodes.get_mut(&id(2)).unwrap().deleted = true;
        assert!(!doc.is_shown(id(2)));
    }

    #[test]
    fn closed_is_one_more_generic_field() {
        assert!(GENERIC.contains(&field::CLOSED));
        assert!(TYPES.iter().all(|(_, _, fields)| fields.contains(&field::CLOSED)));
        assert_eq!(field_def(field::CLOSED).map(|d| (d.name, d.ty)), Some(("closed", FieldType::Bool)));
    }
```

In `store/json.rs`'s `mod tests`, add:

```rust
    #[test]
    fn export_writes_shown_nodes_only_and_import_never_carries_closed() {
        use crate::store::model::{Reg, Stamp, Hlc, ActorId};
        let id = |c: u64| NodeId { actor: ActorId(1), counter: c };
        let mut doc = Doc::new(crate::store::model::DocId([0; 16]));
        let mut group = Node::typed(Kind::Group);
        let stamp = Stamp { hlc: Hlc { ms: 1, logical: 0 }, actor: ActorId(1) };
        group.fields.insert(field::CLOSED, Reg { stamp, enc: encode_field(field::CLOSED, &FieldValue::Bool(true)) });
        doc.nodes.insert(id(1), group);
        let mut inside = Node::typed(Kind::Note);
        inside.group = Some(id(1));
        doc.nodes.insert(id(2), inside);
        doc.nodes.insert(id(3), Node::typed(Kind::Note));
        let ids = |j: &JsonDoc| j.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&doc_to_json(&doc)), [id(3).to_string()], "the closed group and its member stay out");
        assert_eq!(ids(&doc_to_json_all(&doc)).len(), 3, "the forensic dump keeps them");
        assert!(subtree_to_json(&doc, id(1)).nodes.is_empty(), "a closed group exports nothing");
        // Re-importing the dump brings the closed group back shown: `closed` is never imported.
        let mut n = 100;
        let imported = import(&doc_to_json_all(&doc), || { n += 1; id(n) }, Trust::Own).unwrap();
        assert!(imported.iter().all(|i| !i.fields.contains_key(&field::CLOSED)));
    }
```

(Adjust the `use` lines to what `json.rs`'s tests module already imports. `Node::typed`, `encode_field` and `FieldValue` are in `store::model`.)

- [ ] **Step 2: Run them to fail.** Run `cargo test -j 8 -p manifold store::` and expect compile errors (no `CLOSED`, no `placement`).

- [ ] **Step 3: Implement the store side.** In `model.rs`:

  - `TYPES`: append `CLOSED` to every row's list (Link included). Append it to `GENERIC` too:
    `pub(crate) const GENERIC: &[u32] = &[field::POS, field::COLOR, field::PARENT, field::NAME, field::CLOSED];`
  - `field` module, after `IS_DEFAULT`:
    ```rust
        /// Set on the root of a close (spec 2026-10-14-closed-field-design.md): the node and everything under it are
        /// hidden in place, and the node is a Recently Closed entry (`Doc::closed_roots`). Only the close and reopen
        /// verbs write it.
        pub const CLOSED: u32 = 23;
    ```
  - `FIELDS: [FieldDef; 18]`, appending `FieldDef { no: field::CLOSED, name: "closed", ty: FieldType::Bool },`.
  - On `Node`, after `is_group`:
    ```rust
        /// The root of a close: `closed` is set (spec decision 1). Its descendants keep their own `closed` as it was.
        pub fn closed(&self) -> bool {
            self.get(field::CLOSED) == FieldValue::Bool(true)
        }
    ```
  - Before `impl Doc`:
    ```rust
    /// Where a node is shown (`Doc::placement`): in a group (`None` the top level), or not at all because a closed
    /// node sits above it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Placement {
        Shown(Option<NodeId>),
        Hidden,
    }
    ```
  - In `impl Doc`, after `effective_group`:
    ```rust
        /// Where `id` would be shown: `Hidden` when any non-deleted closed node is above it on the raw `group` chain
        /// (checked first; any node, group or not, plan D1), else `Shown(effective_group)`. `id`'s own `closed` is
        /// not read: a closed root's place is its group's (`is_shown` reads it). A missing node or a cycle is
        /// `Shown`, as `effective_group` answers.
        pub fn placement(&self, id: NodeId) -> Placement {
            let mut cur = self.nodes.get(&id).and_then(|n| n.group);
            for _ in 0..=self.nodes.len() {
                let Some(node) = cur.and_then(|c| self.nodes.get(&c)) else { break };
                if !node.deleted && node.closed() {
                    return Placement::Hidden;
                }
                cur = node.group;
            }
            Placement::Shown(self.effective_group(id))
        }

        /// Shown in the graph: live, not itself closed, and not under a closed node. A live link is always shown
        /// (plan D2): the mirror keeps every one, and drawing asks for both ends.
        pub fn is_shown(&self, id: NodeId) -> bool {
            let Some(node) = self.nodes.get(&id).filter(|n| n.is_live()) else { return false };
            node.kind() == Kind::Link || (!node.closed() && self.placement(id) != Placement::Hidden)
        }

        /// The shown nodes, in id order: what the graph and an export show. `live()` stays the commit gate.
        pub fn shown(&self) -> impl Iterator<Item = (NodeId, &Node)> {
            self.live().filter(|&(id, _)| self.is_shown(id))
        }
    ```
  - Update `live()`'s doc to "The live nodes (typed, not deleted; closed ones included), in id order: the commit gate. What the graph shows is `shown()`."

- [ ] **Step 4: Switch the display and export readers to shown.**
  - `json.rs` `doc_to_json_with`: `doc.live().filter(|&(id, node)| (all_links || doc.is_shown(id)) && (all_links || node.kind() != Kind::Link || link_ends_among(node, |id| is_live_card(doc, id))))`. Keep the dump (`all_links`) on `live()`. Update `doc_to_json`'s doc to say "shown nodes".
  - `is_live_card` becomes `doc.is_shown(id) && doc.nodes[&id].kind() != Kind::Link`. Write it as `doc.nodes.get(&id).is_some_and(|n| n.kind() != Kind::Link) && doc.is_shown(id)`, and rename it `is_shown_card`.
  - `subtree_to_json`:
    - the root uses `.filter(|_| doc.is_shown(root))`;
    - both loops use `doc.shown()`;
    - the doc says "shown subtree".
  - `import`: `let Some(def) = field_by_name(name).filter(|def| def.no != field::TYPE && def.no != field::CLOSED) else { continue };`. Add a comment that `closed` is the document's history, never a file's.
  - `store/mod.rs` `testutil::shown`: `doc.shown().map(..)`. Its doc says "shown nodes".
  - `session_sync.rs`:
    - 1469: `for (id, _) in doc.shown() {`;
    - 1484: `doc.shown().map(..)`;
    - 1871: `if !doc.is_shown(root) {`, and the toast reads "it isn't shown in the session".
  - `session_sync/tests.rs`:
    - 88-90: `doc.shown()`;
    - 11-19: add `CLOSED` to each expected list.
  - `mirror.rs` ~586: filter `field::CLOSED` with `PARENT`.

- [ ] **Step 5: Run the tests to pass.** Run `cargo test -j 8 -p manifold`. All pass. The tests that pin Create bytes or field counts may need `CLOSED` added. `Bool(false)` encodes empty and `value_to_json` omits it, so no JSON fixture changes. If a byte-level fixture changes, stop and report.

- [ ] **Step 6: Leftover check.**
  - `git grep -n "\.live()" -- crates/manifold/src/store/json.rs crates/manifold/src/session_sync.rs` may hit only the `doc_to_json_with` line, test code, and `teardown_running_session` (2092, D12).
  - `git grep -n "is_live_card" -- crates` must give no hits.

- [ ] **Step 7: Format, lint, commit.**
  ```bash
  cargo fmt --all
  cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
  git add -A crates && git commit -m "Store: the closed field (23) and shown queries; display and export read shown

  Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
  ```

---

### Task 2: Sync core — three-state remote changes and the hide path

After this task a remote `closed = true` hides the node and its subtree. Hiding keeps the node's personal state, its links, its children's edges and its files. A remote `closed = false` shows it again. No local verb writes the flag yet.

**Files:**
- Modify `crates/manifold/src/store/model.rs`: add `Doc::shown_level`, plus a test.
- Modify `crates/manifold/src/session_sync.rs`:
  - `RemoteChange` at 210, `materialize`'s parking at 515-517;
  - `remote_changes` at 745-753, `apply_remote` at 765-793;
  - `remove_remotely_deleted` at 802-846, `land_remote_in_flight` at 850-856;
  - the save's fixups at 1382-1397;
  - `restore_session` at 1505-1512, `retry_read_local` at 1597-1601;
  - `local_state` at 1613-1662, `apply_local` at 1670-1729;
  - `teardown_running_session` at 2083-2121.
- Modify `crates/manifold/src/tabs.rs`:
  - the `remote_in_flight` type at ~38;
  - `drop_tab` at 826-863.
- Modify `crates/manifold/src/tabs/spawn_result.rs`: 23, 199, 225-227, 290 (types only).
- Modify `crates/manifold/src/node_store.rs`: `drop_node` at 89-105, `forget_id` at 119-148; add `unshow`.
- Modify `crates/manifold/src/mirror.rs`: `forget_link` at 95-101; add `forget_node`.
- Modify `crates/manifold/src/main.rs`: the `App` field list near `closed: Vec<ClosedEntry>` (~442-446) gains `hidden_local`.
- Modify `crates/manifold/src/fetched.rs`: `backing_keep` at 166-181.
- Modify `crates/manifold/src/session.rs`: `backing_names` at 36-63.
- Tests: `session_sync/tests.rs` (new cases, plus the `Option` to `Remote` migrations listed in Step 6); `tabs/tests.rs:53`; `json_level/tests.rs:308`.

**Interfaces:**
- Consumes: Task 1's `Doc::is_shown`, `Doc::shown`, `Node::closed`; the existing `materialize`, `apply_local_node`, `switch_group_level` and `release_backing`.
- Produces:
  - `pub(crate) enum Remote<T> { Shown(T), Hidden, Deleted }` with `shown_mut`;
  - `RemoteChange = (NodeId, Remote<Snapshot>)`;
  - `App::unshow(&mut self, &str) -> Option<GraphNode>`;
  - `App::hide_node(&mut self, &str)`, `App::leave_hidden(&mut self, NodeId)`, `App::forget_deleted(&mut self, &str)`;
  - `App::local_node(&self, &str, &GraphNode) -> LocalNode`, `App::keep_hidden_local(..)`;
  - `Mirror::forget_node(&mut self, &str)`;
  - `Doc::shown_level(&self, NodeId) -> Option<NodeId>`;
  - `App.hidden_local`.

- [ ] **Step 1: Write the failing tests.** In `model.rs` tests:

```rust
    #[test]
    fn shown_level_is_the_nearest_shown_node_up_the_raw_chain() {
        let (doc, id) = doc_of(&[
            (1, Kind::Group, None, false),
            (2, Kind::Group, Some(1), true),
            (3, Kind::Group, Some(2), false),
        ]);
        assert_eq!(doc.shown_level(id(1)), Some(id(1)));
        assert_eq!(doc.shown_level(id(3)), Some(id(1)), "hidden by 2, closed itself: both skipped");
        assert_eq!(doc.shown_level(id(9)), None, "unknown: the top level");
    }
```

In `session_sync/tests.rs`, after `stored` (757-765), add the helpers and tests:

```rust
/// Another replica sets `closed` on `node` (true closes, false reopens).
fn remote_close(other: &mut Journal, node: &str, closed: bool) {
    remote_set(other, node, field::CLOSED, FieldValue::Bool(closed));
}

#[test]
fn a_remote_close_hides_without_deleting_reparenting_or_releasing() {
    let path = temp_dir("sync-remote-close").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let inner = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "in".into(), editing: false }));
    app.edit_group(&inner, Some(g.clone()));
    let outside = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "out".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, failed_tab("https://t/", 1.25));
    app.edit_parent(&outside, Some(tab.clone()));
    let _link = app.create_link(&inner, &outside);
    assert!(app.save_session());
    let links_before: Vec<NodeId> = app.mirror.link_ids().collect();
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &g, true);
    remote_close(&mut other, &tab, true);
    assert!(app.save_session());
    for id in [&g, &inner, &tab] {
        assert!(!app.nodes.contains_key(id) && !app.mirror.has(id), "{id} is hidden");
        assert!(live_in_doc(&app, id), "{id} is still live in the document");
    }
    assert_eq!(app.mirror.link_ids().collect::<Vec<_>>(), links_before, "the link to the outside node stays");
    assert_eq!(app.mirror.parent(&outside), Some(tab.as_str()), "the child keeps its opened-from edge");
    assert_eq!(app.hidden_local[&tab].zoom, Some(1.25), "the hidden tab's personal state is kept");
    assert!(app.writes.pending_deletes.is_empty(), "a hide queues no delete");
    // The clear brings them back, the group before its member, the tab with its zoom.
    remote_close(&mut other, &g, false);
    remote_close(&mut other, &tab, false);
    assert!(app.save_session());
    assert_eq!(app.mirror.group(&inner), Some(g.as_str()));
    assert!(matches!(&app.nodes[&tab].runtime, Runtime::Unloaded(t) if t.zoom_factor == 1.25));
    assert!(!app.hidden_local.contains_key(&tab));
}

#[test]
fn a_remote_move_into_a_closed_group_hides_the_node() {
    let path = temp_dir("sync-remote-move-closed").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &g, true);
    let (node, group) = (note.parse().unwrap(), Some(g.parse().unwrap()));
    other.begin().unwrap().commit(vec![OpBody::Move { node, group }]).unwrap();
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&note));
}

#[test]
fn a_remote_close_of_the_viewed_group_leaves_to_the_nearest_shown_level() {
    let path = temp_dir("sync-remote-close-level").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "Outer");
    let inner = add_group(&mut app, "Inner");
    app.edit_group(&inner, Some(outer.clone()));
    assert!(app.save_session());
    app.switch_group_level(Some(inner.clone()));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &inner, true);
    assert!(app.save_session());
    assert_eq!(app.current_group.as_deref(), Some(outer.as_str()));
}

#[test]
fn hidden_personal_state_survives_a_launch_and_a_delete_drops_it() {
    let path = temp_dir("sync-hidden-local").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://t/", 1.5));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &tab, true);
    assert!(app.save_session());
    app.maybe_write_local(true);
    drop(app);
    let mut back = app_on(&path, 1);
    back.restore_session();
    assert_eq!(back.hidden_local.get(&tab).and_then(|l| l.zoom), Some(1.5), "read back for the hidden node");
    let node = tab.parse().unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node }]).unwrap();
    assert!(back.save_session());
    assert!(!back.hidden_local.contains_key(&tab), "the Delete drops it (forget_deleted)");
}

#[test]
fn backing_keep_holds_hidden_nodes_files() {
    let path = temp_dir("sync-backing-hidden").join("session.mnf");
    let mut app = app_on(&path, 1);
    let jar = add(&mut app, Kind::Jar, Runtime::Jar);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &jar, true);
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&jar));
    assert!(app.backing_keep(None).contains(&jar), "a closed jar keeps its cookie file");
    assert!(!app.backing_keep(Some(&jar)).contains(&jar), "`except` leaves it out");
}

#[test]
fn teardown_forgets_hidden_personal_state() {
    let path = temp_dir("sync-teardown-hidden").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://t/", 1.5));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &tab, true);
    assert!(app.save_session());
    app.teardown_running_session();
    assert!(app.hidden_local.is_empty());
    assert!(app.writes.pending_deletes.contains(&tab.parse().unwrap()), "the closed node is deleted with the session (D12)");
}
```

(`app.connect(edge, from, to) -> Result<(), ConnectError>` is the link verb at `links.rs:149`. Check the edge constant's name with `grep -n "pub(crate) const" crates/manifold/src/links.rs`. The helpers `live_in_doc` (tests.rs:108) and `failed_tab` (tests.rs:1556) already exist; `app_on`, `add`, `add_group`, `remote_set` and `stored` are already `pub(crate)`. `teardown_running_session` is private: give it `pub(crate)` only if the test module cannot reach it, since the tests module is a child of `session_sync`.)

- [ ] **Step 2: Run them to fail.** Run `cargo test -j 8 -p manifold remote_close hidden_ backing_keep_holds shown_level teardown_forgets` and expect compile errors.

- [ ] **Step 3: The `Remote` type.** In `session_sync.rs`, replace line 210:

```rust
/// What another replica's change leaves a node as (spec decision 3): shown with this snapshot, hidden under or as a
/// closed node (kept, out of the graph), or deleted.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Remote<T> {
    Shown(T),
    Hidden,
    Deleted,
}

impl<T> Remote<T> {
    pub(crate) fn shown_mut(&mut self) -> Option<&mut T> {
        match self {
            Remote::Shown(t) => Some(t),
            _ => None,
        }
    }
}

pub(crate) type RemoteChange = (NodeId, Remote<Snapshot>);
```

- `tabs.rs` ~38: `remote_in_flight: HashMap<String, Remote<RemoteNode>>`. Update its doc: "Shown: the change to apply; Hidden: hide it once it lands; Deleted: remove it".
- `materialize` 516: `self.spawns.remote_in_flight.insert(id, Remote::Shown((ty.to_owned(), fields.clone(), group)));`.
- The save fixups:
  - 1384: `if let Some(Remote::Shown((_, fields, _))) = self.spawns.remote_in_flight.get_mut(&id) {`;
  - 1394: `if let Some(Remote::Shown((_, _, group))) = ...`.
- `remove_remotely_deleted` 824: `for (_, _, group) in self.spawns.remote_in_flight.values_mut().filter_map(Remote::shown_mut) {`.
- `spawn_result.rs` 23, 199, 225-227 and 290: these lines only move the value, so they keep compiling once `land_remote_in_flight` takes `Option<Remote<RemoteNode>>`.

- [ ] **Step 4: `remote_changes`, `apply_remote` and `land_remote_in_flight`.**

```rust
    fn remote_changes(doc: &Doc, remote: &[NodeId]) -> Vec<RemoteChange> {
        remote
            .iter()
            .map(|&id| match doc.nodes.get(&id).filter(|n| n.is_live()) {
                None => (id, Remote::Deleted),
                Some(n) if doc.is_shown(id) => (id, Remote::Shown(Snapshot::of(doc, id, n))),
                Some(_) => (id, Remote::Hidden),
            })
            .collect()
    }
```

In `apply_remote`, the sort key becomes `Remote::Shown(s) if s.kind() == Kind::Group => 0`, and the match becomes:

```rust
            match change {
                Remote::Shown(s) => {
                    // A node coming back from hidden takes the personal state it left with (spec decision 8).
                    let local = self.hidden_local.remove(&key);
                    self.materialize(key.clone(), &s.ty, &s.fields, s.group.map(|g| g.to_string()), local.as_ref());
                    if let (Some(local), Some(live)) = (&local, self.nodes.get_mut(&key)) {
                        Self::apply_local_node(live, local);
                    }
                }
                Remote::Hidden => {
                    self.leave_hidden(id);
                    self.hide_node(&key);
                }
                Remote::Deleted if self.spawns.spawning.contains_key(&key) => {
                    self.spawns.remote_in_flight.insert(key, Remote::Deleted);
                }
                Remote::Deleted => self.remove_remotely_deleted(&key),
            }
```

Update the doc comment: "A hidden node (closed, or under a closed node) leaves the graph through `hide_node`; a cleared one comes back through `materialize` with its kept personal state."

`land_remote_in_flight`:

```rust
    pub(crate) fn land_remote_in_flight(&mut self, id: &str, remote: Option<Remote<RemoteNode>>) {
        match remote {
            Some(Remote::Shown((ty, fields, group))) => self.materialize(id.to_string(), &ty, &fields, group, None),
            Some(Remote::Hidden) => {
                if let Ok(node) = id.parse() {
                    self.leave_hidden(node);
                }
                self.hide_node(id);
            }
            Some(Remote::Deleted) => self.remove_remotely_deleted(id),
            None => {}
        }
    }
```

- [ ] **Step 5: The shared primitive, the hide path and the delete split.**

`node_store.rs`, a new `unshow` above `drop_node`. It absorbs `drop_tab`'s 844-861 and `remove_remotely_deleted`'s 835-843:

```rust
    /// Takes `id` out of the live graph without touching the document: its `nodes` entry (a tab's `ViewTab` drops,
    /// shutting its engine host down), a PDF's queued render, its `node_order` slot, the view state naming it
    /// (`focused`, `split_tab`, `viewed_image`, an open find bar), then `forget_id`. Shared by the delete paths
    /// (`drop_tab`, `drop_node`) and the hide path (`hide_node`), which add what each needs. Returns the removed node.
    pub(crate) fn unshow(&mut self, id: &str) -> Option<crate::GraphNode> {
        let removed = self.nodes.remove(id);
        if removed.as_ref().is_some_and(|n| n.as_pdf().is_some()) {
            self.pdf_renderer.cancel(id);
        }
        self.node_order.retain(|d| d != id);
        // A split only makes sense paired with its focused tab (`focus_tab`): losing either half clears it.
        if self.split_tab.as_deref() == Some(id) || self.focused.as_deref() == Some(id) {
            self.split_tab = None;
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
            // Only a showing Tab view drops to Graph: while an Image is maximized `focused` is the hidden last tab.
            if self.showing_tab_view() {
                self.view_mode = ViewMode::Graph;
            }
            // The tab is gone: nothing to send `FindStop` to, so only the UI side of the close.
            self.find.close();
        }
        if self.viewed_image.as_deref() == Some(id) {
            self.viewed_image = None;
            if self.view_mode == ViewMode::Item {
                self.view_mode = ViewMode::Graph;
            }
        }
        self.forget_id(id);
        removed
    }
```

`drop_node` becomes the re-parent loop (unchanged) plus:

```rust
        let group = self.mirror.group(id).map(str::to_owned);
        let removed = self.unshow(id).is_some();
        if self.current_group.as_deref() == Some(id) {
            self.switch_group_level(group.filter(|_| removed));
        }
```

`drop_tab` (`tabs.rs`) becomes the tab check, the re-parent loop, `self.unshow(id);` and `true`. Lines 835-861 go; their comments move into `unshow` as above.

`forget_id`: add `self.hidden_local.remove(id);` right after `self.spawns.forget(id);`. In its doc list, write "`hidden_local`: a hide re-inserts after this (plan D5)".

`mirror.rs`:

```rust
    /// Drops every entry of node `id` (fields, edges, view, Default index): it left the graph without being deleted
    /// (`App::hide_node`), or, for a link, was forgotten (`forget_link`).
    pub(crate) fn forget_node(&mut self, id: &str) {
        self.nodes.remove(id);
        self.links.remove(id);
        self.views.remove(id);
        if let Ok(node) = id.parse::<NodeId>() {
            self.defaults.remove(&node);
        }
    }

    pub(crate) fn forget_link(&mut self, id: NodeId) {
        self.link_nodes.remove(&id);
        self.forget_node(&id.to_string());
    }
```

(Check `defaults`' collection type: `retain(|n| keep(&n.to_string()))` at 190 shows it holds `NodeId`s. Use the matching `remove`.)

`session_sync.rs`, after `remove_remotely_deleted`:

```rust
    /// Hides `id` (spec decision 3): out of `nodes`, `node_order`, the layout and the mirror, its engine unloaded
    /// (`unshow`), its personal state kept in `hidden_local` for a reopen. Nothing in the document changes: no
    /// re-parent, no link delete, no backing-file release. A tab mid-respawn (in `spawning`, not in `nodes`) is
    /// parked as `Remote::Hidden` and hidden when it lands (plan D3); a waking one is hidden here and its landing,
    /// finding no node, only saves (`spawn_result.rs`).
    pub(crate) fn hide_node(&mut self, id: &str) {
        if self.spawns.spawning.contains_key(id) && !self.nodes.contains_key(id) {
            self.spawns.remote_in_flight.insert(id.to_owned(), Remote::Hidden);
            return;
        }
        let Some(node) = self.nodes.get(id) else { return };
        // A note comes back idle: the box it was typed in is gone, and its text was committed.
        let local = LocalNode { editing: false, ..self.local_node(id, node) };
        self.unshow(id);
        self.mirror.forget_node(id);
        self.layout.remove(id);
        self.hidden_local.insert(id.to_owned(), local);
    }

    /// The current level leaves a subtree that is being hidden: when it is `root` or inside it, the level falls to
    /// the nearest shown node above `root` (`Doc::shown_level`), as `dissolve_members` leaves a dissolved group.
    /// Reads the raw chain, so it holds before the close is committed (a local close) and after (a remote one).
    pub(crate) fn leave_hidden(&mut self, root: NodeId) {
        let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
        let Some(level) = self.current_group.as_deref().and_then(|g| g.parse::<NodeId>().ok()) else { return };
        if level != root && !doc.is_ancestor(root, level) {
            return;
        }
        let to = doc.nodes.get(&root).and_then(|n| n.group).and_then(|g| doc.shown_level(g));
        self.switch_group_level(to.map(|g| g.to_string()));
    }

    /// A node the document deleted that this replica no longer shows (an evicted hidden subtree, local or remote):
    /// its backing files go once nothing holds them, and so do its per-id tables and kept personal state.
    pub(crate) fn forget_deleted(&mut self, id: &str) {
        self.release_backing(id);
        self.forget_id(id);
    }
```

In `remove_remotely_deleted`:
- 813 becomes `let Some(node) = self.nodes.get(id) else { return self.forget_deleted(id) };`. That replaces the bare return, so a Delete of a hidden node is still cleaned up (decision 3).
- Delete 835-843 (now in `unshow`).
- Keep `self.spawns.wakes_in_flight.remove(id);` and `self.release_backing(id);`.

`Doc::shown_level` in `model.rs`:

```rust
    /// `id` when it is shown, else the nearest shown node above it on the raw `group` chain; `None` (the top level)
    /// when there is none. Where a level falls back to when the one it names is hidden or gone.
    pub fn shown_level(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = Some(id);
        for _ in 0..=self.nodes.len() {
            let at = cur?;
            if self.is_shown(at) {
                return Some(at);
            }
            cur = self.nodes.get(&at).and_then(|n| n.group);
        }
        None
    }
```

- [ ] **Step 6: Personal state.**
  - `main.rs`: add the `App` field (`Default` derives or lists it):
    ```rust
        /// Personal state (`LocalNode`: settled pos, title, zoom, muted, live url) of nodes this replica keeps out of
        /// the graph because they are closed or under a closed node (spec decision 8). Filled by `hide_node`, read
        /// back by a reopen (`apply_remote`), written to `session.mnf.local` with the rest. `forget_id` drops it.
        pub(crate) hidden_local: HashMap<String, LocalNode>,
    ```
    If `App` has a hand-written `Default`, add `hidden_local: HashMap::new()` there.
  - `local_state`: factor the closure body (1621-1637) into:
    ```rust
        /// `id`'s personal state as the live graph shows it: its settled position, a tab's title, muted, zoom and
        /// unshared live url, a note's `editing`.
        pub(crate) fn local_node(&self, id: &str, node: &crate::GraphNode) -> LocalNode {
            let pos = self.layout_pos(id);
            let mut local = LocalNode { pos: [pos.x, pos.y], ..LocalNode::default() };
            if let Some(tab) = tab_common(&node.runtime) {
                local.title = tab.title.to_string();
                local.muted = tab.muted;
                local.zoom = (tab.zoom_factor != self.settings.zoom_factor).then_some(tab.zoom_factor);
                // Against the shared url the mirror holds (Ruling 9): only an unshared move is recorded.
                let from = self.mirror.url(id);
                local.live_url = (from != tab.url).then(|| LiveUrl { url: tab.url.to_string(), from: from.to_owned() });
            } else if let Runtime::Note(n) = &node.runtime {
                local.editing = n.editing;
            }
            local
        }
    ```
    The map becomes `.map(|(id, node)| (id.clone(), self.local_node(id, node)))`. After the `pending_local` block, add:
    ```rust
            // Hidden nodes keep theirs while the document still holds them (a Delete not yet seen is dropped).
            if let Some(doc) = self.journal.as_ref().map(|j| j.doc()) {
                for (id, node) in &self.hidden_local {
                    if id.parse::<NodeId>().ok().and_then(|n| doc.nodes.get(&n)).is_some_and(Node::is_live) {
                        nodes.entry(id.clone()).or_insert_with(|| node.clone());
                    }
                }
            }
    ```
  - New, beside `apply_local_node`:
    ```rust
        /// Personal-state entries for nodes the document holds but the graph doesn't show (closed, or under a closed
        /// node): kept in `hidden_local` for a reopen, not dropped as stale.
        fn keep_hidden_local(&mut self, entries: impl IntoIterator<Item = (String, LocalNode)>) {
            let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
            let held = |id: &str| id.parse::<NodeId>().ok().and_then(|n| doc.nodes.get(&n)).is_some_and(Node::is_live);
            let here = |id: &str| self.nodes.contains_key(id) || self.spawns.spawning.contains_key(id);
            let keep: Vec<(String, LocalNode)> = entries.into_iter().filter(|(id, _)| held(id) && !here(id)).collect();
            self.hidden_local.extend(keep);
        }
    ```
  - `apply_local`:
    - The `current_group` assignment becomes:
      ```rust
            // A level that is hidden now falls back to its nearest shown ancestor (spec decision 3).
            let level = match (self.journal.as_ref(), state.current_group.as_deref().and_then(|g| g.parse().ok())) {
                (Some(j), Some(g)) => j.doc().shown_level(g).map(|g| g.to_string()),
                _ => state.current_group.clone(),
            };
            if exists(&level) {
                self.current_group = level;
            }
      ```
      Declare `exists` so that it takes `&Option<String>` as now.
    - The node loop gains an `else` arm that pushes `(id, node)` into a `leftover` Vec. After the loop, call `self.keep_hidden_local(leftover);`.
  - `restore_session`'s all-jars branch (1505-1512): KEEP `self.restore_closed(local.closed);` (its tests still exercise it until Task 3) and ADD `self.keep_hidden_local(local.nodes);` after it. Task 3 deletes `restore_closed` and its call sites; here nothing is replaced.
  - `retry_read_local` fresh branch (1599-1600): keep the `closed.extend` at 1598 and the existing fresh-branch body, and add `self.keep_hidden_local(local.nodes);` in the `if unread.fresh` arm. Task 3 removes the `closed` parts.
  - `teardown_running_session`: after `self.writes.held_text.clear();` add `self.hidden_local.clear();`.

- [ ] **Step 7: Backing files from the document.** One helper in `fetched.rs`, used by both callers:

```rust
/// The backing-file names `doc`'s live nodes hold (closed and hidden ones included, spec decision 11): each id
/// (a content copy is `<id>.<ext>`, a jar's cookies `<id>.json`) and each stored `path`'s file name. `except` leaves
/// one node out.
pub(crate) fn doc_backing_names(doc: &crate::store::model::Doc, except: Option<NodeId>) -> HashSet<String> {
    let mut names = HashSet::new();
    for (id, node) in doc.live().filter(|&(id, _)| Some(id) != except) {
        names.insert(id.to_string());
        if let FieldValue::Str(path) = node.get(field::PATH) {
            names.extend(content::file_name(&path));
        }
    }
    names
}
```

`backing_keep` becomes:
- live nodes, keeping their resolved `file_path`, as now;
- then `if let Some(j) = &self.journal { keep.extend(doc_backing_names(j.doc(), except.and_then(|e| e.parse().ok()))); }`;
- delete the `self.closed` loop (176-179) and fix the doc comment.

`session.rs` `backing_names`: the loop body becomes:

```rust
        let doc = Journal::read_intact(&std::fs::read(&path).ok()?).ok()??;
        names.extend(crate::fetched::doc_backing_names(&doc, None));
```

Delete the `LocalState` read. Its doc says "the ids and `path` file names of each one's live nodes, closed ones included", and "`None` when any save can't be read whole". Drop now-unused imports (`LocalState`).

Check `content::file_name`'s parameter type (`grep -n "pub(crate) fn file_name" crates/manifold/src/content.rs`). Pass `&path` or `path.as_str()` to match.

- [ ] **Step 8: Migrate the existing tests' `Option`s.** In `session_sync/tests.rs`, `Some(x)` as a remote change becomes `Remote::Shown(x)`, and `None` becomes `Remote::Deleted`, at:
  - 391, 500, 540-543, 727, 2070, 2073, 2249, 2305, 5117;
  - 5987-5990: `matches!(&changes[0].1, Remote::Shown(s) if s.ty == FUTURE)`.

  Also migrate `tabs/tests.rs:53` (`None` to `Remote::Deleted`) and `json_level/tests.rs:308`. Find any missed one with `git grep -nE "remote_in_flight|apply_remote\(|RemoteChange|remote_changes\(" -- crates/manifold/src` and read each hit.

- [ ] **Step 9: Run the tests to pass.** Run `cargo test -j 8 -p manifold`. The `NOT_FORGOTTEN` test (`main/tests.rs:58-165`) passes, because `hidden_local` appears in `forget_id`'s body (D5).

- [ ] **Step 10: Leftover check.**
  - `git grep -nE 'Option<Snapshot>|Option<Option<RemoteNode>>|values_mut\(\)\.flatten\(\)|Some\(None\) =>' -- crates/manifold/src` gives no hits.
  - `git grep -n "self.closed" -- crates/manifold/src/fetched.rs crates/manifold/src/session.rs` gives no hits.

- [ ] **Step 11: Format, lint, commit.**
  ```bash
  cargo fmt --all
  cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
  git add -A crates && git commit -m "Sync: three-state remote changes and a hide path (unshow, hide_node, leave_hidden, hidden_local); backing files from the document

  Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
  ```

---

### Task 3: Close, reopen, evict; the derived history

The behaviour changes here:
- A close keeps the node's id and hides the node in place.
- Reopen brings back the same node.
- The history is `closed_roots()`, capped at 20 by `evict_closed`.
- The failed-twin and stale-card cleanups delete instead of recording (spec "Behaviour").
- Duplicate still goes through `subtree_entry` and `reopen_closed`, which stay until Task 4.

**Files:**
- Modify `crates/manifold/src/store/model.rs`: add `Doc::closed_roots`, plus a test.
- Modify `crates/manifold/src/store/journal.rs`: add `Journal::actor()` near `doc()` at 855.
- Modify `crates/manifold/src/session_sync.rs`:
  - `PendingWrites` at 359 (add `closed_flags`);
  - `commit_note_buffers` at 996 (make it `pub(crate)`);
  - `save_session` at 1302-1320 (emit the flags) and 1404-1420 (drain them, visibility pass);
  - `apply_remote` (evict at the end);
  - `restore_session` (evict at load; delete `restore_closed` calls at 1509 and 1688);
  - `retry_read_local` 1598;
  - `local_state` 1657, `restore_closed` 1735-1742;
  - `maybe_write_local` 1788-1810 (drop `AFTER_CLOSED`);
  - `replace_running_session` 2056-2064;
  - imports at 23.
- Modify `crates/manifold/src/closed.rs`:
  - add `close_node`, `reopen_node`, `evict_closed`;
  - delete `closed_entry`, `touch_backing`, `record_closed`;
  - make `members_under` `pub(crate)`.
- Modify `crates/manifold/src/node_ops.rs`:
  - `close_node` at 182-197 (moves to `closed.rs`);
  - `destroy_node`'s second guard at 210-213;
  - delete `close_node_and_save` at 223-234.
- Modify `crates/manifold/src/groups.rs`: delete the eject branch at 119-122.
- Modify `crates/manifold/src/jar_defs.rs`: `ensure_default_jar` at 336-350.
- Modify `crates/manifold/src/shortcuts.rs`: `reopen_closed_shortcut` at 372-380.
- Modify `crates/manifold/src/popups.rs`: `build_recently_closed_popup` at 748-795.
- Modify `crates/manifold/src/graph_ui.rs`: `toolbar_recently_closed` at 1140-1150.
- Modify `crates/manifold/src/tabs.rs`: 691 (`record_closed` before the twin's removal).
- Modify `crates/manifold/src/tabs/spawn_result.rs`: 266 and 323.
- Modify `crates/manifold/src/local_state.rs`: `LocalState.closed` at 33-35, `closed_changed_at` at 124-126 and 141, tests at 236-238, 247 and 251.
- Modify `crates/manifold/src/main.rs`: delete the `closed: Vec<ClosedEntry>` field (~442-446).
- Modify the `close_node_and_save` callers: `node_menu.rs:1060`, `popups.rs:1123`, `debug_control.rs:1072` (and the docs at 361 and 1067), `main/tests.rs:302`.
- Tests: `closed.rs` 373-642, `session_sync/tests.rs`, `fetched/tests.rs`, `jar_defs/tests.rs`, `local_state.rs` (table in Step 9).

**Interfaces:**
- Consumes: Tasks 1-2 (`is_shown`, `shown_level`, `hide_node`, `leave_hidden`, `forget_deleted`, `hidden_local`, `Remote`), `worth_reopening`, `members_under`, `queue_delete`, `edit_pos`, `begin_tabs_shutdown`, `sweep_jar_switches`.
- Produces:
  - `Doc::closed_roots(&self) -> Vec<(NodeId, Stamp)>`, `Journal::actor(&self) -> ActorId`;
  - `PendingWrites.closed_flags`;
  - `App::close_node(&mut self, &str)` (now saves itself), `App::reopen_node(&mut self, NodeId)`, `App::evict_closed(&mut self)`.

- [ ] **Step 1: Write the failing tests.**

`model.rs`:

```rust
    #[test]
    fn closed_roots_are_newest_first_and_skip_nested_and_deleted_ones() {
        let (mut doc, id) = doc_of(&[
            (1, Kind::Note, None, true),     // stamp 1
            (2, Kind::Group, None, true),    // stamp 2
            (3, Kind::Note, Some(2), true),  // stamp 3, nested: part of 2's entry
            (4, Kind::Tab, None, true),      // stamp 4
            (5, Kind::Tab, None, false),
        ]);
        let ids = |doc: &Doc| doc.closed_roots().into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(ids(&doc), [id(4), id(2), id(1)]);
        doc.nodes.get_mut(&id(4)).unwrap().deleted = true;
        let reg = doc.nodes.get_mut(&id(2)).unwrap().fields.get_mut(&field::CLOSED).unwrap();
        reg.enc = encode_field(field::CLOSED, &FieldValue::Bool(false));
        assert_eq!(ids(&doc), [id(3), id(1)], "clearing the outer one makes the nested one an entry");
    }
```

`closed.rs` `mod tests`. Rewrite the module around a journal app: `session_sync::tests::{app_on, add, add_group, stored, remote_set}`. The new and rewritten cases:

```rust
    use crate::session_sync::tests::{add, add_group, app_on, stored};
    use crate::store::model::{field, FieldValue, Kind, NodeId};

    fn note(app: &mut App, text: &str) -> String {
        add(app, Kind::Note, Runtime::Note(NotePayload { text: text.into(), editing: false }))
    }

    fn closed_ids(app: &App) -> Vec<String> {
        app.journal.as_ref().unwrap().doc().closed_roots().into_iter().map(|(n, _)| n.to_string()).collect()
    }

    #[test]
    fn close_flags_the_root_hides_the_subtree_and_reopen_brings_the_same_ids_back() {
        let path = store::testutil::temp_dir("closed-same-ids").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let inner = note(&mut app, "kept");
        app.edit_group(&inner, Some(g.clone()));
        let outside = note(&mut app, "outside");
        let _link = app.create_link(&inner, &outside);
        assert!(app.save_session());
        let link: Vec<NodeId> = app.mirror.link_ids().collect();
        app.close_node(&g);
        assert_eq!(stored(&app, &g, field::CLOSED), FieldValue::Bool(true));
        assert_eq!(stored(&app, &inner, field::CLOSED), FieldValue::Bool(false), "only the root is flagged");
        assert!(!app.nodes.contains_key(&g) && !app.nodes.contains_key(&inner));
        assert_eq!(closed_ids(&app), [g.clone()]);
        app.reopen_node(g.parse().unwrap());
        assert!(app.nodes.contains_key(&g) && app.nodes.contains_key(&inner), "same ids");
        assert_eq!(app.mirror.group(&inner), Some(g.as_str()));
        assert_eq!(app.mirror.link_ids().collect::<Vec<_>>(), link, "the link to the outside node survived");
        assert!(closed_ids(&app).is_empty());
    }

    #[test]
    fn a_close_flushes_typed_text_and_the_drawn_position() {
        let path = store::testutil::temp_dir("closed-flush").join("session.mnf");
        let mut app = app_on(&path, 1);
        let n = note(&mut app, "v1");
        assert!(app.save_session());
        app.nodes.get_mut(&n).unwrap().as_note_mut().unwrap().text = "typed".into();
        app.snap_layout(&n, egui::vec2(120.0, 80.0));
        app.close_node(&n);
        assert_eq!(stored(&app, &n, field::TEXT), FieldValue::Str("typed".into()));
        assert_eq!(stored(&app, &n, field::POS), FieldValue::Pos(120.0, 80.0));
        app.reopen_node(n.parse().unwrap());
        assert_eq!(app.note_text(&n), "typed");
        assert_eq!(app.layout_pos(&n), egui::vec2(120.0, 80.0));
    }

    #[test]
    fn a_blank_note_in_a_closing_group_is_deleted_and_a_blank_root_is_not_recorded() {
        let path = store::testutil::temp_dir("closed-blank").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let blank = note(&mut app, "  ");
        app.edit_group(&blank, Some(g.clone()));
        let lone = note(&mut app, "");
        assert!(app.save_session());
        app.close_node(&g);
        let doc = app.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&blank.parse().unwrap()].deleted, "dropped as the snapshot dropped it");
        app.close_node(&lone);
        assert!(app.journal.as_ref().unwrap().doc().nodes[&lone.parse().unwrap()].deleted);
        assert_eq!(closed_ids(&app), [g]);
    }

    #[test]
    fn an_in_progress_download_in_a_closing_group_is_removed() {
        let path = store::testutil::temp_dir("closed-download").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let tab = crate::session_sync::tests::add(
            &mut app,
            Kind::Tab,
            crate::session_sync::tests::sleeping_tab("https://x.test/", "T"),
        );
        app.edit_group(&tab, Some(g.clone()));
        assert!(app.save_session());
        // An in-progress download takes its tab's group and has no document node (`restorable` is false).
        app.insert_download_node(tab.clone(), "f.bin".into(), Some(3), Some("https://x.test/f.bin".into()));
        let download = app.nodes.iter().find(|(_, n)| n.as_download().is_some()).map(|(id, _)| id.clone()).unwrap();
        assert_eq!(app.mirror.group(&download), Some(g.as_str()));
        app.close_node(&g);
        assert!(!app.nodes.contains_key(&download), "the download went with the closing group");
        app.reopen_node(g.parse().unwrap());
        assert!(!app.nodes.contains_key(&download), "reopening the group doesn't bring it back");
        assert!(app.nodes.contains_key(&tab), "the tab came back");
    }

    #[test]
    fn closing_past_twenty_evicts_the_oldest_whole_subtree() {
        let path = store::testutil::temp_dir("closed-evict").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "Oldest");
        let nested = note(&mut app, "nested");
        app.edit_group(&nested, Some(g.clone()));
        let rest: Vec<String> = (0..MAX_CLOSED).map(|i| note(&mut app, &format!("n{i}"))).collect();
        assert!(app.save_session());
        app.close_node(&nested); // a closed node nested in the oldest root: evicted with it
        app.close_node(&g);
        for n in &rest {
            app.close_node(n);
        }
        let doc = app.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&g.parse().unwrap()].deleted && doc.nodes[&nested.parse().unwrap()].deleted);
        assert_eq!(closed_ids(&app).len(), MAX_CLOSED);
    }

    #[test]
    fn reopening_a_group_re_stamps_its_nested_roots_newest_in_old_order() {
        let path = store::testutil::temp_dir("closed-restamp").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let (a, b) = (note(&mut app, "a"), note(&mut app, "b"));
        app.edit_group(&a, Some(g.clone()));
        app.edit_group(&b, Some(g.clone()));
        let other = note(&mut app, "other");
        assert!(app.save_session());
        app.close_node(&a);
        app.close_node(&b);
        app.close_node(&g);
        app.close_node(&other);
        app.reopen_node(g.parse().unwrap());
        assert_eq!(closed_ids(&app), [b, a, other], "the nested ones are newest, b after a as before");
    }

    #[test]
    fn ctrl_shift_t_skips_another_replicas_close() {
        let path = store::testutil::temp_dir("closed-mine").join("session.mnf");
        let mut app = app_on(&path, 1);
        let (mine, theirs) = (note(&mut app, "mine"), note(&mut app, "theirs"));
        assert!(app.save_session());
        app.close_node(&mine);
        let mut other = crate::store::journal::Journal::open(&path, crate::store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, &theirs, field::CLOSED, FieldValue::Bool(true));
        assert!(app.save_session());
        assert_eq!(closed_ids(&app)[0], theirs, "theirs is newest");
        app.reopen_closed_shortcut();
        assert!(app.nodes.contains_key(&mine) && !app.nodes.contains_key(&theirs));
    }

    #[test]
    fn a_replica_that_only_receives_closes_evicts_and_agrees() {
        let path = store::testutil::temp_dir("closed-remote-evict").join("session.mnf");
        let mut a = app_on(&path, 1);
        let notes: Vec<String> = (0..=MAX_CLOSED).map(|i| note(&mut a, &format!("n{i}"))).collect();
        assert!(a.save_session());
        // Another replica closes all MAX_CLOSED + 1 notes; A closes none, it only receives them.
        let mut other = crate::store::journal::Journal::open(&path, crate::store::model::ActorId(2)).unwrap();
        for n in &notes {
            crate::session_sync::tests::remote_set(&mut other, n, field::CLOSED, FieldValue::Bool(true));
        }
        assert!(a.save_session());
        let doc = a.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&notes[0].parse().unwrap()].deleted, "A evicted the oldest on receipt");
        assert_eq!(closed_ids(&a).len(), MAX_CLOSED);
        // A replica restored from the same file agrees on the same set and order.
        let mut b = app_on(&path, 3);
        b.restore_session();
        assert_eq!(closed_ids(&a), closed_ids(&b), "one order on every replica");
    }

    #[test]
    fn a_reopen_concurrent_with_its_eviction_loses() {
        let path = store::testutil::temp_dir("closed-reopen-vs-evict").join("session.mnf");
        let mut a = app_on(&path, 1);
        let notes: Vec<String> = (0..=MAX_CLOSED).map(|i| note(&mut a, &format!("n{i}"))).collect();
        assert!(a.save_session());
        for n in &notes[..MAX_CLOSED] {
            a.close_node(n);
        }
        // B opens now, so it still sees notes[0] closed and has not seen A's 21st close.
        let mut b = crate::store::journal::Journal::open(&path, crate::store::model::ActorId(2)).unwrap();
        a.close_node(&notes[MAX_CLOSED]); // A's 21st close evicts notes[0] ...
        crate::session_sync::tests::remote_set(&mut b, &notes[0], field::CLOSED, FieldValue::Bool(false)); // ... B's stale reopen.
        assert!(a.save_session());
        assert!(a.journal.as_ref().unwrap().doc().nodes[&notes[0].parse().unwrap()].deleted, "Delete wins");
        assert!(!a.nodes.contains_key(&notes[0]));
    }

    #[test]
    fn a_tab_on_a_closed_jar_uses_the_default_and_returns() {
        let path = store::testutil::temp_dir("closed-jar-fallback").join("session.mnf");
        let mut app = app_on(&path, 1);
        app.ensure_default_jar();
        let default = app.default_jar().unwrap();
        let work = app.create_jar("Work", crate::fetched::Placement { group: None, pos: egui::Vec2::ZERO });
        let tab = crate::session_sync::tests::add(
            &mut app,
            Kind::Tab,
            crate::session_sync::tests::sleeping_tab("https://a.com/", "A"),
        );
        app.set_jar_edge(&tab, Some(work));
        assert!(app.save_session());
        assert_eq!(app.effective_jar(&tab), Some((work, false)));
        app.close_node(&work.to_string());
        assert_eq!(app.effective_jar(&tab), Some((default, true)), "the closed jar dangles to the Default");
        app.reopen_node(work);
        assert_eq!(app.effective_jar(&tab), Some((work, false)), "reopened: Work again; the stored edge never changed");
    }

    #[test]
    fn a_default_arriving_inside_a_closed_group_is_moved_out_not_shadowed() {
        let path = store::testutil::temp_dir("closed-default").join("session.mnf");
        let mut app = app_on(&path, 1);
        app.ensure_default_jar();
        let g = add_group(&mut app, "G");
        assert!(app.save_session());
        let default = app.default_jar().unwrap();
        let mut other = crate::store::journal::Journal::open(&path, crate::store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, &g, field::CLOSED, FieldValue::Bool(true));
        let op = crate::store::ops::OpBody::Move { node: default, group: Some(g.parse().unwrap()) };
        other.begin().unwrap().commit(vec![op]).unwrap();
        assert!(app.save_session());
        assert_eq!(app.default_jar(), Some(default), "the same Default, shown again");
        let doc = app.journal.as_ref().unwrap().doc();
        assert_eq!(doc.nodes[&default].group, None);
        let defaults = doc.live().filter(|(_, n)| n.get(field::IS_DEFAULT) == FieldValue::Bool(true)).count();
        assert_eq!(defaults, 1, "no second Default was made");
    }
```

`session_sync/tests.rs`:

```rust
#[test]
fn a_close_commits_its_flag_and_a_reopen_commits_the_clear() {
    // The commit gate: a closed node is live, so the reopen Set and an eviction Delete are not dropped.
    let path = temp_dir("sync-close-gate").join("session.mnf");
    let mut app = app_on(&path, 1);
    let n = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "x".into(), editing: false }));
    assert!(app.save_session());
    app.close_node(&n);
    assert!(live_in_doc(&app, &n));
    app.reopen_node(n.parse().unwrap());
    assert_eq!(stored(&app, &n, field::CLOSED), FieldValue::Bool(false));
    app.close_node(&n);
    app.queue_delete(&n);
    assert!(app.save_session());
    assert!(!live_in_doc(&app, &n), "the eviction-style Delete of a closed node commits");
}
```

- [ ] **Step 2: Run them to fail.** Run `cargo test -j 8 -p manifold closed:: closed_roots a_close_commits` and expect compile errors.

- [ ] **Step 3: Store and journal.** `model.rs`, in `impl Doc`:

```rust
    /// Recently Closed (spec decision 5): the live closed nodes that are not hidden by another closed node, newest
    /// first by their `closed` register's stamp (ties by id, newest first), the same order on every replica.
    pub fn closed_roots(&self) -> Vec<(NodeId, Stamp)> {
        let mut roots: Vec<(NodeId, Stamp)> = self
            .live()
            .filter(|&(id, n)| n.closed() && self.placement(id) != Placement::Hidden)
            .filter_map(|(id, n)| Some((id, n.fields.get(&field::CLOSED)?.stamp)))
            .collect();
        roots.sort_by(|a, b| (b.1, b.0).cmp(&(a.1, a.0)));
        roots
    }
```

`journal.rs`, after `doc()`:

```rust
    /// The actor this journal writes as: what `Ctrl+Shift+T` compares a `closed` stamp's actor with.
    pub fn actor(&self) -> ActorId {
        self.actor
    }
```

- [ ] **Step 4: The flag queue and the visibility pass.** In `PendingWrites`:

```rust
    /// `closed` writes from Close and Reopen (spec decisions 1 and 7), in the order they were made: a reopen's
    /// re-stamps of nested roots must commit in their old relative order, which `pending`'s key order would lose.
    /// Each commits as one `Set` per entry, after the other sets; a node the document doesn't hold live is skipped.
    pub(crate) closed_flags: Vec<(NodeId, bool)>,
```

In `save_session`, after the `pending` loop (1302-1319) and before `ops.extend(deleted ..)`:

```rust
            for &(node, closed) in &self.writes.closed_flags {
                if editable(&node) {
                    let enc = encode_field(field::CLOSED, &FieldValue::Bool(closed));
                    ops.push(OpBody::Set { node, field: field::CLOSED, enc });
                }
            }
```

`moved` is declared inside the loop body before the commit, so it stays in scope. In the `Ok(())` arm:
- after `self.writes.pending_moves.clear();` add `let flagged: BTreeSet<NodeId> = self.writes.closed_flags.drain(..).map(|(n, _)| n).collect();`;
- just before `#[cfg(debug_assertions)] self.check_graph_matches_document();`, add:

```rust
                    // What this commit's flags and moves changed about visibility (a reopen, a nested root
                    // re-stamped, a hidden Default moved out) is shown or hidden the way a remote change is.
                    self.apply_visibility(flagged.into_iter().chain(moved).collect());
```

And the helper, beside `remote_changes`:

```rust
    /// Shows or hides `touched` and everything under them to match the document, for the nodes whose shown state
    /// differs from the graph's: the local half of what `apply_remote` does for other replicas' changes. A local close
    /// already hid its subtree (`close_node`), so this only finds what a reopen or a repair brought back.
    fn apply_visibility(&mut self, touched: BTreeSet<NodeId>) {
        let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
        if touched.is_empty() {
            return;
        }
        let changes: Vec<RemoteChange> = doc
            .with_descendants(touched)
            .into_iter()
            .filter_map(|id| {
                let node = doc.nodes.get(&id).filter(|n| n.is_live() && n.kind() != Kind::Link)?;
                let key = id.to_string();
                let here = self.nodes.contains_key(&key) || self.spawns.spawning.contains_key(&key);
                match (doc.is_shown(id), here) {
                    (true, false) => Some((id, Remote::Shown(Snapshot::of(doc, id, node)))),
                    (false, true) => Some((id, Remote::Hidden)),
                    _ => None,
                }
            })
            .collect();
        if !changes.is_empty() {
            self.apply_remote(changes);
        }
    }
```

`apply_remote` ends with `self.sweep_jar_switches();` then a new `self.evict_closed();` (decision 6: "after applying remote ops"). `evict_closed` saves only when it deletes something, and the save's own visibility pass finds nothing to change for deleted ids. So there is no unbounded recursion: one nested save per eviction, and an eviction cannot trigger another.

Make `commit_note_buffers` `pub(crate)`.

- [ ] **Step 5: The verbs**, in `closed.rs` (replacing `record_closed`, `closed_entry` and `touch_backing`, all deleted). `members_under` becomes `pub(crate)` (Duplicate uses it too).

```rust
impl App {
    /// Whether closing `id` keeps it (spec decision 4): a node the document can hold whose kind and value are worth
    /// reopening (`worth_reopening`). Without a journal nothing is kept: the close is a plain destroy.
    fn restorable(&self, id: &str) -> bool {
        let Some(node) = id.parse::<store::model::NodeId>().ok() else { return false };
        self.journal.is_some() && self.snapshot(node).is_some_and(|s| worth_reopening(&s.ty, &s.fields))
    }

    /// Closes one node (spec decision 4), the single user-facing close: a card's hover ✕, its context-menu
    /// Delete/Close/"Remove from List", the Downloads popup's Remove, Ctrl+W, Close Selected and the rest. Typed text
    /// and the drawn position are written first; a node not worth reopening is destroyed as before; otherwise the
    /// members not worth reopening go (a blank note, a download), `closed` is queued on the root, and the subtree is
    /// hidden in place with its engines unloaded. Saves, then evicts past `MAX_CLOSED`.
    pub(crate) fn close_node(&mut self, id: &str) {
        if self.is_default_jar(id) {
            self.queue_toast("The Default jar can't be closed".to_string());
            return;
        }
        self.commit_note_buffers();
        // A node created this tick has no snapshot yet and its `closed` flag would be dropped as not editable (the
        // hide below would then lose it): flush the pending creates first.
        if !self.writes.pending_creates.is_empty() {
            self.save_session();
        }
        if !self.restorable(id) {
            self.destroy_node(id);
            // A closed jar's tabs now resolve to the Default jar.
            self.sweep_jar_switches();
            self.save_session();
            return;
        }
        let root: store::model::NodeId = id.parse().expect("restorable ids parse");
        // The subtree as this replica holds it: the mirror's membership over `node_order`, so a tab mid-respawn and a
        // member not saved yet are found too.
        let order: Vec<store::model::NodeId> = self.node_order.iter().filter_map(|s| s.parse().ok()).collect();
        let members = members_under(
            root,
            &order,
            |m| self.mirror.group(&m.to_string()).and_then(|g| g.parse().ok()),
            |m| self.is_container(&m.to_string()),
        );
        let subtree: Vec<String> = std::iter::once(root).chain(members).map(|n| n.to_string()).collect();
        // Where each card was drawn becomes its document pos (2b-4 Ruling 10: a reopen lands where it was shown).
        for key in &subtree {
            let carries_pos = Kind::from_type(self.mirror.ty(key)).fields().contains(&field::POS);
            if carries_pos && self.layout.contains_key(key) {
                self.edit_pos(key, Some(self.layout_pos(key)));
            }
        }
        let (kept, gone): (Vec<String>, Vec<String>) =
            subtree.into_iter().partition(|key| key == id || self.restorable(key));
        for key in &gone {
            self.remove_node(key);
        }
        self.writes.closed_flags.push((root, true));
        let tabs: Vec<String> =
            kept.iter().filter(|k| self.nodes.get(*k).is_some_and(|n| n.runtime.saved_tab().is_some())).cloned().collect();
        self.begin_tabs_shutdown(&tabs);
        self.leave_hidden(root);
        for key in &kept {
            self.hide_node(key);
        }
        self.sweep_jar_switches();
        self.save_session();
        self.evict_closed();
    }

    /// Reopen (spec decision 7): clears `closed` on `id` and re-stamps each closed root directly under it, oldest
    /// first, so they enter the history newest in their old order. The save's visibility pass shows what is no
    /// longer hidden (`apply_visibility`), and `apply_remote` sweeps the jar switches. A node that stays hidden
    /// (its group is closed) or lands at another level says so (plan D8).
    pub(crate) fn reopen_node(&mut self, id: store::model::NodeId) {
        let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
        // A closed node `d` under `id` with no other closed node between them.
        let directly_under = |d: store::model::NodeId| {
            let mut cur = doc.nodes.get(&d).and_then(|n| n.group);
            for _ in 0..=doc.nodes.len() {
                let Some(c) = cur else { return false };
                if c == id {
                    return true;
                }
                let Some(n) = doc.nodes.get(&c) else { return false };
                if !n.deleted && n.closed() {
                    return false;
                }
                cur = n.group;
            }
            false
        };
        let mut nested: Vec<(Stamp, store::model::NodeId)> = doc
            .with_descendants(std::collections::BTreeSet::from([id]))
            .into_iter()
            .filter(|&d| d != id && directly_under(d))
            .filter_map(|d| {
                let n = doc.nodes.get(&d).filter(|n| n.is_live() && n.closed())?;
                Some((n.fields.get(&field::CLOSED)?.stamp, d))
            })
            .collect();
        nested.sort();
        self.writes.closed_flags.push((id, false));
        self.writes.closed_flags.extend(nested.into_iter().map(|(_, d)| (d, true)));
        self.save_session();
        let key = id.to_string();
        let reopened = self.journal.as_ref().is_some_and(|j| j.doc().nodes.get(&id).is_some_and(|n| n.is_live() && !n.closed()));
        if !reopened {
            return;
        }
        if !self.nodes.contains_key(&key) && !self.spawns.spawning.contains_key(&key) {
            self.queue_toast("Reopened inside a closed group: it shows when that group is reopened".to_string());
        } else if self.mirror.group(&key) != self.current_group.as_deref() {
            let level = match self.mirror.group(&key) {
                None => "the top level".to_string(),
                Some(g) if self.mirror.name(g).is_empty() => format!("“{}”", crate::names::UNTITLED),
                Some(g) => format!("“{}”", self.mirror.name(g)),
            };
            self.queue_toast(format!("Reopened in {level}"));
        }
    }

    /// Eviction (spec decision 6): every root past `MAX_CLOSED` has its whole subtree Deleted, nested closed nodes
    /// included, so nothing under it resurfaces. Idempotent across replicas; the only place `closed` becomes
    /// `deleted`. After a local close, after remote ops (`apply_remote`) and at load.
    pub(crate) fn evict_closed(&mut self) {
        let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
        let roots = doc.closed_roots();
        if roots.len() <= MAX_CLOSED {
            return;
        }
        let evicted: Vec<String> = doc
            .with_descendants(roots[MAX_CLOSED..].iter().map(|&(id, _)| id).collect())
            .into_iter()
            .filter(|id| doc.nodes.get(id).is_some_and(|n| !n.deleted))
            .map(|id| id.to_string())
            .collect();
        for id in &evicted {
            self.queue_delete(id);
        }
        // Files are released only after the Delete committed (decision 11: `release_backing` reads `backing_keep`).
        if self.save_session() {
            for id in &evicted {
                self.forget_deleted(id);
            }
        }
    }
}
```

`Stamp` must be imported in `closed.rs` (`use crate::store::model::{Kind, Stamp};`). `self.mirror.name` is the accessor `debug_control.rs:1579` uses. Check that `crate::names::UNTITLED` is a `&str` (it is passed to `display_name` at `popups.rs:704`).

Delete `close_node` and `close_node_and_save` from `node_ops.rs`. In `destroy_node`, delete the second guard (210-213) and its comment. In `groups.rs`, delete the eject branch (119-122) and fix the doc above it, which no longer names the Default.

`close_node` now saves, so the callers change:
- `node_menu.rs:1060`, `popups.rs:1123`, `debug_control.rs:1072`: `close_node_and_save` becomes `close_node`.
- `debug_control.rs:361` and `1067`: the docs say "`close_node`".
- `node_ops.rs:65-71` (`close_all_at_current_level`), `153-166` (`close_all_except`), `graph_ui.rs:321` and `shortcuts.rs:386` already call `close_node`. Each close now saves once, as `destroy_tab` did before, so they don't change.

- [ ] **Step 6: The Default-jar invariant** (spec decision 10; plan D11). At the top of `ensure_default_jar`:

```rust
        // A Default never sits in a group, hidden ones included (spec decision 10): one that arrived inside a group
        // moves out. One this replica shows moves now; one hidden under a closed group is queued, and the save's
        // visibility pass shows it, so no second Default is made meanwhile.
        let grouped: Vec<(NodeId, bool)> = self
            .journal
            .as_ref()
            .map(|j| {
                let doc = j.doc();
                doc.live()
                    .filter(|&(id, n)| n.kind() == Kind::Jar && doc.effective_group(id).is_some())
                    .filter(|(_, n)| n.get(field::IS_DEFAULT) == FieldValue::Bool(true))
                    .map(|(id, _)| (id, self.nodes.contains_key(&id.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let mut queued_hidden = false;
        for (id, here) in grouped {
            if here {
                self.edit_group(&id.to_string(), None);
            } else {
                self.writes.pending_moves.insert(id, None);
                queued_hidden = true;
            }
        }
        if queued_hidden {
            return true;
        }
```

Update its doc comment ("..., and a Default inside a group moves to the top level"). `default_jar` reads the mirror's index, which holds shown jars only, so "uniqueness ignores a closed or hidden jar" holds with no further change. Update its doc ("plus a closed one until the next save" becomes "shown jars only: a closed or hidden one has left the mirror").

- [ ] **Step 7: The history readers.**

`shortcuts.rs`:

```rust
    /// `Ctrl+Shift+T` / headless `REOPENTAB`: reopens the newest node this replica closed (spec decision 7: the
    /// `closed` stamp's actor), where it was. Another replica's closes are left to the popup. A no-op with none.
    pub(crate) fn reopen_closed_shortcut(&mut self) {
        let Some(journal) = &self.journal else { return };
        let mine = journal.actor();
        let newest = journal.doc().closed_roots().into_iter().find(|(_, stamp)| stamp.actor == mine);
        if let Some((id, _)) = newest {
            self.reopen_node(id);
        }
    }
```

`popups.rs` `build_recently_closed_popup`. Build the rows before the frame, so the closure only reads:

```rust
        // Every closed root, any replica's, newest first (spec "Recently Closed rows"): label from its fields, title
        // from its kept personal state, a group's member count from its non-deleted descendants.
        let rows: Vec<(store::model::NodeId, store::json::JsonNode, Option<String>, Option<usize>)> = self
            .journal
            .as_ref()
            .map(|j| {
                let doc = j.doc();
                doc.closed_roots()
                    .into_iter()
                    .filter_map(|(id, _)| {
                        let node = doc.nodes.get(&id)?;
                        let key = id.to_string();
                        let json = store::json::node_json(&key, node.ty(), &session_sync::fields_of(node), None);
                        let title = self.hidden_local.get(&key).map(|l| l.title.clone()).filter(|t| !t.is_empty());
                        let members = (node.kind() == Kind::Group).then(|| {
                            let under = doc.with_descendants(std::collections::BTreeSet::from([id]));
                            under.iter().filter(|d| **d != id && doc.nodes.get(d).is_some_and(|n| !n.deleted)).count()
                        });
                        Some((id, json, title, members))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut reopen: Option<store::model::NodeId> = None;
```

The loop becomes `for (id, root, title, members) in &rows`:
- `Self::closed_entry_label(ui, root, title.as_deref())`;
- `reopen = Some(*id)` on click;
- `if let Some(count) = members { ... member_count_text(*count) ... }`.

After the frame:
- `if let Some(id) = reopen { self.reopen_node(id); close = true; }`;
- `if close || rows.is_empty()`.

Delete `reopen_index` and the `closed_changed_at` line. Keep `closed_entry_label`, and its tests at 1795-1839 stay unchanged.

`graph_ui.rs` `toolbar_recently_closed`: `let count = self.journal.as_ref().map_or(0, |j| j.doc().closed_roots().len());`, then `if count > 0 { let label = format!("Recently Closed ({count})"); ... }`.

- [ ] **Step 8: Delete the old history.**
  - `main.rs`: delete the `closed` field and its doc.
  - `local_state.rs`:
    - delete `LocalState.closed` (33-35) and `LocalPersist.closed_changed_at` (124-126, 141);
    - drop `JsonDoc` from its imports if now unused;
    - in tests: 236 and 238 go (`a_non_finite_value_costs_only_itself` keeps the rest). In `nulls_written_by_older_builds_read_back`, keep `"closed":[{"nodes":[]}]` in the bytes and replace line 251 with a comment-backed assertion that an old file's `closed` key is ignored: `assert_eq!(state.nodes.len(), 1, "an old file's Recently Closed list is ignored, not an error");`.
  - `session_sync.rs`:
    - `local_state` 1657 `closed:` line goes;
    - `apply_local` 1688 goes;
    - `restore_closed` (1731-1742) goes;
    - `retry_read_local` 1598 goes, and its doc drops "keeping what was closed since";
    - `restore_session`'s doc drops "Recently Closed is still restored then";
    - `maybe_write_local` drops `AFTER_CLOSED` and both `closed_changed_at` lines, and its doc says "at most every 30 s, and at exit";
    - `replace_running_session` 2056-2064 becomes `if let Some(state) = ... { self.apply_local(state.remap(&map)); }` with the comment deleted;
    - the doc at 2065-2067 drops "may still bring back its Recently Closed, but not";
    - imports at 23 drop `ClosedEntry` (`MAX_CLOSED` too if unused there).
  - `restore_session` both branches: call `self.evict_closed();` before `self.arm_local(..)` (decision 6: at load).
  - `tabs.rs:691` and `spawn_result.rs:266` and `:323`: delete the `self.record_closed(..)` lines and fix the neighbouring docs. A failed twin or stale card is deleted, not recorded (spec "Behaviour").
  - `closed.rs`: delete `closed_entry`, `touch_backing` and `record_closed`. The module doc becomes "Recently Closed: close, reopen and eviction over the document's `closed` field; and Duplicate's snapshot (moves to duplicate.rs in Task 4)". Keep `ClosedEntry` (minus its history doc: it is "a snapshot `reopen_closed` imports") and `reopen_closed` until Task 4. The `MAX_CLOSED` doc says "How many roots the history keeps before `evict_closed` deletes the oldest".

- [ ] **Step 9: Rewrite or delete the tests that pinned the old behaviour.** Each change below says why.

| Test (file:line as of 3d853d5) | Change |
|---|---|
| `closed.rs` 387 `closed_entry_snapshots_the_whole_subtree` | Delete: `closed_entry` is gone. Its subtree walk is covered by `close_flags_the_root_...` above and by Task 4's Duplicate test. |
| `closed.rs` 444 (live url) | Rewrite: close a tab whose live url differs from the shared one, reopen, assert the sleeping card's url is the live one (via `hidden_local`'s `live_url`). |
| `closed.rs` 475 (position) | Covered by `a_close_flushes_typed_text_and_the_drawn_position`; delete. |
| `closed.rs` 498 (Duplicate) | Keep; Task 4 moves it. |
| `closed.rs` 523 (typed text) | Covered by the flush test; delete. |
| `closed.rs` 537, 547 (`worth_reopening`) | Keep unchanged. |
| `closed.rs` 570 (cycle) | Keep; it tests `members_under`. |
| `closed.rs` 586 (reopened jar) | Rewrite: the reopened jar has the same id and its tabs resolve to it again. |
| `closed.rs` 612 | Rewrite: same id back; the cookie file was never copied or touched and still exists. |
| `closed.rs` 631 `closing_a_jar_touches_its_cookie_file` | Delete: `touch_backing` is gone (decision 11). |
| `fetched/tests.rs` 248-293 | Rewrite on a journal app (`app_on`): drop the `ClosedEntry` pushes and `app.closed.len()`; close the node with `close_node`; assert its file is in `backing_keep` and `release_backing` keeps it. |
| `fetched/tests.rs` 296-316 | Line 316 asserts `closed` in the document, not an entry. |
| `fetched/tests.rs` 425-440 (`touch_backing`) | Delete. |
| `fetched/tests.rs` 688-705 | Switch to `app_on`: without a journal, a close is a plain destroy. |
| `fetched/tests.rs` 543-553 | Unchanged here; Task 5 revisits its `parent` read. |
| `fetched/tests.rs` 183-208 `a_reload_keeps_the_stored_path_and_closing_records_the_resolved_one` | Drop the last two lines (`closed_entry`, deleted here) and rename to `a_reload_keeps_the_stored_path`; Task 4 re-adds the resolved-path assertion through `subtree_json`. |
| `session_sync/tests.rs` ~2366 (release-backing test) | The doc-based `backing_keep` keeps a node live in the document, so a real remote Delete must be driven: use a second `Journal` (actor 2) and `remote_delete`-style `OpBody::Delete`, then `save_session`, then assert the file is released. |
| `local_state.rs` 236-238, 247, 251 | As in Step 8. |
| `main/tests.rs` 302 | `close_node`. The message becomes "closing never wipes credentials". |
| `jar_defs/tests.rs` 215, 501, 503 | `close_node_and_save` becomes `close_node`. |
| `jar_defs/tests.rs` 56-66, 650-658 | Unchanged: `App::default` has no journal, so a close is a destroy, as before. |
| `jar_defs/tests.rs` 661-670 | Unchanged here (Duplicate); Task 4 re-checks. |
| `session_sync/tests.rs` 1601 `closing_an_unloaded_tab_reparents_its_children` | Flip and rename to `closing_an_unloaded_tab_keeps_its_childrens_edge`. The leaf's stored and mirrored `parent` stays `mid` (decision 9). Task 5 adds `opened_from(leaf) == None`. |
| 1709 `tab_lists_include_sleeping_tabs` | Unchanged: `close_all_at_current_level` still empties `nodes` at the level. |
| 1940 `a_successful_spawn_clears_failed_cards_for_its_url` | The assertion becomes "the stale card is deleted in the document and not in `closed_roots`". |
| 1945 `a_cleared_failed_card_reopens_from_recently_closed` | Delete: a deliberate behaviour change (spec "Behaviour"). |
| 2110, 2396, 4767, 4786, 4808, 5821, 6023 | `close_node_and_save` becomes `close_node`; then the per-test changes below. |
| 2122 | Unchanged (`remove_node`). |
| 2366 `a_remote_jar_delete_releases_its_file_unless_recently_closed_holds_it` | Rewrite: `held` is closed in the document (`close_node`) instead of `record_closed`. A remote Delete of `released` releases its file; `held`'s stays. |
| 2390 | Unchanged (`App::default`). |
| 2460 `a_reopened_tab_comes_back_asleep_with_its_zoom_and_title` | Rewrite through `close_node` and `reopen_node`, with the same id. |
| 2517 `an_empty_session_keeps_recently_closed_across_a_launch` | Rewrite: a closed tab survives a relaunch as history (`closed_roots` holds it), and `hidden_local` keeps its title. |
| 2539 `personal_state_startup_could_not_read...` | Drop the closed parts (2569-2592); keep the unread and retry assertions. |
| 2630 `opening_a_session_while_unread_takes_only_recently_closed_on_retry` | Rename to `opening_a_session_while_unread_applies_nothing_on_retry`; keep the view assert and drop the closed one. |
| 5017, 5025 | The other card is deleted, not recorded. |
| 5462 `reopening_a_closed_group_restores_links_inside_it_only` | Flip and rename to `reopening_a_closed_group_restores_every_link_with_its_id`. Every link survives with the same ids, the outside one included. |
| 5616 (closing an outer group with links) | Rewrite: the blank note is deleted with its link; the other links stay in the mirror and are drawn again after a reopen. |
| 5682 | Unchanged. |
| 5738 `closing_a_group_commits_one_frame` | Keep the one-frame assertion; the group is live and `closed` in the document, not deleted. |
| 5816 `closing_a_jar_records_it...` | Rename to `closing_a_jar_keeps_it_for_reopen`: the same id comes back. |
| 5999 `an_unknown_card_copies_as_a_shell...` 6023-6038 | The reopen half asserts the same id with its raw fields intact (Model: "A closed unknown node keeps its raw fields and reopens whole"); the copy half is unchanged. |
| 356-378 teardown | Add a closed node to the teardown test: it is deleted too (D12). |
| 267 `destroy_node` | Unchanged. |

Find any test the table misses with `git grep -nE "record_closed|app\.closed|\.closed\.(len|push|pop|is_empty|iter)|closed_changed_at|reopen_closed\(|close_node_and_save" -- crates/manifold/src`. Each remaining hit is updated by the same rule: same id, flag in the document, no snapshot. Report each one changed beyond the table.

- [ ] **Step 10: Run the tests to pass.** `cargo test -j 8 -p manifold`.

- [ ] **Step 11: Leftover check.** `git grep -nE 'record_closed|closed_changed_at|AFTER_CLOSED|restore_closed|touch_backing|close_node_and_save|closed_entry\(|app\.closed\b|self\.closed\b|LocalState \{[^}]*closed|eject' -- crates` gives no hits. `ClosedEntry` and `reopen_closed` may still hit `closed.rs`; Task 4 removes them.

- [ ] **Step 12: Format, lint, commit.**
  ```bash
  cargo fmt --all
  cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
  git add -A crates && git commit -m "Close sets closed, reopen clears it, eviction past 20; history derived from the document

  Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
  ```

---

### Task 4: Duplicate moves to `duplicate.rs`

Duplicate becomes a pure export and paste of a shown subtree (spec decision 13). `ClosedEntry` and `reopen_closed` go.

**Files:**
- Create `crates/manifold/src/duplicate.rs`.
- Modify `crates/manifold/src/closed.rs`. Move out `ClosedEntry`, `DUPLICATE_OFFSET`, `tab_zoom`, `tab_title`, `live_fields`, `subtree_entry`, `duplicate_nodes`, `duplicate_selection` and `reopen_closed`. Delete `ClosedEntry`. Move the tests at 498 and 570 (the cycle test stays with `members_under` in `closed.rs`).
- Modify `crates/manifold/src/main.rs`: `mod closed` at 12 and `use closed::*` at 82; add `mod duplicate;` and `use duplicate::*;` beside them.
- Modify `crates/manifold/src/fetched/tests.rs` 183-208 (the reload-path test), as below.
- Modify `crates/manifold/src/jar_defs/tests.rs:661-670`, if it names a moved item.

**Interfaces:**
- Consumes: `members_under` (`closed.rs`, `pub(crate)` since Task 3), `App::snapshot`, `store::json::{node_json, import, JsonDoc}`, `materialize_imported`, `insertion_group`, `duplicate_tab`.
- Produces: `duplicate::subtree_json(app, roots, offset) -> Option<JsonDoc>` and `App::paste_copy(&mut self, JsonDoc)`. `App::duplicate_nodes` and `App::duplicate_selection` keep their signatures.

- [ ] **Step 1: Write the failing test** in `duplicate.rs`'s `mod tests` (the closed.rs 498 Duplicate test moves here unchanged, beside it):

First make `restored_unknown` and `FUTURE` in `session_sync/tests.rs` (5906-5911) `pub(crate)`, plus `doc_fields` if used. `links_naming` and `Mirror::link_ends` are already `pub(crate)`.

```rust
    #[test]
    fn duplicating_a_group_copies_an_unknown_member_as_a_shell_a_jar_and_inside_links_only() {
        use crate::node::NotePayload;
        use crate::session_sync::tests::{add, add_group, add_jar, restored_unknown, FUTURE};
        use crate::store::model::GENERIC;
        let (mut app, unknown, _path) = restored_unknown();
        let u = unknown.to_string();
        let note_rt = |t: &str| Runtime::Note(NotePayload { text: t.into(), editing: false });
        let g = add_group(&mut app, "G");
        let note = add(&mut app, Kind::Note, note_rt("n"));
        let jar = add_jar(&mut app, "Work").to_string();
        let outside = add(&mut app, Kind::Note, note_rt("o"));
        for member in [&note, &u, &jar] {
            app.edit_group(member, Some(g.clone()));
        }
        let _inside = app.create_link(&note, &u);
        let _across = app.create_link(&note, &outside);
        assert!(app.save_session());
        let before: std::collections::BTreeSet<String> = app.nodes.keys().cloned().collect();
        app.duplicate_nodes(std::slice::from_ref(&g));
        let fresh: Vec<String> = app.nodes.keys().filter(|k| !before.contains(*k)).cloned().collect();
        assert_eq!(fresh.len(), 4, "a group and its three members, all with fresh ids");
        let of_type = |ty: &str| fresh.iter().find(|k| app.mirror.ty(k) == ty).cloned();
        let new_group = of_type(Kind::Group.type_str().unwrap()).expect("a new group");
        let new_note = of_type(Kind::Note.type_str().unwrap()).expect("a new note");
        let new_jar = of_type(Kind::Jar.type_str().unwrap()).expect("a new jar");
        let new_unknown = of_type(FUTURE).expect("a new unknown card");
        for member in [&new_note, &new_jar, &new_unknown] {
            assert_eq!(app.mirror.group(member), Some(new_group.as_str()), "members land in the new group");
        }
        // The unknown copy is a shell of the stored type: only the type, the group and the generic fields.
        let doc = app.journal.as_ref().unwrap().doc();
        let shell = &doc.nodes[&new_unknown.parse().unwrap()];
        assert_eq!(shell.ty(), FUTURE);
        assert!(shell.fields.keys().all(|no| *no == field::TYPE || *no == field::GROUP || GENERIC.contains(no)));
        assert_ne!(doc.nodes[&new_jar.parse().unwrap()].get(field::IS_DEFAULT), FieldValue::Bool(true));
        // Links: the one inside the group is copied between the copies; the one leaving it is not.
        assert_eq!(app.links_naming(&new_note).len(), 1, "only the inside link");
        assert_eq!(app.links_naming(&new_unknown).len(), 1);
        assert_eq!(app.links_naming(&outside).len(), 1, "the outside card gained no link");
        let link = app.links_naming(&new_note).remove(0);
        let (from, to) = app.mirror.link_ends(&link);
        let ends = [from.map(|n| n.to_string()), to.map(|n| n.to_string())];
        assert!(ends.contains(&Some(new_note.clone())) && ends.contains(&Some(new_unknown.clone())));
    }
```

- [ ] **Step 2: Run it to fail** (`cargo test -j 8 -p manifold duplicate::`): `duplicate` does not exist yet.

- [ ] **Step 3: Move and rename.** `duplicate.rs`:

```rust
//! Duplicate: a copy of shown nodes beside them, with fresh ids (spec 2026-10-14-closed-field-design.md decision 13).
//! A pure export of the subtree (`subtree_json`) pasted back through `store::json::import` (`App::paste_copy`),
//! independent of Recently Closed.

use super::*;

/// How far a duplicate lands from its original, in the level's world units (`App::duplicate_nodes`).
const DUPLICATE_OFFSET: f32 = 40.0;
```

Then:
- `tab_zoom`, `tab_title` and `live_fields` move verbatim. In `live_fields`' doc, "Recently Closed reopens" becomes "A copy is".
- `subtree_entry` becomes `pub(crate) fn subtree_json(app: &App, roots: &[&str], offset: egui::Vec2) -> Option<store::json::JsonDoc>`:
  - delete the `keep` parameter: its only caller now passes `|_, _| true`;
  - drop the `keep` checks;
  - return `JsonDoc { nodes, local }` instead of wrapping it in `ClosedEntry`;
  - keep the Default skip, and rewrite its comment to "The Default never sits in a group (spec decision 10); a copy of it would be a second Default";
  - call `crate::closed::members_under` (or plain `members_under` through `use super::*`, since `closed::*` is glob-imported in `main.rs`).
- `duplicate_nodes`: `let entry = subtree_json(self, &roots, egui::Vec2::splat(DUPLICATE_OFFSET));` and `if let Some(doc) = entry { self.paste_copy(doc); }`.
- `duplicate_selection` moves unchanged.
- `reopen_closed(entry: ClosedEntry)` becomes `pub(crate) fn paste_copy(&mut self, doc: store::json::JsonDoc)`:
  - it reads `doc.local` directly: `let local = |key: &str, label: &str| doc.local.as_ref()?.get(key)?.get(label).cloned();`;
  - delete the `rebind_jar_files` call (D9);
  - its doc becomes "Pastes a Duplicate's export where the original was: fresh ids via `store::json::import`, links inside remapped, the root at `insertion_group`, tabs asleep with the zoom and title the export recorded. Ends with `save_session`";
  - the error toast becomes "Couldn't duplicate: {err}".

`closed.rs` keeps:
- `MAX_CLOSED`, `worth_reopening`, `members_under` and `restorable`;
- `close_node`, `reopen_node` and `evict_closed`;
- `rebind_jar_files` (session import only; or move it beside `replace_running_session` in `session_sync.rs`, its only caller: recommended, since it is no longer a closed-node concern);
- the tests at 537, 547 and 570, plus Task 3's.

`fetched/tests.rs` `a_reload_keeps_the_stored_path_and_closing_records_the_resolved_one` (183-208): Task 3 dropped its last two lines (`closed_entry`). Here re-add them as `let doc = crate::duplicate::subtree_json(&app, &[id.as_str()], egui::Vec2::ZERO).unwrap(); assert_eq!(doc.nodes[0].fields.get("path").and_then(|v| v.as_str()), Some(moved.as_str()));`, so the resolved-path guarantee of Duplicate stays tested. The test name drops "closing_records" in Task 3 and becomes `a_reload_keeps_the_stored_path_and_duplicating_exports_the_resolved_one` here.

- [ ] **Step 4: Run the tests to pass.** `cargo test -j 8 -p manifold`.

- [ ] **Step 5: Leftover check.** `git grep -nE 'ClosedEntry|reopen_closed\(|subtree_entry|closed_entry' -- crates` gives no hits. `git grep -n rebind_jar_files -- crates` hits only its definition and `replace_running_session`.

- [ ] **Step 6: Format, lint, commit** (`git commit -m "Duplicate moves to duplicate.rs as a pure export and paste; ClosedEntry and reopen_closed go"`, with the attribution line).

---

### Task 5: Edges — one reader of `opened-from`

A closed node leaves the mirror, but its children keep their `parent` edge (decision 9). `App::opened_from(id)` is the one reader: it returns `None` unless the parent is a live card. Link drawing already requires both ends in `App.nodes` (`level.rs:170-177`), and `edges_of` requires the far end to be shown (`links.rs:244-247`), so links need no change.

**Files:**
- Modify `crates/manifold/src/links.rs`: add `opened_from` beside `edges_of`; convert 253.
- Modify `crates/manifold/src/level.rs`: 156.
- Modify `crates/manifold/src/graph_nav.rs`: 264, 344 and 545.
- Modify `crates/manifold/src/session_sync.rs`: `children_of` at 1156-1158.
- Modify `crates/manifold/src/debug_control.rs`: 1576.
- Modify `crates/manifold/src/tabs/sleep.rs`: 123.
- Modify `crates/manifold/src/fetched.rs`: 152.
- Modify `crates/manifold/src/event_loop.rs`: 1787.
- Modify `crates/manifold/src/session_sync/tests.rs`: 1601's test (renamed in Task 3) gains the accessor assertion.

**Interfaces:**
- Consumes: `Mirror::parent`.
- Produces: `App::opened_from(&self, &str) -> Option<&str>`.

- [ ] **Step 1: Write the failing assertion.** In `closing_an_unloaded_tab_keeps_its_childrens_edge`, add:

```rust
    assert_eq!(app.mirror.parent(&leaf), Some(mid.as_str()), "the edge is kept");
    assert_eq!(app.opened_from(&leaf), None, "and reads as none while its parent is closed");
    app.reopen_node(mid.parse().unwrap());
    assert_eq!(app.opened_from(&leaf), Some(mid.as_str()), "and is back after a reopen");
```

(The test needs a journal for the close to hide; switch it to `app_on` if it uses `App::default`.)

- [ ] **Step 2: Run it to fail** (no `opened_from`).

- [ ] **Step 3: Implement and convert.**

```rust
    /// The card `id` was opened from, when that card is live here (spec decision 9). The stored edge survives a
    /// close of its parent and reads as none until the parent is reopened; a deleted parent reads as none forever.
    /// Every reader of the `opened-from` edge goes through this; the writers (`edit_parent`, a spawn's carried
    /// `parent`, the save) read `Mirror::parent` directly.
    pub(crate) fn opened_from(&self, id: &str) -> Option<&str> {
        self.mirror.parent(id).filter(|p| self.nodes.contains_key(*p))
    }
```

Readers to convert (`self.mirror.parent(x)` becomes `self.opened_from(x)`, `app.mirror.parent` becomes `app.opened_from`). Each is a reader that interprets the edge as a live relation:
- `level.rs:156`: its following `self.nodes.get(parent_id)` check stays; it is now redundant but still guards `in_level`.
- `links.rs:253`.
- `graph_nav.rs:264`: the quick-switch walk up the opener chain.
- `graph_nav.rs:344` and `:545`.
- `session_sync.rs:1157` `children_of`, so that a hidden node's children are not its children.
- `debug_control.rs:1576`: EXPORTSTATE `parent`.
- `tabs/sleep.rs:123`, `fetched.rs:152` and `event_loop.rs:1787`. Read each: if it decides something about a live opener, convert it; if it copies the edge to a new node, leave it raw and add a one-line comment saying so.

Left raw (writers and the save):
- `fetched.rs:281`, `:334`, `:557`;
- `tabs.rs:439` (a spawn carries its `parent`), `tabs.rs:830` and `node_store.rs:90` (the delete path's re-parent, which reads the raw edge on purpose);
- `session_sync.rs:417`, `:817`, `:1059`, `:1373`, `:1386`;
- `links.rs:131` and `:166` (`connect` checks the stored edge).

`children_of` now hides children of hidden nodes. That is right for its two users, `edges_of` and the delete path's re-parent. A deleted node is never hidden at delete time: `remove_remotely_deleted` runs only for nodes in `nodes`, and `drop_node` and `drop_tab` run before `unshow`.

- [ ] **Step 4: Run the tests to pass.** `cargo test -j 8 -p manifold`.

- [ ] **Step 5: Leftover check.** `git grep -n "mirror\.parent(" -- crates/manifold/src ':!*tests.rs' ':!crates/manifold/src/mirror.rs'` hits only the raw list above and `opened_from` itself.

- [ ] **Step 6: Format, lint, commit** (`git commit -m "Edges: opened_from is the one reader of the opened-from edge; a closed parent reads as none"`, with the attribution line).

---

### Task 6: Gate stage `closed`, flipped gate expectations, EXPORTSTATE, `REOPENNODE`, docs

**Files:**
- Modify `crates/manifold/src/debug_control.rs`:
  - add `ReopenNode(String)` to `DebugCommand` (beside `ReopenTab` at 122);
  - parse `REOPENNODE id` beside 753 and dispatch beside 1212;
  - add the EXPORTSTATE `closed` list and per-link `shown` at 1614-1623 and 1682;
  - the verb's doc beside 361.
- Modify `crates/gate/src/unknown_type.rs`: the module doc at 1-5; 136-145.
- Modify `crates/gate/src/smoke.rs`: 683-738 and 872-969.
- Create `crates/gate/src/closed.rs`.
- Modify `crates/gate/src/main.rs`: doc line ~6, `mod closed;`, dispatch and usage.
- Modify `crates/gate/src/all.rs`: doc lines 5, 9 and 14; the stage after `unknown-type` at 219.
- Modify `crates/gate/src/tier.rs` if it lists the stages per tier (grep `unknown-type`).
- Docs:
  - `README.md` lines 82, 326, 388, 391 and 428 (re-grep "Recently Closed");
  - `docs/superpowers/specs/2026-10-04-node-model-design.md`;
  - `docs/superpowers/specs/2026-10-04-homogeneity-decisions.md` item 1 (~35);
  - `docs/superpowers/specs/2026-10-14-unknown-type-card-design.md` decision 4 (~32-36, 95-97);
  - the jars-as-cards spec's reopen-copy text (`git grep -n "reopen" docs/superpowers/specs/*jar*`);
  - the closed-field spec's status line.

**Interfaces:**
- Consumes: `App::reopen_node`, `Doc::closed_roots`, and the gate crate's `Instance`, `State`, `check*` and `Section`.
- Produces: the `REOPENNODE id` verb, EXPORTSTATE `"closed": [id, ...]` (newest first) with `"shown": bool` per link, and `gate closed`.

- [ ] **Step 1: The debug verb and EXPORTSTATE.**
  - `REOPENNODE id`: `DebugCommand::ReopenNode(id)`. Dispatch: `if let Ok(node) = id.parse() { self.reopen_node(node) }`. Its doc reads "`REOPENNODE id` -- reopens closed node `id` as the Recently Closed popup does (`reopen_node`), whichever replica closed it."
  - EXPORTSTATE: in the `links` map, add `"shown": [from, to].iter().all(|e| e.is_some_and(|n| self.nodes.contains_key(&n.to_string())))`. Add a key:
    ```rust
            // Recently Closed as the document orders it, newest first (`Doc::closed_roots`).
            "closed": self.journal.as_ref().map_or_else(Vec::new, |j| j.doc().closed_roots().into_iter().map(|(id, _)| id.to_string()).collect()),
    ```
  - A unit test beside `close_node_takes_any_node` (1983):
    ```rust
    #[test]
    fn reopennode_reopens_by_id_and_exportstate_lists_the_history() {
        let path = crate::store::testutil::temp_dir("debug-reopennode").join("session.mnf");
        let mut app = crate::session_sync::tests::app_on(&path, 1);
        let g = crate::session_sync::tests::add_group(&mut app, "G");
        assert!(app.save_session());
        assert!(app.debug_close_node(&g));
        assert_eq!(app.export_state()["closed"], serde_json::json!([g]));
        app.run_debug_command(DebugCommand::ReopenNode(g.clone()));
        assert!(app.nodes.contains_key(&g));
    }
    ```
    Use the names the module actually has for building the state value and running one command (`grep -n "fn export_state\|fn run_debug\|fn handle_debug" crates/manifold/src/debug_control.rs`), and report them.

- [ ] **Step 2: Flip the gate expectations.**
  - `unknown_type.rs`:
    - The module doc's last sentence becomes "then closes it (it stays stored, hidden) and reopens it from Recently Closed with its id, group and link".
    - 136-145 becomes:
      ```rust
          i.send(&format!("CLOSENODE {u}"))?;
          let s5 = i.export_until("s5", |s| s.0["mirror_groups"].get(u.as_str()).is_none())?;
          check("Close hides it", s5.0["mirror_groups"].get(u.as_str()).is_none());
          check_eq("Recently Closed lists it", serde_json::json!([u]), s5.0["closed"].clone());
          let session = i.export_session("session5")?;
          check_eq("a closed node is not exported", 1, stored_named(&session, "Future"));
          i.send("REOPENTAB")?;
          i.send("WAITIDLE")?;
          let s6 = i.export_until("s6", |s| py(&s.0["mirror_groups"][u.as_str()]) == g)?;
          check_eq("Reopen brings the same node back into its group", g.clone(), py(&s6.0["mirror_groups"][u.as_str()]));
          check("its link to the tab is back", linked(&s6, &u, &t));
          let session = i.export_session("session6")?;
          check_eq("both are stored with the real type", 2, stored_named(&session, "Future"));
          Run::stop(i)
      ```
    - The removed check "Reopen brings a shell back beside the copy" is the superseded unknown-type decision 4. The export count is 1 at s5, because Export Session writes shown nodes only (decision 14).
  - `smoke.rs` reopen-nested (683-738):
    - The comment says "REOPENTAB brings the same group back with its whole subtree, still nested".
    - Replace the `match new_group(..)` with the direct id:
      ```rust
          check_query(&reopened, "reopened", "REOPENTAB brings the same group back", &format!("'{empty}' in groups"), |s| s.groups().contains(&empty));
          let outer = empty.clone();
      ```
      The `Some(outer)` arm's body follows, unindented.
  - `smoke.rs` jar section (872-969):
    - The section comment becomes "... close and reopen a group holding all of them: the same group, tab and jars come back, and the tab's edge still names its jar".
    - After REOPENTAB: `check_found(&reopened, "jar_group_reopened", "REOPENTAB brings the jar group back", &group_id)` with `let outer = group_id.clone();`.
    - Replace `new_tab` and the 955-965 query with:
      ```rust
          check_query(
              &inside,
              "jar_group_in",
              "the reopened tab is the same card and its edge names the same jar",
              &format!("cards['{tab_id}']['jar'] == '{jar_id}' and len(jars) == 2"),
              |s| s.card(&tab_id).is_some_and(|c| c["jar"] == jar_id.as_str()) && count_of(s, JAR) == 2,
          );
      ```
    - Delete the now-unused `before` binding if clippy flags it.
  - `jar.rs` (CLOSETAB only) stays as it is. A close still unloads the engine and frees the slot. Run its stage only if the controller's gate tier includes it.

- [ ] **Step 3: The `closed` stage.** `crates/gate/src/closed.rs` follows `unknown_type.rs`'s `Run` pattern (launch, setup by `--import-json`, `drive`, logs on failure, `KEEP`):

```rust
//! `gate closed`: the closed field end to end (spec docs/superpowers/specs/2026-10-14-closed-field-design.md,
//! "Testing"). One fresh profile seeded with a group holding a note, an outside note linked to the inner
//! note, and 21 more notes with text. Instance A closes the group, reopens it (same ids, the link back), then two
//! instances on the one profile close notes in turn: both list the same history, newest first, and when the 21st
//! root is closed both evict the same oldest one.
//!
//! A failing check does not stop the run; a runner error (a timeout, a launch failure) prints an `ERROR:` line, counts
//! as a failure and ends the run early. Instances are killed on every exit path (`Instance`'s Drop plus the job).

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use crate::check::{check, check_eq, exit_code, failures, note_failure, Section};
use crate::instance::{log_tail, seed_settings, Instance};
use crate::link::bad;
use crate::state::{State, GROUP, NOTE};
use crate::windows_main::{manifold_exe, work_dir};

const QUIT_TIMEOUT: Duration = Duration::from_secs(60);
/// `store::model::TYPE_LINK`; the gate crate does not depend on the manifold crate.
const LINK: &str = "application/vnd.manifold.link";
/// `MAX_CLOSED` in `closed.rs`.
const MAX_CLOSED: usize = 20;

// `Run { exe, work, profile }`, `Run::start(&self, log)` and `Run::stop(inst)` are copied from unknown_type.rs
// unchanged. `setup` differs only in its seed:
//     let mut nodes = vec![
//         json!({"id": "g", "type": GROUP, "name": "G", "pos": [0.0, 0.0]}),
//         json!({"id": "in", "type": NOTE, "name": "inside", "text": "inside", "group": "g", "pos": [0.0, 0.0]}),
//         json!({"id": "out", "type": NOTE, "name": "out", "text": "outside", "pos": [400.0, 0.0]}),
//         json!({"id": "l", "type": LINK, "from": "in", "to": "out"}),
//     ];
//     nodes.extend((0..=MAX_CLOSED).map(|n| {
//         json!({"id": format!("n{n}"), "type": NOTE, "name": format!("n{n}"), "text": format!("note {n}"),
//                "pos": [n as f64 * 50.0, 300.0]})
//     }));
//     doc = json!({"nodes": nodes})
// (a cross-level link is fine in an import: `import` does not check scope).

/// The ids in EXPORTSTATE's `closed`, newest first.
fn closed_ids(s: &State) -> Vec<String> {
    s.0["closed"].as_array().map_or_else(Vec::new, |a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
}

/// The id of the one top-level card named `name`.
fn named_card(s: &State, name: &str) -> io::Result<String> {
    let ids: Vec<&str> = s.cards().iter().filter(|c| c["name"] == name).filter_map(|c| c["id"].as_str()).collect();
    match ids.as_slice() {
        [one] => Ok(one.to_string()),
        _ => Err(bad(format!("expected one card named {name}, found {}", ids.len()))),
    }
}

/// The `shown` flag EXPORTSTATE gives the link with id `link`, if the link is exported at all.
fn link_shown(s: &State, link: &str) -> Option<bool> {
    s.0["links"].as_array()?.iter().find(|l| l["id"] == link)?["shown"].as_bool()
}

/// The id of the link joining `a` and `b`, either way round.
fn link_of(s: &State, a: &str, b: &str) -> Option<String> {
    let l = s.0["links"].as_array()?.iter().find(|k| (k["from"] == a && k["to"] == b) || (k["from"] == b && k["to"] == a))?;
    l["id"].as_str().map(str::to_owned)
}

fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));

    next("close-reopen");
    let mut a = run.start("a.log")?;
    let s0 = a.export_until("a0", |s| s.cards().iter().any(|c| c["name"] == "n20"))?;
    let g = s0.groups().into_iter().next().ok_or_else(|| bad("no group".to_string()))?;
    let out = named_card(&s0, "out")?;
    let inside = s0.0["mirror_groups"].as_object().and_then(|m| m.iter().find(|(_, v)| **v == g.as_str()))
        .map(|(k, _)| k.clone()).ok_or_else(|| bad("no member in the group".to_string()))?;
    let link = link_of(&s0, &inside, &out).ok_or_else(|| bad("the seeded link is missing".to_string()))?;
    let ids: Vec<String> = (0..=MAX_CLOSED).map(|n| named_card(&s0, &format!("n{n}"))).collect::<io::Result<_>>()?;
    check_eq("nothing is closed at the start", Vec::<String>::new(), closed_ids(s0));
    a.send(&format!("CLOSENODE {g}"))?;
    let s1 = a.export_until("a1", |s| closed_ids(s) == [g.clone()])?;
    check("the group is gone from the graph", s1.card(&g).is_none());
    check_eq("its link is kept but hidden", Some(false), link_shown(&s1, &link));
    a.send(&format!("REOPENNODE {g}"))?;
    a.send("WAITIDLE")?;
    let s2 = a.export_until("a2", |s| s.card(&g).is_some() && closed_ids(s).is_empty())?;
    check_eq("the same link is shown again", Some(true), link_shown(&s2, &link));
    check_eq("the member is back in the same group", g.clone(), py(&s2.0["mirror_groups"][inside.as_str()]));

    next("two-instances");
    let mut b = run.start("b.log")?;
    for (n, id) in ids[..MAX_CLOSED].iter().enumerate() {
        let who = if n % 2 == 0 { &mut a } else { &mut b };
        who.send(&format!("CLOSENODE {id}"))?;
        who.send("WAITIDLE")?;
    }
    let want: Vec<String> = ids[..MAX_CLOSED].iter().rev().cloned().collect();
    let sa = a.export_until("a3", |s| closed_ids(s) == want)?;
    let sb = b.export_until("b3", |s| closed_ids(s) == want)?;
    check_eq("A lists the closes newest first", want.clone(), closed_ids(sa));
    check_eq("B lists the same", want.clone(), closed_ids(sb));

    next("eviction");
    a.send(&format!("CLOSENODE {}", ids[MAX_CLOSED]))?;
    let oldest = ids[0].clone();
    let sa = a.export_until("a4", |s| closed_ids(s).len() == MAX_CLOSED && closed_ids(s)[0] == ids[MAX_CLOSED])?;
    let sb = b.export_until("b4", |s| closed_ids(s).len() == MAX_CLOSED && closed_ids(s)[0] == ids[MAX_CLOSED])?;
    check_eq("both agree after the 21st close", closed_ids(sa), closed_ids(sb));
    check("the oldest was evicted on both", !closed_ids(sa).contains(&oldest) && !closed_ids(sb).contains(&oldest));
    let session = b.export_session("b-session")?;
    check("the evicted node is deleted, not hidden", session["nodes"].as_array().is_some_and(|n| n.iter().all(|n| n["name"] != "n0")));
    Run::stop(b)?;
    Run::stop(a)
}
```

`py` and the `closed(parent)` entry (the `Run` setup, `KEEP`, the log tail on failure, `exit_code`) are copied from `unknown_type.rs` with `unknown-type` renamed `closed`. The two instances share one profile and one work dir as in `two/mod.rs`, so every export name is unique per instance (`a0`.., `b3`..). Every wait is `export_until` with a predicate, never a fixed sleep. Verify against the built exe that `LINK`, `WAITIDLE` and `CLOSENODE` take the forms used; adjust the check text only, not the structure.

Register it:
- `gate/src/main.rs`: `mod closed;`, a dispatch arm `"closed" => closed::closed(parent)`, the usage line, and the stage list in the doc;
- `all.rs` after line 219: `g.stage("closed", &sibling(&log, "closed.log"), gate("closed"));`;
- `all.rs` doc lines 5, 9 and 14: add `closed` to each list and `.closed.log` to the log list;
- `tier.rs`: add `closed` wherever `unknown-type` is listed.

- [ ] **Step 4: Docs.**
  - `README.md`: re-grep `Recently Closed|reopen|Ctrl\+Shift\+T` and rewrite each hit:
    - Recently Closed lists closed nodes from the document, newest first, any replica's, capped at 20;
    - Ctrl+Shift+T reopens the newest this replica closed;
    - a reopen restores the same node, with its links and group;
    - add `gate closed` to the gate stage list (~428).
  - `2026-10-04-node-model-design.md`: put a note at the top of sections 1 (39-43), 3 (98-99) and 5 (130-167): "Superseded by `2026-10-14-closed-field-design.md` where it differs". Fix the stale claims the closed-field spec names:
    - `JAR_SET` and `dissolve_group`'s re-snapshot are gone (lines ~90, 150-152);
    - `diff` is gone (~19, 180-188);
    - "18 fields to 16" (~200) becomes the current count, 18 numbered fields including `closed`;
    - section 6 items 5 (189) and 8 (200).

    Re-read each before editing; line numbers are approximate.
  - Homogeneity list, item 1: "done 2026-10-14 (closed field)".
  - Unknown-type spec, decision 4 (~32-36, 95-97): "Superseded for Close and Reopen by the closed-field design: a closed unknown node keeps its raw fields and reopens whole; only Duplicate and import make shells."
  - Jars-as-cards spec: the reopen-copies-the-jar-file text becomes "superseded: a reopened jar is the same node and keeps its file; `rebind_jar_files` serves session import only".
  - Closed-field spec status line: append "Implemented 2026-10-14 (plan `2026-10-14-closed-field.md`); deviations D1-D12 there."

- [ ] **Step 5: Build, test, check.**
  ```bash
  cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate
  cargo test -j 8 -p manifold -p gate
  ```
  Then run the new stage alone, detached at BelowNormal, and read its log:
  ```powershell
  $p = Start-Process -FilePath target\debug\gate.exe -ArgumentList 'closed' -RedirectStandardOutput gate-closed.out -WindowStyle Hidden -PassThru
  $p.PriorityClass = 'BelowNormal'
  ```
  Wait on `$p.HasExited` with a deadline (a Monitor loop, no fixed sleep), then read `gate-closed.out`. All checks pass.

- [ ] **Step 6: Leftover check.** Run the full set from "Leftover-reference check" above; both commands give no hits beyond the stated exception.

- [ ] **Step 7: Format, lint, commit** (`git commit -m "Gate closed stage; flipped smoke and unknown-type expectations; REOPENNODE; EXPORTSTATE closed; docs"`, with the attribution line).

---

### Closing step (controller)

- [ ] **Leftover-free and green:**
  - run the "Leftover-reference check" set;
  - `cargo fmt --all --check`;
  - the clippy command;
  - `cargo test -j 8 -p manifold -p gate`.
- [ ] **No stray processes:** `Get-Process python, manifold*, gate -ErrorAction SilentlyContinue` lists none from this branch.
- [ ] **Full gate** (CLAUDE.md "Gate"; this branch changes sync, the store, the mirror and session restore, so `full`):
  ```powershell
  cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate
  $p = Start-Process -FilePath target\debug\gate.exe -ArgumentList 'all','gate-closed-field.log','full' -WindowStyle Hidden -PassThru
  $p.PriorityClass = 'BelowNormal'; $p.Id
  ```
  - Wait on the process with a Monitor until-loop that has a deadline (expect 30-50 min).
  - The log's first and last lines record CPU load and Defender (`cpu=`, `defender_rt=`). Report both with the verdict lines, the `GATE ... exit=` line of every stage, and the slowest-steps table.
  - A failing stage gets its own log read (`gate-closed-field.log.<stage>.log`) before any rerun.
- [ ] **Final review:** one Opus whole-branch review against the spec's 14 decisions and this plan's D1-D12. Fix rounds go to Sonnet.

---

## Self-review log

One adversarial pass over the whole plan, for build-green ordering and for spec coverage. It caught these findings, each fixed in the text above:

1. **Dead code across task boundaries.** `closed_roots` (needs a consumer) moved from Task 1 to Task 3. `shown_level` moved to Task 2, where `leave_hidden` and `apply_local` use it. `members_under` turns `pub(crate)` in Task 3, where `close_node` first uses it from outside its old caller. `REOPENNODE` stays in Task 6 with its test, which uses it at once.
2. **`ClosedEntry` cannot go in Task 3.** Duplicate still imports through `reopen_closed(ClosedEntry)` until Task 4. Task 3's leftover grep therefore excludes `ClosedEntry` and `reopen_closed`; Task 4's includes them.
3. **Files pruned at launch between tasks.** `backing_keep` read `nodes` plus `self.closed`. After Task 2's hide path (or Task 3's close), a hidden node's files would have been pruned at the next launch. The change moved to Task 2 (D11).
4. **The Default hidden by a group close.** Deleting the eject branch in Task 5, as the brief had it, would leave Task 3 able to hide the Default. The invariant moved into Task 3 (D11), with a test for the hidden case.
5. **Closing several nodes with one action.** `close_node` saving once per node matches `destroy_tab`'s per-tab save today, so `closing_a_group_commits_one_frame` (5738) still holds. A bulk close commits one frame per root, as before.
6. **The re-stamp order.** `pending` is a BTreeMap keyed by `(node, field)`, which would commit nested re-stamps in id order, not their old order. Hence the ordered `closed_flags` queue.
7. **Reopen materialization.** Nothing showed a reopened node: the mirror early-return in `edit_field` and the hide path's `forget_node` meant no local path re-creates it. Hence the post-commit `apply_visibility` pass, which reuses `apply_remote`. It is the same mechanism that shows a repaired hidden Default. This is one general mechanism, not a reopen special case.
8. **`leave_hidden` ordering on a remote close.** Walking to `shown_group(root)` could land inside the subtree being hidden when descendants are processed before their root. The walk now reads the document (`shown_level` over the raw chain, from `root`'s group).
9. **Teardown.** The brief's "walks non-deleted doc nodes" is already true: `live()` includes closed nodes (D12). Only `hidden_local.clear()` was missing.
10. **Spec coverage.** Each decision maps to a task:
    - 1 and 2 to Task 1;
    - 3 and 8 to Task 2;
    - 4, 5, 6, 7, 10 and 12 to Task 3 (12 by not touching Dissolve and merge);
    - 9 to Tasks 2, 3 and 5;
    - 11 to Task 2;
    - 13 to Task 4;
    - 14 to Tasks 1 (export) and 2 (teardown, D12).

    The spec's "Testing" list maps to tests in Tasks 1-4 and to the stage in Task 6:
    - "two replicas evicting the same root agree" is `a_replica_that_only_receives_closes_evicts_and_agrees` plus the gate's eviction section;
    - "reopen versus eviction loses" is `a_reopen_concurrent_with_its_eviction_loses`.

    Parked items (Dissolve and merge undo, shared-history UI, redo, scroll across close) stay parked, and nothing here touches them.

11. **Test helper names.** After writing, `connect`, `link_ids`, `has`, `snap_layout`, `note_text`, `debug_close_node`, `live_in_doc` and `failed_tab` were checked against the code. All exist. `connect` returns a `Result`, so the test calls gained `.unwrap()`; otherwise `unused_must_use` fails clippy.

A second pass over the fixed text found nothing material.
