# Session Store App Integration (Plan B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The app keeps its session in the op-log store (`session.mnf` plus `session.mnf.local`) instead of `session.json`. Two instances on one profile merge live, and every graph snapshot (session export, group export, Recently Closed, named sessions) uses the store's one node model.

**Architecture:**
- A new `session_sync.rs` holds a pure core: each live node's shared fields, and `diff(doc, live) -> Vec<OpBody>`. The `impl App` half does three things:
  - projects `App::nodes` into that shape;
  - materializes store nodes into live `GraphNode`s, for restore, other replicas' ops, imports and reopen;
  - runs the save loop: `begin`, apply `remote`, diff, `commit`.
- The ~31 `save_session()` call sites stay as they are; only the function body changes.
- Personal state (settled positions, view, titles, Recently Closed) is a serde JSON `LocalState` in the sidecar, written on a timer.

**Tech Stack:** Rust 2021, winit + egui, serde/serde_json (already dependencies), the in-crate `store` module. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-25-session-op-log-design.md`. Also binding: "Contracts Plan B must honour" in `docs/superpowers/plans/2026-09-25-session-store-core.md` (end of its Outcome section).

## Global Constraints

- Work on `dev` (or a branch from `dev` that merges back into it), never `main`.
- Run `cargo fmt --all` before each commit. `cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings` must pass. Never use `--no-verify`. Never silence a lint without a comment saying why.
- Build and test with `-j 12`: `cargo test -q -j 12 -p manifold`.
- Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never touch the real `%APPDATA%\manifold`. Headless runs go only through `scripts/drive.sh`, `scripts/headless-smoke-test.sh` or the new `scripts/two-instance-test.sh`, which all isolate `APPDATA`. Rebuild the exe (`cargo build -j 12 -p manifold`) before any headless run, since the scripts run the existing exe.
- Run Python only as `python -P script.py < /dev/null`, with the script in a file. Never `python -` or `python -P -`: it hangs.
- **Clean break:** the file is `session.mnf`. An existing `session.json` is ignored and left in place; nothing reads or deletes it. No legacy reader.
- **Contracts** (store-core plan, verbatim in substance):
  - A fresh `ActorId::random()` per `Journal::open`.
  - Diff and commit only inside a `Txn`, against `txn.doc()`. After any `Err`, `begin` again and re-diff, never retry the same bodies.
  - Keep a `Txn` short. Check `read_only` after `begin`.
  - Call `sync_if_due()` every frame, and `close()` on a clean exit.
  - Call `read_local()` once after `open`, and treat `Err`/`None` as a fresh start. Call `write_local()` only when the state changed: at most every 30 s, 2.5 s after a Recently Closed change, and at exit before `close()`. Settled positions count as changed only past 1 px.
  - Missing means deleted: never name a non-live node as a new op's `group` or `parent`, and apply `remote` to the live graph before diffing.
  - Call `take_recovery()` after `open` and after each `begin`. Show the user what was lost and where the original was saved. Don't repeat an identical notice.
  - Pass only `field::*` constants to `Node::get`. Never emit a non-finite `pos`.
  - Never pass the journal's own path to `save_copy`.
- Node ids in the app stay `String` keys in `NodeMap`. A new node's key is the `Display` form of a store `NodeId` (`"1f3a9c00000000ab-17"`). Nothing in the app relies on the old `tab-N`/`grp-N` prefixes (checked: only tests and comments mention them).

## Rulings made while writing this plan

The user decided the first two on 2026-09-26. The rest are the planner's, each recorded as what, why, and the cost if wrong.

1. **Open (named session) replaces the running session's contents.** It does not switch the live file: `session.mnf` stays the one live file, and Open deletes every live node and recreates the named file's nodes under fresh ids. Why: one live file means nothing has to remember which file to reopen, and a second instance just sees deletes and creates. Task 1 updates the spec line "Open switches the live file".
2. **Import Session keeps the document id.** It is delete-all plus create-imported inside `session.mnf`. Why: a doc id matters only once sync exists. Task 1 updates the spec line "Save As and Import Session create a new document id" to cover Save As only.
3. **The shared `url` is set on every url change the engine reports** (`NavigationStateChanged`), not only on user-started navigations. Redirects write it too.
   - Why: the host can't tell a clicked link from a redirect; the engines don't report the cause.
   - Cost: one machine never disagrees about redirects, so nothing bounces in sub-project 1. *(Wrong for a site that redirects to a fresh url on every load: two instances bounced it about 40 times a second. Since fixed: a url change following a load the store started isn't shared. See Outcome, "After the merge".)* Sub-project 2 must get the navigation cause from the engines before sync, or replicas with different logins will bounce tabs.
   - Follow-on: the personal "current url" duplicates the shared url, so Plan B does not store it.
4. **Personal state is serde JSON** inside the sidecar's opaque `state` bytes. Why: `JsonDoc.local` is already JSON, and EXPORTSESSION embeds it. Cost: about 1.5× the binary estimate (≈50 KB at 300 tabs), which is fine.
5. **One camera rect, for the current level.** The app has a single `GraphCamera`, and the spec's "camera rect per level" assumed more. Plan B saves `camera.rect` and restores it when the level matches; otherwise it fits as today. Cost: none until per-level cameras exist.
6. **A remote `pos` jumps rather than eases.** There is no easing mechanism for nodes today. Cost: a visible jump when another local instance drags a node.
7. **Recently Closed entries are `JsonDoc`s:** root first, then the subtree, with `local` carrying tab zoom. Reopening goes through `json::import`, the same path as group import. They persist in `LocalState.closed`, oldest first, as `App::closed` already orders them. Task 1 changes the spec's "newest first".
8. **In-flight tab spawns count as present.** An engine swap and crash recovery remove a tab's node and respawn it under the same id. A save in between would emit `Delete` and then recreate a tombstoned id. So `App::spawning: HashSet<String>` holds every id with a spawn in flight, and the diff never deletes those. This also replaces the `restore_spawns_in_flight > 0` guard.
9. **`EXPORTSESSION` saves first, then writes `doc_to_json(journal.doc())` with `local` = the current `LocalState`.** Probes that read positions must read `local.nodes[id].pos`, because shared `pos` is only the last explicit placement.
10. **drive.sh `SEED`:** `$SEED/session.json` in the new snapshot shape is converted with `manifold --import-json … --out …/session.mnf`. Other `*.json` files are copied as today.
11. **`history.json` saves on change, at save points and at exit.** Browsing history is app-wide, and it is written like `bookmarks.json`, but only when it changed since the last write.

---

## File Structure

- Create `crates/manifold/src/session_sync.rs`:
  - the pure core (`Fields`, `Shared`, `kind_fields`, `fields_of`, conversions, `diff`), with its unit tests;
  - `impl App`: `shared_nodes`, `materialize`, `apply_remote`, `save_session`, `restore_session`, `replace_running_session`, `poll_store`, `close_store`.
- Create `crates/manifold/src/local_state.rs`: the `LocalState` and `LocalNode` serde types, `differs`, `remap`, and their tests. The `impl App` side is `local_state()`, `apply_local()` and `maybe_write_local()`.
- Create `crates/manifold/src/cli.rs`: `--dump`, `--check` and `--import-json`.
- Create `crates/manifold/src/history.rs`: `history.json`, mirroring `bookmarks.rs`.
- Create `scripts/two-instance-test.sh`: headless tests with two instances, a compaction during the other's run, and a kill.
- Modify `crates/manifold/src/store/journal.rs` (`Txn::read_only`) and `crates/manifold/src/store/json.rs` (`node_json`, import order doc).
- Modify `crates/manifold/src/main.rs`:
  - App fields: `journal`, `spawning`, `explicit`, `shared_pos` and the local-state timers; drop `ids`, `session_fingerprint` and `restore_spawns_in_flight`.
  - Id allocation; `closed_entry`/`reopen_closed`; group export and import; the old `build_session_snapshot`, `apply_session` and `session_place` go away.
- Modify `crates/manifold/src/event_loop.rs`: debug commands, per-frame `poll_store`, `exiting`, and url marks in `NavigationStateChanged`.
- Modify `card.rs`, `graph_ui.rs`, `popups.rs` and `settings_page.rs` at the drag-end and placement sites, and the named-session UI.
- Modify `crates/manifold/src/session.rs`: only paths remain (`session_dir`, `store_path`, the named `.mnf` helpers).
- Delete `crates/manifold/src/group_export.rs`.
- Modify `scripts/drive.sh` and `scripts/headless-smoke-test.sh` (fixtures), plus README and docs.

---

### Task 1: Record this plan's rulings in the spec and contracts

**Files:**
- Modify: `docs/superpowers/specs/2026-09-25-session-op-log-design.md`
- Modify: `docs/superpowers/plans/2026-09-25-session-store-core.md` (the "Contracts Plan B must honour" and "Deferred" lists)

**Interfaces:** Consumes nothing. Produces the spec wording later tasks cite.

- [ ] **Step 1: Edit the spec.**
  - "Named sessions" bullet (Journal section): replace "Save As writes a compacted copy (no `.prev`) and the copy's personal-state file; Open switches the live file." with: "Save As writes a compacted copy (no `.prev`) under a new document id, with the copy's personal-state file. Open replaces the running session's contents: it deletes every live node and recreates the file's nodes under fresh ids, remapping its personal state; `session.mnf` stays the one live file."
  - "Document id" paragraph: replace "Save As and Import Session create a new document id; only sync shares one." with "Save As creates a new document id; Import Session and Open replace the live document's nodes and keep its id; only sync shares one."
  - Shared-document table, `url` row: append "Plan B sets it on every url change the engine reports, redirects included, because no engine reports a navigation's cause; sub-project 2 must narrow it before sync."
  - "Personal state" list: change "a tab's `muted`, `zoom`, `title` and current url (the page it actually shows, which restore reopens)" to "a tab's `muted`, `zoom` and `title` (the current url equals the shared `url` while Plan B sets it on every change)". Change "Recently Closed: up to 20 entries, newest first" to "Recently Closed: up to 20 entries, oldest first". Change "camera rect per level (key: group id, or none for the top level)" to "the camera rect, for the level it was saved at (the app has one camera)". Add a line: "Encoded as JSON (`local_state::LocalState`) inside the file's state bytes."
  - "Positions": append "Plan B sets a remote `pos` directly; easing is left for later."
- [ ] **Step 2: Edit the contracts.** In "Contracts Plan B must honour", add after the Transactions bullet: "- **In-flight spawns.** A tab whose spawn is in flight (restore, swap, crash recovery) has no live node; the diff must treat its id as present (`App::spawning`)." In "Deferred, with reasons", under "Test gaps", remove "A real multi-process appender test." (landed in 8a8e0e3).
- [ ] **Step 3: Commit.**
```bash
git add docs/superpowers/specs/2026-09-25-session-op-log-design.md docs/superpowers/plans/2026-09-25-session-store-core.md
git commit -m "Spec: Plan B rulings (Open/Import keep one live file, url on every change, JSON personal state)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Store touch-ups the app needs

**Files:**
- Modify: `crates/manifold/src/store/journal.rs` (`impl Txn<'_>`, near `pub fn doc`)
- Modify: `crates/manifold/src/store/json.rs` (`node_to_json`, `import` doc)

**Interfaces:**
- Produces: `Txn::read_only(&self) -> bool`.
- Produces: `json::node_json(id: &str, kind: Kind, fields: &BTreeMap<u32, FieldValue>, group: Option<NodeId>) -> JsonNode`.
- Documents that `json::import` returns nodes in `json.nodes` order.

