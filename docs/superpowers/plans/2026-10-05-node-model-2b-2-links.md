---
status: done
---

# Node model 2b-2: parent and group in the mirror Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `GraphNode.parent` and `GraphNode.group` become mirror state written through one writer each, so the
`diff` no longer compares them for existing nodes; the tab-carry code that reads the mirror is written once.

**Architecture:** Same shape as 2b-1. `Mirror` gains string-keyed *links* (`group`, `parent`) beside its decoded
fields. `App::edit_parent` queues a `Set PARENT` in `App::pending`; `App::edit_group` queues a `Move` in a new
`App::pending_moves`. Both update the mirror at once; `save_session` commits them after `diff`'s ops, after resolving each
target against what the document and this commit hold. `mirror_remote` applies another replica's values except where a
local edit is pending. Cascades (a node removed, a group dissolved or merged) stay in the app and call the writers per
member.

**Tech Stack:** Rust, egui, the in-repo `store` journal (`OpBody::{Create, Set, Move, Delete}`).

**Spec:** `docs/superpowers/specs/2026-10-04-node-model-design.md` (section 6 items 4, 6, 10; "Rename plan and order",
2b "Read model"). Facts about today's flow: `docs/superpowers/specs/2026-10-05-sync-flow-map.md`. This plan follows
`2026-10-05-node-model-2b-1-mirror.md`; read its Rulings first, they all still hold.

## Global Constraints

- Work on `dev`, never on `main` (`CLAUDE.md`). Commit with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Run `cargo fmt --all` before each commit. `cargo fmt --all --check` and
  `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass.
  Never bypass `.githooks/pre-commit`. Don't silence a lint to pass: fix it, or `#[allow]` it with a comment saying why.
- No test may read or write the real `%APPDATA%\manifold`. Tests that need files use `store::testutil::temp_dir`.
- `cargo test -j 8 -p manifold --bins` baseline before this plan: 752 passed, 4 ignored. `ipc-protocol`: 43.
- Builds and tests: `-j 8`, BelowNormal priority for long runs (`LONG_RUNS.md`). The repo is LF; run Python edit scripts
  as `python -P script.py < /dev/null` (a heredoc with `< /dev/null` never runs).
- Hand-built test nodes use ids that are not store ids (`"g"`, `"p"`, `"c"`). Writers must accept them: they update the
  mirror only and queue nothing (as `edit_field` does).
- Jars are not graph nodes and have neither group nor parent; nothing here changes `jar_defs`.
- Behavior stays as today except where a Ruling says otherwise. Local edits still show at once; the store write still
  happens on the next `save_session`.

## Rulings

1. **Links are `Option<String>`, not `FieldValue::Id`.** Hand-built test ids cannot be a `NodeId`, and per-frame level
   filters compare `Option<&str>` without allocating. `Mirror` keeps `links: HashMap<String, Links>` beside its
   `Fields`. Reads are `Mirror::group(id) -> Option<&str>` and `Mirror::parent(id) -> Option<&str>`.
2. **`parent` is a `Set`, `group` is a `Move`.** In the document `group` is the tree edge (`OpBody::Move`, read with
   `Doc::effective_group`), not a field. So `parent` rides `App::pending` (key `(NodeId, field::PARENT)`) and `group`
   rides `App::pending_moves: BTreeMap<NodeId, Option<NodeId>>`.
3. **Writers are no-ops for an unchanged value** and queue nothing for ids or targets that are not store ids (mirror
   only), exactly as `edit_field`.
