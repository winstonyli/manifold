---
status: draft
---

# Properties 2: trees in the app (mirror, edits, undo, copies, export) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The app reads and writes a node's `PROPS` tree: the mirror holds it with a version counter, `App::edit_prop` changes it (queued, committed as path `Set`s, also for a node created in the same commit), `UndoKind::Props` undoes it, Duplicate and respawn carry it, JSON export and import round-trip it. No UI reads it yet (labels are plan 3).

**Architecture:** Plan 1 gave the store path-keyed registers. A tree is `Tree = BTreeMap<TreePath, Leaf>` (the live leaves of one field, as `Node::tree` returns them, stamps dropped) and `Trees = BTreeMap<u32, Tree>` (by field, non-empty ones only). `Trees` rides wherever `Fields` rides: `Snapshot.trees`, `Carried.trees`, `Imported.trees`; the mirror stores one `Tree` plus a version per `(node, field)`. A remote refresh replaces the mirror's tree with the document's, then re-applies the edits still queued (decision 2a). Every tree field goes through one table, `TREE_FIELDS`, so plan 4's `BODY` is one more row.

**Tech Stack:** Rust, egui app crate `crates/manifold`, `serde_json` (`preserve_order`), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-24-properties-design.md` §2 (app), §5 phase 2, §6 tests, §8 D2/D4. Plan 1 (store): `docs/superpowers/plans/2026-10-24-properties-1-store.md` (done). Plans 3–4 (labels; inline Json) follow.

## Settled for this plan (user, 2026-10-24)

- **1a** `Trees` travels on `Snapshot` and `Carried`; the mirror keeps `(Tree, version)` per `(node, field)`.
- **2a** A remote refresh overlays the still-queued tree edits on the document's tree.
- **3a** A node created in a commit gets its tree as `Set`s right after its `Create`.
- **4** Undo steps are recorded by the caller at an explicit action (`edit_prop` returns the record; it does not push one). `PropEdit` is the register form; unifying `Revert` into it is parked (T7).

## Rulings made while planning (check them; each is cheap to undo)

- **R1 (refines D2).** The mirror stores the *live leaves* (`Node::tree` minus stamps), not the "children win" resolution. Resolving drops a scalar at `/a` that has a child; if the mirror dropped it, tombstoning the last child locally would leave `/a` hidden while the document shows it again (the known quirk in spec §1.2), and local and remote state would differ. `Mirror::props` therefore gives the live leaves and `tree::resolved` gives what a reader shows. Readers (plan 3) call `resolved`.
- **R2.** `edit_subtree_delete` is not a separate function: `edit_prop(.., None)` writes a tombstone at the path, which hides the whole subtree (spec §1.2), so it is one operation.
- **R3.** `materialize` takes a `&Snapshot` instead of `(ty, fields, group)`, and the tuple `RemoteNode` is deleted (it was a `Snapshot` without a name). One signature change instead of a fifth parameter.
- **R4.** `PROPS` import drops empty containers (`{}`/`[]` sentinels are BODY-only, plan 4) and rejects a non-empty array. Plan 4 adds the BODY dialect to `tree_json`.
- **R5.** Edits to a root path are refused (`debug_assert`): the root register of a tree field is not a leaf the app writes.

## Global Constraints

- `kind: app`. Do not bump the version, edit `CHANGELOG.md`, tag, or push. No migration code.
- No store change in this plan except `store/tree.rs` helpers, `store/tree_json.rs` (new) and `store/json.rs`. The journal `FORMAT_VERSION` (4) and `PROTOCOL_VERSION` do not change. A pre-plan-2 export has no `props` key and imports unchanged; an old build ignores `props` (it is not a `FieldDef`).
- Tree field numbers: `field::PROPS` = 25 (plan 4: `BODY` = 26). Sentinels `EmptyObj`/`EmptyArr` never appear in PROPS.
- Merge rule (plan 1): a register at A with stamp s hides every proper-descendant register with a lower stamp. A local write is newer than everything the replica has seen, so a local edit at A removes A's whole subtree from the mirror.
- Queued tree edits: per `(node, field, path)`, last write wins; a queued edit at A evicts queued edits strictly under A (they are hidden). Commit order is path order, so an ancestor's op is stamped before a descendant's, matching the order the edits were made in.
- Tests never touch the real `%APPDATA%\manifold` (`store::testutil::temp_dir`). Python, if any, only as `python -P f.py < /dev/null`.
- `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass; `.githooks/pre-commit` runs both; never `--no-verify`. Cargo `-j 8`, one cargo per target dir (another session's `cargo.exe` may hold the lock: wait, do not stop it). Files are LF.
- No gate run in this plan (needs `gate all full` before the user tags).
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Local equals remote:** the same edit applied locally and received from the document gives equal mirror trees; a remote refresh while an edit is queued keeps the edit and takes the rest (T3, T4).
2. **Created nodes:** a duplicated or imported card arrives with its tree in the same commit; a node whose `Create` and an edit are queued together does not lose the edit (T4, T6).
3. **Undo guard:** undo skips an edit whose register changed since, undoing a subtree delete restores nested leaves, redo repeats it (T5).
4. **Hostile import:** a file with 100-deep or 20,000-leaf `props`, a `$ref` or `@id` key naming no node in the file, a key starting with `@`, an array, or `props` on an unknown type imports without panic and without foreign ids (T6).
5. **Respawn and forget:** a tab's tree survives a respawn (`Carried`); the version never repeats for the same `(node, field)` after a forget (T3).
6. **Awkward keys in export:** empty, NUL, `@`-leading and non-ASCII keys round-trip through export and import (T6).

## Files

- Modify `store/tree.rs` (T1: `Tree`, `Trees`, `TREE_FIELDS`, `edit`, `subtree`, `resolved`, `Node::trees`; T7: drop the module allow).
- Create `store/tree_json.rs` (T6); modify `store/mod.rs`, `store/json.rs` (T6), `store/model.rs` (T7: drop the `PROPS` allow).
- Modify `session_sync.rs`, `session_sync/remote.rs`, `session_sync/writes.rs`, `session_files.rs`, `view_install/tests.rs` (T2, T3), `mirror.rs` (T3), `session_sync/edits.rs` (T4), `undo.rs` + `undo/tests.rs` (T5), `duplicate.rs` (T6).
- New tests live beside the code (`session_sync/tests/props.rs`, `mirror.rs` tests, `undo/tests.rs`, `store/tree.rs`, `store/json.rs`). Docs: spec, `PARKED.md`, this plan (T7).

---

### Task 1: `Tree`, `Trees`, `TREE_FIELDS` and the tree operations

**Files:** Modify `crates/manifold/src/store/tree.rs`.

**Interfaces:**
- Consumes: `TreePath` (`subtree_end`, `is_ancestor_of`, `is_root`), `Leaf`, `Node::tree(no) -> Vec<(TreePath, Leaf, Stamp)>`, `field::PROPS`.
- Produces (all `pub` in `store::tree`):
  - `type Tree = BTreeMap<TreePath, Leaf>`; `type Trees = BTreeMap<u32, Tree>`.
  - `const TREE_FIELDS: [(u32, &str); 1] = [(field::PROPS, "props")]` — field number and its JSON key.
  - `fn subtree(tree: &Tree, path: &TreePath) -> Vec<(TreePath, Leaf)>` — the leaves at `path` and under it, in path order.
  - `fn edit(tree: &mut Tree, path: &TreePath, leaf: Option<Leaf>) -> Option<Vec<(TreePath, Leaf)>>` — what a local write of `leaf` (`None` = tombstone) at `path` does: removes the subtree, inserts the leaf. Returns the leaves it replaced, or `None` when nothing changed.
  - `fn resolved(tree: &Tree) -> impl Iterator<Item = (&TreePath, &Leaf)>` — "children win": skips a leaf that has a descendant.
  - `Node::trees(&self) -> Trees` — the live leaves of every tree field, non-empty ones only.

- [ ] **Step 1 (tests first).** In `tree.rs`'s `tests` module add (reuse the module's `id`, `key` helpers and the way its other tests put registers in a `Node`, `Node::placeholder()` plus `node.fields.insert(RegKey { field, path }, Reg { stamp, enc })`):

