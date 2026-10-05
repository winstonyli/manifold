# Node Model 2b-1: Mirror, Writer and the COLOR/PINNED Pilot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce the app-side decoded mirror of the document's fields and the one writer that updates it and queues the journal op, and prove both by moving two field groups, `color` and `pinned`, off `GraphNode` and the tab payloads.

**Architecture:** A `Mirror` (new `mirror.rs`) holds decoded `FieldValue`s per node id for the fields in `MIRRORED`. `App::edit_field` is the only writer: it updates the mirror at once and queues the op in `App::pending`. `save_session` commits `pending` beside what `diff` still emits for every other field; `diff` skips mirrored fields on existing nodes. Remote changes update the mirror through `mirror_remote`, which leaves a field with a pending local edit alone. Every read goes through `Mirror::get`-style accessors so a later step can make `Doc` the app's own state (node model spec, 2b "Read model") by changing only where they read.

**Tech Stack:** Rust, egui, the in-repo journal (`crates/manifold/src/store`).

**Spec:** `docs/superpowers/specs/2026-10-04-node-model-design.md` (item 2, "2b ... Read model (decided 2026-10-05)"); background in `docs/superpowers/specs/2026-10-05-sync-flow-map.md`.

## Global Constraints

- Work on branch `dev`; never `main`. New commits end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Run `cargo fmt --all` before each commit. `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass. Never `--no-verify`. Don't silence a lint to pass: fix it, or `#[allow]` it with a comment saying why.
- Style is `rustfmt.toml` (width 120, `use_small_heuristics = "Max"`). Files are LF.
- No test may read or write the real `%APPDATA%\manifold`: use `store::testutil::temp_dir` and pass paths in. `profile::tests` fails if another source file mentions `"APPDATA"` or `etcetera`.
- Cargo runs with `-j 8`, and long builds at BelowNormal priority (never more than 12 busy threads). Scratch files go in the session scratchpad, never `%TEMP%` or `/tmp`. Run Python as `python -P script < /dev/null`.
- Baseline before this plan: `cargo test -j 8 -p manifold --bins` 726 passed, 4 ignored; `-p ipc-protocol` 43 passed.
- Names: new code says `mirror`, `edit_field`, `pending`; it never adds a new `NodePayload` variant, `ItemNodeKind` variant or `Kind` match arm.

## Rulings

1. **Mirror keyed by `String`, like `App::nodes`.** Hand-built test nodes use ids such as `"r"` that are not store ids. `edit_field` updates the mirror for any id, and queues an op only when the id parses as a `NodeId`.
2. **Invariant: the mirror equals the document, except fields with a pending edit.** `materialize` writes the mirror from the document (guarded by `pending`); `edit_field` writes both the mirror and `pending`. Nothing else writes the mirror, except `Mirror::init` at spawn landing (Ruling 5).
3. **`diff` skips `MIRRORED` fields on existing nodes; `Create` still carries them** (from `shared_nodes`, which reads the mirror). Pending ops for a node the same commit creates are dropped (the `Create` carries the value); pending ops for a node not live in the document are dropped (a `Set` on a missing node makes a placeholder).
4. **`pending` is cleared only on a successful commit**, like `explicit`, so a failed commit retries. A remote round (`continue` in the `save_session` loop) leaves it alone.
5. **Spawn landing uses `Mirror::init`, which writes only when the id has no entry.** A new tab id (duplicate, new tab, restore) gets its carried `color` and `pinned`; a wake or respawn of an existing id keeps whatever the mirror holds, so an edit made during the wake survives. `TabCarry`/`TabSpawn` keep their `color` and `pinned` fields in this plan; their sources change.
6. **Jars keep their colour in `jar_defs`.** Jar definitions are not graph nodes; their move is sub-project C. `shared_nodes` still reads `def.color` for jars.
7. **Pin applies to tabs only.** Every pin site keeps today's guard: `payload.saved_tab().is_some()` (a live or unloaded tab). A note or group has no `pinned` field.
8. **A node replaced in place under the same id** (`reload_fetched_node`, the missing-file retry in `fetched.rs`) needs no colour copy-back: the mirror entry belongs to the id and survives the replacement. Delete those copy lines.
9. **Mirror entries of removed nodes are swept after each successful save** (`retain` against `App::nodes`). Ids are never reused, so a stale entry is harmless until then.