- [ ] **Step 1: Write the failing tests** (append to `json.rs`'s `mod tests`):
```rust
#[test]
fn node_json_matches_a_document_nodes_json() {
    let mut fields = BTreeMap::new();
    fields.insert(field::URL, FieldValue::Str("https://a.example/".into()));
    fields.insert(field::PINNED, FieldValue::Bool(true));
    fields.insert(field::COLOR, FieldValue::Color(None));
    let json = node_json("n1", Kind::Tab, &fields, None);
    assert_eq!(json.kind, "tab");
    assert_eq!(json.fields.get("url"), Some(&Value::String("https://a.example/".into())));
    assert_eq!(json.fields.get("pinned"), Some(&Value::Bool(true)));
    assert!(!json.fields.contains_key("color"), "defaults are left out");
    assert!(!json.fields.contains_key("group"));
}

#[test]
fn import_keeps_file_order() {
    let json: JsonDoc = serde_json::from_str(r#"{"nodes":[{"id":"b","kind":"note"},{"id":"a","kind":"note"}]}"#).unwrap();
    let mut next = ids(7);
    let first = next();
    let imported = import(&json, ids(7)).unwrap();
    assert_eq!(imported[0].id, first, "the first node in the file gets the first fresh id");
}
```
- [ ] **Step 2: Run them to see them fail.** `cargo test -q -j 12 -p manifold -- store::json` → FAIL: `node_json` not found.
- [ ] **Step 3: Implement.** In `json.rs`, add `node_json` and make `node_to_json` delegate to it:
```rust
/// A node as JSON from its field values, `group` given separately: what
/// `doc_to_json` writes for a document node, for callers holding values
/// rather than a `Node` (Recently Closed, `session_sync`).
pub fn node_json(id: &str, kind: Kind, fields: &BTreeMap<u32, FieldValue>, group: Option<NodeId>) -> JsonNode {
    let mut out = Map::new();
    let mut put = |no: u32, value: &FieldValue| {
        if let (Some(def), Some(json)) = (field_def(no), value_to_json(value)) {
            out.insert(def.name.to_owned(), json);
        }
    };
    for (&no, value) in fields {
        if no != field::GROUP {
            put(no, value);
        }
    }
    put(field::GROUP, &FieldValue::Id(group));
    JsonNode { id: id.to_owned(), kind: kind.name().expect("a known kind").to_owned(), fields: out }
}

/// `node` as JSON, its group given separately (the effective one).
fn node_to_json(id: NodeId, node: &Node, group: Option<NodeId>) -> JsonNode {
    let fields = node.fields.keys().filter(|&&no| field_def(no).is_some()).map(|&no| (no, node.get(no))).collect();
    node_json(&id.to_string(), node.kind, &fields, group)
}
```
  Extend `import`'s doc comment with: "The result is in `json.nodes` order, so `imported[i]` is `json.nodes[i]` (callers map labels to fresh ids by index)." In `journal.rs` `impl Txn<'_>`, add:
```rust
    /// A newer build compacted the file: commits are refused. Check it
    /// after `begin` (contract "Read-only").
    pub fn read_only(&self) -> bool {
        self.journal.read_only
    }
```
- [ ] **Step 4: Run the tests.** `cargo test -q -j 12 -p manifold -- store::` → all pass, including the existing json tests.
- [ ] **Step 5: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src/store/json.rs crates/manifold/src/store/journal.rs
git commit -m "store: node_json for callers holding field values; Txn::read_only" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `session_sync` pure core: shared fields and the diff

**Files:**
- Create: `crates/manifold/src/session_sync.rs`
- Modify: `crates/manifold/src/main.rs` (add `mod session_sync;` in the `mod` list)

**Interfaces:**
- Consumes: `store::model::{encode_field, field, field_def, Doc, FieldValue, Kind, Node, NodeId}`, `store::ops::OpBody`, `process_manager::EngineKind`.
- Produces (all `pub(crate)`):
  - `type Fields = BTreeMap<u32, FieldValue>`
  - `fn kind_fields(kind: Kind) -> &'static [u32]`
  - `struct Shared { kind: Kind, group: Option<NodeId>, fields: Fields }`
  - `fn fields_of(node: &Node) -> Fields`
  - `fn color_value(Option<egui::Color32>) -> FieldValue`, `fn color_of(&FieldValue) -> Option<egui::Color32>`
  - `fn engine_code(EngineKind) -> u64`, `fn engine_of(u64) -> EngineKind`
  - `fn diff(doc: &Doc, live: &BTreeMap<NodeId, Shared>, explicit: &BTreeSet<(NodeId, u32)>, keep: &BTreeSet<NodeId>) -> Vec<OpBody>`

- [ ] **Step 1: Write the module with its tests.** The whole file:
```rust
//! The live graph and the session store (spec "Local edits become ops").
//! This half is pure: each live node's shared fields (`Shared`), and the
//! diff that turns local edits into ops. The `impl App` half (restore,
//! saving, other replicas' ops) follows it.

use std::collections::{BTreeMap, BTreeSet};

use crate::process_manager::EngineKind;
use crate::store::model::{encode_field, field, field_def, Doc, FieldValue, Kind, Node, NodeId};
use crate::store::ops::OpBody;

/// A node's shared fields by number: every field of its kind but `group`.
pub(crate) type Fields = BTreeMap<u32, FieldValue>;

/// The fields each kind carries, `group` aside (spec, "The shared document").
pub(crate) fn kind_fields(kind: Kind) -> &'static [u32] {
    use field::*;
    match kind {
        Kind::Tab => &[POS, COLOR, PARENT, URL, ENGINE, PINNED],
        Kind::Group => &[POS, COLOR, NAME, FROM_JSON],
        Kind::Note => &[POS, COLOR, TEXT],
        Kind::Image | Kind::Pdf | Kind::Json | Kind::Download => &[POS, COLOR, PATH],
        Kind::JsonLeaf => &[POS, COLOR, PATH, KEY, VALUE],
        Kind::Unknown(_) => &[],
    }
}

/// One live node as the store sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shared {
    pub kind: Kind,
    pub group: Option<NodeId>,
    /// Exactly `kind_fields(kind)`, absent values included, so a field
    /// cleared live reads as a change.
    pub fields: Fields,
}

/// A document node's fields for its kind, shaped like `Shared::fields`.
pub(crate) fn fields_of(node: &Node) -> Fields {
    kind_fields(node.kind).iter().map(|&no| (no, node.get(no))).collect()
}

pub(crate) fn color_value(color: Option<egui::Color32>) -> FieldValue {
    FieldValue::Color(color.map(|c| u32::from_be_bytes([0, c.r(), c.g(), c.b()])))
}

pub(crate) fn color_of(value: &FieldValue) -> Option<egui::Color32> {
    match value {
        FieldValue::Color(Some(rgb)) => {
            let [_, r, g, b] = rgb.to_be_bytes();
            Some(egui::Color32::from_rgb(r, g, b))
        }
        _ => None,
    }
}

/// `store::model::ENGINES` order: code `n` is `ENGINES[n - 1]`.
pub(crate) fn engine_code(kind: EngineKind) -> u64 {
    match kind {
        EngineKind::Servo => 1,
        EngineKind::Cef => 2,
        EngineKind::Webkit => 3,
        EngineKind::Webview2 => 4,
    }
}

/// Unknown codes fall back to Servo, as `engine_kind_serde` does for names.
pub(crate) fn engine_of(code: u64) -> EngineKind {
    match code {
        2 => EngineKind::Cef,
        3 => EngineKind::Webkit,
        4 => EngineKind::Webview2,
        _ => EngineKind::Servo,
    }
}

/// The ops that make `doc` show `live` (spec "Local edits become ops").
/// - A live node the document lacks is created, each group before the
///   nodes inside it. A live id the document holds as a tombstone is
///   never recreated (ids are never reused).
/// - A node shown in another group moves. Its group is compared with
///   `effective_group`, so re-homing out of a deleted group is not a move.
/// - A diffed field that differs is `Set`. An explicit field (`pos`,
///   `url`) is `Set` only when `explicit` names it.
/// - A document node missing from `live` is deleted, unless `keep` names
///   it (a tab whose spawn is in flight).
/// A group or parent naming a node that is neither live in the document
/// nor created here is written as none (contract "Missing means deleted").
pub(crate) fn diff(
    doc: &Doc,
    live: &BTreeMap<NodeId, Shared>,
    explicit: &BTreeSet<(NodeId, u32)>,
    keep: &BTreeSet<NodeId>,
) -> Vec<OpBody> {
    let in_doc = |id: &NodeId| doc.nodes.get(id).is_some_and(Node::is_live);
    let new: BTreeSet<NodeId> = live.keys().filter(|id| !doc.nodes.contains_key(id)).copied().collect();
    let valid = |id: Option<NodeId>| id.filter(|id| in_doc(id) || new.contains(id));
    let mut ops = Vec::new();

    // Creates, each group's chain of new ancestors first.
    let mut order = Vec::new();
    let mut queued = BTreeSet::new();
    for &start in &new {
        let mut chain = Vec::new();
        let mut at = Some(start);
        while let Some(id) = at.filter(|id| new.contains(id) && !queued.contains(id) && !chain.contains(id)) {
            chain.push(id);
            at = live[&id].group;
        }
        for id in chain.into_iter().rev() {
            queued.insert(id);
            order.push(id);
        }
    }
    let mut created = BTreeSet::new();
    for id in order {
        let node = &live[&id];
        // A group created later in this batch (only a cycle does that)
        // would make a placeholder: create at the top level instead.
        let group = valid(node.group).filter(|g| in_doc(g) || created.contains(g));
        let mut all = node.fields.clone();
        all.insert(field::GROUP, FieldValue::Id(group));
        if let Some(FieldValue::Id(parent)) = all.get_mut(&field::PARENT) {
            *parent = valid(*parent);
        }
        let fields = all.iter().flat_map(|(&no, value)| encode_field(no, value)).collect();
        ops.push(OpBody::Create { node: id, kind: node.kind, fields });
        created.insert(id);
    }

    // Existing nodes: moves and sets.
    for (&id, node) in live {
        let Some(current) = doc.nodes.get(&id).filter(|n| n.is_live()) else { continue };
        let group = valid(node.group);
        if group != doc.effective_group(id) {
            ops.push(OpBody::Move { node: id, group });
        }
        for (&no, value) in &node.fields {
            if field_def(no).is_some_and(|def| def.explicit) && !explicit.contains(&(id, no)) {
                continue;
            }
            let value = match value {
                FieldValue::Id(parent) => FieldValue::Id(valid(*parent)),
                other => other.clone(),
            };
            let enc = encode_field(no, &value);
            if current.fields.get(&no).map_or(&[][..], |reg| &reg.enc[..]) != &enc[..] {
                ops.push(OpBody::Set { node: id, field: no, enc });
            }
        }
    }

    for (id, _) in doc.live() {
        if !live.contains_key(&id) && !keep.contains(&id) {
            ops.push(OpBody::Delete { node: id });
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{ActorId, DocId, Hlc};
    use crate::store::ops::Op;
    use crate::store::testutil::XorShift;

    const A: ActorId = ActorId(0xA);

    fn id(counter: u64) -> NodeId {
        NodeId { actor: A, counter }
    }

    /// Applies `bodies` to `doc` as actor `A`'s next ops.
    fn apply(doc: &mut Doc, bodies: Vec<OpBody>) {
        for body in bodies {
            let counter = doc.version.get(&A).copied().unwrap_or(0).max(1000);
            let hlc = Hlc { ms: doc.clock.ms + 1, logical: 0 };
            doc.apply(&Op { id: id(counter), hlc, body });
        }
    }

    fn shared(kind: Kind, group: Option<NodeId>, set: &[(u32, FieldValue)]) -> Shared {
        let mut fields: Fields =
            kind_fields(kind).iter().map(|&no| (no, FieldValue::absent(field_def(no).unwrap().ty))).collect();
        for (no, value) in set {
            fields.insert(*no, value.clone());
        }
        Shared { kind, group, fields }
    }

    fn note(text: &str, group: Option<NodeId>) -> Shared {
        shared(Kind::Note, group, &[(field::TEXT, FieldValue::Str(text.into()))])
    }

    fn group(name: &str) -> Shared {
        shared(Kind::Group, None, &[(field::NAME, FieldValue::Str(name.into()))])
    }

    /// What `doc` shows, shaped like `live`.
    fn shown(doc: &Doc) -> BTreeMap<NodeId, Shared> {
        doc.live()
            .map(|(id, node)| (id, Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) }))
            .collect()
    }

    fn all_explicit(live: &BTreeMap<NodeId, Shared>) -> BTreeSet<(NodeId, u32)> {
        live.keys().flat_map(|&id| [(id, field::POS), (id, field::URL)]).collect()
    }

    #[test]
    fn new_nodes_are_created_groups_first() {
        let doc = Doc::new(DocId([1; 16]));
        // The member's id sorts before its group's.
        let live = BTreeMap::from([(id(0), note("inside", Some(id(1)))), (id(1), group("G"))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        assert!(matches!(ops[0], OpBody::Create { node, .. } if node == id(1)));
        let mut doc = doc;
        apply(&mut doc, ops);
        assert_eq!(shown(&doc), live);
    }

    #[test]
    fn an_unchanged_graph_emits_nothing() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        apply(&mut doc, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
        assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
    }

    #[test]
    fn explicit_fields_change_only_when_marked() {
        let mut doc = Doc::new(DocId([1; 16]));
        let mut live = BTreeMap::from([(id(0), note("a", None))]);
        apply(&mut doc, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
        live.get_mut(&id(0)).unwrap().fields.insert(field::POS, FieldValue::Pos(5.0, 6.0));
        assert_eq!(diff(&doc, &live, &BTreeSet::new(), &BTreeSet::new()), vec![], "physics motion is not an edit");
        let marked = BTreeSet::from([(id(0), field::POS)]);
        assert!(matches!(&diff(&doc, &live, &marked, &BTreeSet::new())[..], [OpBody::Set { field: field::POS, .. }]));
    }

    #[test]
    fn a_missing_node_is_deleted_unless_kept() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        apply(&mut doc, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
        let empty = BTreeMap::new();
        assert_eq!(diff(&doc, &empty, &BTreeSet::new(), &BTreeSet::from([id(0)])), vec![]);
        assert_eq!(diff(&doc, &empty, &BTreeSet::new(), &BTreeSet::new()), vec![OpBody::Delete { node: id(0) }]);
    }

    #[test]
    fn a_tombstone_is_never_recreated() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        apply(&mut doc, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
        apply(&mut doc, vec![OpBody::Delete { node: id(0) }]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        assert!(!ops.iter().any(|op| matches!(op, OpBody::Create { .. })), "{ops:?}");
    }

    #[test]
    fn re_homing_out_of_a_deleted_group_is_not_a_move() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", Some(id(1)))), (id(1), group("G"))]);
        apply(&mut doc, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
        // Another replica deleted the group only; the note is shown at the top.
        apply(&mut doc, vec![OpBody::Delete { node: id(1) }]);
        let rehomed = BTreeMap::from([(id(0), note("a", None))]);
        assert_eq!(diff(&doc, &rehomed, &BTreeSet::new(), &BTreeSet::new()), vec![]);
    }

    #[test]
    fn a_parent_naming_a_missing_node_is_written_as_none() {
        let doc = Doc::new(DocId([1; 16]));
        let tab = shared(Kind::Tab, None, &[(field::PARENT, FieldValue::Id(Some(id(9))))]);
        let live = BTreeMap::from([(id(0), tab)]);
        let mut doc2 = doc.clone();
        apply(&mut doc2, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
        assert_eq!(doc2.nodes[&id(0)].get(field::PARENT), FieldValue::Id(None));
    }

    #[test]
    fn random_edits_converge_in_one_diff() {
        let mut rng = XorShift(0x5E55);
        let mut doc = Doc::new(DocId([1; 16]));
        let mut live: BTreeMap<NodeId, Shared> = BTreeMap::new();
        let mut next = 0;
        for _ in 0..200 {
            match rng.below(4) {
                0 | 1 => {
                    let groups: Vec<NodeId> =
                        live.iter().filter(|(_, s)| s.kind == Kind::Group).map(|(id, _)| *id).collect();
                    let group = (!groups.is_empty() && rng.below(2) == 0)
                        .then(|| groups[rng.below(groups.len() as u64) as usize]);
                    let node = if rng.below(3) == 0 { group_in("g", group) } else { note("n", group) };
                    live.insert(id(next), node);
                    next += 1;
                }
                2 if !live.is_empty() => {
                    let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                    live.get_mut(&key).unwrap().fields.insert(field::COLOR, FieldValue::Color(Some(rng.below(1 << 24) as u32)));
                }
                3 if !live.is_empty() => {
                    let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                    // Deleting a group deletes what it shows, as the app does.
                    let doomed: Vec<NodeId> = live.keys().filter(|&&n| n == key || is_inside(&live, n, key)).copied().collect();
                    for n in doomed {
                        live.remove(&n);
                    }
                }
                _ => {}
            }
            apply(&mut doc, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
            assert_eq!(shown(&doc), live);
            assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
        }
    }

    fn group_in(name: &str, parent: Option<NodeId>) -> Shared {
        Shared { group: parent, ..group(name) }
    }

    fn is_inside(live: &BTreeMap<NodeId, Shared>, node: NodeId, ancestor: NodeId) -> bool {
        let mut at = live.get(&node).and_then(|s| s.group);
        while let Some(g) = at {
            if g == ancestor {
                return true;
            }
            at = live.get(&g).and_then(|s| s.group);
        }
        false
    }

    #[test]
    fn engine_codes_follow_the_store_table() {
        for kind in [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit, EngineKind::Webview2] {
            let name = crate::store::model::ENGINES[engine_code(kind) as usize - 1];
            assert_eq!(serde_json::to_value(Wrap(kind)).unwrap(), serde_json::json!(name));
            assert_eq!(engine_of(engine_code(kind)), kind);
        }
    }

    #[derive(serde::Serialize)]
    struct Wrap(#[serde(with = "crate::process_manager::engine_kind_serde")] EngineKind);

    #[test]
    fn colors_round_trip() {
        let c = egui::Color32::from_rgb(1, 2, 3);
        assert_eq!(color_of(&color_value(Some(c))), Some(c));
        assert_eq!(color_value(Some(egui::Color32::BLACK)), FieldValue::Color(Some(0)), "black is encoded");
        assert_eq!(color_of(&color_value(None)), None);
    }
}
```
  If `EngineKind` lacks `PartialEq`/`Debug` for the asserts, compare `engine_code(engine_of(engine_code(kind))) == engine_code(kind)` instead. If `engine_kind_serde` has no `serialize` usable through `#[serde(with)]` on a tuple struct, drop `Wrap` and compare against the strings `"servo"`, `"cef"`, `"webkit"`, `"webview2"` directly, citing `process_manager.rs:146-149`.
- [ ] **Step 2: Register the module.** Add `mod session_sync;` after `mod session;` in `main.rs`. Add `#![cfg_attr(not(test), allow(dead_code))]` at the top of `session_sync.rs`, with a comment: "// Wired into `App` by the next task; this attribute goes then."
- [ ] **Step 3: Run the tests.** `cargo test -q -j 12 -p manifold -- session_sync` → 10 pass. If `random_edits_converge_in_one_diff` fails, print the ops and the two maps: it is the property the whole integration rests on, so fix `diff`, not the test.
- [ ] **Step 4: Commit.**
```bash
cargo fmt --all
cargo clippy -j 12 -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/session_sync.rs crates/manifold/src/main.rs
git commit -m "session_sync: shared fields per kind, and the diff from live nodes to ops" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Open the journal at startup; ids come from it

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - `struct NodeIds`, `next_id` and `bump_past_id` at main.rs:398-430, plus their tests at ~5870;
  - `alloc_*_id` at 2938-3060;
  - `App` fields and `App::default`;
  - `session_generation`'s doc at 1432-1438.
- Modify: `crates/manifold/src/session.rs` (a `store_path()` helper beside `session_path()`).
- Modify: `crates/manifold/src/event_loop.rs:61` region (where `restore_session` is called in `resumed`).

**Interfaces:**
- Produces:
  - `App::journal: Option<store::journal::Journal>`;
  - `App::alloc_node_id(&mut self) -> String`;
  - `session::store_path() -> Option<PathBuf>` (`%APPDATA%\manifold\session.mnf`);
  - `App::open_store(&mut self)`, called once in `resumed` before `restore_session`.
- Every `alloc_tab_id`/`alloc_note_id`/`alloc_image_id`/`alloc_pdf_id`/`alloc_json_id`/`alloc_group_id` call becomes `alloc_node_id`. Download ids (`insert_download_node`) too.

- [ ] **Step 1: Write the failing test** (main.rs `mod tests`), replacing the `bump_past_id` test at ~5870:
```rust
#[test]
fn node_ids_without_a_store_are_unique_store_ids() {
    let mut app = App::default();
    let a = app.alloc_node_id();
    let b = app.alloc_node_id();
    assert_ne!(a, b);
    let a: crate::store::model::NodeId = a.parse().expect("a store node id");
    let b: crate::store::model::NodeId = b.parse().unwrap();
    assert_eq!((a.actor, a.counter + 1), (b.actor, b.counter));
}
```
- [ ] **Step 2: Run it to see it fail.** `cargo test -q -j 12 -p manifold -- node_ids_without_a_store` → FAIL: no `alloc_node_id`.
- [ ] **Step 3: Implement.**
  - In `session.rs`, next to `session_path`: `pub fn store_path() -> Option<PathBuf> { session_dir().map(|d| d.join("session.mnf")) }`. Match `session_path`'s own `Option` shape; if `session_dir()` returns a plain `PathBuf`, mirror that.
  - Replace `ids: NodeIds` on `App` with:
```rust
    /// The session store (spec "Journal"): `None` before `open_store`, when
    /// `%APPDATA%` is unset, or when the file couldn't be opened (toasted).
    journal: Option<store::journal::Journal>,
    /// Ids for nodes made while there is no journal (unit tests, a failed
    /// open): this launch's own actor, so they parse as store ids.
    fallback_ids: (store::model::ActorId, u64),
```
    In `App::default`, set `journal: None` and `fallback_ids: (store::model::ActorId::random(), 0)`.
  - Add:
```rust
    /// A fresh node id: the journal's counter (contract "Actors and ids"),
    /// or this launch's fallback actor without one.
    fn alloc_node_id(&mut self) -> String {
        match &mut self.journal {
            Some(journal) => journal.next_id().to_string(),
            None => {
                let (actor, counter) = &mut self.fallback_ids;
                let id = store::model::NodeId { actor: *actor, counter: *counter };
                *counter += 1;
                id.to_string()
            }
        }
    }

    /// Opens `session.mnf` under a fresh actor. A failure is toasted and
    /// leaves the app running without a store.
    fn open_store(&mut self) {
        let Some(path) = session::store_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match store::journal::Journal::open(&path, store::model::ActorId::random()) {
            Ok(journal) => self.journal = Some(journal),
            Err(err) => self.queue_toast(format!("Couldn't open the session file {}: {err}", path.display())),
        }
    }
```
  - Delete `struct NodeIds`, `fn next_id`, `fn bump_past_id`, their test, and every `alloc_*_id` fn. Replace each call with `self.alloc_node_id()` (grep `alloc_[a-z]*_id` and `self.ids.`; the download counter is used in `insert_download_node`).
  - `apply_session` still sets `self.ids.*` and calls `bump_past_id`: delete those lines. Old `session.json` ids (`tab-3`) still load as plain strings until Task 5, and new ids can't collide with them.
  - Rewrite `session_generation`'s doc so it no longer says "(both count from `tab-0`)". A spawn from a replaced session still must not land in the new scene. Say that, without the id-collision clause.
  - In `resumed` (event_loop.rs, before `if !self.restore_session()`), call `self.open_store();`.
- [ ] **Step 4: Run the tests and clippy.** `cargo test -q -j 12 -p manifold` → all pass (closed-entry tests use literal ids and are unaffected).
- [ ] **Step 5: Headless smoke.** `cargo build -j 12 -p manifold && scripts/headless-smoke-test.sh` → passes. The IMPORTGROUP fixtures still use the old format here; Task 9 rewrites them.
- [ ] **Step 6: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src/main.rs crates/manifold/src/session.rs crates/manifold/src/event_loop.rs
git commit -m "Open session.mnf at startup; node ids come from the journal's counter" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Save and restore through the store; merge other instances' edits

This is the core switch. After it, `session.json` is neither read nor written.

**Files:**
- Modify: `crates/manifold/src/session_sync.rs` (the `impl App` half)
- Modify: `crates/manifold/src/main.rs`:
  - delete `save_session` (2199-2222), `build_session_snapshot`, `collect_session_field`, `session_place`, `saved_group`, `apply_session`, `restore_group_and_color` and the old `restore_session`;
  - `replace_running_session` moves to `session_sync`;
  - `spawn_tab_async` and `handle_tab_spawn_result` maintain `spawning`;
  - remove the `session_fingerprint` and `restore_spawns_in_flight` fields.
- Modify: `crates/manifold/src/event_loop.rs`: `poll_store()` each `RedrawRequested`; a new `fn exiting(&mut self, _: &ActiveEventLoop)` in the `ApplicationHandler` impl calling `self.close_store()`.
- Modify: `crates/manifold/src/session.rs`: delete `fingerprint`, `content_fingerprint`, `conflict_path`, `Session::load`/`save`, and the `session_path()` users. `Session*` structs stay until Task 9, which moves Import/Open off them.

**Interfaces:**
- Consumes: Task 3's core and Task 4's `journal`/`alloc_node_id`.
- Produces (on `App`, `pub(crate)` inside the crate):
  - `spawning: HashSet<String>`: ids whose tab spawn is in flight.
  - `explicit: BTreeSet<(NodeId, u32)>`: marked explicit fields (Task 6 adds the marking sites).
  - `shared_pos: HashMap<String, egui::Vec2>`: the shared `pos` each live node last had, so a remote change moves a node only when its `pos` changed.
  - `fn shared_nodes(&self) -> BTreeMap<NodeId, Shared>`
  - `fn materialize(&mut self, id: String, kind: Kind, fields: &Fields, group: Option<String>, purpose: SpawnPurpose)`: creates the live node, or updates it when it exists.
  - `fn apply_remote(&mut self, changes: Vec<(NodeId, Option<(Kind, Fields, Option<NodeId>)>)>)`
  - `fn save_session(&mut self)`, `fn restore_session(&mut self) -> bool`, `fn poll_store(&mut self)`, `fn close_store(&mut self)`
  - `fn replace_running_session(&mut self, json: store::json::JsonDoc)`: used by Task 9.

- [ ] **Step 1: `spawning` replaces the restore guard.** In `spawn_tab_async` (main.rs:2712), insert `spawn.id.clone()` into `self.spawning` for every purpose. In `handle_tab_spawn_result` (2731), remove `outcome.spawn.id` from `self.spawning` first thing, before any early return, including the stale-generation drop. Delete `restore_spawns_in_flight` and its increments and decrements. Keep a `restore_done` notion if other code uses it (grep); the save guard itself goes.
- [ ] **Step 2: Projection.** In `session_sync.rs`'s `impl App`:
```rust
impl App {
    /// Every live node the store keeps, as `Shared`. Skipped: an
    /// in-progress download (it can't resume), and a node whose id isn't a
    /// store id (only hand-built test nodes).
    pub(crate) fn shared_nodes(&self) -> BTreeMap<NodeId, Shared> {
        let parse = |id: &Option<String>| id.as_deref().and_then(|id| id.parse::<NodeId>().ok());
        let mut out = BTreeMap::new();
        for (key, node) in &self.nodes {
            let Ok(id) = key.parse::<NodeId>() else { continue };
            let pos = FieldValue::Pos(node.pos.x, node.pos.y);
            let mut fields = Fields::from([(field::POS, pos), (field::COLOR, color_value(node.color))]);
            let str = |s: &str| FieldValue::Str(s.to_owned());
            let kind = match &node.payload {
                NodePayload::Tab(tab) => {
                    fields.insert(field::URL, str(&tab.url));
                    fields.insert(field::ENGINE, FieldValue::Engine(engine_code(tab.tab.kind())));
                    fields.insert(field::PINNED, FieldValue::Bool(tab.pinned));
                    fields.insert(field::PARENT, FieldValue::Id(parse(&node.parent)));
                    Kind::Tab
                }
                NodePayload::FailedTab(tab) => {
                    fields.insert(field::URL, str(&tab.url));
                    fields.insert(field::ENGINE, FieldValue::Engine(engine_code(tab.kind)));
                    fields.insert(field::PINNED, FieldValue::Bool(tab.pinned));
                    fields.insert(field::PARENT, FieldValue::Id(parse(&node.parent)));
                    Kind::Tab
                }
                NodePayload::Download(dl) if dl.completed => {
                    fields.insert(field::PATH, str(&dl.path));
                    Kind::Download
                }
                NodePayload::Download(_) => continue,
                NodePayload::Group(g) => {
                    fields.insert(field::NAME, str(&g.name));
                    fields.insert(field::FROM_JSON, FieldValue::Bool(g.from_json));
                    Kind::Group
                }
                NodePayload::Note(n) => {
                    fields.insert(field::TEXT, str(&n.text));
                    Kind::Note
                }
                NodePayload::Image(i) => {
                    fields.insert(field::PATH, str(&i.path));
                    Kind::Image
                }
                NodePayload::Pdf(p) => {
                    fields.insert(field::PATH, str(&p.path));
                    Kind::Pdf
                }
                NodePayload::Json(j) => {
                    fields.insert(field::PATH, str(&j.path));
                    match &j.key {
                        None => Kind::Json,
                        Some(key) => {
                            fields.insert(field::KEY, str(key));
                            fields.insert(field::VALUE, FieldValue::Str(j.value.to_string()));
                            Kind::JsonLeaf
                        }
                    }
                }
            };
            debug_assert_eq!(fields.keys().copied().collect::<Vec<_>>(), {
                let mut want = kind_fields(kind).to_vec();
                want.sort();
                want
            });
            out.insert(id, Shared { kind, group: parse(&node.group), fields });
        }
        out
    }
}
```
  How a `TabPayload` names its engine: use whatever `EngineTab` exposes (grep `fn kind` in process_manager.rs). If nothing does, `TabSpawn` has `kind`, so carry `kind: EngineKind` on `TabPayload` where it is built in `handle_tab_spawn_result`. A pos that isn't finite (it shouldn't happen) becomes `(0.0, 0.0)`: guard with `node.pos.x.is_finite() && node.pos.y.is_finite()` (contract "Fields").
- [ ] **Step 3: Materialize.** One function creates or updates a live node from shared fields, for startup restore, remote ops, imports and reopen:
```rust
fn str_of(fields: &Fields, no: u32) -> String {
    match fields.get(&no) {
        Some(FieldValue::Str(s)) => s.clone(),
        _ => String::new(),
    }
}
fn bool_of(fields: &Fields, no: u32) -> bool {
    matches!(fields.get(&no), Some(FieldValue::Bool(true)))
}
fn pos_of(fields: &Fields) -> egui::Vec2 {
    match fields.get(&field::POS) {
        Some(FieldValue::Pos(x, y)) => egui::vec2(*x, *y),
        _ => egui::Vec2::ZERO,
    }
}
fn id_of(fields: &Fields, no: u32) -> Option<String> {
    match fields.get(&no) {
        Some(FieldValue::Id(Some(id))) => Some(id.to_string()),
        _ => None,
    }
}

impl App {
    /// Makes live node `id` show `fields`. A new node is inserted, and a
    /// new tab is spawned with `purpose` (it joins `spawning`). An existing
    /// node is updated in place:
    /// - `pos` moves only when the shared pos changed (`shared_pos`);
    /// - a tab's url change navigates without marking `url`;
    /// - an engine change swaps engines.
    /// A kind mismatch between the live node and `kind` is ignored (the
    /// store never changes a node's kind).
    pub(crate) fn materialize(
        &mut self,
        id: String,
        kind: Kind,
        fields: &Fields,
        group: Option<String>,
        purpose: SpawnPurpose,
    ) {
        let pos = pos_of(fields);
        let color = fields.get(&field::COLOR).and_then(color_of);
        let pos_changed = self.shared_pos.insert(id.clone(), pos) != Some(pos);
        if self.spawning.contains(&id) {
            return; // the spawn in flight carries what it was given
        }
        if let Some(node) = self.nodes.get_mut(&id) {
            node.group = group;
            node.color = color;
            if pos_changed {
                node.pos = pos;
                node.vel = egui::Vec2::ZERO;
            }
            if kind == Kind::Tab {
                node.parent = id_of(fields, field::PARENT);
            }
            let (url, engine) = (str_of(fields, field::URL), fields.get(&field::ENGINE).cloned());
            match &mut node.payload {
                NodePayload::Tab(tab) => {
                    tab.pinned = bool_of(fields, field::PINNED);
                    let want = engine_of(match engine { Some(FieldValue::Engine(c)) => c, _ => 0 });
                    let url_changed = tab.url != url;
                    if url_changed {
                        tab.url = url.clone();
                        tab.tab.send(&HostToEngine::Navigate { url, referrer: None });
                    }
                    if engine_code(tab.tab.kind()) != engine_code(want) {
                        self.start_swap_to(&id, want);
                    }
                }
                NodePayload::FailedTab(tab) => {
                    tab.pinned = bool_of(fields, field::PINNED);
                    tab.url = url;
                }
                NodePayload::Group(g) => {
                    g.name = str_of(fields, field::NAME);
                    g.from_json = bool_of(fields, field::FROM_JSON);
                }
                NodePayload::Note(n) => n.text = str_of(fields, field::TEXT),
                _ => {} // paths never change after creation
            }
            return;
        }
        match kind {
            Kind::Tab => {
                let engine = match fields.get(&field::ENGINE) { Some(FieldValue::Engine(c)) => *c, _ => 0 };
                let spawn = TabSpawn::new(id, str_of(fields, field::URL), engine_of(engine), pos, purpose, self.settings.zoom_factor);
                let carry = TabCarry { pinned: bool_of(fields, field::PINNED), color, ..spawn.carry };
                self.spawn_tab_async(TabSpawn { parent: id_of(fields, field::PARENT), group, carry, ..spawn });
            }
            Kind::Group => {
                self.insert_group_at(id.clone(), pos);
                if let Some(g) = self.nodes.get_mut(&id).and_then(|n| n.as_group_mut()) {
                    g.name = str_of(fields, field::NAME);
                    g.from_json = bool_of(fields, field::FROM_JSON);
                }
                self.set_group_and_color(&id, group, color);
            }
            Kind::Note => {
                self.insert_note_at(id.clone(), pos);
                if let Some(n) = self.nodes.get_mut(&id).and_then(|n| n.as_note_mut()) {
                    n.text = str_of(fields, field::TEXT);
                }
                self.set_group_and_color(&id, group, color);
            }
            Kind::Image => {
                let path = str_of(fields, field::PATH);
                if self.insert_image_node_with_id(id.clone(), path.clone(), pos, group.clone()).is_err() {
                    // Keep the card, showing its missing-file state, so the
                    // node isn't deleted from the store by the next diff.
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Image(ImagePayload { path, width: 0, height: 0, error: Some("file not found".into()) }));
                }
                self.set_group_and_color(&id, group, color);
            }
            Kind::Pdf => {
                self.insert_pdf_node_with_id(id.clone(), str_of(fields, field::PATH), pos);
                self.set_group_and_color(&id, group, color);
            }
            Kind::Json => {
                let path = str_of(fields, field::PATH);
                if self.insert_json_node_with_id(id.clone(), path.clone(), pos, group.clone()).is_err() {
                    self.insert_placeholder_node(id.clone(), pos, NodePayload::Json(JsonPayload { path, value: serde_json::Value::Null, key: None, error: Some("file not found".into()) }));
                }
                self.set_group_and_color(&id, group, color);
            }
            Kind::JsonLeaf => {
                let text = str_of(fields, field::VALUE);
                let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
                let payload = JsonPayload { path: str_of(fields, field::PATH), value, key: Some(str_of(fields, field::KEY)), error: None };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Json(payload));
                self.set_group_and_color(&id, group, color);
            }
            Kind::Download => {
                let path = str_of(fields, field::PATH);
                let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
                let payload = DownloadPayload { path, bytes_received: bytes, total_bytes: Some(bytes), completed: true };
                self.insert_placeholder_node(id.clone(), pos, NodePayload::Download(payload));
                self.set_group_and_color(&id, group, color);
            }
            Kind::Unknown(_) => {}
        }
    }

    fn set_group_and_color(&mut self, id: &str, group: Option<String>, color: Option<egui::Color32>) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.group = group;
            node.color = color;
        }
    }
}
```
  Adapt each insert helper call to its real signature. Read each at main.rs:4179 (`insert_note_at`), `insert_group_at`, 4240 (`insert_placeholder_node`), 4328, 4437 and 4474. Keep the semantics:
  - never drop a node because its file is missing (a later diff would delete it from the store);
  - never toast per node during a bulk restore, if a helper toasts (check the comment at 4336; batch into one toast or skip it for restore).

  If `insert_placeholder_node`'s payload parameter or the payload struct fields differ, use the real ones. `ImagePayload`/`JsonPayload` fields are listed in node.rs:213-285. `as_group_mut`/`as_note_mut` exist in node.rs:24-125; use whatever the accessors are named.
- [ ] **Step 4: Remote apply, save, restore, poll, close.**
```rust
/// One node another replica changed: its kind, fields and effective
/// group, or `None` when it is no longer live (deleted or pruned).
pub(crate) type RemoteChange = (NodeId, Option<(Kind, Fields, Option<NodeId>)>);