```rust
fn p(segs: &[&str]) -> TreePath {
    TreePath::of(&segs.iter().map(|s| key(s)).collect::<Vec<_>>())
}

#[test]
fn edit_replaces_a_subtree_and_reports_what_it_removed() {
    let mut tree = Tree::new();
    assert_eq!(edit(&mut tree, &p(&["a", "b"]), Some(Leaf::Int(1))), Some(vec![]));
    assert_eq!(edit(&mut tree, &p(&["a", "c"]), Some(Leaf::Int(2))), Some(vec![]));
    assert_eq!(edit(&mut tree, &p(&["z"]), Some(Leaf::Int(9))), Some(vec![]));
    // The same value again is no change.
    assert_eq!(edit(&mut tree, &p(&["z"]), Some(Leaf::Int(9))), None);
    // A write at /a hides /a/b and /a/c.
    let removed = edit(&mut tree, &p(&["a"]), Some(Leaf::Str("s".into()))).unwrap();
    assert_eq!(removed, vec![(p(&["a", "b"]), Leaf::Int(1)), (p(&["a", "c"]), Leaf::Int(2))]);
    assert_eq!(tree.keys().cloned().collect::<Vec<_>>(), vec![p(&["a"]), p(&["z"])]);
    // A tombstone removes the subtree and inserts nothing; one over nothing is no change.
    assert_eq!(edit(&mut tree, &p(&["a"]), None), Some(vec![(p(&["a"]), Leaf::Str("s".into()))]));
    assert_eq!(edit(&mut tree, &p(&["a"]), None), None);
}

#[test]
fn a_key_that_is_a_text_prefix_of_another_is_not_its_ancestor() {
    let mut tree = Tree::new();
    edit(&mut tree, &p(&["ab"]), Some(Leaf::Int(1)));
    edit(&mut tree, &p(&["a"]), None); // must not touch /ab
    assert_eq!(subtree(&tree, &p(&["ab"])), vec![(p(&["ab"]), Leaf::Int(1))]);
}

#[test]
fn resolved_drops_a_leaf_that_has_a_descendant() {
    let mut tree = Tree::new();
    for (path, n) in [(p(&["a"]), 1), (p(&["a", "b"]), 2), (p(&["c"]), 3)] {
        tree.insert(path, Leaf::Int(n));
    }
    let shown: Vec<_> = resolved(&tree).map(|(path, _)| path.clone()).collect();
    assert_eq!(shown, vec![p(&["a", "b"]), p(&["c"])]);
}

#[test]
fn a_node_reports_its_live_leaves_per_tree_field() {
    let mut node = Node::placeholder();
    let stamp = |ms| Stamp { hlc: Hlc { ms, logical: 0 }, actor: ActorId(1) };
    let mut put = |path: TreePath, ms, leaf: Option<Leaf>| {
        let enc = leaf.map_or_else(Vec::new, |l| l.encode());
        node.fields.insert(RegKey { field: field::PROPS, path }, Reg { stamp: stamp(ms), enc });
    };
    put(p(&["a"]), 1, Some(Leaf::Int(1)));
    put(p(&["a", "b"]), 2, Some(Leaf::Int(2)));
    put(p(&["gone"]), 3, None); // a tombstone is not a leaf
    let trees = node.trees();
    assert_eq!(trees.len(), 1);
    assert_eq!(trees[&field::PROPS].len(), 2);
    assert!(Node::placeholder().trees().is_empty());
}
```

  Adjust imports (`Stamp`, `Hlc`, `ActorId`, `Reg`, `field`, `BTreeMap`) to what the module already has. If `TreePath::is_ancestor_of` is not proper-ancestor (check its body at `tree.rs:84`), `resolved` must test `p != q && p.is_ancestor_of(q)`.
- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold store::tree` — Expected: FAIL to compile (`Tree`, `edit`, … not found).
- [ ] **Step 3:** In `tree.rs` above the `tests` module add:

```rust
/// The live leaves of one tree field, as `Node::tree` gives them without their stamps. A scalar and its descendants
/// can both be here (a child written after the scalar); `resolved` is what a reader shows.
pub type Tree = BTreeMap<TreePath, Leaf>;
/// A node's trees by field number; a field with no leaves has no entry.
pub type Trees = BTreeMap<u32, Tree>;

/// The tree fields and their JSON keys (export, import). Plan 4 adds `BODY`.
pub const TREE_FIELDS: [(u32, &str); 1] = [(field::PROPS, "props")];

/// The leaves at `path` and under it, in path order.
pub fn subtree(tree: &Tree, path: &TreePath) -> Vec<(TreePath, Leaf)> {
    tree.range(path.clone()..path.subtree_end()).map(|(p, l)| (p.clone(), l.clone())).collect()
}

/// A local write of `leaf` (`None`: a tombstone) at `path`. It is newer than every register this replica has seen, so
/// it hides the whole subtree at `path`; the leaf then stands alone there. Returns the leaves it replaced, `None`
/// when the tree already shows exactly this.
pub fn edit(tree: &mut Tree, path: &TreePath, leaf: Option<Leaf>) -> Option<Vec<(TreePath, Leaf)>> {
    let before = subtree(tree, path);
    let after: Vec<(TreePath, Leaf)> = leaf.iter().map(|l| (path.clone(), l.clone())).collect();
    if before == after {
        return None;
    }
    for (p, _) in &before {
        tree.remove(p);
    }
    if let Some(leaf) = leaf {
        tree.insert(path.clone(), leaf);
    }
    Some(before)
}

/// What a reader shows: a leaf with a descendant is overridden by it ("children win", spec 1.2). A subtree is one
/// contiguous key range, so a leaf has a descendant exactly when the next key is one.
pub fn resolved(tree: &Tree) -> impl Iterator<Item = (&TreePath, &Leaf)> {
    let mut it = tree.iter().peekable();
    std::iter::from_fn(move || loop {
        let (path, leaf) = it.next()?;
        if it.peek().is_some_and(|(next, _)| path.is_ancestor_of(next)) {
            continue;
        }
        return Some((path, leaf));
    })
}

impl Node {
    /// Every tree field's live leaves (`tree`), stamps dropped, the empty fields left out.
    pub fn trees(&self) -> Trees {
        TREE_FIELDS
            .iter()
            .filter_map(|&(no, _)| {
                let tree: Tree = self.tree(no).into_iter().map(|(path, leaf, _)| (path, leaf)).collect();
                (!tree.is_empty()).then_some((no, tree))
            })
            .collect()
    }
}
```

  Add `use std::collections::BTreeMap;` and `use super::model::field;` if missing. (`TreePath::is_ancestor_of` is strict and `subtree_end` is exclusive, both checked at `tree.rs:84-93`, so `resolved` and `subtree` are right as written.)
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold store::tree` — Expected: all pass (the 8 old and the 4 new).
- [ ] **Step 5:** `cargo fmt --all`, clippy, full `cargo test -j 8 -p manifold --bin manifold`; commit `store: Tree, Trees and the local tree edit` + trailer.