## File Structure

- Create `crates/manifold/src/mirror.rs`: `MIRRORED`, `Mirror`. Pure, no `App`.
- Modify `crates/manifold/src/main.rs`: `mod mirror;`, `App::mirror`, `App::pending`, their defaults, and the `color`/`pinned` read and write sites in this file.
- Modify `crates/manifold/src/session_sync.rs`: `edit_field`, `mirror_remote`, `diff` skip, `shared_nodes`, `update_live_node`, `insert_materialized`, `save_session`.
- Modify `crates/manifold/src/node.rs`: remove `GraphNode::color`, `TabPayload::pinned`, `UnloadedTabPayload::pinned`, `NodePayload::pinned`, `NodePayload::pinned_mut`.
- Modify the other read/write sites (found by the compiler): `card.rs`, `node_menu.rs`, `shortcuts.rs`, `popups.rs`, `graph_nav.rs`, `fetched.rs`, `tabs.rs`, `debug_control.rs`, `closed.rs` if it reads either field.
- Modify `crates/manifold/src/session_sync/tests.rs` (and any other test the compiler flags).
- Modify docs in Task 4.

---

### Task 1: The `Mirror` type

**Files:**
- Create: `crates/manifold/src/mirror.rs`
- Modify: `crates/manifold/src/main.rs` (add `mod mirror;` beside the other `mod` lines)