4. **Commit order:** `diff` ops (Creates, then non-mirrored Sets, then Deletes), then pending `Move`s, then pending
   `Set`s. A pending edit for a node this commit Creates or Deletes, or that the document does not show live, is dropped
   (the Create carries the mirror's values; a `Set`/`Move` on a missing node would make a placeholder).
5. **Target validity, resolved at commit** (this is `diff`'s `valid` rule, section 6 item 4): a pending group or parent
   target is kept if it is live in the document and not Deleted by this commit, or Created by this commit; otherwise it
   is written as none. `Create` already does this through `diff`.
6. **A pending `Move` whose target group is Created in this same commit is emitted after that `Create`** (it is: pending
   ops are appended after `diff`'s).
7. **Cascades stay in the app.** `remove_node` still re-parents children to the removed node's parent;
   `dissolve_group_quietly` and `merge_groups` still move members up; each call becomes one `edit_group`/`edit_parent`
   per member. The document's `effective_group` makes the redundant `Move` harmless.
8. **`diff` skips `PARENT` and the group `Move` for every existing non-jar node**, as it skips `MIRRORED` fields.
   `shared_nodes` reads both from the mirror, so a `Create` carries them. `Shared.group` stays (diff's Create ordering
   and `spawn_shared` use it).
9. **Remote:** `mirror_remote` also sets `group` (from the effective group `materialize` receives) and `parent` (from
   the `PARENT` field), skipping a link with a pending edit (local wins, last writer per link). A parked
   `remote_in_flight` snapshot is patched on commit for these too.
10. **`Mirror::init`-style seeding stays "only if the id has no entry".** `Carried` (Task 1) is the one value that seeds
    a landing tab and builds a respawn; it replaces the repeated mirror reads (review items M1/M2 of 2b-1).
11. **`pos` is not touched here** (it is Task 2b-4; see the spec's `pos` note). `GraphNode` keeps `pos`, `vel`, `payload`.

## Staging (later plans, for orientation only)

2b-3 `url`/`engine`/`jar`/`jar_set`; 2b-4 `name`/`text`/`view`/`pos` (document `pos` in the mirror, layout `pos`/`vel` in
a runtime side-table); 2b-5 content kinds, then delete `diff`, `explicit`, `keep`, the enums and the caps bridge,
rewrite fixtures, add the grep guard.

## File Structure

- `crates/manifold/src/mirror.rs`: `Links`, `Mirror::{group, parent, set_group, set_parent, seed}`, `Carried`.
- `crates/manifold/src/session_sync.rs`: `edit_parent`, `edit_group`, `mirror_remote` links, `diff` skips,
  `shared_nodes` reads links, `save_session` commits and clears pending moves, `update_live_node` and
  `insert_materialized` stop writing `node.parent`/`node.group`.
- `crates/manifold/src/main.rs`: `App::pending_moves`, `App::carried`, `TabSpawn::respawn`, `remove_node`,
  `dissolve_group_quietly`, `remove_from_group`.
- `crates/manifold/src/node.rs`: `GraphNode` loses `parent`, then `group`.
- Reads and writes at the sites listed in each task (compiler-led).
- `crates/manifold/src/session_sync/tests.rs`: new tests per task.
- Docs: spec "2b-2 done" note, sync-flow map "After 2b-2".

---

### Task 1: `Carried`, one reader and one seeder for a tab's mirror state

Pure refactor of the colour and pin code from 2b-1 (review items M1, M2). Nothing changes behavior.

**Files:**
- Modify: `crates/manifold/src/mirror.rs`
- Modify: `crates/manifold/src/main.rs` (`TabSpawn::respawn` ~800-830, `TabCarry`, `App::carried` new, line ~2444)
- Modify: `crates/manifold/src/tabs.rs` (~376, ~448, ~567, ~985), `crates/manifold/src/event_loop.rs` (~1267),
  `crates/manifold/src/view_modes.rs` (~273)
- Test: `crates/manifold/src/mirror.rs` (unit)

**Interfaces:**
- Produces: `pub(crate) struct Carried { pub color: Option<egui::Color32>, pub pinned: bool }` (Task 2 adds `parent`,
  Task 3 adds `group`); `Mirror::carried(&self, id: &str) -> Carried`; `Mirror::seed(&mut self, id: &str, carried: &Carried)`
  (writes only if the id has no entry, like `init`).
- Consumes: `Mirror::{color, pinned, init}` from 2b-1.

- [ ] **Step 1: Write the failing test** in `mirror.rs` `mod tests`:

```rust
    #[test]
    fn seed_then_carried_round_trips_and_never_overwrites() {
        let mut mirror = Mirror::default();
        mirror.seed("7", &Carried { color: Some(egui::Color32::RED), pinned: true });
        assert_eq!(mirror.carried("7"), Carried { color: Some(egui::Color32::RED), pinned: true });
        mirror.seed("7", &Carried { color: None, pinned: false });
        assert_eq!(mirror.carried("7"), Carried { color: Some(egui::Color32::RED), pinned: true });
    }
```

- [ ] **Step 2: Run it, expect a compile error** (`Carried` not defined):
  `cargo test -j 8 -p manifold --bins mirror::` .

- [ ] **Step 3: Implement** in `mirror.rs` (derive `Debug, Clone, PartialEq`):

```rust
/// What a tab carries across a respawn or lands with: the mirror's values for it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Carried {
    pub color: Option<egui::Color32>,
    pub pinned: bool,
}

impl Mirror {
    pub(crate) fn carried(&self, id: &str) -> Carried {
        Carried { color: self.color(id), pinned: self.pinned(id) }
    }

    /// `init` for everything a tab carries.
    pub(crate) fn seed(&mut self, id: &str, carried: &Carried) {
        self.init(
            id,
            [(field::COLOR, color_value(carried.color)), (field::PINNED, FieldValue::Bool(carried.pinned))],
        );
    }
}
```

Import `color_value` from `crate::session_sync` next to `color_of`.

- [ ] **Step 4: Switch the sites.** `TabCarry` keeps its `color` and `pinned` fields for now but is built from a
  `Carried`; replace each pair `(self.mirror.color(id), self.mirror.pinned(id))` and the two
  `self.mirror.init(&id, [(field::COLOR, ..), (field::PINNED, ..)])` blocks in `tabs.rs` with
  `self.mirror.carried(id)` and `self.mirror.seed(&id, &Carried { color, pinned })`. `TabSpawn::respawn` takes
  `carried: Carried` instead of the `color` and `pinned` arguments. The compiler lists every caller:
  `cargo check -j 8 -p manifold --all-targets`.

- [ ] **Step 5: Run everything.**
  `cargo test -j 8 -p manifold --bins` (expect 753 passed, 4 ignored), then clippy and fmt per Global Constraints.

- [ ] **Step 6: Commit** `Mirror: Carried is the one reader and seeder of a tab's mirror state`.

---

### Task 2: `parent` through the mirror

**Files:**
- Modify: `crates/manifold/src/mirror.rs` (`Links`, accessors, `retain`), `crates/manifold/src/session_sync.rs`,
  `crates/manifold/src/main.rs`, `crates/manifold/src/node.rs` (remove `GraphNode::parent`)
- Modify (compiler-led, about 63 read sites and 8 write sites): `fetched.rs` (~102, ~241, ~537), `graph_nav.rs` (~126,
  `would_create_cycle`), `main.rs` (`remove_node` ~2201, `TabSpawn`, 9 `GraphNode {` literals), `tabs.rs`
  (`remove_tab`, ~976 child filter, landing), `closed.rs`, `card.rs`, `graph_ui.rs`, `level.rs`, `json_level.rs`,
  `jar_defs.rs` and tests that set `.parent = Some(..)`
- Test: `crates/manifold/src/session_sync/tests.rs`

**Interfaces:**
- Consumes: Task 1's `Carried`, `Mirror::seed`; `App::pending: BTreeMap<(NodeId, u32), Vec<u8>>`;
  `encode_field(no, &FieldValue) -> Vec<u8>`.
- Produces:
  - `Mirror::parent(&self, id: &str) -> Option<&str>`; `Mirror::set_parent(&mut self, id: &str, parent: Option<String>)`.
  - `Carried.parent: Option<String>` (added here).
  - `App::edit_parent(&mut self, id: &str, parent: Option<String>)`.
  - `App::children_of(&self, id: &str) -> impl Iterator<Item = &str>`: every node whose mirror parent is `id`.

- [ ] **Step 1: Failing tests** in `session_sync/tests.rs`. They use the existing helpers `app_on`, `add`, `sleeping_tab`,
  `remote_set`, `stored`, `temp_dir`, `id`:

```rust
/// A parent edit shows at once and is committed by the next save.
#[test]
fn a_parent_edit_reads_back_and_commits() {
    let path = temp_dir("parent-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (add(&mut app, sleeping_tab("https://a/", "")), add(&mut app, sleeping_tab("https://b/", "")));
    assert!(app.save_session());
    app.edit_parent(&b, Some(a.clone()));
    assert_eq!(app.mirror.parent(&b), Some(a.as_str()));
    assert_eq!(stored(&app, &b, field::PARENT), FieldValue::Id(None), "not written before the save");
    assert!(app.save_session());
    assert_eq!(stored(&app, &b, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}

/// An unsaved local parent beats another replica's edit to the same link; their other fields still arrive.
#[test]
fn a_local_parent_edit_wins_over_a_remote_one() {
    let path = temp_dir("parent-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b, c) = (
        add(&mut app, sleeping_tab("https://a/", "")),
        add(&mut app, sleeping_tab("https://b/", "")),
        add(&mut app, sleeping_tab("https://c/", "")),
    );
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_parent(&c, Some(a.clone()));
    remote_set(&mut other, &c, field::PARENT, FieldValue::Id(Some(b.parse().unwrap())));
    assert!(app.save_session());
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()));
    assert_eq!(stored(&app, &c, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}

/// A parent that is deleted in the same commit is written as none (spec section 6 item 4).
#[test]
fn a_parent_deleted_by_the_same_commit_is_written_as_none() {
    let path = temp_dir("parent-deleted").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (add(&mut app, sleeping_tab("https://a/", "")), add(&mut app, sleeping_tab("https://b/", "")));
    assert!(app.save_session());
    app.edit_parent(&b, Some(a.clone()));
    app.nodes.remove(&a); // gone from the live graph: `diff` Deletes it
    assert!(app.save_session());
    assert_eq!(stored(&app, &b, field::PARENT), FieldValue::Id(None));
}

/// Hand-built ids only touch the mirror.
#[test]
fn a_parent_edit_on_a_non_store_id_queues_nothing() {
    let mut app = App::default();
    app.edit_parent("c", Some("p".into()));
    assert_eq!(app.mirror.parent("c"), Some("p"));
    assert!(app.pending.is_empty());
}

/// A remote parent change reaches the mirror; a node removal re-parents children through the writer.
#[test]
fn remote_parent_change_and_removal_reparent_through_the_mirror() {
    let path = temp_dir("parent-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b, c) = (
        add(&mut app, sleeping_tab("https://a/", "")),
        add(&mut app, sleeping_tab("https://b/", "")),
        add(&mut app, sleeping_tab("https://c/", "")),
    );
    app.edit_parent(&b, Some(a.clone()));
    app.edit_parent(&c, Some(b.clone()));
    assert!(app.save_session());
    app.remove_node(&b);
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()), "children pass up to the removed node's parent");
    assert!(app.save_session());
    assert_eq!(stored(&app, &c, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}
```

The second replica is `Journal::open(&path, ActorId(2)).unwrap()`, as in the existing test
`a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another` (line ~518); `remote_set` is its helper.

- [ ] **Step 2: Run, expect compile errors** (`edit_parent`, `Mirror::parent` missing).

- [ ] **Step 3: `mirror.rs`.**

```rust
#[derive(Default)]
struct Links {
    group: Option<String>,
    parent: Option<String>,
}
// in Mirror:
    links: HashMap<String, Links>,

    /// The node this one was opened from (`None` for a root).
    pub(crate) fn parent(&self, id: &str) -> Option<&str> {
        self.links.get(id)?.parent.as_deref()
    }

    pub(crate) fn set_parent(&mut self, id: &str, parent: Option<String>) {
        self.links.entry(id.to_owned()).or_default().parent = parent;
    }
```

`retain` also retains `links`. `Carried` gains `pub parent: Option<String>`; `carried` reads `self.parent(id)`;
`seed` writes the parent only if the id has no `links` entry (`if !self.links.contains_key(id) { self.set_parent(..) }`).

- [ ] **Step 4: `session_sync.rs` writer and commit.**

```rust
    /// The one writer of a node's parent link (Ruling 2): the mirror shows it at once and the next save commits a
    /// `Set PARENT`. A non-store id or target only updates the mirror.
    pub(crate) fn edit_parent(&mut self, id: &str, parent: Option<String>) {
        if self.mirror.parent(id) == parent.as_deref() {
            return;
        }
        if let (Ok(node), Some(target)) = (id.parse::<NodeId>(), parent_id(&parent)) {
            self.pending.insert((node, field::PARENT), encode_field(field::PARENT, &FieldValue::Id(target)));
        }
        self.mirror.set_parent(id, parent);
    }
```

`parent_id(&Option<String>) -> Option<Option<NodeId>>`: `None` input gives `Some(None)`; an unparsable id gives `None`.

In `diff`, extend the existing mirrored-field skip so a non-jar existing node's `PARENT` is skipped:
`if (MIRRORED.contains(&no) || no == field::PARENT) && node.kind != Kind::Jar { continue; }`.

In `save_session`, where pending `Set`s are collected, resolve `PARENT` values by Ruling 5. Build
`let created: BTreeSet<NodeId>` and `let deleted: BTreeSet<NodeId>` from `ops`; for a pending `(node, field::PARENT)`,
decode the stored `enc` with `decode_field`, then write `FieldValue::Id(target.filter(|t| (in_doc(t) && !deleted.contains(t)) || created.contains(t)))`.
Factor the rule into one closure shared with Task 3's moves:

```rust
let resolve = |target: Option<NodeId>| {
    target.filter(|t| (doc.nodes.get(t).is_some_and(Node::is_live) && !deleted.contains(t)) || created.contains(t))
};
```

`mirror_remote`: also `if !self.pending.contains_key(&(node, field::PARENT)) { self.mirror.set_parent(id, id_of(fields, field::PARENT)); }`.
`shared_nodes`: `parse_id(&node.parent)` becomes `parse_id(&self.mirror.parent(key).map(str::to_owned))`.
`update_live_node` and `insert_materialized`: delete the `node.parent = ...` lines (`mirror_remote` runs first in
`materialize`). A node that arrives through `insert_materialized` has been through `mirror_remote`, so its link is set.
Commit success: patch parked `remote_in_flight` snapshots for pending `PARENT` edits the way 2b-1 does for
`MIRRORED` (`fields.insert(no, FieldValue::Id(...))`), then `pending.clear()` as now.

- [ ] **Step 5: Flip the reads and writes** (remove `GraphNode::parent`, let the compiler list sites). Replacement table:

| Old | New |
| --- | --- |
| `node.parent.clone()` / `n.parent.clone()` | `self.mirror.parent(id).map(str::to_owned)` |
| `n.parent.as_deref() == Some(id)` (child filters) | `self.mirror.parent(child_id) == Some(id)`; use `App::children_of(id)` |
| `node.parent = Some(x)` / `= parent` | `self.edit_parent(id, Some(x))` / `self.edit_parent(id, parent)` |
| `GraphNode { parent: None, group, pos, vel, payload }` | drop the `parent` field; seed with `Mirror::seed` |
| respawn / landing `parent` | `Carried.parent` (`App::carried` reads it; `TabSpawn.parent` stays, seeded at landing) |

`remove_node` becomes:

```rust
        let parent = self.mirror.parent(id).map(str::to_owned);
        let children: Vec<String> = self.children_of(id).map(str::to_owned).collect();
        for child in children {
            self.edit_parent(&child, parent.clone());
        }
        let removed = self.nodes.remove(id);
```

Where a loop borrows `self.nodes` and needs `self.mirror`, they are disjoint fields; call `self.mirror.parent(..)`
directly rather than `&self` methods to keep the borrow checker satisfied.

- [ ] **Step 6: Run everything.** `cargo test -j 8 -p manifold --bins` (all pass), clippy, fmt. Then
  `cargo build -j 8 -p manifold` and `scripts/headless-smoke-test.sh` once (this stage must not change its results).

- [ ] **Step 7: Commit** `Parent lives in the mirror`.

---

### Task 3: `group` through the mirror

**Files:**
- Modify: `crates/manifold/src/mirror.rs`, `crates/manifold/src/session_sync.rs`, `crates/manifold/src/main.rs`,
  `crates/manifold/src/node.rs` (remove `GraphNode::group`)
- Modify (compiler-led, about 150 read sites, 12 write sites): `graph_nav.rs` (Join ~106, GroupWith ~118, Eject,
  `merge_groups` ~153, `settle_current_level` ~410-435), `graph_ui.rs` (`GroupSelected` ~329), `main.rs`
  (`remove_from_group` ~326, `dissolve_group_quietly` ~2373, `place_node`, carry), `fetched.rs` (`place_node` ~67, ~444),
  `card.rs` (~934 `in_group`), `level.rs`, `closed.rs`, `popups.rs`, `tabs.rs`, `jar_defs.rs` and tests
- Test: `crates/manifold/src/session_sync/tests.rs`

**Interfaces:**
- Consumes: Task 2's `resolve` closure shape, `Carried.parent`.
- Produces:
  - `Mirror::group(&self, id: &str) -> Option<&str>`; `Mirror::set_group(&mut self, id: &str, group: Option<String>)`.
  - `Carried.group: Option<String>`.
  - `App::pending_moves: BTreeMap<NodeId, Option<NodeId>>` (default empty).
  - `App::edit_group(&mut self, id: &str, group: Option<String>)`.
  - `App::in_level(&self, id: &str) -> bool` (`self.mirror.group(id) == self.current_group.as_deref()`).
  - `App::members_of(&self, group: &str) -> impl Iterator<Item = &str>`.

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn a_group_edit_reads_back_and_commits_as_a_move() {
    let path = temp_dir("group-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add(&mut app, NodePayload::Group(GroupPayload::named("G")));
    let t = add(&mut app, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.edit_group(&t, Some(g.clone()));
    assert_eq!(app.mirror.group(&t), Some(g.as_str()));
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), None);
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), Some(g.parse().unwrap()));
}

/// A group created in the same commit is the target of a move that follows its Create.
#[test]
fn a_move_into_a_group_created_by_the_same_save_follows_the_create() {
    let path = temp_dir("group-new").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = add(&mut app, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    let g = add(&mut app, NodePayload::Group(GroupPayload::named("G")));
    app.edit_group(&t, Some(g.clone()));
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), Some(g.parse().unwrap()));
}

/// A move into a group the same commit deletes lands at the top level (spec section 6 item 4).
#[test]
fn a_move_into_a_group_deleted_by_the_same_save_is_written_as_none() {
    let path = temp_dir("group-gone").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add(&mut app, NodePayload::Group(GroupPayload::named("G")));
    let t = add(&mut app, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.edit_group(&t, Some(g.clone()));
    app.nodes.remove(&g);
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), None);
}