impl App {
    fn remote_changes(doc: &Doc, remote: &[NodeId]) -> Vec<RemoteChange> {
        remote
            .iter()
            .map(|&id| {
                let live = doc.nodes.get(&id).filter(|n| n.is_live());
                (id, live.map(|n| (n.kind, fields_of(n), doc.effective_group(id))))
            })
            .collect()
    }

    /// Shows other replicas' changes. A deleted node is removed without a
    /// Recently Closed entry, and a tab's engine host is shut down. Groups
    /// are materialized before the nodes inside them, so a new member
    /// never names a group that doesn't exist yet.
    pub(crate) fn apply_remote(&mut self, mut changes: Vec<RemoteChange>) {
        changes.sort_by_key(|(_, c)| !matches!(c, Some((Kind::Group, ..))));
        for (id, change) in changes {
            let key = id.to_string();
            match change {
                Some((kind, fields, group)) => {
                    self.materialize(key, kind, &fields, group.map(|g| g.to_string()), SpawnPurpose::New)
                }
                None => {
                    self.shared_pos.remove(&key);
                    self.remove_remotely_deleted(&key);
                }
            }
        }
    }

    /// Saves local edits (spec "Local edits become ops"): take the lock,
    /// show what other replicas wrote, diff the live graph against the
    /// document, and commit. On error the explicit marks stay for the next
    /// save, which re-diffs (contract "Transactions").
    pub(crate) fn save_session(&mut self) {
        // Each round applies what others wrote since the last; four rounds
        // of fresh remote frames means another instance is busy: try again
        // at the next save point rather than spin here.
        for _ in 0..4 {
            let live = self.shared_nodes();
            let keep: BTreeSet<NodeId> = self.spawning.iter().filter_map(|id| id.parse().ok()).collect();
            let Some(journal) = self.journal.as_mut() else { return };
            let txn = match journal.begin() {
                Ok(txn) => txn,
                Err(err) => return self.store_error(err),
            };
            if !txn.remote.is_empty() {
                let changes = Self::remote_changes(txn.doc(), &txn.remote);
                drop(txn);
                self.take_store_recovery();
                self.apply_remote(changes);
                continue;
            }
            if txn.read_only() {
                drop(txn);
                return self.take_store_recovery();
            }
            let ops = diff(txn.doc(), &live, &self.explicit, &keep);
            let result = if ops.is_empty() { Ok(()) } else { txn.commit(ops) };
            self.take_store_recovery();
            match result {
                Ok(()) => {
                    self.explicit.clear();
                    for (id, s) in &live {
                        if let Some(FieldValue::Pos(x, y)) = s.fields.get(&field::POS) {
                            self.shared_pos.entry(id.to_string()).or_insert(egui::vec2(*x, *y));
                        }
                    }
                    self.last_session_save = Some(std::time::Instant::now());
                }
                Err(err) => self.store_error(err),
            }
            return;
        }
    }