**Interfaces:**
- Consumes: `crate::session_sync::{Fields, color_of, color_value}` (existing, `pub(crate)`), `crate::store::model::{field, field_def, FieldValue}`.
- Produces (used by Tasks 2 and 3):
  - `pub(crate) const MIRRORED: &[u32]`
  - `#[derive(Default)] pub(crate) struct Mirror`
  - `Mirror::get(&self, id: &str, no: u32) -> FieldValue` (the field's absent value when unknown)
  - `Mirror::set(&mut self, id: &str, no: u32, value: FieldValue)`
  - `Mirror::init(&mut self, id: &str, fields: impl IntoIterator<Item = (u32, FieldValue)>)` (writes only if `id` has no entry)
  - `Mirror::retain(&mut self, keep: impl Fn(&str) -> bool)`
  - `Mirror::color(&self, id: &str) -> Option<egui::Color32>`

- [ ] **Step 1: Write the module with its tests**

Create `crates/manifold/src/mirror.rs`:

```rust
//! The app's decoded copy of the document's fields that have moved off `GraphNode` and its payloads (node model
//! spec, 2b "Read model"). Every read goes through the accessors here, so making `Doc` the app's own state later
//! changes only where they read.
//!
//! The mirror equals the document except for fields with an edit still waiting for a save (`App::pending`);
//! `App::edit_field` is the one writer, and `App::mirror_remote` applies another replica's change.

use std::collections::HashMap;

use crate::session_sync::{color_of, Fields};
use crate::store::model::{field, field_def, FieldValue};

/// The fields the mirror holds. `diff` leaves them to `App::pending` on existing nodes.
pub(crate) const MIRRORED: &[u32] = &[field::COLOR];

#[derive(Default)]
pub(crate) struct Mirror {
    nodes: HashMap<String, Fields>,
}

impl Mirror {
    /// `id`'s value of field `no`; the field's absent value when the mirror has none.
    pub(crate) fn get(&self, id: &str, no: u32) -> FieldValue {
        match self.nodes.get(id).and_then(|fields| fields.get(&no)) {
            Some(value) => value.clone(),
            None => FieldValue::absent(field_def(no).expect("a known field").ty),
        }
    }

    pub(crate) fn set(&mut self, id: &str, no: u32, value: FieldValue) {
        debug_assert!(MIRRORED.contains(&no), "field {no} is not mirrored");
        self.nodes.entry(id.to_owned()).or_default().insert(no, value);
    }

    /// Gives `id` these values only if it has no entry yet (a spawn landing: a new tab takes what it was started
    /// with, an existing node keeps what the mirror already holds).
    pub(crate) fn init(&mut self, id: &str, fields: impl IntoIterator<Item = (u32, FieldValue)>) {
        if !self.nodes.contains_key(id) {
            for (no, value) in fields {
                self.set(id, no, value);
            }
        }
    }

    /// Drops the entries `keep` rejects.
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.nodes.retain(|id, _| keep(id));
    }

    pub(crate) fn color(&self, id: &str) -> Option<egui::Color32> {
        color_of(&self.get(id, field::COLOR))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_sync::color_value;

    #[test]
    fn an_unknown_node_reads_as_absent() {
        let mirror = Mirror::default();
        assert_eq!(mirror.get("7", field::COLOR), FieldValue::Color(None));
        assert_eq!(mirror.color("7"), None);
    }

    #[test]
    fn a_set_value_reads_back() {
        let mut mirror = Mirror::default();
        mirror.set("7", field::COLOR, color_value(Some(egui::Color32::RED)));
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
        mirror.set("7", field::COLOR, color_value(None));
        assert_eq!(mirror.color("7"), None);
    }

    #[test]
    fn init_never_overwrites_an_entry() {
        let mut mirror = Mirror::default();
        mirror.init("7", [(field::COLOR, color_value(Some(egui::Color32::RED)))]);
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
        mirror.init("7", [(field::COLOR, color_value(Some(egui::Color32::BLUE)))]);
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
    }

    #[test]
    fn retain_drops_rejected_entries() {
        let mut mirror = Mirror::default();
        mirror.set("a", field::COLOR, color_value(Some(egui::Color32::RED)));
        mirror.set("b", field::COLOR, color_value(Some(egui::Color32::RED)));
        mirror.retain(|id| id == "a");
        assert_eq!(mirror.color("a"), Some(egui::Color32::RED));
        assert_eq!(mirror.color("b"), None);
    }
}
```

Add `mod mirror;` to `main.rs` next to the existing `mod` declarations (keep them in the file's current order, alphabetical).

- [ ] **Step 2: Run the tests, expecting a dead-code failure only in clippy**

Run: `cargo test -j 8 -p manifold --bins mirror::`
Expected: 4 passed.

Run: `cargo clippy -j 8 -p manifold --all-targets -- -D warnings`
Expected: FAIL with dead-code warnings for `Mirror`'s methods (nothing uses them outside tests yet).

- [ ] **Step 3: Allow the dead code until Task 2 wires it**

Put this on the line above `pub(crate) const MIRRORED`:

```rust
#[cfg_attr(not(test), allow(dead_code))] // wired in by `App::edit_field` (plan 2b-1, Task 2); remove then
```

and the same attribute (same comment) above `pub(crate) struct Mirror` and above `impl Mirror`. Run clippy again.
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/mirror.rs crates/manifold/src/main.rs
git commit -m "Mirror: the app's decoded copy of mirrored fields

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The writer, the commit path, and COLOR moved onto the mirror

**Files:**
- Modify: `crates/manifold/src/main.rs` (fields `mirror`, `pending`; `App::default` literal near line 1525; the `color` sites in this file: `~811`, `~2430`, `TabCarry` builds)
- Modify: `crates/manifold/src/session_sync.rs`
- Modify: `crates/manifold/src/node.rs` (remove `GraphNode::color`)
- Modify: `crates/manifold/src/mirror.rs` (remove the Task 1 `allow`s)
- Modify: `card.rs` (~933), `node_menu.rs` (~673, ~813, ~818), `fetched.rs` (~68, ~227, ~243, ~287, ~330, tests ~1069-1107), `tabs.rs` (~374, ~423, ~560), `debug_control.rs` (~1059), `main.rs` (~3410, ~3450 `GraphNode` literals), plus whatever else the compiler flags
- Test: `crates/manifold/src/session_sync/tests.rs`

**Interfaces:**
- Consumes: Task 1's `Mirror`, `MIRRORED`.
- Produces (used by Task 3):
  - `App::mirror: Mirror` and `App::pending: BTreeMap<(NodeId, u32), Vec<u8>>`
  - `App::edit_field(&mut self, id: &str, no: u32, value: FieldValue)`: the one writer
  - `App::mirror_remote(&mut self, id: &str, fields: &Fields)`
  - The save path commits `pending` and `diff` skips `MIRRORED`.

- [ ] **Step 1: Write the failing tests**

Append to `session_sync/tests.rs` (the helpers `app_on`, `add`, `remote_set`, `stored` already exist there):

```rust
fn red() -> FieldValue {
    color_value(Some(egui::Color32::RED))
}

fn blue() -> FieldValue {
    color_value(Some(egui::Color32::BLUE))
}

fn a_note(app: &mut App) -> String {
    add(app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }))
}

#[test]
fn a_local_color_edit_is_committed_by_the_next_save() {
    let path = temp_dir("edit-field").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.edit_field(&note, field::COLOR, red());
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED), "the mirror shows it at once");
    assert_eq!(stored(&app, &note, field::COLOR), FieldValue::Color(None), "nothing is written before a save");
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::COLOR), red());
    assert!(app.pending.is_empty(), "a commit clears what it wrote");
}

#[test]
fn a_color_set_before_the_first_save_rides_the_create() {
    let path = temp_dir("edit-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    app.edit_field(&note, field::COLOR, red());
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::COLOR), red());
    assert!(app.pending.is_empty());
}

#[test]
fn a_remote_color_edit_reaches_the_mirror() {
    let path = temp_dir("edit-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &note, field::COLOR, red());
    assert!(app.save_session());
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED));
}

#[test]
fn a_pending_local_color_edit_wins_over_a_remote_edit_of_the_same_field() {
    let path = temp_dir("edit-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_field(&note, field::COLOR, blue());
    remote_set(&mut other, &note, field::COLOR, red());
    // The remote round applies first, the next round commits the local edit over it.
    assert!(app.save_session());
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::BLUE));
    assert_eq!(stored(&app, &note, field::COLOR), blue());
}

#[test]
fn an_edit_to_a_node_the_store_dropped_is_not_written_back() {
    let path = temp_dir("edit-dropped").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.edit_field(&note, field::COLOR, red());
    app.remove_node(&note);
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert!(!doc.nodes[&note.parse().unwrap()].is_live(), "the delete stands; no placeholder or recreate");
}

#[test]
fn a_non_store_id_edits_the_mirror_and_queues_nothing() {
    let mut app = App::default();
    app.edit_field("r", field::COLOR, red());
    assert_eq!(app.mirror.color("r"), Some(egui::Color32::RED));
    assert!(app.pending.is_empty());
}

#[test]
fn an_unchanged_edit_queues_nothing() {
    let path = temp_dir("edit-noop").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    app.edit_field(&note, field::COLOR, FieldValue::Color(None));
    assert!(app.pending.is_empty());
}

#[test]
fn diff_leaves_mirrored_fields_of_existing_nodes_to_pending() {
    let mut doc = Doc::new(DocId([1; 16]));
    let mut live = BTreeMap::from([(id(0), note("a", None))]);
    let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
    apply(&mut doc, ops);
    live.get_mut(&id(0)).unwrap().fields.insert(field::COLOR, red());
    assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
}
```

Also delete the assertion `assert_eq!(live.color, Some(egui::Color32::RED));` in `a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another` and replace it (and the later `app.nodes[&note].color` assertion) with `app.mirror.color(&note)` (same expected values); the `PINNED` half of that test stays until Task 3.

- [ ] **Step 2: Run to confirm they fail**

Run: `cargo test -j 8 -p manifold --bins session_sync:: 2>&1 | head -40`
Expected: FAIL to compile (`edit_field`, `app.mirror`, `app.pending` do not exist).

- [ ] **Step 3: Add the state and the writer**

In `main.rs`, beside `explicit` (~line 1018) add:

```rust
    /// The decoded copy of the document's mirrored fields (`mirror::Mirror`).
    mirror: mirror::Mirror,
    /// Local edits to mirrored fields that the next save commits as `Set`s, by node and field. Cleared by a
    /// successful commit.
    pending: std::collections::BTreeMap<(store::model::NodeId, u32), Vec<u8>>,
```

and in the `App` default literal (~line 1525) add `mirror: mirror::Mirror::default(), pending: std::collections::BTreeMap::new(),`.

In `session_sync.rs`, imports: `use crate::mirror::MIRRORED;`. In `impl App` (next to `mark`) add:

```rust
    /// The one writer of mirrored fields (node model spec, 2b "Read model"): the mirror shows `value` at once, and
    /// the next save commits it as a `Set`. An id that isn't a store id (a hand-built test node) only updates the
    /// mirror. A value the mirror already shows queues nothing.
    pub(crate) fn edit_field(&mut self, id: &str, no: u32, value: FieldValue) {
        debug_assert!(MIRRORED.contains(&no), "field {no} is not mirrored");
        if self.mirror.get(id, no) == value {
            return;
        }
        if let Ok(node) = id.parse::<NodeId>() {
            self.pending.insert((node, no), encode_field(no, &value));
        }
        self.mirror.set(id, no, value);
    }

    /// Shows another replica's values of the mirrored fields in `fields`, except where this replica has an edit
    /// waiting for a save: it commits next and wins (last writer, per field).
    fn mirror_remote(&mut self, id: &str, fields: &Fields) {
        let node = id.parse::<NodeId>().ok();
        for &no in MIRRORED {
            let Some(value) = fields.get(&no) else { continue };
            if node.is_some_and(|node| self.pending.contains_key(&(node, no))) {
                continue;
            }
            self.mirror.set(id, no, value.clone());
        }
    }
```

Remove the three Task 1 `cfg_attr(allow(dead_code))` lines in `mirror.rs`.

- [ ] **Step 4: Commit `pending` in `save_session`, and make `diff` skip mirrored fields**

In `diff`, inside the loop over `node.fields` for existing nodes, directly after the `explicit` check, add:

```rust
            // A mirrored field is written by `App::pending`, not compared.
            if MIRRORED.contains(&no) {
                continue;
            }
```

In `save_session`, replace `let ops = diff(txn.doc(), &live, &self.explicit, &keep);` with:

```rust
            let mut ops = diff(txn.doc(), &live, &self.explicit, &keep);
            // Edits to mirrored fields. A node this commit creates carries its value in the `Create`; one the
            // document doesn't show live would get a placeholder from a `Set`, so its edit is dropped.
            let created: BTreeSet<NodeId> = ops
                .iter()
                .filter_map(|op| match op {
                    OpBody::Create { node, .. } => Some(*node),
                    _ => None,
                })
                .collect();
            let doc = txn.doc();
            ops.extend(
                self.pending
                    .iter()
                    .filter(|((node, _), _)| !created.contains(node) && doc.nodes.get(node).is_some_and(Node::is_live))
                    .map(|(&(node, no), enc)| OpBody::Set { node, field: no, enc: enc.clone() }),
            );
```

In the `Ok(())` arm, next to `self.explicit.clear();` add:

```rust
                    self.pending.clear();
                    let nodes = &self.nodes;
                    self.mirror.retain(|id| nodes.contains_key(id));
```

- [ ] **Step 5: Read and write COLOR through the mirror in the sync layer**

- `shared_nodes`: replace `(field::COLOR, color_value(node.color))` with `(field::COLOR, self.mirror.get(key, field::COLOR))` (`key` is the `&String` the loop already has). Leave the jar loop on `def.color` (Ruling 6).
- `materialize`: delete `let color = fields.get(&field::COLOR).and_then(color_of);` and the `color` argument of `update_live_node`; call `self.mirror_remote(&id, fields)` once, after the `spawning` early return and before `update_live_node`.
- `update_live_node`: delete the `color` parameter, the `changed(field::COLOR)` block and `#[allow(clippy::too_many_arguments)]` if the count then permits (otherwise keep it).
- `insert_materialized`: delete its `color` local; the new node's colour is already in the mirror from `materialize`'s `mirror_remote`. (`materialize` calls `mirror_remote` before either path, so a new node arrives with its stored colour.)
- `spawn_shared` keeps `color_value(spawn.carry.color)`.

- [ ] **Step 6: Remove `GraphNode::color` and fix every site the compiler names**

Delete the `color` field and its doc comment from `GraphNode` in `node.rs`. Run `cargo check -j 8 -p manifold --all-targets` and fix each error with this table:

| Was | Now |
|---|---|
| read `node.color` (card view, menu view, debug state, close/wake/respawn carry) | `self.mirror.color(id)` with the node's id string |
| `GraphNode { .., color: None, .. }` or `color,` in a literal | drop the field |
| `GraphNode { color, .. }` at tab spawn landing (`tabs.rs` ~423) and failed-card landing (`tabs.rs` ~560) | drop the field; after the insert call `self.mirror.init(&id, [(field::COLOR, session_sync::color_value(color))]);` |
| `node.color = color;` copy-back after replacing a node under the same id (`fetched.rs` ~243, ~330) | delete it and the `color` it carried (Ruling 8) |
| `NodeAction::SetColor(color)` | `self.edit_field(id, field::COLOR, session_sync::color_value(color));` |
| `NodeAction::CycleColor` | `let next = next_tag_color(self.mirror.color(id)); self.edit_field(id, field::COLOR, session_sync::color_value(next));` |
| tests that set `node.color` directly (`fetched.rs` ~1069, ~1097) | `app.edit_field("r", field::COLOR, color_value(Some(egui::Color32::RED)))`; read back with `app.mirror.color("r")` |

`SetColor` and `CycleColor` keep their existing behaviour for kinds that have no colour (`Download`): check `kind_fields(kind).contains(&field::COLOR)` is already true for every kind but `Jar`'s separate path, so no new guard is needed; run the node-menu tests to confirm.

- [ ] **Step 7: Run everything**

Run: `cargo test -j 8 -p manifold --bins`
Expected: PASS, with the baseline plus this task's new tests (726 + 8 new from this task and Task 1's 4 = 738, 4 ignored). Any other failure is a site the table didn't cover: fix it at its cause.

Run: `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings`
Expected: both clean.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add -A crates
git commit -m "Colour lives in the mirror; one writer queues the op

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: PINNED onto the mirror

**Files:**
- Modify: `crates/manifold/src/mirror.rs` (`MIRRORED`, `Mirror::pinned`)
- Modify: `crates/manifold/src/node.rs` (remove `pinned` from both tab payloads, remove `NodePayload::pinned`, `pinned_mut`)
- Modify: `session_sync.rs`, `main.rs` (~752, ~785, ~810, ~2438, `set_all_pinned` ~2565), `node_menu.rs` (~676, ~775), `shortcuts.rs` (~204), `popups.rs` (~1039-1041), `graph_nav.rs` (~430-438), `card.rs` (~938 and its tests), `debug_control.rs` (~1060), `tabs.rs` (~100, ~346-440), tests
- Test: `crates/manifold/src/session_sync/tests.rs`, `node_menu.rs` tests, `node.rs` tests

**Interfaces:**
- Consumes: Task 2's `edit_field`, `mirror_remote`, `pending`, `Mirror::init`.
- Produces: `Mirror::pinned(&self, id: &str) -> bool`; `MIRRORED == [COLOR, PINNED]`.

- [ ] **Step 1: Write the failing tests**

Append to `session_sync/tests.rs`:

```rust
#[test]
fn a_tab_pin_is_committed_and_a_remote_pin_reaches_the_mirror() {
    let path = temp_dir("pin-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, NodePayload::Unloaded(failed_tab()));
    assert!(app.save_session());
    app.edit_field(&tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.mirror.pinned(&tab));
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::PINNED), FieldValue::Bool(true));

    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(false));
    assert!(app.save_session());
    assert!(!app.mirror.pinned(&tab));
}

#[test]
fn pinning_skips_a_node_that_is_not_a_tab() {
    let mut app = App::default();
    let note = a_note(&mut app);
    app.focused = Some(note.clone());
    app.pin_focused_tab_shortcut();
    assert!(!app.mirror.pinned(&note));
}

#[test]
fn pin_all_pins_every_tab_and_only_tabs() {
    let mut app = App::default();
    let (a, b) = (add(&mut app, NodePayload::Unloaded(failed_tab())), add(&mut app, NodePayload::Unloaded(failed_tab())));
    let note = a_note(&mut app);
    app.set_all_pinned(true);
    assert!(app.mirror.pinned(&a) && app.mirror.pinned(&b) && !app.mirror.pinned(&note));
    app.set_all_pinned(false);
    assert!(!app.mirror.pinned(&a) && !app.mirror.pinned(&b));
}
```

and the helper (use it to replace the repeated `UnloadedTabPayload { .. }` literals in this file as the compiler forces edits there):

```rust
fn failed_tab() -> UnloadedTabPayload {
    UnloadedTabPayload {
        url: "https://u0/".into(),
        title: String::new(),
        kind: EngineKind::Servo,
        muted: false,
        zoom_factor: 1.0,
        state: UnloadedState::Failed("x".into()),
        jar: Default::default(),
    }
}
```

- [ ] **Step 2: Run to confirm they fail**

Run: `cargo test -j 8 -p manifold --bins session_sync:: 2>&1 | head -30`
Expected: FAIL to compile (`Mirror::pinned` missing; `failed_tab` has no `pinned` while the payload still does).

- [ ] **Step 3: Extend the mirror**

In `mirror.rs`: `pub(crate) const MIRRORED: &[u32] = &[field::COLOR, field::PINNED];` and

```rust
    pub(crate) fn pinned(&self, id: &str) -> bool {
        matches!(self.get(id, field::PINNED), FieldValue::Bool(true))
    }
```

Add a unit test in `mirror::tests`:

```rust
    #[test]
    fn pinned_reads_a_bool_and_defaults_to_false() {
        let mut mirror = Mirror::default();
        assert!(!mirror.pinned("7"));
        mirror.set("7", field::PINNED, FieldValue::Bool(true));
        assert!(mirror.pinned("7"));
    }
```

- [ ] **Step 4: Move the sync layer**

- `shared_nodes`: `tab_fields(.., *pinned, ..)` now takes `self.mirror.pinned(key)`; remove `pinned` from the `TabPayload` / `UnloadedTabPayload` patterns there.
- `spawn_shared` keeps `spawn.carry.pinned`.
- `update_live_node`: delete both `if changed(field::PINNED) { .. }` blocks (`mirror_remote` already handles it, Task 2 Step 5).
- `insert_materialized`: delete the `bool_of(fields, field::PINNED)` reads and the `pinned` field of the `UnloadedTabPayload` literal.
- Landing (`tabs.rs`): `self.mirror.init(&id, [(field::COLOR, color_value(color)), (field::PINNED, FieldValue::Bool(pinned))]);` (extends Task 2's call).

- [ ] **Step 5: Remove `pinned` from the payloads and fix every site**

Delete `pinned` from `TabPayload` and `UnloadedTabPayload`, and `NodePayload::pinned` / `pinned_mut`. Fix the compiler's errors with this table:

| Was | Now |
|---|---|
| `node.payload.pinned()` (card view, menu view, debug, popups, graph_nav) | `self.mirror.pinned(id)` with the node's id string |
| `tab.pinned` in `TabCarry`/`TabSpawn` builds (`main.rs` ~810, ~2438; `tabs.rs` ~100, ~374) | `self.mirror.pinned(id)` |
| toggle: `node_menu.rs` `TogglePin`, `shortcuts.rs` `pin_focused_tab_shortcut` | `if self.nodes.get(id).is_some_and(\|n\| n.payload.saved_tab().is_some()) { let on = !self.mirror.pinned(id); self.edit_field(id, field::PINNED, FieldValue::Bool(on)); }` |
| `set_all_pinned` | `let tabs: Vec<String> = self.nodes.iter().filter(\|(_, n)\| n.payload.saved_tab().is_some()).map(\|(id, _)\| id.clone()).collect(); for id in tabs { self.edit_field(&id, field::PINNED, FieldValue::Bool(pin)); }` |
| `graph_nav.rs` pinned set for the layout | `.filter(\|(id, n)\| n.group == self.current_group && self.mirror.pinned(id))` |
| `popups.rs` ~1039-1041 | `self.mirror.pinned(focused_id)` and `.map(\|(id, _)\| self.mirror.pinned(id))` over the saved tabs |
| literals with `pinned: false/true` in test payloads | drop the field; where a test needs a pinned tab, `app.edit_field(&id, field::PINNED, FieldValue::Bool(true))` after `add` |

In `a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another`, replace the `matches!(.. t.pinned ..)` assertion with `app.mirror.pinned(&tab)` and an assertion that the tab's `url` is still `"https://u1/"`.

- [ ] **Step 6: Run everything**

Run: `cargo test -j 8 -p manifold --bins`
Expected: PASS (738 + 4 new in this task = 742, 4 ignored).

Run: `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings`
Expected: both clean.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add -A crates
git commit -m "Pinned lives in the mirror

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Verify against the running app, and record the stage

**Files:**
- Modify: `docs/superpowers/specs/2026-10-04-node-model-design.md`
- Modify: `docs/superpowers/specs/2026-10-05-sync-flow-map.md`

- [ ] **Step 1: Rebuild the exe, then run the probes**

The probes run the existing exe; `cargo check`/`test` do not rebuild it. Build at BelowNormal, then run each and report PASS/FAIL with counts:

```bash
cargo build -j 8 -p manifold
scripts/headless-smoke-test.sh
scripts/two-instance-test.sh
```

Expected: the smoke test passes (48/48 at the last run); the two-instance test shows a colour or pin set in one instance reaching the other. If `two-instance-test.sh` does not touch colour or pin, extend it with one `SETCOLOR`-style step only if the headless debug command set already has it; otherwise report that the cross-instance path was covered by the unit tests only.

- [ ] **Step 2: Record the stage in the spec and the sync map**

In the node model spec, after the 2b bullet, add: `2b-1 done: <commit hashes of Tasks 1-3>. The mirror, \`App::edit_field\` and \`App::pending\` exist; \`color\` and \`pinned\` read through \`Mirror\`; \`diff\` skips \`MIRRORED\` fields on existing nodes. Next: 2b-2, written when this lands (field groups left: pos, group/parent, url/engine/jar, name, text/view, content kinds; then delete \`diff\`, \`explicit\`, \`keep\`, the enums and the caps bridge, rewrite fixtures, add the grep guard).`

In the sync map, add a section "After 2b-1" with three lines: mirrored fields are written only by `edit_field`; `save_session` commits `pending` beside `diff`; `mirror_remote` skips a field with a pending edit.

- [ ] **Step 3: Commit**

```bash
git add docs
git commit -m "Docs: record 2b-1 (mirror, writer, colour and pin)

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```