#[test]
fn a_group_edit_on_a_non_store_id_queues_nothing() {
    let mut app = App::default();
    app.edit_group("t", Some("g".into()));
    assert_eq!(app.mirror.group("t"), Some("g"));
    assert!(app.pending_moves.is_empty());
}

/// An unsaved local move beats another replica's move of the same node.
#[test]
fn a_local_group_move_wins_over_a_remote_one() {
    let path = temp_dir("group-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g1 = add(&mut app, NodePayload::Group(GroupPayload::named("A")));
    let g2 = add(&mut app, NodePayload::Group(GroupPayload::named("B")));
    let t = add(&mut app, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_group(&t, Some(g1.clone()));
    other
        .begin()
        .unwrap()
        .commit(vec![OpBody::Move { node: t.parse().unwrap(), group: Some(g2.parse().unwrap()) }])
        .unwrap();
    assert!(app.save_session());
    assert_eq!(app.mirror.group(&t), Some(g1.as_str()));
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), Some(g1.parse().unwrap()));
}

/// Dissolving a group moves its members up through the writer and the store agrees after a save.
#[test]
fn dissolving_a_group_moves_members_up_and_saves() {
    let path = temp_dir("group-dissolve").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add(&mut app, NodePayload::Group(GroupPayload::named("O")));
    let inner = add(&mut app, NodePayload::Group(GroupPayload::named("I")));
    let t = add(&mut app, sleeping_tab("https://t/", ""));
    app.edit_group(&inner, Some(outer.clone()));
    app.edit_group(&t, Some(inner.clone()));
    assert!(app.save_session());
    app.dissolve_group_quietly(&inner);
    assert_eq!(app.mirror.group(&t), Some(outer.as_str()));
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), Some(outer.parse().unwrap()));
}
```

(Use whatever the file's existing second-replica helper is for the conflict test; see Task 2 Step 1.)

- [ ] **Step 2: Run, expect compile errors.**

- [ ] **Step 3: `mirror.rs`:** `group`, `set_group` as for `parent`; `Carried.group`; `seed` seeds both links together.

- [ ] **Step 4: `session_sync.rs`.**

```rust
    /// The one writer of a node's group (Ruling 2): shown at once, committed as a `Move` by the next save.
    pub(crate) fn edit_group(&mut self, id: &str, group: Option<String>) {
        if self.mirror.group(id) == group.as_deref() {
            return;
        }
        if let (Ok(node), Some(target)) = (id.parse::<NodeId>(), parent_id(&group)) {
            self.pending_moves.insert(node, target);
        }
        self.mirror.set_group(id, group);
    }