---

### Task 2: `materialize` takes a `Snapshot`; `RemoteNode` goes (R3)

**Files:** Modify `session_sync.rs`, `session_sync/remote.rs`, `session_sync/writes.rs`, `session_files.rs`, `tabs.rs` (type of `remote_in_flight`), `view_install/tests.rs`, and the three test call sites (`grep -rn "\.materialize(\|Snapshot {" crates/manifold/src/session_sync/tests`).

**Interfaces:**
- Consumes: `Snapshot { ty, group, fields }`, `Remote<T>`, `App::materialize(id, ty, &Fields, group, local)`.
- Produces: `Snapshot` gains `pub trees: Trees` (empty everywhere in this task); `App::materialize(&mut self, id: NodeId, snap: &Snapshot, local: Option<&LocalNode>)`; `Remote<Snapshot>` in `remote_in_flight`; `RemoteNode` deleted.

A pure refactor: behaviour and all existing tests unchanged.

- [ ] **Step 1:** `grep -rn "RemoteNode" crates` and list every user. Expected: `session_sync.rs` (the alias), `tabs.rs` (field type), `remote.rs` (`land_remote_in_flight`, the `Remote::Shown((..))` tuples), `writes.rs:434,443`.
- [ ] **Step 2:** Edit `session_sync.rs`: add `use crate::store::tree::Trees;` and to `Snapshot` add

```rust
    /// The node's tree fields (`PROPS`), non-empty ones only. Travels wherever `fields` does.
    pub trees: Trees,
```

  `Snapshot::of` sets `trees: node.trees()`; `spawn_snapshot` and the two `Snapshot { .. }` literals in `writes.rs` (~:327, :331) and `view_install/tests.rs:998` set `trees: Trees::new()`. Delete `pub(crate) type RemoteNode`.
- [ ] **Step 3:** `remote.rs`: `materialize(&mut self, id, snap: &Snapshot, local: Option<&LocalNode>)` with `let (ty, fields, group) = (snap.ty.as_str(), &snap.fields, snap.group);` as its first line (the body is otherwise unchanged); `remote_in_flight.insert(id, Remote::Shown(snap.clone()))`; `land_remote_in_flight` matches `Some(Remote::Shown(snap)) => self.materialize(id, &snap, None)`; `apply_remote` passes `&s` (it destructures `s` already, line ~355). `writes.rs:434`: `if let Some(Remote::Shown(s)) = ...get_mut(&node) { s.fields... }` and `:443`: `s.group = ..` (read the two blocks and keep their logic). `session_files.rs:378`: build `Snapshot { ty: node.ty.clone(), group, fields, trees: Trees::new() }` and call `self.materialize(node.id, &snap, None)`. `session_sync.rs:347,353,366` pass `s` (they hold a `Snapshot` already; for 347, which passes `None` as group, pass `&Snapshot { group: None, ..s.clone() }`). `tabs.rs:40`: `Remote<session_sync::Snapshot>`.
- [ ] **Step 4:** Update the test call sites the compiler lists. `cargo test -j 8 -p manifold --bin manifold` — Expected: the same tests pass as before the task (record the count in the ledger; a refactor adds none).
- [ ] **Step 5:** fmt, clippy, commit `session_sync: materialize takes a Snapshot; RemoteNode removed` + trailer.

---

### Task 3: The mirror holds trees; remote refresh keeps queued edits

**Files:** Modify `mirror.rs`, `session_sync/remote.rs`, `session_sync/writes.rs` (`PendingWrites::queue_tree`/`tree_edits`/`tree_queued`, `App::snapshot`, the `spawn_snapshot` call site), `session_sync.rs` (`Snapshot::of` already done), `session_sync/tests.rs` (helper), new `session_sync/tests/props.rs`.

**Interfaces:**
- Consumes: T1's `Tree`, `Trees`, `TREE_FIELDS`, `edit`, `subtree`, `Leaf`; `Snapshot.trees`.
- Produces:
  - `Mirror::tree(&self, id, no) -> Option<&Tree>`; `Mirror::tree_version(&self, id, no) -> u64` (0 when the node has no leaves in that field; otherwise a value that never repeats for that `(id, no)`); `Mirror::trees_of(&self, id) -> Trees`; `Mirror::set_tree(&mut self, id, no, tree: Tree) -> bool` (changed); `Mirror::edit_tree(&mut self, id, no, path: &TreePath, leaf: Option<Leaf>) -> Option<Vec<(TreePath, Leaf)>>`.
  - `Carried.trees: Trees` (public field, `Default` empty), filled by `Mirror::carried`, applied by `Mirror::seed` in its vacant branch.
  - `PendingWrites::queue_tree(node, field, path: TreePath, enc: Vec<u8>)`, `tree_edits(node, field) -> impl Iterator<Item = (&TreePath, &Vec<u8>)>` (path order), `tree_queued(node, field) -> bool`, `tree_sets() -> impl Iterator<Item = (NodeId, u32, &TreePath, &Vec<u8>)>`. `NodeWrites` gains `trees: BTreeMap<(u32, TreePath), Vec<u8>>`, counted in `is_empty` and cleared in `clear_edits`.
  - `mirror_remote(&mut self, id, fields: &Fields, trees: &Trees, group)` (new parameter), which pushes a tree field's number onto `changed` when the mirror's tree changed.
  - `App::snapshot` fills `trees` from `mirror.trees_of(id)`.

- [ ] **Step 1 (tests first).** In `mirror.rs`'s tests module (pattern: `seed_then_carried_round_trips_and_never_overwrites`, `crate::card_id::testutil::node(n)`):

```rust
fn leaf_at(segs: &[&str]) -> TreePath {
    TreePath::of(&segs.iter().map(|s| Seg::Key((*s).to_owned())).collect::<Vec<_>>())
}

#[test]
fn a_tree_edit_bumps_the_version_and_a_repeat_does_not() {
    let (mut mirror, id) = (Mirror::default(), node(1));
    assert_eq!(mirror.tree_version(id, field::PROPS), 0);
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1)));
    let v1 = mirror.tree_version(id, field::PROPS);
    assert!(v1 > 0);
    assert!(mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1))).is_none());
    assert_eq!(mirror.tree_version(id, field::PROPS), v1);
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), None);
    assert!(mirror.tree(id, field::PROPS).is_none(), "an emptied tree is dropped");
    assert!(mirror.tree_version(id, field::PROPS) > v1);
}

#[test]
fn a_version_never_repeats_after_a_forget() {
    let (mut mirror, id) = (Mirror::default(), node(1));
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1)));
    let before = mirror.tree_version(id, field::PROPS);
    mirror.forget_node(id);
    assert_eq!(mirror.tree_version(id, field::PROPS), 0);
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1)));
    assert!(mirror.tree_version(id, field::PROPS) > before);
}

#[test]
fn carried_trees_round_trip_and_seed_never_overwrites() {
    let (mut mirror, id, other) = (Mirror::default(), node(1), node(2));
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a", "b"]), Some(Leaf::Str("x".into())));
    let carried = mirror.carried(id);
    assert_eq!(carried.trees[&field::PROPS].len(), 1);
    mirror.seed(other, &carried);
    assert_eq!(mirror.trees_of(other), mirror.trees_of(id));
    mirror.edit_tree(other, field::PROPS, &leaf_at(&["a", "b"]), Some(Leaf::Int(2)));
    mirror.seed(other, &carried); // an existing entry stays as it is
    assert_eq!(mirror.tree(other, field::PROPS).unwrap()[&leaf_at(&["a", "b"])], Leaf::Int(2));
}
```

  In `session_sync/tests/props.rs` (new; declare it in the tests module list like the sibling files). Reuse the helpers named in `session_sync/tests.rs`: `app_on(path, actor)`, `add(...)`, `remote_set(other, node, no, value)`, `snapshots(app)`, `shown(&doc)`. Add `remote_set_tree` beside `remote_set`:

```rust
/// Another replica's write of `leaf` (`None`: a tombstone) at `path` in `node`'s `field`.
pub(crate) fn remote_set_tree(other: &mut Journal, node: NodeId, field: u32, path: TreePath, leaf: Option<Leaf>) {
    let enc = leaf.map_or_else(Vec::new, |l| l.encode());
    commit(other, vec![OpBody::Set { node, field, path, enc }]); // use the helper `remote_set` uses to commit
}
```

  Tests (adjust the setup lines to the helpers' real signatures; the assertions are the contract):

```rust
#[test]
fn a_remote_tree_edit_reaches_the_mirror_and_equals_the_document() {
    let mut app = app_on(&dir.join("a.mnf"), ActorId(1));
    let card = add_note(&mut app, "n");           // a committed card
    let mut other = replica_of(&app);             // a second journal on the same history
    remote_set_tree(&mut other, card, field::PROPS, key_path(&["k"]), Some(Leaf::Int(7)));
    app.poll_store();                              // applies the remote change
    assert_eq!(app.doc.mirror.tree(card, field::PROPS).unwrap().len(), 1);
    assert_eq!(snapshots(&app)[&card].trees, shown_trees(&app, card));
}

#[test]
fn a_remote_refresh_keeps_a_queued_edit_and_takes_the_rest() {
    // local: queue /mine; remote: writes /theirs and an older /mine.
    // After the refresh the mirror shows /mine (local) and /theirs.
}

#[test]
fn a_queued_ancestor_edit_evicts_queued_descendants() {
    // queue_tree /a/b, then /a: tree_edits yields only /a; then /a/b again keeps both, /a first.
}
```

  Write the second and third bodies out fully against the real helpers (the first shows the shape). The refresh test queues the edit with `app.doc.writes.queue_tree(..)` and `app.doc.mirror.edit_tree(..)` directly (T4 adds `edit_prop`, which this test then should switch to).
- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold mirror` — Expected: FAIL to compile.
- [ ] **Step 3 (mirror).** In `mirror.rs`: add to `Mirror`

```rust
    /// Each tree field's live leaves (`store::tree`) with the version they were last changed at, by `(node, field)`.
    /// Only non-empty trees have an entry. A version is a tick of `tree_clock`, which no forget rewinds.
    trees: HashMap<(NodeId, u32), (Tree, u64)>,
    tree_clock: u64,
```

  with these methods (and `forget_node` removes `for (no, _) in TREE_FIELDS { self.trees.remove(&(id, no)); }`, `retain` gets `self.trees.retain(|(id, _), _| keep(*id));`; read `retain` at ~:177 and copy how it treats `views`):

```rust
    pub(crate) fn tree(&self, id: NodeId, no: u32) -> Option<&Tree> {
        self.trees.get(&(id, no)).map(|(tree, _)| tree)
    }

    /// 0 when there is no tree; otherwise a number that changes whenever the tree does and is never reused for this
    /// `(id, no)`, so a cache keyed on it needs no content hash.
    pub(crate) fn tree_version(&self, id: NodeId, no: u32) -> u64 {
        self.trees.get(&(id, no)).map_or(0, |(_, version)| *version)
    }

    /// Every non-empty tree of `id`.
    pub(crate) fn trees_of(&self, id: NodeId) -> Trees {
        TREE_FIELDS.iter().filter_map(|&(no, _)| Some((no, self.tree(id, no)?.clone()))).collect()
    }

    /// Replaces `id`'s tree of field `no` (an empty one removes it). True when it differed.
    pub(crate) fn set_tree(&mut self, id: NodeId, no: u32, tree: Tree) -> bool {
        let same = match self.tree(id, no) {
            Some(old) => *old == tree,
            None => tree.is_empty(),
        };
        if same {
            return false;
        }
        self.tree_clock += 1;
        if tree.is_empty() {
            self.trees.remove(&(id, no));
        } else {
            self.trees.insert((id, no), (tree, self.tree_clock));
        }
        true
    }
```

  and

```rust
    /// A local write at `path` (`store::tree::edit`); returns the leaves it replaced, `None` for no change.
    pub(crate) fn edit_tree(&mut self, id: NodeId, no: u32, path: &TreePath, leaf: Option<Leaf>) -> Option<Vec<(TreePath, Leaf)>> {
        let mut tree = self.tree(id, no).cloned().unwrap_or_default();
        let removed = tree::edit(&mut tree, path, leaf)?;
        self.set_tree(id, no, tree);
        Some(removed)
    }
```

  (Cloning the tree per edit is O(leaves); fine for PROPS. Plan 4's per-keystroke body editing may need an in-place variant: note it in `PARKED.md`, T7.) `Carried` gets `pub trees: Trees` (derives already `Default`); `Mirror::carried` sets `trees: self.trees_of(id)` in its struct literal; `seed`'s `Vacant` branch ends with `for (&no, tree) in &carried.trees { self.set_tree(id, no, tree.clone()); }`.
- [ ] **Step 4 (queue).** In `writes.rs`: the `NodeWrites.trees` field and the four `PendingWrites` methods:

```rust
    pub(crate) fn queue_tree(&mut self, node: NodeId, field: u32, path: TreePath, enc: Vec<u8>) {
        let trees = &mut self.record(node).trees;
        // An edit at `path` is newer than anything queued under it, which it hides.
        let hidden: Vec<_> = trees.range((field, path.clone())..(field, path.subtree_end())).map(|(k, _)| k.clone()).collect();
        for key in hidden {
            trees.remove(&key);
        }
        trees.insert((field, path), enc);
    }

    pub(crate) fn tree_edits(&self, node: NodeId, field: u32) -> impl Iterator<Item = (&TreePath, &Vec<u8>)> + '_ {
        self.nodes.get(&node).into_iter().flat_map(move |w| w.trees.range((field, TreePath::default())..).take_while(move |((f, _), _)| *f == field).map(|((_, path), enc)| (path, enc)))
    }

    pub(crate) fn tree_queued(&self, node: NodeId, field: u32) -> bool {
        self.tree_edits(node, field).next().is_some()
    }

    pub(crate) fn tree_sets(&self) -> impl Iterator<Item = (NodeId, u32, &TreePath, &Vec<u8>)> + '_ {
        self.nodes.iter().flat_map(|(&n, w)| w.trees.iter().map(move |((f, path), enc)| (n, *f, path, enc)))
    }
```

  `NodeWrites::is_empty` adds `&& self.trees.is_empty()`; `clear_edits` adds `w.trees.clear();`.
- [ ] **Step 5 (remote).** `mirror_remote(&mut self, id, fields, trees: &Trees, group)`: before the `PARENT` line add

```rust
        // A tree is replaced by the document's, then the edits still waiting for a save go back on top: they commit
        // next and win, and everything else the other replica changed shows now.
        for &(no, _) in &TREE_FIELDS {
            let mut tree = trees.get(&no).cloned().unwrap_or_default();
            for (path, enc) in self.doc.writes.tree_edits(id, no) {
                tree::edit(&mut tree, path, Leaf::decode(enc).ok().flatten());
            }
            if self.doc.mirror.set_tree(id, no, tree) {
                changed.push(no);
            }
        }