    /// Startup: shows the document and (Task 7) personal state. `false`
    /// when there is nothing to show, so the caller spawns the first tab.
    pub(crate) fn restore_session(&mut self) -> bool {
        let Some(journal) = self.journal.as_mut() else { return false };
        let doc = journal.doc();
        let nodes: Vec<RemoteChange> = doc
            .live()
            .map(|(id, n)| (id, Some((n.kind, fields_of(n), doc.effective_group(id)))))
            .collect();
        self.take_store_recovery();
        if nodes.is_empty() {
            return false;
        }
        let mut nodes = nodes;
        nodes.sort_by_key(|(_, c)| !matches!(c, Some((Kind::Group, ..))));
        for (id, change) in nodes {
            if let Some((kind, fields, group)) = change {
                self.materialize(id.to_string(), kind, &fields, group.map(|g| g.to_string()), SpawnPurpose::Restore);
            }
        }
        // Today's `apply_session` tail: focus the first tab, re-fit once
        // restored tabs land. Task 7 restores the saved view instead.
        self.focused = self.node_order.iter().find(|id| self.nodes.get(*id).is_some_and(|n| n.as_tab().is_some())).cloned()
            .or_else(|| self.spawning.iter().next().cloned());
        self.camera.frame_restored = true;
        true
    }

    /// Each frame: pick up other instances' edits and flush a debounced fsync.
    pub(crate) fn poll_store(&mut self) {
        let Some(journal) = self.journal.as_mut() else { return };
        if let Err(err) = journal.sync_if_due() {
            self.store_error(err);
        }
        if self.journal.as_ref().is_some_and(|j| j.changed_on_disk()) {
            self.save_session();
        }
    }

    /// Clean exit: save, then close (compacts only if something changed).
    pub(crate) fn close_store(&mut self) {
        self.save_session();
        if let Some(journal) = self.journal.as_mut() {
            if let Err(err) = journal.close() {
                eprintln!("session: closing the store failed: {err}");
            }
        }
    }

    /// Replaces the running session with `json`'s nodes under fresh ids
    /// (Import Session, Open; spec ruling 1-2). Returns the label map
    /// (file id → new id) for remapping personal state (Task 7).
    pub(crate) fn replace_running_session(&mut self, json: &store::json::JsonDoc) -> HashMap<String, String> {
        let imported = match store::json::import(json, || self.alloc_node_id().parse().expect("a store id")) {
            Ok(imported) => imported,
            Err(err) => {
                self.queue_toast(format!("Couldn't import the session: {err}"));
                return HashMap::new();
            }
        };
        self.teardown_running_session();
        let map: HashMap<String, String> =
            json.nodes.iter().zip(&imported).map(|(node, i)| (node.id.clone(), i.id.to_string())).collect();
        let mut imported = imported;
        imported.sort_by_key(|i| i.kind != Kind::Group);
        for node in imported {
            let group = match node.fields.get(&field::GROUP) {
                Some(FieldValue::Id(Some(g))) => Some(g.to_string()),
                _ => None,
            };
            let mut fields: Fields = kind_fields(node.kind)
                .iter()
                .map(|&no| (no, node.fields.get(&no).cloned().unwrap_or_else(|| FieldValue::absent(field_def(no).unwrap().ty))))
                .collect();
            fields.remove(&field::GROUP);
            self.materialize(node.id.to_string(), node.kind, &fields, group, SpawnPurpose::Restore);
        }
        self.focused = self.spawning.iter().next().cloned();
        self.camera.frame_restored = true;
        self.save_session();
        map
    }
}
```
  - Build `teardown_running_session` from the teardown half of today's `replace_running_session` (main.rs:2567-2584): clear `nodes`, `node_order`, `focused`, `current_group`, `dragging`, `hovered`, `selected_tabs`, `rect_select_start`, `graph_settled_key` and `failed_tab_retries`, bump `session_generation`, and also clear `spawning` and `shared_pos`.
    - Check how today's teardown shuts engine hosts. It relies on `EngineTab`'s `Drop` when `nodes.clear()` runs; keep that.
    - Today's "no-op if the import has no tabs" rule: keep it as `if !imported.iter().any(|i| i.kind == Kind::Tab) { toast "The session has no tabs"; return }`, before the teardown.
  - Write `remove_remotely_deleted(&mut self, id: &str)`:
    - for a tab, do what `destroy_tab` does, minus any Recently Closed push (read `destroy_tab` at 3849 and `close_node`'s push at 2032);
    - for other kinds, call `remove_node`;
    - clear `focused`, `viewed_image` and `split_tab` if they name it;
    - if `current_group` names it, set `current_group` to the removed group's own group.
  - `store_error(&mut self, err: JournalError)` and `take_store_recovery(&mut self)`: for now, `store_error` does `eprintln!` plus `queue_toast(format!("Session not saved: {err}"))`, deduplicated by remembering the last message in `self.last_store_notice: Option<String>`. `take_store_recovery` takes the recovery and toasts a one-line summary. Task 10 refines both.
  - Borrow note: `save_session` computes `live` and `keep` before `begin`, because the `Txn` borrows `self.journal` mutably. The live graph is re-projected each round.
  - The old `restore_session` in main.rs falls back to `spawn_root_tab`/`spawn_first_launch_demo`. Keep that caller logic as it is; only the function body moves.
- [ ] **Step 5: Wire frames and exit.** In event_loop.rs, at the top of the `RedrawRequested` arm, call `self.poll_store();`. Add to `impl ApplicationHandler<UserEvent> for App`:
```rust
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.close_store();
    }