```

`diff`: for existing nodes drop the `group != doc.effective_group(id)` Move block for non-jar nodes (jars have
`group: None` and never moved). `Create` still reads `node.group` from `Shared`, now filled by `shared_nodes` from the
mirror (`parse_id(&self.mirror.group(key).map(str::to_owned))`).
`save_session`: after the pending `Set` collection from Task 2, add, before them (Ruling 4 order: Moves, then Sets):

```rust
            ops.extend(
                self.pending_moves
                    .iter()
                    .filter(|(node, _)| !skip.contains(node) && doc.nodes.get(node).is_some_and(Node::is_live))
                    .map(|(&node, &group)| OpBody::Move { node, group: resolve(group) }),
            );
```

A `Move` whose resolved group equals the document's current `effective_group` is skipped (`filter`), to avoid noise.
On commit success clear `pending_moves`; on failure keep it (as `pending`). Patch parked `remote_in_flight` snapshots'
third element (the group) for pending moves, as for `PARENT`.
`mirror_remote` receives the group as a parameter now: change its signature to
`mirror_remote(&mut self, id: &str, fields: &Fields, group: Option<&str>)` and skip the link if
`pending_moves.contains_key(node)`. `materialize` passes `group.as_deref()`.
`update_live_node` and `insert_materialized`: delete the `node.group = group` writes; `update_live_node`'s `group`
parameter goes, as does the `seen.group != now.group` check.
`touched` collection already includes `Move`.

- [ ] **Step 5: Flip reads and writes** (remove `GraphNode::group`; compiler-led). Replacement table:

| Old | New |
| --- | --- |
| `n.group == self.current_group` in a `(id, n)` filter | `self.in_level(id)` |
| `node.group.clone()` | `self.mirror.group(id).map(str::to_owned)` |
| `node.group.as_deref() == Some(g)` (member scans) | `self.mirror.group(id) == Some(g)`; use `App::members_of(g)` |
| `node.group = Some(g)` / `= parent` | `self.edit_group(id, Some(g))` / `self.edit_group(id, parent)` |
| `node.group.is_some()` | `self.mirror.group(id).is_some()` |
| `GraphNode { parent, group, pos, vel, payload }` | `GraphNode { pos, vel, payload }`; seed the links via `Mirror::seed` in `place_node` and the spawn-landing paths |

`remove_from_group(nodes: &mut NodeMap, id)` (main.rs ~326) becomes `App::remove_from_group(&mut self, id)`:

```rust
        let group = self.mirror.group(id).map(str::to_owned);
        let up = group.as_deref().and_then(|g| self.mirror.group(g)).map(str::to_owned);
        self.edit_group(id, up);