```

  Callers: `materialize` passes `&snap.trees` (and `&Trees::new()` in the link branch); `restore_session` and the other `materialize` callers already pass a `Snapshot`. `App::snapshot` (mirror branch) sets `trees: self.doc.mirror.trees_of(id)`; the link branch keeps `Trees::new()`. At the `spawn_snapshot(..)` call site(s) (`grep -n spawn_snapshot`) set the result's `trees` to `self.doc.mirror.trees_of(id)` when the mirror has the node (a respawn), else leave it empty.
- [ ] **Step 6 (debug invariant).** In `check_graph_matches_document` (`writes.rs`, called from `after_commit`), for each shown node with no tree edit queued (`!tree_queued`) assert `mirror.trees_of(id) == node.trees()`; read the function first and follow how it handles the scalar fields (it skips queued ones the same way).
- [ ] **Step 7:** `cargo test -j 8 -p manifold --bin manifold` — Expected: all pass, including the new tests; then fmt, clippy; commit `mirror: trees with a version counter; remote refresh keeps queued edits` + trailer.

---

### Task 4: `edit_prop` and the commit

**Files:** Modify `session_sync/edits.rs`, `undo.rs` (the `PropEdit` struct only), `session_sync/writes.rs` (`build_ops`), `session_sync/tests/props.rs`.

**Interfaces:**
- Consumes: T3's `Mirror::edit_tree`, `PendingWrites::queue_tree`/`tree_sets`, `Snapshot.trees`; `OpBody::Set { node, field, path, enc }`.
- Produces:
  - `pub(crate) struct PropEdit { pub node: NodeId, pub field: u32, pub path: TreePath, pub before: Vec<(TreePath, Leaf)>, pub after: Option<Leaf> }` (in `undo.rs`, `Clone, Debug, PartialEq`).
  - `App::edit_prop(&mut self, id: NodeId, field: u32, path: TreePath, leaf: Option<Leaf>) -> Option<PropEdit>` — the mirror shows it at once and the next save commits it; `None` when the tree already showed exactly this. `leaf == None` deletes the subtree (R2).
  - `build_ops` emits, after the `Create`s, one `Set` per leaf of each created node's `Snapshot.trees`, and, after the scalar `Set`s, one `Set` per queued tree edit of an editable node.

- [ ] **Step 1 (tests first)** in `session_sync/tests/props.rs`:

```rust
#[test]
fn an_edit_shows_at_once_and_commits_as_a_path_set() {
    let mut app = app_on(&path, ActorId(1));
    let card = add_note(&mut app, "n");
    app.edit_prop(card, field::PROPS, key_path(&["a", "b"]), Some(Leaf::Int(1))).unwrap();
    assert_eq!(app.doc.mirror.tree(card, field::PROPS).unwrap().len(), 1);
    assert!(app.edit_prop(card, field::PROPS, key_path(&["a", "b"]), Some(Leaf::Int(1))).is_none(), "a repeat queues nothing");
    app.save_session();
    assert_eq!(doc_trees(&app, card), app.doc.mirror.trees_of(card));
    assert!(!app.doc.writes.tree_queued(card, field::PROPS), "the commit clears the queue");
}

#[test]
fn a_subtree_delete_is_one_tombstone_and_hides_a_late_older_write() {
    // set /a/b and /a/c, commit; edit_prop(/a, None) returns both leaves in `before`; commit;
    // the document's tree for the card is empty and so is the mirror's. (A late OLDER write under a tombstone is
    // covered at the store level by plan 1's `a_hidden_register_stays_hidden_across_a_snapshot`.)
}

#[test]
fn a_card_created_with_a_tree_commits_it_in_the_same_save() {
    let mut app = app_on(&path, ActorId(1));
    let card = new_note_not_yet_saved(&mut app);       // queued Create, not committed
    app.edit_prop(card, field::PROPS, key_path(&["label"]), Some(Leaf::Str("x".into()))).unwrap();
    app.save_session();
    assert_eq!(doc_trees(&app, card)[&field::PROPS].len(), 1);
}

#[test]
fn a_queued_edit_on_a_node_deleted_in_the_same_save_is_dropped() {
    // edit_prop then close/delete the card before saving: the save commits the delete and no Set for it.
}
```

  Fill in the two sketched bodies and the helpers (`doc_trees(app, id)` = `app.journal doc node.trees()`; `key_path`) from what `session_sync/tests.rs` offers; if the Create-then-edit order is awkward, create the node with the same helper `add_note` uses but without saving.
- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold session_sync::tests::props` — Expected: FAIL (no `edit_prop`).
- [ ] **Step 3:** `undo.rs` gets `PropEdit` (above `UndoKind`). `edits.rs`:

```rust
    /// The one writer of a node's tree fields: the leaf at `path` becomes `leaf`, or, for `None`, `path` and
    /// everything under it is deleted (one tombstone hides the subtree, so a subtree delete is the same call). The
    /// mirror shows it at once; the next save commits it as a `Set` at `path`. Returns what an undo needs, or `None`
    /// when the mirror already showed exactly this. The caller records the step (`record_undo(.., UndoKind::Props)`).
    pub(crate) fn edit_prop(
        &mut self,
        id: NodeId,
        field: u32,
        path: TreePath,
        leaf: Option<Leaf>,
    ) -> Option<PropEdit> {
        debug_assert!(TREE_FIELDS.iter().any(|&(no, _)| no == field), "a tree field");
        debug_assert!(!path.is_root(), "a leaf has a path");
        debug_assert!(self.doc.mirror.has(id), "a node the mirror knows");
        let before = self.doc.mirror.edit_tree(id, field, &path, leaf.clone())?;
        let enc = leaf.as_ref().map_or_else(Vec::new, Leaf::encode);
        self.doc.writes.queue_tree(id, field, path.clone(), enc);
        Some(PropEdit { node: id, field, path, before, after: leaf })
    }
```
- [ ] **Step 4:** `build_ops` (`writes.rs`): right after `let created: BTreeSet<NodeId> = new.keys().copied().collect();` add

```rust
    // A created node's tree rides with its `Create`: the snapshot holds the mirror's tree as of this save, edits
    // included, so the queued edits of a created node are dropped below like its scalar ones.
    for (id, snap) in new.iter().filter(|(id, _)| !deleted.contains(id)) {
        for (&field, tree) in &snap.trees {
            for (path, leaf) in tree {
                ops.push(OpBody::Set { node: *id, field, path: path.clone(), enc: leaf.encode() });
            }
        }
    }
```

  and after the `for (node, no, enc) in writes.fields() { .. }` loop:

```rust
    for (node, field, path, enc) in writes.tree_sets() {
        if editable(&node) {
            ops.push(OpBody::Set { node, field, path: path.clone(), enc: enc.clone() });
        }
    }
```

  (Ops of one `Txn` are stamped in push order, so within a field an ancestor's `Set` precedes its descendants'.) Check that `create_ops`' result is named `ops` before `created` is defined; if `new` is moved into `create_ops`, borrow it instead.
- [ ] **Step 5:** `cargo test -j 8 -p manifold --bin manifold` — Expected: all pass. fmt, clippy; commit `session_sync: edit_prop; trees commit with their Create` + trailer.

---

### Task 5: `UndoKind::Props`

**Files:** Modify `undo.rs`, `undo/tests.rs`.

**Interfaces:**
- Consumes: T4's `PropEdit`, `edit_prop`; `tree::subtree`; `Mirror::tree`.
- Produces: `UndoKind::Props(Vec<PropEdit>)`, replayed by `App::replay_props(&[PropEdit], redo) -> (usize, usize)`.