```
  `DebugCommand::Quit` ends in `event_loop.exit()`, which also reaches `exiting`, so headless quits close cleanly too.
- [ ] **Step 6: Unit test the projection and materialize round trip.** In `session_sync.rs` `mod tests` add:
```rust
#[test]
fn a_restored_note_group_and_leaf_project_back_to_their_fields() {
    let mut app = crate::App::default();
    let g = NodeId { actor: A, counter: 1 };
    let n = NodeId { actor: A, counter: 2 };
    let l = NodeId { actor: A, counter: 3 };
    let group_fields = shared(Kind::Group, None, &[(field::NAME, FieldValue::Str("G".into())), (field::POS, FieldValue::Pos(1.0, 2.0))]).fields;
    let note_fields = shared(Kind::Note, Some(g), &[(field::TEXT, FieldValue::Str("hi".into())), (field::COLOR, FieldValue::Color(Some(0x010203)))]).fields;
    let leaf_fields = shared(Kind::JsonLeaf, Some(g), &[(field::KEY, FieldValue::Str("k".into())), (field::VALUE, FieldValue::Str("[1,2]".into()))]).fields;
    app.materialize(g.to_string(), Kind::Group, &group_fields, None, crate::SpawnPurpose::Restore);
    app.materialize(n.to_string(), Kind::Note, &note_fields, Some(g.to_string()), crate::SpawnPurpose::Restore);
    app.materialize(l.to_string(), Kind::JsonLeaf, &leaf_fields, Some(g.to_string()), crate::SpawnPurpose::Restore);
    let shared_now = app.shared_nodes();
    assert_eq!(shared_now[&g].fields, group_fields);
    assert_eq!(shared_now[&n], Shared { kind: Kind::Note, group: Some(g), fields: note_fields });
    assert_eq!(shared_now[&l].fields, leaf_fields);
}
```
  If `App::default()` can't insert nodes without a window (for example, `insert_note_at` touching GPU state), make the test build `GraphNode`s directly into `app.nodes` and check only `shared_nodes`. The materialize path is then covered by the headless tests in Tasks 9 and 13; say so in the test's comment.
- [ ] **Step 7: Run all tests and clippy.** `cargo test -q -j 12 -p manifold` → pass. `grep -rn "session_fingerprint\|conflict_path\|restore_spawns_in_flight\|build_session_snapshot" crates/manifold/src` → no hits.
- [ ] **Step 8: Headless check.** `cargo build -j 12 -p manifold`, then write `target/plan-b/t5.cmds`:
```
NEWNOTE
NEWGROUP
WAITTABS
EXPORTSTATE {repo}\target\plan-b\t5-a.json
QUIT
```
  Run `KEEP_APPDATA=1 scripts/drive.sh target/plan-b/t5.cmds`. Check that the kept profile has `manifold/session.mnf` and no `session.json`. Run `target/debug/manifold.exe --check` on it once Task 11 exists; until then, check that the file starts with `MNF` (`head -c 3`). Then relaunch on the same profile. drive.sh makes a fresh profile each run, so for this check copy the kept profile's `manifold/` into a `SEED`-like dir by hand, or extend drive.sh with `APPDATA_DIR=<dir>` to reuse a profile. Add that `APPDATA_DIR` option to drive.sh now (document it in its header), since Task 13 needs it. The second run's EXPORTSTATE must list the note and group ids from the first.
- [ ] **Step 9: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src scripts/drive.sh
git commit -m "The session lives in session.mnf: save by diffing into the store, restore and merge other instances' edits" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Explicit fields: mark `pos` and `url` at user actions

**Files:**
- Modify: `crates/manifold/src/session_sync.rs` (a `mark` helper)
- Modify: the drag-end and placement sites (`card.rs`, `graph_ui.rs`, `main.rs`, `event_loop.rs`), `navigate_tab` (main.rs:2109), and `NavigationStateChanged` (event_loop.rs:1137-1168)

**Interfaces:**
- Produces: `App::mark(&mut self, id: &str, field: u32)`.
- Consumes: `App::explicit` (Task 5).

- [ ] **Step 1: Add the helper.**
```rust
impl App {
    /// Marks explicit field `field` (`pos` or `url`) of `id` for the next
    /// save (spec "Local edits become ops"). New nodes need no mark: their
    /// `Create` carries every field.
    pub(crate) fn mark(&mut self, id: &str, field: u32) {
        if let Ok(id) = id.parse() {
            self.explicit.insert((id, field));
        }
    }
}
```
- [ ] **Step 2: Mark `pos` at every user placement of an existing node.** Find the drag ends with `grep -n "dragging = None\|drag_stopped\|drag_released" crates/manifold/src/*.rs`. The survey lists card.rs:732, event_loop.rs:1192, and main.rs:2952, 3106, 3873, 3937 and 4394.
  - Of those, `remove_node` (2952), `dissolve_group` (3106), `destroy_tab` (3873), `finish_swap` (3937), `redirect_tab_to_image` (4394) and `replace_running_session` are cleanup, not drag ends: leave them.
  - For each real drag end (a card release in `card.rs`, the graph's drag release in `graph_ui.rs`/event_loop.rs:1192, a rect-selection group drag if any), call `self.mark(&id, field::POS)` for every node the drag moved. A group drag moves its members' displayed positions but not their stored positions; check what the code does and mark exactly the nodes whose `pos` the gesture set.
  - Also mark `pos` where a user action places an existing node:
    - dropping a tab onto a group or tab (reparenting sets `group`/`parent`, and `pos` if the code sets it);
    - Reset Layout and "arrange" actions, if any (`grep -n "fn arrange\|fn reset_layout\|fn tidy"`): they are user actions, so their positions are shared.
  - Don't mark physics steps (`graph_layout::step`) or eject animations.
- [ ] **Step 3: Mark `url`.** In `navigate_tab` (main.rs:2109), after `node.url` is set, call `self.mark(tab_id, field::URL)`. In `NavigationStateChanged` (event_loop.rs:1137), where `node.url != url` is detected, record the tab id; after the drain loop, before the batched `save_session()` at event_loop.rs:1019-1026, mark `url` for each recorded id. This is ruling 3: every reported url change is shared. Back/forward reach the same handler, so they need nothing extra.
- [ ] **Step 4: Test the marks reach the store.** A unit test in `session_sync.rs`:
```rust
#[test]
fn a_marked_pos_is_saved_and_an_unmarked_one_is_not() {
    let dir = crate::store::testutil::temp_dir("marks");
    let mut app = crate::App::default();
    app.journal = Some(crate::store::journal::Journal::open(&dir.join("session.mnf"), ActorId::random()).unwrap());
    let id = app.alloc_node_id();
    app.insert_note_at(id.clone(), egui::vec2(1.0, 1.0));
    app.save_session();
    app.nodes.get_mut(&id).unwrap().pos = egui::vec2(50.0, 50.0);
    app.save_session();
    let stored = |app: &crate::App| app.journal.as_ref().unwrap().doc().nodes[&id.parse().unwrap()].get(field::POS);
    assert_eq!(stored(&app), FieldValue::Pos(1.0, 1.0), "physics motion stays local");
    app.mark(&id, field::POS);
    app.save_session();
    assert_eq!(stored(&app), FieldValue::Pos(50.0, 50.0));
}
```
  If `insert_note_at` can't run without a window, build the `GraphNode` directly, as Task 5 Step 6 does.
- [ ] **Step 5: Run the tests, clippy, and the smoke test** (after `cargo build -j 12 -p manifold`). All pass.
- [ ] **Step 6: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "session_sync: drag ends and placements share pos; url changes share url" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Personal state in `session.mnf.local`

**Files:**
- Create: `crates/manifold/src/local_state.rs`
- Modify: `crates/manifold/src/main.rs` (`mod local_state;`, App fields), `session_sync.rs` (`restore_session`, `close_store`, `replace_running_session`), and event_loop.rs (`maybe_write_local` per frame)

**Interfaces:**
- Produces (`pub(crate)`):
  - `struct LocalState { focused, view_item, current_group, viewed_image, split_tab, groups_collapsed, camera: Option<([f32; 4], Option<String>)>, nodes: BTreeMap<String, LocalNode>, closed: Vec<store::json::JsonDoc> }`
  - `struct LocalNode { pos: [f32; 2], title: String, muted: bool, zoom: Option<f64>, editing: bool }`
  - `fn differs(a: &LocalState, b: &LocalState) -> bool` (positions within 1 px are equal)
  - `fn remap(&self, map: &HashMap<String, String>) -> LocalState`
  - On `App`: `fn local_state(&self) -> LocalState`, `fn apply_local(&mut self, state: LocalState)`, `fn maybe_write_local(&mut self, force: bool)`.
  - App fields: `local_written: Option<LocalState>`, `local_written_at: std::time::Instant`, `closed_changed_at: Option<std::time::Instant>`, and `pending_local: Option<LocalState>` (read at startup, applied as nodes appear).

- [ ] **Step 1: Write `local_state.rs` with tests.**
```rust
//! Personal state (spec "Personal state"): what this profile shows and
//! how, never merged. JSON inside `session.mnf.local`'s state bytes
//! (plan ruling 4).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::store::json::JsonDoc;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LocalState {
    #[serde(default)]
    pub focused: Option<String>,
    /// Item view (a tab or image full-size) rather than Graph view.
    #[serde(default)]
    pub view_item: bool,
    #[serde(default)]
    pub current_group: Option<String>,
    #[serde(default)]
    pub viewed_image: Option<String>,
    #[serde(default)]
    pub split_tab: Option<String>,
    #[serde(default)]
    pub groups_collapsed: bool,
    /// The camera rect `[min x, min y, max x, max y]` and the level it was
    /// saved at (plan ruling 5).
    #[serde(default)]
    pub camera: Option<([f32; 4], Option<String>)>,
    /// Per node, by live id.
    #[serde(default)]
    pub nodes: BTreeMap<String, LocalNode>,
    /// Recently Closed, oldest first (plan ruling 7).
    #[serde(default)]
    pub closed: Vec<JsonDoc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LocalNode {
    /// The settled position, so the graph reopens as it was left.
    pub pos: [f32; 2],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub editing: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// Whether `a` and `b` differ enough to write: settled positions within
/// 1 px count as equal, so physics jitter never causes a write.
pub(crate) fn differs(a: &LocalState, b: &LocalState) -> bool {
    let strip = |s: &LocalState| {
        let mut s = s.clone();
        s.nodes.values_mut().for_each(|n| n.pos = [0.0, 0.0]);
        s
    };
    if strip(a) != strip(b) {
        return true;
    }
    a.nodes.iter().any(|(id, n)| {
        let m = &b.nodes[id]; // same keys: `strip` compared them
        (n.pos[0] - m.pos[0]).abs() > 1.0 || (n.pos[1] - m.pos[1]).abs() > 1.0
    })
}

impl LocalState {
    /// This state with ids renamed by `map` (an import gave nodes fresh
    /// ids); ids `map` lacks are dropped.
    pub(crate) fn remap(&self, map: &HashMap<String, String>) -> LocalState {
        let id = |old: &Option<String>| old.as_ref().and_then(|o| map.get(o)).cloned();
        LocalState {
            focused: id(&self.focused),
            current_group: id(&self.current_group),
            viewed_image: id(&self.viewed_image),
            split_tab: id(&self.split_tab),
            camera: self.camera.clone().filter(|(_, level)| level.is_none() || id(level).is_some()).map(|(r, level)| (r, id(&level))),
            nodes: self.nodes.iter().filter_map(|(k, v)| Some((map.get(k)?.clone(), v.clone()))).collect(),
            ..self.clone()
        }
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("personal state serializes")
    }

    /// `None` for bytes that aren't this shape: treated as a fresh start.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<LocalState> {
        serde_json::from_slice(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_pos(x: f32) -> LocalState {
        let node = LocalNode { pos: [x, 0.0], ..LocalNode::default() };
        LocalState { nodes: BTreeMap::from([("a".to_owned(), node)]), ..LocalState::default() }
    }

    #[test]
    fn jitter_under_a_pixel_is_not_a_change() {
        assert!(!differs(&with_pos(10.0), &with_pos(10.9)));
        assert!(differs(&with_pos(10.0), &with_pos(11.5)));
        assert!(differs(&with_pos(10.0), &LocalState::default()), "a node appearing is a change");
    }

    #[test]
    fn remap_renames_ids_and_drops_unknown_ones() {
        let mut state = with_pos(1.0);
        state.focused = Some("a".into());
        state.current_group = Some("gone".into());
        let map = HashMap::from([("a".to_owned(), "b".to_owned())]);
        let out = state.remap(&map);
        assert_eq!(out.focused.as_deref(), Some("b"));
        assert_eq!(out.current_group, None);
        assert!(out.nodes.contains_key("b"));
    }

    #[test]
    fn bytes_round_trip_and_garbage_is_a_fresh_start() {
        let state = with_pos(3.0);
        assert_eq!(LocalState::from_bytes(&state.to_bytes()), Some(state));
        assert_eq!(LocalState::from_bytes(b"\x00not json"), None);
    }
}
```
- [ ] **Step 2: Run the tests.** `cargo test -q -j 12 -p manifold -- local_state` → 3 pass.
- [ ] **Step 3: Build and apply it on `App`** (in `session_sync.rs`):
  - `local_state(&self) -> LocalState` reads:
    - `focused`; `view_mode == ViewMode::Item`; `current_group`;
    - `viewed_image`, only while `view_mode == Item` (keep the invariant noted in the old `build_session_snapshot`);
    - `split_tab`, `groups_collapsed`, and `camera.rect` with `current_group`;
    - for every node in `nodes` with a store id: `pos`, a tab's `title`/`muted`/`zoom_factor` (zoom only when it differs from `settings.zoom_factor`), and a note's `editing`;
    - `closed`: empty until Task 8.
  - `apply_local(&mut self, state: LocalState)`, called at the end of `restore_session` (Task 5) with the state read at startup:
    - set the view fields when their ids exist, live or spawning; otherwise keep the Task 5 defaults;
    - for each existing node, set `pos` (and `vel = ZERO`), `title`, `muted`, `zoom` and `editing`;
    - for tabs still spawning, keep the rest in `self.pending_local`, and apply each entry in `handle_tab_spawn_result` when that id's node is inserted (look it up and remove it);
    - `camera`: if the saved level equals `current_group`, set `camera.rect` and skip `frame_restored`.
  - In `restore_session`, read it right after taking recovery. The read must come before materializing, because `local.nodes[id].pos` replaces the shared pos for the initial placement:
```rust
let local = match self.journal.as_ref().map(|j| j.read_local()) {
    Some(Ok(Some(bytes))) => LocalState::from_bytes(&bytes).unwrap_or_default(),
    _ => LocalState::default(), // contract: Err or None is a fresh start
};
```
    Pass `local.nodes.get(&id).map(|n| egui::vec2(n.pos[0], n.pos[1]))` as the starting position: materialize takes the shared `pos` from `fields`, so overwrite `fields[POS]` in the copy passed to `materialize` for restore, but set `shared_pos` from the real shared `pos`. Implement this by adding an `initial_pos: Option<egui::Vec2>` parameter to `materialize` (update Task 5's callers to pass `None`).
- [ ] **Step 4: Write it on the spec's schedule.**
```rust
impl App {
    /// Writes personal state when it changed (contract "Personal state"):
    /// at most every 30 s, 2.5 s after a Recently Closed change, and at
    /// exit (`force`).
    pub(crate) fn maybe_write_local(&mut self, force: bool) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(30);
        const AFTER_CLOSED: std::time::Duration = std::time::Duration::from_millis(2500);
        let due = force
            || self.closed_changed_at.is_some_and(|t| t.elapsed() >= AFTER_CLOSED)
            || self.local_written_at.elapsed() >= EVERY;
        if !due || self.journal.is_none() {
            return;
        }
        let state = self.local_state();
        self.local_written_at = std::time::Instant::now();
        if self.local_written.as_ref().is_some_and(|w| !crate::local_state::differs(w, &state)) {
            self.closed_changed_at = None;
            return;
        }
        match self.journal.as_ref().unwrap().write_local(&state.to_bytes()) {
            Ok(()) => {
                self.local_written = Some(state);
                self.closed_changed_at = None;
            }
            Err(err) => self.store_error(err),
        }
    }
}
```
  Call `self.maybe_write_local(false)` in `poll_store`. In `close_store`, call `self.maybe_write_local(true)` after `save_session()` and before `journal.close()`. Initialize `local_written` to the state read at startup (after `apply_local`), so an idle launch writes nothing: it compares equal until something changes.
- [ ] **Step 5: Import and Open carry personal state.** In `replace_running_session`, after materializing: if `json.local` is `Some(value)` and `serde_json::from_value::<LocalState>(value)` succeeds, `apply_local(state.remap(&map))`.
- [ ] **Step 6: Test that an idle launch leaves both files byte-identical.** Put it in `session_sync.rs` tests, using the no-window path of Task 5 Step 6:
```rust
#[test]
fn an_idle_open_and_close_writes_nothing() {
    let dir = crate::store::testutil::temp_dir("idle-app");
    let path = dir.join("session.mnf");
    {
        let mut app = crate::App::default();
        app.journal = Some(crate::store::journal::Journal::open(&path, ActorId::random()).unwrap());
        let id = app.alloc_node_id();
        app.insert_note_at(id, egui::vec2(3.0, 4.0));
        app.close_store();
    }
    let before = (std::fs::read(&path).unwrap(), std::fs::read(crate::store::local::path_for(&path)).unwrap());
    let mut app = crate::App::default();
    app.journal = Some(crate::store::journal::Journal::open(&path, ActorId::random()).unwrap());
    assert!(app.restore_session());
    app.close_store();
    let after = (std::fs::read(&path).unwrap(), std::fs::read(crate::store::local::path_for(&path)).unwrap());
    assert!(before == after, "an idle launch rewrote the session or its personal state");
}
```
- [ ] **Step 7: Run the tests, clippy, and the smoke test** after a rebuild. All pass.
- [ ] **Step 8: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "Personal state in session.mnf.local: settled positions, view, titles, written when changed" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Recently Closed holds store snapshots and survives restarts

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - `ClosedEntry`/`ClosedNode`/`ClosedPiece` (1020-1052);
  - `closed_entry` (1088-1119);
  - the push at 2032-2034;
  - `reopen_closed` (2045-2106);
  - the test at 5679.
- Modify: `popups.rs:205-254` and `graph_ui.rs:897` (the dropdown's labels read `ClosedPiece`).
- Modify: `session_sync.rs` (`local_state`/`apply_local` carry `closed`).

**Interfaces:**
- Produces: `struct ClosedEntry { doc: store::json::JsonDoc }`. `doc.nodes[0]` is the closed node, followed by every member at any depth in draw order. `doc.local` is `Some({"zoom": {label: f64}})` when a tab's zoom differs from the default.
- Produces: `fn closed_entry(app: &App, id: &str) -> Option<ClosedEntry>`, a method or free fn over `shared_nodes()`.
- Produces: `App::reopen_closed(&mut self, entry: ClosedEntry)`, via `json::import` and `materialize`.

- [ ] **Step 1: Rewrite the test first.** Adapt `closed_entry_snapshots_the_whole_subtree` (main.rs:5679). The keys move from `grp-0`… to store ids: build them with `NodeId { actor: ActorId(1), counter: n }.to_string()`. Assert on `entry.doc.nodes` labels and kinds:
  - root first, then members in draw order;
  - the member's `group` field names its group's label;
  - a JSON leaf (key `Some`) outside a group gives `None`;
  - a blank note gives `None`.

  These are today's selection rules; keep them exactly. Run it: FAIL (the type differs).
- [ ] **Step 2: Implement.**
  - `closed_entry` keeps today's selection logic (which ids, which order, which yield `None`). Each node's representation becomes `store::json::node_json(&id, shared.kind, &shared.fields, shared.group)` from `shared_nodes()`. An in-progress download isn't in `shared_nodes()`, so it drops out, matching today's `ClosedPiece` having no download variant.
  - The root's group is written as `None`: reopen places it at the current level, as today.
  - Tab zoom goes into `local`: `{"zoom": {"<label>": 1.25}}`.
  - `reopen_closed`:
    - `store::json::import(&entry.doc, || self.alloc_node_id().parse().unwrap())`;
    - root first: its group is `self.current_group`, and its pos stays the entry's pos (today's reopen uses `ClosedNode.pos`; keep that);
    - then members, via `materialize` with `SpawnPurpose::New` (keep whatever purpose today's `reopen_closed` uses for tabs);
    - apply each zoom from `local` once the tab spawns, through `pending_local`;
    - end with `self.save_session()` as today (main.rs:2106).
  - Every `self.closed.push`/`remove`/`pop` sets `self.closed_changed_at = Some(Instant::now())`: the push at 2032, `reopen_closed_shortcut` at 4740, and the popup's remove at popups.rs:205-254.
  - `local_state()` writes `closed: self.closed.iter().map(|e| e.doc.clone()).collect()`. `apply_local` restores `self.closed` from it, truncated to the last `MAX_CLOSED`.
  - The dropdown labels (popups.rs/graph_ui.rs) read `ClosedPiece` today. Derive the same text from `entry.doc.nodes[0]`: its `kind`, plus `name` for a group, `url` for a tab, `text` for a note, `path` for files.
- [ ] **Step 3: Run the tests and clippy.** Pass.
- [ ] **Step 4: Headless check.** Commands: `NEWNOTE`, `WAITTABS`, `EXPORTSTATE {repo}\target\plan-b\t8-a.json`, then close the note:
  - check whether a debug command closes a focused note (`CLOSETAB` closes tabs);
  - if none does, use a tab: `NEWTAB`, `WAITTABS`, `CLOSETAB`;
  - then `SLEEP 3` so the 2.5 s write fires, and `QUIT`.

  Relaunch on the same profile (`APPDATA_DIR`) with `REOPENTAB`, `WAITTABS` and `EXPORTSTATE`. The reopened tab must exist, under a new id.
- [ ] **Step 5: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "Recently Closed holds store snapshots and is saved with personal state" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: JSON snapshots everywhere: session and group export/import, named sessions

**Files:**
- Modify: `crates/manifold/src/event_loop.rs:756-857` (EXPORTSESSION, IMPORTSESSION, CONFIRMIMPORT, EXPORTGROUP, IMPORTGROUP)
- Modify: `crates/manifold/src/main.rs`:
  - `export_group`/`import_group_export` (5056-5230);
  - the popup "Export Group..." (5323);
  - `load_named_session` (2552);
  - `pending_import`'s type;
  - the Export/Import Session dialog handlers (grep `rfd::FileDialog`).
- Modify: `crates/manifold/src/session.rs`: the `Session*` structs go; named paths become `.mnf`.
- Modify: `settings_page.rs` and `popups.rs` (named-session list, Save As, Delete).
- Delete: `crates/manifold/src/group_export.rs`, and `mod group_export;` in main.rs.
- Modify: `scripts/headless-smoke-test.sh` (the `pair` and `nine` fixtures at lines ~298-360).

**Interfaces:**
- Consumes: `json::doc_to_json`, `json::subtree_to_json`, `json::import`, `JsonDoc`, `Journal::save_copy`, `Journal::read`, `local::read`; Task 5's `replace_running_session`; Task 7's `LocalState`.
- Produces:
  - `App::export_session_json(&mut self) -> Option<JsonDoc>` (saves first, adds `local`);
  - `App::export_group_json(&mut self, group: &str) -> Option<JsonDoc>`;
  - `App::import_group_json(&mut self, json: &JsonDoc)`;
  - `session::named_session_path(name) -> Option<PathBuf>` (`sessions/<name>.mnf`), plus `list_named()` and `delete_named()` over `.mnf`.

- [ ] **Step 1: Session export and import.**
```rust
impl App {
    /// The session as a JSON snapshot (spec "JSON snapshots"), saved first
    /// so it matches the file, with personal state as `local`.
    pub(crate) fn export_session_json(&mut self) -> Option<store::json::JsonDoc> {
        self.save_session();
        let doc = self.journal.as_ref()?.doc();
        let mut json = store::json::doc_to_json(doc);
        json.local = Some(serde_json::to_value(self.local_state()).expect("personal state serializes"));
        Some(json)
    }
}
```
  - `EXPORTSESSION path` and the Export Session dialog write `serde_json::to_string_pretty(&json)` to the path, toasting on error like today's `save_to`.
  - `IMPORTSESSION path` and the Import dialog read a `JsonDoc` (`serde_json::from_str`), with errors toasted, into `pending_import: Option<JsonDoc>`.
  - `CONFIRMIMPORT` calls `replace_running_session(&json)`.
  - With no journal, `export_session_json` returns `None`. Toast "No session file", which only happens when `%APPDATA%` is unset.
- [ ] **Step 2: Group export and import.** `export_group_json(group)`: `save_session()`, then `json::subtree_to_json(journal.doc(), group.parse().ok()?)`. Only saved nodes are exported, which matches the old export; synthetic `from_json` groups are now saved, so they export too. `import_group_json(json)` keeps today's placement:
  - the root goes at the current level at a jitter position (read `import_group_export` at 5144 for how today picks it: `spawn_jitter` around the viewport center);
  - members keep their file positions relative to the root;
  - materialize with fresh ids from `json::import`;
  - the root's group is `self.current_group`;
  - end with `save_session()`.

  Popup "Export Group..." and `EXPORTGROUP` write the JSON; `IMPORTGROUP` reads it. Delete `group_export.rs` and every `group_export::` use.
- [ ] **Step 3: Named sessions.**
  - Save As: `journal.save_copy(&named_session_path(name)?, Some(&self.local_state().to_bytes()))`, toasting on error. The path is never the journal's own, since `sessions/` differs from `manifold/`.
  - Open: read `std::fs::read(path)` into `Journal::read(&bytes)` → `Doc`. Personal state: `store::local::read(path, doc.id)` → `LocalState::from_bytes`. Build `let mut json = json::doc_to_json(&doc); json.local = local.map(|l| serde_json::to_value(l).unwrap());`, then `replace_running_session(&json)`.
  - A read error is toasted. `ReadOnly` (a newer version) still reads, because `Journal::read` doesn't refuse it; check, and if it does refuse, toast "written by a newer manifold".
  - `list_named` lists `*.mnf` stems. `delete_named` removes `<name>.mnf`, `<name>.mnf.local` and `<name>.mnf.prev`.
  - Delete the rest of `session.rs`'s `Session*` structs, `load_from`, `save_to` and their tests. `json_file` stays for settings and bookmarks.
- [ ] **Step 4: Rewrite the smoke-test fixtures** in the snapshot shape. `pair`:
```bash
cat >"$out_dir/pair.group.json" <<'EOF'
{"nodes": [
 {"id": "g", "kind": "group", "name": "Pair"},
 {"id": "a", "kind": "tab", "url": "manifold://welcome", "engine": "webview2", "pos": [-150.0, 0.0], "group": "g"},
 {"id": "b", "kind": "tab", "url": "manifold://welcome", "engine": "webview2", "pos": [150.0, 0.0], "group": "g"}]}
EOF
```
  `nine` (inside its Python heredoc):
```python
import json, sys
tab = lambda i: {"id": f"t{i}", "kind": "tab", "url": "manifold://welcome", "engine": "webview2", "group": "g"}
json.dump({"nodes": [{"id": "g", "kind": "group", "name": "Nine"}] + [tab(i) for i in range(9)]}, open(sys.argv[1], "w"))
```
  That Python block runs as `python - "$out_dir/nine.group.json" <<'EOF'`, a heredoc on stdin with a file argument, which is the smoke test's existing and working pattern. Don't change how it's invoked.
- [ ] **Step 5: Tests.** Add to `session_sync.rs` tests, again without a window where possible:
```rust
#[test]
fn a_group_export_imports_as_fresh_nodes() {
    let dir = crate::store::testutil::temp_dir("group-json");
    let mut app = crate::App::default();
    app.journal = Some(crate::store::journal::Journal::open(&dir.join("session.mnf"), ActorId::random()).unwrap());
    let g = app.alloc_node_id();
    app.insert_group_at(g.clone(), egui::vec2(0.0, 0.0));
    let n = app.alloc_node_id();
    app.insert_note_at(n.clone(), egui::vec2(10.0, 0.0));
    app.nodes.get_mut(&n).unwrap().group = Some(g.clone());
    let json = app.export_group_json(&g).unwrap();
    assert_eq!(json.nodes.len(), 2);
    assert_eq!(json.nodes[0].kind, "group");
    app.import_group_json(&json);
    assert_eq!(app.shared_nodes().len(), 4, "the import added a second group and note");
}
```
- [ ] **Step 6: Run the tests, clippy, and the smoke test** after a rebuild. All pass. `grep -rn "group_export\|SessionTab\|session::Session\b" crates/manifold/src` → no hits.
- [ ] **Step 7: Commit.**
```bash
cargo fmt --all
git add -A crates/manifold/src scripts/headless-smoke-test.sh
git commit -m "Session and group export/import and named sessions use the store's JSON snapshots; group_export.rs goes" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Tell the user about repairs, save failures and read-only files

**Files:**
- Modify: `crates/manifold/src/session_sync.rs` (`store_error`, `take_store_recovery`)

**Interfaces:**
- Consumes: `Recovery` fields `damaged`, `skipped_frames`, `header_damaged`, `from_prev`, `new_document` and `saved`, and `JournalError` variants.
- Produces: the user-facing notice strings; `App::last_store_notice: Option<String>`.

- [ ] **Step 1: Write the notice text as a pure function, with tests.**
```rust
/// What to tell the user about a repair (contract "Repairs").
pub(crate) fn recovery_notice(r: &Recovery) -> String {
    let what = if r.new_document {
        "The session file's start was lost; what survived was kept as a new session".to_owned()
    } else if r.from_prev {
        "The session file was damaged and rebuilt from its previous copy".to_owned()
    } else {
        let n = r.damaged.len().max(r.skipped_frames);
        format!("The session file was damaged; {n} damaged part{} skipped", if n == 1 { " was" } else { "s were" })
    };
    match &r.saved {
        Some(path) => format!("{what}. The original is saved at {}", path.display()),
        None => format!("{what}. The original could not be saved, so nothing new is written until it can be"),
    }
}

#[test]
fn recovery_notices_say_what_was_lost_and_where_the_original_is() {
    let r = Recovery { damaged: vec![(10, 20)], saved: Some("C:/x/session.mnf.damaged-1".into()), ..Recovery::default() };
    assert_eq!(recovery_notice(&r), "The session file was damaged; 1 damaged part was skipped. The original is saved at C:/x/session.mnf.damaged-1");
    let r = Recovery { from_prev: true, saved: None, ..Recovery::default() };
    assert!(recovery_notice(&r).contains("rebuilt from its previous copy"));
}
```
- [ ] **Step 2: Wire it.**
  - `take_store_recovery` turns `journal.take_recovery()` into `Some(r)`, then `notice(recovery_notice(&r))`.
  - `store_error(err)` calls `notice(format!("Session not saved: {err}"))`, except `JournalError::ReadOnly` → `notice("This session was written by a newer manifold; changes aren't saved")`.
  - After `begin` in `save_session`, if `txn.read_only()`, give the same read-only notice.
  - `notice(msg)`: if `self.last_store_notice.as_deref() == Some(&msg)`, return. Otherwise `eprintln!("session: {msg}")`, `queue_toast(msg.clone())`, and remember it. This dedupes identical repeated notices (contract: a `BadCrc` tail in a read-only file is reported after every reload).
  - A later successful commit clears `last_store_notice`, so a new failure after a recovery is shown again.
- [ ] **Step 3: Run the tests and clippy.** Pass.
- [ ] **Step 4: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src/session_sync.rs
git commit -m "session_sync: say what a repair lost and where the original is; one notice per distinct failure" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: CLI `--dump`, `--check` and `--import-json`; drive.sh `SEED`

**Files:**
- Create: `crates/manifold/src/cli.rs`
- Modify: `crates/manifold/src/main.rs` (`mod cli;`, and a call at the top of `main` after `crash_report::install()`)
- Modify: `scripts/drive.sh` (the `SEED` handling and header)

**Interfaces:**
- Produces: `cli::run(args: &[String]) -> Option<i32>`. It returns `None` when no CLI flag is present, so the GUI starts; otherwise the exit code.

- [ ] **Step 1: Write the module with tests.**
```rust
//! Command-line tools for session files (spec "JSON snapshots", CLI):
//! `--dump <file.mnf>` prints frames and the document as JSON, `--check
//! <file.mnf>` reports damage and a torn tail, and `--import-json <in>
//! --out <file.mnf>` builds a session file from a JSON snapshot (used by
//! drive.sh's `SEED`).

use std::path::Path;

use crate::local_state::LocalState;
use crate::store::journal::{inspect, Journal};
use crate::store::json::{self, JsonDoc};
use crate::store::model::ActorId;

/// Runs the tool the arguments name: `None` when they name none.
pub(crate) fn run(args: &[String]) -> Option<i32> {
    let arg = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let result = if let Some(file) = arg("--dump") {
        dump(Path::new(file))
    } else if let Some(file) = arg("--check") {
        check(Path::new(file))
    } else if let Some(input) = arg("--import-json") {
        match arg("--out") {
            Some(out) => import_json(Path::new(input), Path::new(out)),
            None => Err("--import-json needs --out <file.mnf>".into()),
        }
    } else {
        return None;
    };
    Some(match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("manifold: {err}");
            2
        }
    })
}

fn dump(path: &Path) -> Result<i32, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let info = inspect(&bytes).map_err(|e| e.to_string())?;
    println!("version {} generation {} prev_generation {}", info.version, info.generation, info.prev_generation);
    for (offset, ty, size) in &info.frames {
        println!("frame at {offset}: type {ty}, {size} bytes");
    }
    let doc = Journal::read(&bytes).map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&json::doc_to_json(&doc)).expect("JSON serializes"));
    Ok(0)
}

/// Exit status 0 when clean, 1 when damaged or torn.
fn check(path: &Path) -> Result<i32, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let info = inspect(&bytes).map_err(|e| e.to_string())?;
    if info.header_damaged {
        println!("header damaged");
    }
    for (start, end) in &info.damaged {
        println!("damaged: bytes {start}..{end}");
    }
    if let Some((at, why)) = &info.torn {
        println!("torn tail at {at}: {why:?}");
    }
    let clean = !info.header_damaged && info.damaged.is_empty() && info.torn.is_none();
    println!("{}", if clean { "clean" } else { "damaged" });
    Ok(if clean { 0 } else { 1 })
}

fn import_json(input: &Path, out: &Path) -> Result<i32, String> {
    if out.exists() {
        return Err(format!("{} exists; not overwriting it", out.display()));
    }
    let text = std::fs::read_to_string(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let json: JsonDoc = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut journal = Journal::open(out, ActorId::random()).map_err(|e| e.to_string())?;
    let mut txn = journal.begin().map_err(|e| e.to_string())?;
    let imported = json::import(&json, || txn.next_id())?;
    txn.commit(json::creates(&imported)).map_err(|e| e.to_string())?;
    if let Some(local) = json.local.as_ref().and_then(|v| serde_json::from_value::<LocalState>(v.clone()).ok()) {
        let map = json.nodes.iter().zip(&imported).map(|(n, i)| (n.id.clone(), i.id.to_string())).collect();
        journal.write_local(&local.remap(&map).to_bytes()).map_err(|e| e.to_string())?;
    }
    journal.close().map_err(|e| e.to_string())?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_json_builds_a_clean_file_that_dumps_back() {
        let dir = crate::store::testutil::temp_dir("cli");
        let input = dir.join("seed.json");
        std::fs::write(&input, r#"{"nodes":[{"id":"g","kind":"group","name":"G"},{"id":"n","kind":"note","text":"hi","group":"g"}]}"#).unwrap();
        let out = dir.join("session.mnf");
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(run(&args(&["--import-json", input.to_str().unwrap(), "--out", out.to_str().unwrap()])), Some(0));
        assert_eq!(run(&args(&["--check", out.to_str().unwrap()])), Some(0));
        let doc = Journal::read(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(doc.live().count(), 2);
        assert_eq!(run(&args(&["--import-json", input.to_str().unwrap(), "--out", out.to_str().unwrap()])), Some(2), "never overwrites");
        assert_eq!(run(&args(&["--headless"])), None);
    }
}
```
  `Stop` must derive `Debug` for `{why:?}`; it does if `inspect`'s tests print it. If not, add `#[derive(Debug)]`.
- [ ] **Step 2: Call it.** First thing in `main` after `crash_report::install();`:
```rust
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }
```
  Put it before the DPI call if that call has side effects a CLI run shouldn't have. It doesn't matter much, but CLI first is cleaner, so move the `SetProcessDpiAwarenessContext` block below it.
- [ ] **Step 3: drive.sh.** In the `SEED` block:
  - copy every `*.json` except `session.json` as today;
  - then, if `$SEED/session.json` exists, run `"$exe" --import-json "$(cygpath -w "$SEED/session.json")" --out "$(cygpath -w "$D/appdata/manifold/session.mnf")"` and fail the script if it fails. Use drive.sh's existing variable for the exe path.

  Update the header: "`SEED=<dir>`: copy `<dir>/*.json` (settings.json, bookmarks.json, …) into the isolated profile. `<dir>/session.json`, a JSON snapshot (`{"nodes": […]}`, see `store/json.rs`), becomes the profile's `session.mnf`."
- [ ] **Step 4: Run the tests and clippy.** Rebuild, then check `target/debug/manifold.exe --check` on the Task 5 profile (if still kept) → `clean`.
- [ ] **Step 5: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src/cli.rs crates/manifold/src/main.rs scripts/drive.sh
git commit -m "CLI --dump/--check/--import-json; drive.sh SEED builds session.mnf from a JSON snapshot" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Browsing history in `history.json`

**Files:**
- Create: `crates/manifold/src/history.rs`, mirroring `bookmarks.rs` (its path helper, `load` via `json_file::load_or_move_aside`, `save`)
- Modify: `crates/manifold/src/main.rs`:
  - `HistoryEntry` gets `#[derive(Serialize, Deserialize, Clone)]` and moves into `history.rs`;
  - `App::history` loads in `App::default` beside `bookmarks`;
  - add `history_dirty: bool`, set wherever an entry is pushed (main.rs:2121-2126, event_loop.rs:1144-1147) and where the popup clears history (popups.rs:451).
- Modify: `session_sync.rs`: in `save_session` (after the store commit) and in `close_store`, `if self.history_dirty { history::save(&self.history); self.history_dirty = false; }`.

**Interfaces:**
- Produces: `history::load() -> Vec<HistoryEntry>`, `history::save(&[HistoryEntry])`, and `history::path() -> Option<PathBuf>` (`%APPDATA%\manifold\history.json`).

- [ ] **Step 1: Write the failing test** (in `history.rs`):
```rust
#[test]
fn entries_round_trip_through_json() {
    let entries = vec![HistoryEntry { title: "A".into(), url: "https://a.example/".into() }];
    let text = serde_json::to_string(&entries).unwrap();
    let back: Vec<HistoryEntry> = serde_json::from_str(&text).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].url, "https://a.example/");
}
```
- [ ] **Step 2: Implement** by copying `bookmarks.rs`'s shape, trimmed to `load`/`save`/`path` (no import or export: YAGNI). Keep `MAX_HISTORY_ENTRIES` where it is. `load` truncates to it.
- [ ] **Step 3: Run the tests and clippy.** Pass.
- [ ] **Step 4: Commit.**
```bash
cargo fmt --all
git add crates/manifold/src
git commit -m "Browsing history is saved in history.json" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Headless tests: two instances, a compaction during the other's run, a kill

**Files:**
- Create: `scripts/two-instance-test.sh`
- Modify: `README.md` (the headless tests list)

**Interfaces:**
- Consumes: `drive.sh`'s isolation pattern (`APPDATA` under `target/`), `--import-json`, and the debug commands `NEWNOTE`, `IMPORTGROUP`, `EXPORTSESSION`, `WAITTABS`, `SLEEP` and `QUIT`.

- [ ] **Step 1: Write the script.** It is self-contained. Model the coprocess plumbing on `headless-smoke-test.sh` (read its `send` and startup lines ~1-120 first) and reuse its helpers by copying them; don't source it.
```bash
#!/usr/bin/env bash
# Usage: scripts/two-instance-test.sh
#
# Two headless instances on one isolated APPDATA (spec "Testing",
# Headless). Never touches the real %APPDATA%\manifold. Build first:
# cargo build -j 12 -p manifold.
#   1. A imports a group, B adds a note: both EXPORTSESSION node sets agree.
#   2. A imports enough notes to compact while B stays open; B edits
#      after: both agree and nothing is lost.
#   3. A adds a note and is killed; a fresh instance still has it.
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
profile="$repo_root/target/two-instance-appdata-$$"
out="$repo_root/target/two-instance-out-$$"
mkdir -p "$profile/manifold" "$out"
trap 'rm -rf "$profile"' EXIT
export APPDATA="$(cygpath -w "$profile")"
echo '{"has_shown_first_launch_demo": true}' >"$profile/manifold/settings.json"
win() { cygpath -w "$1"; }

coproc A { "$exe" --headless; }
coproc B { "$exe" --headless; }
sa() { echo "$1" >&"${A[1]}"; }
sb() { echo "$1" >&"${B[1]}"; }
wait_file() { for _ in $(seq 100); do [ -s "$1" ] && return 0; sleep 0.2; done; echo "FAIL: $1 never appeared"; exit 1; }

# Compares two EXPORTSESSION files' shared nodes (ids, kinds, fields;
# `local` is personal and may differ).
same_nodes() {
    python -P "$repo_root/scripts/compare_sessions.py" "$1" "$2" </dev/null
}
```
  `compare_sessions.py` is a new small script beside it, run as a file per the Global Constraints:
```python
import json, sys
a, b = (json.load(open(p))["nodes"] for p in sys.argv[1:3])
key = lambda nodes: sorted(json.dumps(n, sort_keys=True) for n in nodes)
if key(a) != key(b):
    print("FAIL: sessions differ", sys.argv[1:3]); sys.exit(1)
print("ok:", len(a), "nodes")
```
  Then the three scenarios. For each: send commands, `SLEEP 1` so the other instance's per-frame `poll_store` picks up the change (each debug command draws a frame; `SLEEP` draws none, so follow it with a no-op that draws, like `EXPORTSTATE` to a scratch file), then `EXPORTSESSION` from both and `same_nodes`.
  - **Scenario 1:** `sa "IMPORTGROUP $(win "$out/pair.json")"` using Task 9's `pair` fixture format, `sa WAITTABS`, then `sb NEWNOTE`. Export both; `same_nodes`. Also assert with Python that the note and both tabs are in A's export.
  - **Scenario 2:** write `$out/big.json`, a group with 200 notes of 500 characters each (≈100 KB of ops, past `COMPACT_MIN` and twice the snapshot). `sa "IMPORTGROUP …big.json"`, then `sb NEWNOTE`. Export both; `same_nodes`. Assert `$profile/manifold/session.mnf.prev` exists (A compacted) and that B's export holds all 200 notes plus its own.
  - **Scenario 3:** `sa NEWNOTE`, `sa "EXPORTSESSION …a3.json"`, `wait_file`. Kill A with `taskkill //F //PID "$A_PID"`, which is TerminateProcess, not QUIT. `sb QUIT` and wait for both. Start C on the same profile with `EXPORTSESSION` and `QUIT`. The note A added (its id from `a3.json`) must be in C's export.

  End with `echo "two-instance test: ok"`.
- [ ] **Step 2: Run it** after `cargo build -j 12 -p manifold`: `scripts/two-instance-test.sh` → `ok` for all three. If scenario 1 flakes on timing, poll `EXPORTSESSION` until two consecutive exports from B agree with A's, up to 10 s, rather than lengthening a fixed sleep.
- [ ] **Step 3: README.** In the headless-tests section, add `scripts/two-instance-test.sh` with its three scenarios. Note that a kill is TerminateProcess, not power loss.
- [ ] **Step 4: Commit.**
```bash
git add scripts/two-instance-test.sh scripts/compare_sessions.py README.md
git commit -m "Headless test: two instances on one profile, a compaction mid-run, and a kill" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Cleanup and docs

**Files:**
- Modify: `crates/manifold/src/store/mod.rs` (remove `#![allow(dead_code)]` and its comment; fix or justify each warning that appears)
- Modify: `crates/manifold/src/session_sync.rs` (remove Task 3's `cfg_attr` allow)
- Modify: doc comments that still say `session.json` (`grep -rn "session\.json" crates/manifold/src`: card.rs:995, debug_control.rs:193/426, json_file.rs:1, main.rs:1631, node.rs:177/207, and others). Point each at `session.mnf` or personal state, whichever is true now.
- Modify: `README.md`: the session persistence description (files, the two-instance merge replacing the conflict toast, SEED, CLI).
- Modify: `docs/superpowers/plans/2026-09-26-session-store-app-integration.md`: add an Outcome section.

- [ ] **Step 1: Remove the allows and fix warnings.** `cargo clippy -j 12 -p manifold -p ipc-protocol --all-targets -- -D warnings`. Store items still unused (for example `Journal::changed_on_disk`, if poll uses something else) get either a use or deletion. An item kept only for sub-project 2 gets `#[allow(dead_code)] // sub-project 2: <what uses it>`.
- [ ] **Step 2: Docs sweep.** Fix every stale `session.json` mention. In README, describe:
  - `session.mnf` (shared, merged across instances), `session.mnf.local` (personal), `session.mnf.prev` (last compaction), and damaged-copy files;
  - `history.json`;
  - that an old `session.json` is ignored;
  - the CLI tools.
- [ ] **Step 3: Full verification.**
  - `cargo test -q -j 12 -p manifold` passes;
  - clippy is clean;
  - `cargo build -j 12 -p manifold && scripts/headless-smoke-test.sh && scripts/two-instance-test.sh` all pass;
  - write the counts into the Outcome.
- [ ] **Step 4: Outcome section.** Record the commit range, the test counts, and any ruling made during execution. List deferred items:
  - remote `pos` easing;
  - per-level cameras;
  - the url cause from engines (needed before sync);
  - the save loop's own cost (`shared_nodes` + `diff`) at 300 and 3000 tabs. The store side was measured before Task 5 (store-core plan, Deferred → Performance: a save's `begin` + commit is 0.14 ms at 300 tabs and 0.4–0.75 ms at 3000).
- [ ] **Step 5: Commit.**
```bash
cargo fmt --all
git add -A crates/manifold/src README.md docs/superpowers/plans/2026-09-26-session-store-app-integration.md
git commit -m "Plan B cleanup: store in use (no dead-code allow), docs describe session.mnf" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Outcome (executed 2026-09-26)

Landed on `session-store-app` as 524a706..6a4fc2d (BASE for Task 14 was `ac9249e`): `cargo test -q -j 12 -p manifold` reports 128 passed, 2 ignored (timing probes), and clippy is clean with no `dead_code` allow anywhere in `store/` or `session_sync.rs`. `cargo build -p manifold`, `scripts/headless-smoke-test.sh` (26 checks, all PASS), and `scripts/two-instance-test.sh` (all 3 scenarios ok) all pass against the freshly built exe.

Task 14 itself removed `store/mod.rs`'s `#![allow(dead_code)]` and resolved every warning it had been hiding:
- `Journal::actor()` — deleted; nothing calls it and nothing in the spec's sub-project 2 notes names a use for it.
- `Journal::read_only()` — `#[cfg(test)]`: only `a_newer_format_version_opens_read_only` calls it (the live path uses `Txn::read_only()`, which was already reachable).
- `Inspect::file_len` — kept and used: `--dump` now prints it alongside version/generation.
- `frame::RETIRED`, `model::RESERVED` — `#[cfg(test)]`: both are guard-rail constants ("never reuse a retired number") whose only consumer is the test that asserts live values never collide with them.
- `Node::message()`, `tlv::Writer::raw()` — `#[cfg(test)]`: `message()`'s only caller is the journal fuzz test's `use_damaged` helper, which pulled `raw()` along as its only other caller (besides a `tlv` test).
- `session_sync.rs`'s Task 3 `cfg_attr` allow was already gone by the time this task ran (removed in an earlier commit on this branch) — nothing left to do there.

No item needed a bare `#[allow(dead_code)] // sub-project 2: …` — everything left over either had a real caller, was deleted, or was test-only and moved under `#[cfg(test)]`.

The docs sweep found the brief's specific line numbers (`card.rs:995`, `debug_control.rs:193/426`, `json_file.rs:1`, `main.rs:1631`, `node.rs:177/207`) already clean — those had been fixed by earlier tasks on this branch, and the brief's snapshot of them was stale. The only surviving stale doc comment was `node.rs:340` ("Not persisted to `session.json`" → "Not persisted to the session store"). `settings_page.rs:341`'s `"session.json"` was left alone: it is the *default filename for Export Session's JSON snapshot*, which is a real, currently-correct `.json` interchange format, not a reference to the old store.

README's session-persistence paragraph was rewritten to describe `session.mnf` / `.mnf.local` / `.mnf.prev` / `.mnf.damaged-<generation-hex>`, that an old `session.json` is left in place and ignored, `sessions/<name>.mnf` for named sessions, `history.json`, Export/Import's JSON snapshot format, and the `--dump`/`--check`/`--import-json` CLI tools. The two-instance-test.sh section from Task 13 was left as written — it was already accurate and needed no extension.

### Final review

The whole-branch review's fix pass (brief: `.superpowers/sdd/2026-09-26-session-store-app-integration/final-fix-brief.md`) fixed:
- **Import Group and Reopen Closed dropped a tab whose spawn failed** (they spawned as `New`; only `Respawn`/`Restore` keep a failed card). The three import loops are now one helper, `App::materialize_imported`, which always spawns `Restore` and collapses toasts; callers pass only the root's level/position and whether a live parent outside the file is kept (Reopen only).
- **A member whose group is outside the file** now lands with the root (the level being viewed), as the doc comments said, instead of at the top level. Import Session is unchanged (no root: such a node goes to the top level).
- **An empty session lost Recently Closed on the next launch**: `restore_session` now restores the closed list (shared `restore_closed`, same filter and cap) and sets `local_written` before returning `false`.
- **Quit or crash while tabs were still spawning lost them**: `App::spawning` is now a map to each in-flight tab's `Shared` (`session_sync::spawn_shared`, built from the `TabSpawn`), and `save_session` creates the ones the store doesn't hold yet (any id the document has, live or tombstone, is only kept). A spawn that fails without a card now saves, so a tab created while in flight doesn't linger.
- `history::load` kept the oldest 200 entries when capping; it now keeps the newest. `export_group_json` returns `None` and toasts when the store doesn't hold the group live. `local_state()` keeps a still-spawning node's `pending_local` entry and a failed card's zoom.

Windowless tests park spawns in a `#[cfg(test)]` `App::test_spawns` (no event loop) so they can fail them through `handle_tab_spawn_result`. Tests: `cargo test -q -j 12 -p manifold` 136 passed, 2 ignored (8 new); clippy clean; `headless-smoke-test.sh` all PASS; `two-instance-test.sh` all 3 scenarios ok. Parked (out of scope): `last_shared` lingering, the resurrection race, the mid-swap engine change, landing at the top level after a remote group removal, and the release-path drag test.

After the merge into `dev`, two follow-ups:
- **A save that didn't commit is retried.** That covers a commit error, a `begin` error, or four rounds of remote frames. Before, local edits waited for the next local action or remote write. Now `save_session` sets `App::save_retry_at` to 1 s later (`SAVE_RETRY`), `poll_store` saves once it's due, and a successful commit clears it. A read-only store isn't retried. The test is `a_failed_commit_keeps_the_explicit_marks`.
- **Atomic writes.** `settings.json`, `bookmarks.json` (including exports) and `history.json` are now written through `json_file::save`: a temp file beside the target, fsynced, then renamed over it (`store::local::replace`). A crash mid-write leaves the old file whole instead of a truncated one that the next launch moves aside. The temp name carries the process id, since instances write these files without a lock. The test is `save_replaces_whole_or_not_at_all`.
- **A url that changes on every load no longer bounces.** On a site that redirects to a fresh url each load (`scripts/nonce_server.py` models a consumed nonce), instance B followed A's url, got a new one, and shared it back. The two instances then navigated each other about 40 times a second. A url change that follows a load the store started (a remote `url` applied in `materialize`, or a `Restore` spawn landing) is now kept local while the tab is loading and for `STORE_LOAD_GRACE` (5 s) after it was last seen loading (`App::store_loads`, `shares_url_change`). A typed navigation clears it. The url stays local, since the store writes `url` only when it is marked. The tests are `url_changes_after_a_store_started_load_stay_local_until_it_settles`, and two-instance scenario 3: before the fix, 283 → 705 requests in 10 s; after it, 4 requests in total.
- **Engines report how a navigation started** (`EngineToHost::NavigationStateChanged::user_started`, scope A: WebView2 and CEF report it; Servo and WebKit send `None` and keep the grace above). `shares_url_change` shares `Some(true)`, never `Some(false)`, and uses the grace for `None`. Each engine labels a navigation when it starts: the host's `Navigate` (initial load included) is `false`, host back/forward `true`, a redirect keeps its navigation's label while that one is under way. A client redirect after the load (WebView2 flags a page's `setTimeout` navigation as redirected) gets its own label. The label is cleared after the load's end is sent. Host labels expire after 2 s, as a `Navigate` can change nothing.
  - CEF reads `RequestHandler::on_before_browse`'s `user_gesture` and `is_redirect`. `Request::transition_type` is avoided: the binding makes it a Rust enum, and a source combined with qualifier flags is no variant of it.
  - WebView2's `IsUserInitiated` is unusable: measured true for every navigation, including a `setTimeout` navigation in a tab that never got input. The engine instead counts a navigation as the user's when it delivered a mouse-down or key-down to the page within 5 s (Chromium's transient-activation lifespan), and a navigation start uses that input up. Too wide only shares one navigation too many: the load it causes elsewhere is a host `Navigate`, never shared back.
  - `navigate_tab` now saves at once. A save writes the live `url`, so the mark used to pick up whatever the page moved to before the next save: a typed `/js` that script-navigated to `/dest` shared `/dest`.
  - Two-instance scenario 4: a link click on `/link` is shared (B asks again for the served-once `/r?n=` A landed on), and a script navigation on `/js` isn't (no `/dest?n=` is asked for twice). Passed three times on WebView2; on CEF only with a per-process cache dir (next point).
  - **The 5 s input window, checked against real patterns** (a two-instance probe on local pages, both engines): a click whose handler fetches for 1.5 s and then navigates is shared. So are a timer navigation 3 s after a click and one 2 s after a key press. CEF's `user_gesture`, Chromium's own transient activation, labels those three the same way, so WebView2's window matches the browser's definition, and both over-share alike: at most one page-started navigation per click or key press, never shared back. Kept.
  - **Same-document url changes** (`history.pushState`/`replaceState`, a new `#fragment`) were the real gap. WebView2 never reported them: neither `NavigationStarting` nor `NavigationCompleted` fires, so a single-page app's tab kept its first url, in the address bar and in the store. CEF reported them via `on_address_change` unlabelled, so the grace shared them, a timer's or a scroll handler's included (the other instance would reload on each). Now WebView2 handles `SourceChanged` (when `IsNewDocument` is false), and CEF's `on_address_change` (main frame only; it used to take subframes' addresses too) labels a change no navigation is under way for. Such a change is labelled like a navigation start: the host's label within 2 s, else input within 5 s. During a load it keeps the load's label, since routers rewrite the url as the page starts. Probe: a click's pushState and a fragment link are shared; a timer's pushState and a router's `replaceState` on load aren't. Scenario 4 checks the single-page-app case (`/spa` in `nonce_server.py`); passed on WebView2, CEF, WebView2.
  - **Checked on real sites** (two live headless instances, both engines): a GitHub folder click (an in-page navigation) and a Wikipedia contents link (`#Safety`) are shared; scrolling afterwards leaves the url alone. No sampled docs site (VitePress, Svelte, GitBook) rewrites its url on scroll. The check found a host race on CEF: typing a url after the GitHub click left the stored url on GitHub. CEF reported the new load as loading while still at the old url, unlabelled, and `navigate_tab` had just cleared the tab's grace entry, so the host shared the old url and the typed one's `Some(false)` never replaced it. Fixed in the host: `navigate_tab` notes the load like a store load (`store_loads` renamed `host_loads`, `note_host_load`, `HOST_LOAD_GRACE`), so unlabelled reports stay local while it loads. For Servo and WebKit this also keeps a typed url's redirects local, which the spec wanted all along. Not reproduced with a local page (one retitling itself every 30 ms), so the two-instance test doesn't cover it: its trigger is CEF's event order on a heavier page.
  - Found on the way, fixed afterwards: every CEF engine process used the default `%LOCALAPPDATA%\CEF\User Data`, so Chromium's process singleton allowed one CEF tab at a time machine-wide; a second one never connected (spawn timed out after 10 s). It also escaped the tests' APPDATA isolation. The host now gives each CEF engine-host a throwaway cache dir under `profile::cef_cache_dir()` (`ENGINE_CACHE_DIR`), so CEF tabs don't share logins. Chosen over one shared CEF process per profile, which would share logins but end one process per tab (README, known limitations). `manifold-engine-cef` also didn't build on its own: `CreateJobObjectW` needs the `windows` crate's `Win32_Security` feature, now added.
- **A restart reopens a tab's live url.** A url change that stays local (a redirect, a script's navigation) used to be lost on restart: the tab reopened at the shared `url` and redid the redirect. `LocalNode::live_url` now records the current url with the shared url it moved from (`LiveUrl { url, from }`, written by `local_state()` when the tab's url differs from `last_shared`). `materialize` opens `live_url` while the stored url is still `from`; once another replica shares a new url, that wins. Tests: `a_restart_reopens_a_tabs_live_url_while_the_shared_url_is_unchanged`, and two-instance scenario 6 (a lone instance types `/js`, whose script moves it to `/dest?n=`, and quits; the next reopens that `/dest`, not `/js`; it fails with the restore disabled). Checked live on CEF: a `/boot` page that script-navigates to `/boot/route?n=…` reopened at the route, with no fresh `/boot` request, and the stored url stayed `/boot`.
- **A typed url no longer flashes back to the old one.** After `navigate_tab`, an engine can still report the page it is leaving (CEF did on GitHub), which set `node.url` back and added a history entry. `App::navigated_from` remembers the url a typed navigation left; `reported_url` ignores reports still at it for 2 s (`NAVIGATE_START_WINDOW`) or until a different url arrives. Test: `a_report_still_at_the_url_a_typed_navigation_left_is_stale`. Checked live on CEF: GitHub → folder click → typed Wikipedia left no stray GitHub entry in history.

### Rulings made during execution (from `progress.md`)

- **Diff/apply contract**: `diff` writes a dangling group as `None` rather than walking to the nearest live ancestor; safe only because `save_session` always applies `remote` before diffing (a caller contract, not enforced in `diff` itself — kept that way to keep `diff` simple).
- **Personal-state "missing/explicit" semantics**: generalized `shared_pos` into a per-node snapshot of every last-*shared* field value; `materialize` applies a remote field only when the store's value differs from that snapshot, so a remote touch to an unrelated field never clobbers local state.
- Also fixed while in the area: `save_session` gained tests against a real `Journal` (including a remote update to an existing node); the Save Now hover text and stale comments were corrected; `export_session_json`/`export_group_json` now toast when the preceding save didn't commit.
- **Drag/positions**: `pos` is marked once at release for every node the drag moved ("drag ends", per the brief), with the filter-at-release simplification applied in the controller instead of clearing `drag_moved` on every dragged frame.
- **Store notices**: `last_store_notice` (one slot) was replaced with `store_notices` (a set, cleared on commit), so two different repeating notices (e.g. read-only and a repair) don't silently overwrite each other; a notice identical to one already shown this uncommitted stretch stays silent (intended).
- **Group import**: `import_group_json` keeps every member's file position untouched (`pos` is local to a node's own immediate group, matching the old per-group export format); a `parent` outside the imported file is dropped, mirroring `replace_running_session`'s existing parent policy.
- **Two-instance test**: instance B was made to wait for A's `.prev` to appear before B edits, so the compaction-mid-run scenario can't race.
- Several one-line/mechanical fixes were applied without a separate re-review pass (a clippy `?`-rewrite, a stray `drag_moved` clear) since they were trivial and covered by the existing test/smoke suite.

### Deferred

From the brief:
- Remote `pos` easing (a remotely-moved node jumps instead of animating into place).
- Per-level cameras (Graph view's camera is one global value, not saved per group).
- The URL cause from engines (redirect vs. user-initiated) — needed before sub-project 2 can narrow `url` to user-started navigations per the spec's own sub-project 2 notes.
- The save loop's own cost (`shared_nodes` + `diff`) at 300 and 3000 tabs was not (re-)measured this task. The store side alone was measured before Task 5 (store-core plan, Deferred → Performance): a save's `begin` + commit is 0.14 ms at 300 tabs and 0.4–0.75 ms at 3000.

From `progress.md`'s own parked items (rare, or explicitly owned by a later/out-of-scope task):
- A one-frame local-delete resurrection; a mid-swap remote engine change getting lost (converges on its own); a spawning tab landing at the top level when its remote group was deleted concurrently.
- `last_shared` entries for locally-deleted nodes linger until teardown (ids are never reused, so this is harmless memory growth only).
- The drag-release marking path has no automated test (`card.rs`'s UI isn't drivable headless); a regression there would be caught only by review or a two-instance test.
- Pre-existing teardown leftovers, and two `FailedTab`/closed-determinism coverage gaps flagged for the final review's fix pass rather than fixed inline.
- The three import loops (`replace_running_session`, `reopen_closed`, `import_group_json`) were near-copies; merged in the final review (see "Final review"), which also fixed `export_group_json`'s empty-doc case.
- `import_keeps_file_order` (Task 2) checks only `imported[0]`, which is weak but true for the cases it covers.

## Edge cases and where each is handled

| Case | Handling | Where |
|---|---|---|
| Engine swap or crash recovery removes a tab's node, then respawns it under the same id | `spawning` keeps the id out of `Delete`s | Ruling 8, Task 5 Step 1 |
| A save during startup restore, with tabs still spawning | Same: spawning ids are kept | Task 5 |
| A live node names a group another instance deleted | `apply_remote` re-homes it (effective group) before the diff; the diff writes a missing group as none | Tasks 3, 5 |
| Another instance keeps appending during our save | Up to 4 apply-and-retry rounds, then `poll_store` retries after `SAVE_RETRY` (1 s) | Task 5 `save_session` |
| Commit fails (disk full, lock I/O) | Marks kept, deduped notice, `poll_store` retries after `SAVE_RETRY`; each save re-diffs from a fresh `begin` | Tasks 5, 10 |
| The file becomes read-only mid-session (a newer build compacted it) | `txn.read_only()` checked after `begin`; notice once; no commits | Tasks 2, 5, 10 |
| Image/JSON file missing at restore | Card kept in its error state, so it isn't deleted from the store | Task 5 Step 3 |
| In-progress download | Not in the store; a completed one is | Task 5 Step 2 |
| Physics jitter | `pos` is explicit (never diffed); personal positions change only past 1 px | Tasks 3, 7 |
| Idle launch | No ops and no personal-state write; `close` doesn't compact a clean file | Task 7 Step 6 |
| `%APPDATA%` unset, or open fails | `journal: None`: the app runs, ids come from `fallback_ids`, saves are no-ops, one toast | Task 4 |
| Import of a snapshot with no tabs | Refused with a toast before teardown, as today | Task 5 |
| Named session from a newer version | Read if `Journal::read` allows it, else a toast | Task 9 |
| Old `session.json` present | Ignored, left in place | Global Constraints |
| Imported `parent` dangling (a JSON import keeps unknown real ids) | The diff writes a missing parent as none | Task 3 (`valid`) |
| A cycle among new groups (never live, but defended) | Create order cuts it: the cycle's first group is created at the top level | Task 3 |