```

`dissolve_group_quietly`: collect `members_of(id)` first, `edit_group(member, parent.clone())` for each, then remove the
group node. `merge_groups`: the ancestor walk uses `self.mirror.group(..)`; the member loop uses `members_of(source)`
collected first, then `edit_group(member, Some(target))`.
`settle_current_level` (graph_nav ~410-435) builds its four maps with `self.in_level(id)`.

- [ ] **Step 6: Run everything.** Tests, clippy, fmt, `cargo build -j 8 -p manifold`, then
  `scripts/headless-smoke-test.sh` twice, `scripts/scale-test.sh` and `scripts/chrome-shots.sh`; compare with the
  results recorded in the 2b-1 ledger (restore 3-4 s, one live tab, the rest asleep).

- [ ] **Step 7: Commit** `Group lives in the mirror`.

---

### Task 4: Docs, sync map, final verification

**Files:**
- Modify: `docs/superpowers/specs/2026-10-04-node-model-design.md` (2b section: "2b-2 done" note and the field groups left),
  `docs/superpowers/specs/2026-10-05-sync-flow-map.md` (new "After 2b-2" section), comments that still say
  `GraphNode::parent`/`group` (`rg "GraphNode.*(parent|group)|\.group\b.*GraphNode" crates/manifold/src`)

- [ ] **Step 1:** In the spec's 2b paragraph, add after the 2b-1 note: "2b-2 done: <commit hashes>. `group` and `parent`
  live in `Mirror` links; `edit_group` queues a `Move` (`App::pending_moves`), `edit_parent` a `Set PARENT`; `diff` no
  longer compares either for an existing non-jar node. Next: 2b-3 (url, engine, jar, jar_set); 2b-4 (name, text, view,
  pos: the document `pos` goes in the mirror, the layout `pos`/`vel` become a runtime side-table); 2b-5 (content kinds,
  then delete `diff`, `explicit`, `keep`, the enums and the caps bridge, rewrite fixtures, add the grep guard)."
- [ ] **Step 2:** In the sync-flow map, add "After 2b-2": links are written only by `edit_group`/`edit_parent`; commit
  order is `diff`, pending `Move`s, pending `Set`s, with targets resolved against the document and this commit;
  `mirror_remote` takes the effective group and skips a link with a pending edit.
- [ ] **Step 3:** Run `cargo fmt --all --check`, clippy per Global Constraints, `cargo test -j 8 -p manifold --bins -p ipc-protocol`.
- [ ] **Step 4:** Two-instance check (the 2b-1 procedure): move a tab into a group in one instance, confirm the other
  shows it after its next poll; move the same tab in both before saving and confirm the local move wins on each.
- [ ] **Step 5: Commit** `Docs: 2b-2 note and sync map`.

---

## Self-review (run 2026-10-05)

- **Spec coverage:** section 6 item 4 (one validity rule) is Ruling 5 and Tasks 2-3 tests; item 10 (direct ops) advances
  for two link kinds; the carry dedupe (2b-1 review items M1/M2) is Task 1. Items 2, 3, 6, 7, 8, 9 belong to later plans.
- **Placeholders:** none. The second replica is `Journal::open(&path, ActorId(2))`, checked against the existing tests.
  Tests were written against the helpers `app_on`, `add`, `sleeping_tab`, `remote_set`, `stored`; verify
  `Journal`, `ActorId` and `OpBody` are in scope in `tests.rs` (they are used by the neighbouring tests).
- **Type consistency:** `Carried` gains `parent` in Task 2 and `group` in Task 3; Task 1's tests construct it with two
  fields and Task 2 Step 3 must update them. `resolve` is defined in Task 2 Step 4 and reused in Task 3 Step 4.
- **Risks:** ~210 read sites are mechanical but wide; Task 3 is the largest and may deserve two review rounds. Borrowing
  `self.mirror` inside loops over `self.nodes` works because the fields are disjoint, but not through `&self` methods.

---

## Outcome (2026-10-05)

Done on `dev`: 5e82da0, dc1d1e6, 4741dab, 566b0c3, 9fa6631, 952fe1c, 808fc0e. 783 tests pass; fmt and clippy clean. The two-instance GUI check was not run; four two-replica tests cover local-wins and remote sync of group moves.

### Rulings I made

- T2's deleted-parent test stays: it documents the commit rule, and the mirror/doc gap is unreachable through app cascades (costs a stale mirror link only in a hand-made state).
- T2 minors 1-5 were fixed in Task 3 (parent and group share the code paths); minor 6 stayed deferred.
- The final-review fix covered the Important (tab mid-respawn missed by cascades), minor 3 (a Create keeping a target the same commit deletes), minor 5 (sync-map wording) and the missing tests. Minors 1, 2, 4, 6 were parked (rare, pre-existing or cheap).

### Parked (not fixed)

(The cycle items, the mirror taking the resolved target after a refused Move and the unguarded `merge_groups`/`would_create_cycle` walks, were fixed in 10372ac.)

- A pending `Set PARENT` is written even when it equals the document's value.
- A large remote delete costs O(deleted x nodes) through `children_of`.
- A local cascade does not patch a parked `remote_in_flight` entry for a respawning tab, so landing can show a deleted group (display only).
- `destroy_group` with a respawning member leaves that tab one level up; a respawning tab keeps its old jar snapshot after a local dissolve or merge (`refollow` skips it).
- The mirror can show the grandparent while the document still names a remotely deleted parent (same as live nodes).
- `seed_landing` has no direct test of its own beyond the respawn tests; `insert_content` uses `edit_group` where it could seed.
- Minors left after the cycle fix (10372ac, reviewed 2026-10-05): destroying a group cycle while `current_group` is inside it can leave `current_group` on a removed group (transient cycle window only); no test covers the post-commit readback together with a parked `remote_in_flight` group, or a target that is not live.