- [ ] **Step 1 (tests first)** in `undo/tests.rs` (helper pattern: `fold(tag)` at the top; see `a_join_is_undone_and_redone` for how a step is recorded and stepped):

```rust
fn prop(app: &mut App, card: NodeId, segs: &[&str], leaf: Option<Leaf>) -> PropEdit {
    app.edit_prop(card, field::PROPS, path_of(segs), leaf).expect("a change")
}

#[test]
fn a_property_edit_is_undone_and_redone() {
    let (mut app, card) = app_with_card();
    let edit = prop(&mut app, card, &["a"], Some(Leaf::Int(1)));
    app.record_undo("Set a".into(), UndoKind::Props(vec![edit]));
    app.step_history(None, false);
    assert!(app.doc.mirror.tree(card, field::PROPS).is_none());
    app.step_history(None, true);
    assert_eq!(app.doc.mirror.tree(card, field::PROPS).unwrap().len(), 1);
}

#[test]
fn undoing_a_subtree_delete_restores_every_nested_leaf() {
    let (mut app, card) = app_with_card();
    prop(&mut app, card, &["a", "b"], Some(Leaf::Int(1)));
    prop(&mut app, card, &["a", "c", "d"], Some(Leaf::Str("x".into())));
    let before = app.doc.mirror.trees_of(card);
    let edit = prop(&mut app, card, &["a"], None);
    assert!(app.doc.mirror.tree(card, field::PROPS).is_none());
    app.record_undo("Delete a".into(), UndoKind::Props(vec![edit]));
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.trees_of(card), before);
    app.step_history(None, true);
    assert!(app.doc.mirror.tree(card, field::PROPS).is_none());
}

#[test]
fn an_undo_skips_an_edit_whose_register_changed_since() {
    let (mut app, card) = app_with_card();
    let edit = prop(&mut app, card, &["a"], Some(Leaf::Int(1)));
    app.record_undo("Set a".into(), UndoKind::Props(vec![edit]));
    prop(&mut app, card, &["a"], Some(Leaf::Int(2))); // changed again (or by another replica)
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.tree(card, field::PROPS).unwrap()[&path_of(&["a"])], Leaf::Int(2));
    // "Nothing to undo" toast, and the entry is dropped (done == 0).
}

#[test]
fn an_undo_of_a_closed_card_does_nothing() {
    // record, close the card (self.nodes no longer holds it), undo: (0, 1), no panic, mirror unchanged.
}
```

  Complete the last body and `app_with_card`/`path_of` with the file's real helpers.
- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold undo` — Expected: FAIL to compile (`UndoKind::Props`).
- [ ] **Step 3:** In `undo.rs` add the variant with its doc comment and the replay:

```rust
    /// Tree edits (`App::edit_prop`): labels, properties, Json. Each part changed the leaves at `path` and under it
    /// from `before` to `after`; undo reverses them newest first, redo repeats them.
    Props(Vec<PropEdit>),
```

```rust
    /// Puts each edit's leaves back (undo) or forward (redo), but only where the mirror still shows exactly what is
    /// being left (the rule of `replay_revert`): a register edited again here or by another replica is skipped, as
    /// is a card closed since. The restore is a tombstone at `path` and a write per arriving leaf, ancestors first,
    /// which commits as the same registers the edit had.
    fn replay_props(&mut self, edits: &[PropEdit], redo: bool) -> (usize, usize) {
        let mut done = 0;
        let ordered: Vec<&PropEdit> = if redo { edits.iter().collect() } else { edits.iter().rev().collect() };
        for e in ordered {
            let after: Vec<(TreePath, Leaf)> = e.after.iter().map(|l| (e.path.clone(), l.clone())).collect();
            let (leaving, arriving) = if redo { (&e.before, &after) } else { (&after, &e.before) };
            let now = self.doc.mirror.tree(e.node, e.field).map(|t| tree::subtree(t, &e.path)).unwrap_or_default();
            if now != *leaving || !self.nodes.contains_key(&e.node) {
                continue;
            }
            self.edit_prop(e.node, e.field, e.path.clone(), None);
            for (path, leaf) in arriving {
                self.edit_prop(e.node, e.field, path.clone(), Some(leaf.clone()));
            }
            done += 1;
        }
        (done, edits.len())
    }
```

  Add `UndoKind::Props(edits) => self.replay_props(edits, redo),` to `replay`. Update the module doc ("drops and folds" → "drops, folds and property edits").
- [ ] **Step 4:** full test run, fmt, clippy; commit `undo: UndoKind::Props` + trailer.

---

### Task 6: Export, import and Duplicate

**Files:** Create `store/tree_json.rs`; modify `store/mod.rs` (declare it), `store/json.rs`, `session_files.rs` (`materialize_imported`), `duplicate.rs`; tests in `store/tree_json.rs`, `store/json.rs`, `session_sync/tests/props.rs`.

**Interfaces:**
- Consumes: `Tree`, `Trees`, `TREE_FIELDS`, `resolved`, `Seg`, `TreePath::{segments, child}`, `Leaf`, `Imported`, `Trust`, `JsonNode`.
- Produces:
  - `tree_json::tree_to_json(&Tree) -> serde_json::Value` (a JSON object of the resolved leaves; `{}` for none) and `tree_json::tree_from_json(&Value, resolve: &dyn Fn(&str) -> Option<NodeId>) -> Result<Tree, String>`; consts `MAX_DEPTH = 64`, `MAX_LEAVES = 10_000`.
  - `store::json::node_json(id, ty, fields, trees: &Trees, group)` (new parameter); `Imported.trees: Trees`; `creates()` emits each created node's tree `Set`s after its `Create`.

**Format.** `"props": { "k": 1, "m": { "n": "x" }, "r": { "$ref": "<id>" }, "@<id>": true }`.
- Object keys are path segments; an object nests. A scalar is `Null`/`Bool`/`Int`/`Float`/`Str`; a JSON number beyond `i64` is `Num` (its text).
- `{"$ref": "<id>"}` (exactly that one key, string value) is `Leaf::Ref`. PROPS only.
- A `Seg::Id` key is written `@<id text>`; a `Seg::Key` that begins with `@` is written with one more `@` in front (`@x` → `@@x`). Reading: `@@rest` is the key `@rest`; `@id` is an id segment; anything else is a key.
- Import runs ids through the file's table like `parent`: a name in the file becomes its fresh id; under `Trust::Own` another real id is kept; under `Trust::Foreign`, or when the text is no id, the leaf (or the whole id-keyed entry) is dropped. A node of a type this build doesn't know takes no tree (decision 4). `props` on any other kind is kept in this plan; plan 3 restricts it with `Kind::card()` (D7).

- [ ] **Step 1 (tests first).** `store/tree_json.rs` tests:

```rust
fn key(text: &str) -> Seg { Seg::Key(text.to_owned()) }
fn at(segs: &[Seg]) -> TreePath { TreePath::of(segs) }
fn same_ids(s: &str) -> Option<NodeId> { s.parse().ok() }

#[test]
fn a_tree_round_trips_through_json() {
    let mut tree = Tree::new();
    let card = NodeId { actor: ActorId(3), counter: 9 };
    for (segs, leaf) in [
        (vec![key("a"), key("b")], Leaf::Int(-4)),
        (vec![key("a"), key("c")], Leaf::Str("x".into())),
        (vec![key("f")], Leaf::Float(1.5)),
        (vec![key("big")], Leaf::Num("18446744073709551615".into())),
        (vec![key("n")], Leaf::Null),
        (vec![key("t")], Leaf::Bool(true)),
        (vec![key("r")], Leaf::Ref(card)),
        (vec![key("m"), Seg::Id(card)], Leaf::Bool(true)),
        (vec![key("@at"), key("")], Leaf::Int(1)),
        (vec![key("nul\0"), key("ünï")], Leaf::Int(2)),
    ] {
        tree.insert(at(&segs), leaf);
    }
    let json = tree_to_json(&tree);
    assert_eq!(tree_from_json(&json, &same_ids).unwrap(), tree);
}

#[test]
fn export_shows_children_over_a_scalar() {
    let mut tree = Tree::new();
    tree.insert(at(&[key("a")]), Leaf::Int(1));
    tree.insert(at(&[key("a"), key("b")]), Leaf::Int(2));
    assert_eq!(tree_to_json(&tree), serde_json::json!({"a": {"b": 2}}));
}

#[test]
fn import_resolves_ids_through_the_callback_and_drops_the_unknown() {
    let json = serde_json::json!({"r": {"$ref": "nope"}, "@nope": {"x": 1}, "keep": 1});
    let tree = tree_from_json(&json, &|_| None).unwrap();
    assert_eq!(tree.keys().cloned().collect::<Vec<_>>(), vec![at(&[key("keep")])]);
}

#[test]
fn import_refuses_what_would_exhaust_it() {
    let mut deep = serde_json::json!(1);
    for _ in 0..MAX_DEPTH + 2 { deep = serde_json::json!({ "k": deep }); }
    assert!(tree_from_json(&deep, &same_ids).is_err());
    let wide: serde_json::Map<_, _> = (0..MAX_LEAVES + 1).map(|i| (i.to_string(), serde_json::json!(i))).collect();
    assert!(tree_from_json(&serde_json::Value::Object(wide), &same_ids).is_err());
    assert!(tree_from_json(&serde_json::json!({"a": [1, 2]}), &same_ids).is_err());
    assert!(tree_from_json(&serde_json::json!([1]), &same_ids).is_err());
    assert!(tree_from_json(&serde_json::json!({"a": {}, "b": []}), &same_ids).unwrap().is_empty());
}
```

  `store/json.rs` tests (use `doc_from`/`ids` helpers there): `a_document_with_props_exports_imports_and_creates_them` (build a doc with two nodes, one carrying a `Ref` to the other under `PROPS`; `doc_to_json` → `import(.., Trust::Own)` → `creates` → apply to a fresh `Doc`; the new `Node::trees()` equals the old with the ref remapped to the new id of the second node), `a_foreign_import_drops_refs_outside_the_file`, `props_on_an_unknown_type_are_not_copied`, `an_old_export_without_props_imports_unchanged` (the existing tests cover this; just confirm they still pass).
- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold store::` — Expected: FAIL to compile.
- [ ] **Step 3:** Create `store/tree_json.rs`:

```rust
//! JSON for a node's tree fields (spec 2026-10-24-properties-design.md section 2.3). Dialect of PROPS; plan 4 adds
//! BODY's (empty containers and arrays are values there, and `$ref` is plain data).

use serde_json::{Map, Number, Value};

use super::model::NodeId;
use super::tree::{resolved, Leaf, Seg, Tree, TreePath};

/// Nesting deeper than this, or more leaves than this, is not a tree the app wrote.
pub const MAX_DEPTH: usize = 64;
pub const MAX_LEAVES: usize = 10_000;

fn seg_text(seg: &Seg) -> String {
    match seg {
        Seg::Key(k) if k.starts_with('@') => format!("@{k}"),
        Seg::Key(k) => k.clone(),
        Seg::Id(id) => format!("@{id}"),
    }
}

fn leaf_json(leaf: &Leaf) -> Value {
    match leaf {
        Leaf::Null => Value::Null,
        Leaf::Bool(b) => Value::Bool(*b),
        Leaf::Int(i) => Value::from(*i),
        Leaf::Float(f) => Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Leaf::Num(text) => serde_json::from_str::<Value>(text).ok().filter(Value::is_number).unwrap_or_else(|| Value::String(text.clone())),
        Leaf::Str(s) => Value::String(s.clone()),
        Leaf::Ref(id) => serde_json::json!({ "$ref": id.to_string() }),
        Leaf::EmptyObj => Value::Object(Map::new()),
        Leaf::EmptyArr => Value::Array(Vec::new()),
    }
}

/// The resolved leaves as nested objects.
pub fn tree_to_json(tree: &Tree) -> Value {
    let mut root = Map::new();
    for (path, leaf) in resolved(tree) {
        let segs = path.segments();
        let Some((last, parents)) = segs.split_last() else { continue };
        let mut at = &mut root;
        for seg in parents {
            let slot = at.entry(seg_text(seg)).or_insert_with(|| Value::Object(Map::new()));
            at = slot.as_object_mut().expect("`resolved` leaves no leaf above another");
        }
        at.insert(seg_text(last), leaf_json(leaf));
    }
    Value::Object(root)
}

/// `value` as a tree. `resolve` maps an id text to this document's id, or `None` to drop what names it.
pub fn tree_from_json(value: &Value, resolve: &dyn Fn(&str) -> Option<NodeId>) -> Result<Tree, String> {
    let Value::Object(map) = value else { return Err("not an object".to_owned()) };
    let mut tree = Tree::new();
    walk(map, &TreePath::default(), resolve, &mut tree, 1)?;
    Ok(tree)
}

fn walk(map: &Map<String, Value>, at: &TreePath, resolve: &dyn Fn(&str) -> Option<NodeId>, tree: &mut Tree, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("nested deeper than {MAX_DEPTH}"));
    }
    for (name, value) in map {
        let seg = match name.strip_prefix('@') {
            Some(rest) if rest.starts_with('@') => Seg::Key(rest.to_owned()),
            Some(rest) => match resolve(rest) {
                Some(id) => Seg::Id(id),
                None => continue,
            },
            None => Seg::Key(name.clone()),
        };
        let path = at.child(&seg);
        let leaf = match value {
            Value::Null => Leaf::Null,
            Value::Bool(b) => Leaf::Bool(*b),
            Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                (Some(i), _) => Leaf::Int(i),
                (None, Some(f)) if f.is_finite() && !n.is_u64() => Leaf::Float(f),
                _ => Leaf::Num(n.to_string()),
            },
            Value::String(s) => Leaf::Str(s.clone()),
            Value::Object(m) if m.len() == 1 && m.contains_key("$ref") => {
                match m["$ref"].as_str().and_then(resolve) {
                    Some(id) => Leaf::Ref(id),
                    None => continue,
                }
            }
            Value::Object(m) if m.is_empty() => continue,
            Value::Object(m) => {
                walk(m, &path, resolve, tree, depth + 1)?;
                continue;
            }
            Value::Array(a) if a.is_empty() => continue,
            Value::Array(_) => return Err(format!("{name:?}: an array")),
        };
        tree.insert(path, leaf);
        if tree.len() > MAX_LEAVES {
            return Err(format!("more than {MAX_LEAVES} leaves"));
        }
    }
    Ok(())
}
```

  Declare `pub mod tree_json;` in `store/mod.rs` next to `tree`. Check `NodeId` has `Display`/`FromStr` agreeing with each other (`n.id.parse()` is used in `duplicate.rs`); the round-trip test pins it. A `Num` whose text came from a u64 > i64 round-trips as `Num` (`as_f64` exists but `is_u64` routes it to `Num`).
- [ ] **Step 4 (`json.rs`).** `node_json(id, ty, fields, trees: &Trees, group)`: after the `group` put, `for &(no, name) in &TREE_FIELDS { if let Some(tree) = trees.get(&no).filter(|t| resolved(t).next().is_some()) { out.insert(name.into(), tree_to_json(tree)); } }`. Update its callers (`grep -rn "node_json(" crates`): `node_to_json` passes `&node.trees()`; the three in `duplicate.rs` pass `&root.trees`, `&member.trees`, `&snap.trees` (use the snapshot's, not the document's). `Imported` gains `pub trees: Trees`. In `import`, after the field loop, per node:

```rust
        let mut trees = Trees::new();
        if kind != Kind::Unknown {
            let resolve = |text: &str| match fresh.get(text) {
                Some(id) => Some(*id),
                None if trust == Trust::Own => text.parse().ok(),
                None => None,
            };
            for &(no, name) in &TREE_FIELDS {
                if let Some(value) = node.fields.get(name) {
                    let tree = tree_from_json(value, &resolve).map_err(|e| format!("node {:?}: bad {name}: {e}", node.id))?;
                    if !tree.is_empty() {
                        trees.insert(no, tree);
                    }
                }
            }
        }
        out.push(Imported { id: fresh[node.id.as_str()], ty: ty.to_owned(), fields, trees });
```

  `create_op` stays; `creates` becomes `.flat_map(|n| std::iter::once(create_op(n)).chain(tree_sets(n)))` with `fn tree_sets(n: &Imported) -> impl Iterator<Item = OpBody> + '_` producing the `Set { node: n.id, field, path, enc }` per leaf. Unknown field names were already skipped by `field_by_name`, so `"props"` never reaches the scalar loop.
- [ ] **Step 5 (app).** `materialize_imported` builds `Snapshot { .., trees: node.trees.clone() }` (T2 left it empty). `duplicate.rs`: `subtree_json` uses the snapshot's trees via the new `node_json` parameter. A tab duplicate (`duplicate_tab`, `grep -rn "fn duplicate_tab" crates`) must carry the original's trees: read it, and if it builds a `Carried` or snapshot, set `trees` from `self.doc.mirror.trees_of(original)`. Add the app test in `session_sync/tests/props.rs`:

```rust
#[test]
fn a_duplicate_carries_the_tree_and_remaps_refs_inside_the_copy() {
    // card A with /label = Str, /link = Ref(B); B another card; duplicate_nodes(&[A, B]);
    // each copy has the same /label; the copy of A's /link names the copy of B, not B.
}
#[test]
fn a_tab_duplicate_and_a_respawn_keep_the_tree() { /* edit_prop on a tab; duplicate_tab; wake/respawn; trees_of equal */ }
```

  Write both out against the helpers the other duplicate tests use (`session_sync/tests/*.rs` has duplicate tests; `grep -rn "duplicate_nodes" crates/manifold/src/session_sync/tests`).
- [ ] **Step 6:** `cargo test -j 8 -p manifold --bin manifold` — Expected: all pass. fmt, clippy; commit `export, import and Duplicate carry property trees` + trailer.

---

### Task 7: Remove the dead-code allows; docs; parked work

**Files:** Modify `store/tree.rs`, `store/model.rs`, spec, `PARKED.md`, this plan.

- [ ] **Step 1:** Delete `#![allow(dead_code, ..)]` from the top of `store/tree.rs` and the `#[allow(dead_code)]` on `field::PROPS` (`model.rs`; keep `BODY`'s, whose reason becomes "plan 4"). Run clippy. For each remaining `dead_code` item: if plan 3 or 4 needs it (see spec §3–§4), give it `#[allow(dead_code, reason = "plan 3: …")]` naming the use; otherwise delete it and its test. Expected to survive nowhere: `TreePath::parent` is the likely candidate; grep before deleting.
- [ ] **Step 2 (docs).** (a) Spec §2.1: the mirror holds live leaves with `resolved` for readers (R1), `tree_version` is a tick that never repeats (T3), `Tree`/`Trees` named; §2.2: `PropEdit`, recorded by the caller; §2.3: `edit_subtree_delete` folded into `edit_prop(.., None)` (R2); D2's wording gets "(refined in plan 2, R1)". (b) `PARKED.md`, one entry each in the file's format: **Unify `UndoKind::Revert` into register edits** (why parked: its replay has per-field hooks `recheck_edge_scope`, `snap_layout`, cycle and closed-destination checks; trigger: a fourth undo kind, or a guard bug in `replay_revert`; evidence `undo.rs`, date); **In-place `Mirror::edit_tree`** (clones the tree per edit; trigger: plan 4's per-keystroke body edits or a profile); **Ref leaves to deleted or closed cards** (readers must treat a dangling `Ref` as absent; trigger: plan 3 labels UI); **`props` on non-cards** (restrict with `Kind::card()` in plan 3, D7). (c) `grep -rn "RemoteNode\|edit_subtree_delete\|resolved tree" docs crates --include=*.md --include=*.rs` and fix stale wording; list the files found. (d) Set this plan's `status: done`; the spec's phase-2 line in §5 gets "(plan 2, done)".
- [ ] **Step 3:** Full suite, fmt, clippy; commit `properties: plan 2 docs; dead-code allows removed` + trailer naming the stale files fixed.

---

## Self-review

- **Spec coverage:** §2.1 mirror trees and version (T3), `edit_prop` (T4, R2), §2.2 undo (T5), §2.3 duplicate/Carried (T3 `Carried`, T6 Duplicate and tab), export/import with remap (T6), §5 phase 2 "removes the dead_code allow" (T7), §6 tests "local edit equals remote", "undo skips a changed register" (T3, T5), D1–D7 untouched except D2 refined (R1). Not here: labels, UI, `Kind::card()` (plan 3); BODY (plan 4).
- **Placeholder scan:** three test bodies are sketched in comments (T3 refresh and eviction, T4 two, T5 closed card, T6 two app tests): each states the exact assertion and names the helpers; the executor writes them against the helpers' real signatures, which this plan could not read in full. Everything else is code.
- **Type consistency:** `Tree`/`Trees`/`TREE_FIELDS`/`edit`/`subtree`/`resolved` (T1) are used under those names in T3–T6; `PropEdit { node, field, path, before, after }` (T4) is what T5 reads; `Snapshot.trees` (T2) is what `App::snapshot`, `build_ops` and `materialize_imported` use; `queue_tree`/`tree_edits`/`tree_sets` (T3) are what `edit_prop`, `mirror_remote` and `build_ops` call.
- **Checked against the code while writing:** `TreePath::is_ancestor_of` is strict and `subtree_end` exclusive; `Kind::Unknown` exists; `NodeId` has `Display` and `FromStr`; `create_ops` takes `&new` and creates every node in it, so a created node's tree `Set`s always follow a `Create`; `field::PROPS` carries the allow T7 removes.
- **To rule on while executing (ledger):** the exact `spawn_snapshot` call sites (T3 Step 5); the real helper names in `session_sync/tests.rs` the sketched tests use.
- **Adversarial rounds run:** 2. Round 1 (against the code surveyed): found the mirror-resolution divergence (R1), a lost ordering in the queue (fixed by eviction plus path order), the `RemoteNode` tuple that would have needed a fifth parameter (R3), and the created-node `editable` rule (3a). Round 2 (against the plan text): fixed a garbled `set_tree` snippet, a clumsy `edit` no-change test, a hard-coded test count, an unreachable "late older write" app test, a missing `has(id)` assert in `edit_prop`, and the unresolved `Kind::Unknown` name.
