---
status: draft
---

# Label name collisions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Same-key live labels read as one label (the lowest `NodeId`), on every replica, with no write that can race.

**Architecture:** `Mirror` keeps a member-to-canonical map for labels, rebuilt at its three label write points. Reads
(`card_labels`, `label_defs`) see only canonical labels; writes (`set_label`, rename, recolor, delete) address the whole
group. No stored shape changes.

**Tech Stack:** Rust, egui; tests in `mirror.rs` and `session_sync/tests/labels.rs`.

**Spec:** `docs/superpowers/specs/2026-10-26-label-name-collisions-design.md`

## Global Constraints

- `cargo fmt --all` before each commit; clippy `-D warnings` is a pre-commit hook (never `--no-verify`).
- A raw `.name` read outside the allowed files fails `names::tests`; label name reads stay in `mirror.rs`/`labels.rs`.
- Commit messages end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Python, if used, runs as `python -P f.py < /dev/null`.

## Review Focus

- A card carrying only the duplicate: the chip shows once; unlabel removes it (Task 2 test).
- Three members: lowest id wins, the others fold in (Task 1 test).
- Rename of the canonical label must not promote its duplicate (Task 2 test).
- Delete leaves no label behind (Task 2 test).
- A label whose key is empty is never grouped (Task 1 test).

---

### Task 1: Mirror groups same-key labels; reads see the canonical one

**Files:**
- Modify: `crates/manifold/src/mirror.rs` (struct, `set`, `add_stored`, `forget_stored`, `card_labels`; new `regroup_labels`, `canonical_label`, `label_members`; tests module at ~486)
- Modify: `crates/manifold/src/labels.rs` (`label_defs`)

**Interfaces:**
- Produces: `Mirror::canonical_label(&self, id: NodeId) -> NodeId`; `Mirror::label_members(&self, canonical: NodeId) -> Vec<NodeId>` (ascending, canonical first); `App::label_defs()` now canonical-only; `Mirror::card_labels` returns canonical ids, no repeats.

- [ ] **Step 1: Write the failing tests** in `mirror.rs` `mod tests` (adapt helper names to that module: it builds a `Mirror` and calls `add_stored(id, Kind::Label)` then `set(id, field::NAME, FieldValue::Str(..))`):

```rust
fn label(m: &mut Mirror, id: u64, name: &str) -> NodeId {
    let id = NodeId::from_raw(id); // use the module's existing id constructor
    m.add_stored(id, Kind::Label);
    m.set(id, field::NAME, FieldValue::Str(name.to_owned()));
    id
}

#[test]
fn same_key_labels_group_under_the_lowest_id() {
    let mut m = Mirror::default();
    let (c, a, b) = (label(&mut m, 5, "Urgent"), label(&mut m, 2, "urgent"), label(&mut m, 9, " URGENT "));
    assert_eq!([m.canonical_label(a), m.canonical_label(b), m.canonical_label(c)], [a, a, a]);
    assert_eq!(m.label_members(a), vec![a, c, b]);
}

#[test]
fn an_empty_key_is_its_own_group() {
    let mut m = Mirror::default();
    let (a, b) = (label(&mut m, 1, "\u{200b}"), label(&mut m, 2, "\u{200b}"));
    assert_eq!([m.canonical_label(a), m.canonical_label(b)], [a, b]);
}

#[test]
fn renaming_a_member_splits_and_reforms_the_group() {
    let mut m = Mirror::default();
    let (a, b) = (label(&mut m, 1, "x"), label(&mut m, 2, "x"));
    m.set(a, field::NAME, FieldValue::Str("y".to_owned()));
    assert_eq!([m.canonical_label(a), m.canonical_label(b)], [a, b]);
    m.set(a, field::NAME, FieldValue::Str("x".to_owned()));
    assert_eq!(m.canonical_label(b), a);
    m.forget_stored(a);
    assert_eq!(m.canonical_label(b), b, "the survivor is canonical again");
}

#[test]
fn a_card_carrying_two_members_shows_one_chip_with_the_earlier_stamp() {
    // build a card with PROPS labels/@a = 20 and labels/@b = 10 (the module's existing tree helper), a < b, same key
    // expect card_labels(card) == vec![a]
}
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold mirror::tests 2>&1 | tail`. Expected: compile error or FAIL (`canonical_label` missing).

- [ ] **Step 3: Implement.** In `Mirror` add `label_group: HashMap<NodeId, NodeId>` (member to canonical; identity ids absent) with a doc line. Then:

```rust
/// Rebuilds `label_group`: live labels with the same non-empty `name_key` fold into the lowest id.
fn regroup_labels(&mut self) {
    let mut first: HashMap<String, NodeId> = HashMap::new();
    let mut group = HashMap::new();
    for id in self.stored_ids(Kind::Label) {
        let key = crate::names::name_key(self.str_field(id, field::NAME));
        if key.is_empty() {
            continue;
        }
        match first.get(&key) {
            Some(&canonical) => {
                group.insert(id, canonical);
            }
            None => {
                first.insert(key, id);
            }
        }
    }
    self.label_group = group;
}

/// The label `id` reads as: the lowest id among live labels with its name key (itself when alone or unnamed).
pub(crate) fn canonical_label(&self, id: NodeId) -> NodeId {
    self.label_group.get(&id).copied().unwrap_or(id)
}

/// `canonical` and every label folded into it, ascending.
pub(crate) fn label_members(&self, canonical: NodeId) -> Vec<NodeId> {
    let mut all: Vec<NodeId> =
        self.label_group.iter().filter(|&(_, &c)| c == canonical).map(|(&m, _)| m).collect();
    all.push(canonical);
    all.sort_unstable();
    all
}
```

Call `self.regroup_labels()` at the end of `add_stored` when `kind == Kind::Label`; in `set` after the insert when `no == field::NAME && self.stored_nodes.contains(&id) && self.kind_of(id) == Kind::Label`; in `forget_stored` after `forget_node` when the node was a `Label` (read `kind_of` before forgetting). In `card_labels`, after the existing `.filter(..)` map each pair to `(at, self.canonical_label(label))`, sort, then keep the first entry per label:

```rust
found.sort_unstable();
let mut seen = BTreeSet::new();
found.into_iter().filter(|(_, l)| seen.insert(*l)).map(|(_, l)| l).collect()
```

Update `card_labels`'s doc comment (canonical ids, one per name). In `labels.rs`, `label_defs` filters `.filter(|&id| self.doc.mirror.canonical_label(id) == id)`; update its doc ("the live labels, one per name, by id").

- [ ] **Step 4: Run** `cargo test -j 8 -p manifold mirror label names 2>&1 | grep -E "^error|test result|FAILED"`. Expected: all pass, including the existing label tests.
- [ ] **Step 5: Commit** `cargo fmt --all && git add crates && git commit -m "labels: same-key labels read as one (the lowest id)"`.

### Task 2: Writes address the whole group

**Files:**
- Modify: `crates/manifold/src/labels.rs` (`set_label`, `rename_label`, `recolor_label`, `delete_label`)
- Test: `crates/manifold/src/session_sync/tests/labels.rs`

**Interfaces:**
- Consumes: `Mirror::canonical_label`, `Mirror::label_members` (Task 1).

- [ ] **Step 1: Write the failing tests.** Add a helper that makes a colliding second label the way a remote replica would (use the idiom at `session_sync/tests/remote.rs:495`: `app.apply_remote(vec![(id, Remote::Shown(snap(Kind::Label, named("urgent").fields, None)))])` with an id above the local label's, taking the id from `app.alloc_node_id()`; adapt the exact `snap`/`named` helpers):

```rust
/// `app` with a local label "urgent" and a remote duplicate of it with a higher id. Returns (canonical, duplicate).
fn collided(app: &mut App) -> (NodeId, NodeId) { /* create_label("urgent") then apply_remote the duplicate */ }

#[test]
fn a_collision_shows_one_row_and_a_card_with_only_the_duplicate_shows_one_chip() {
    let mut app = app_on(&temp_dir("label-collide-a").join("session.mnf"), 1);
    let (a, b) = collided(&mut app);
    assert_eq!(app.label_defs().keys().copied().collect::<Vec<_>>(), [a]);
    let n = add_note(&mut app, "n");
    // membership under the duplicate only
    app.edit_prop(n, field::PROPS, labels_path(b), Some(Leaf::Int(5)));
    assert_eq!(app.doc.mirror.card_labels(n), [a]);
    app.set_label(&[n], a, false);
    assert!(app.doc.mirror.card_labels(n).is_empty(), "unlabel clears a card that carried the duplicate");
}

#[test]
fn renaming_recolouring_and_deleting_reach_every_member() {
    let mut app = app_on(&temp_dir("label-collide-b").join("session.mnf"), 1);
    let (a, b) = collided(&mut app);
    assert!(app.rename_label(a, "later"));
    assert_eq!(app.doc.mirror.label_def(b).name, "later");
    assert_eq!(app.label_defs().keys().copied().collect::<Vec<_>>(), [a], "no promotion of the duplicate");
    app.recolor_label(a, Some(egui::Color32::RED));
    assert_eq!(app.doc.mirror.label_def(b).color, Some(egui::Color32::RED));
    app.delete_label(a);
    assert!(app.label_defs().is_empty());
}

#[test]
fn a_rename_to_the_groups_own_name_is_not_a_collision() {
    let mut app = app_on(&temp_dir("label-collide-c").join("session.mnf"), 1);
    let (a, _) = collided(&mut app);
    assert!(app.rename_label(a, "URGENT"), "the duplicate is the same label, not another");
}
```

- [ ] **Step 2: Run** `cargo test -j 8 -p manifold collide 2>&1 | tail`. Expected: FAIL (unlabel leaves the chip; rename leaves `b`'s name; delete leaves `b`).

- [ ] **Step 3: Implement** in `labels.rs`:
  - `set_label`: `let canon = self.doc.mirror.canonical_label(label); let members = self.doc.mirror.label_members(canon);` (keep the `is_stored(label)` guard). Per card, `has` is `members.iter().any(|&m| tree.contains_key(&labels_path(m)))`. On: `edit_prop(card, PROPS, labels_path(canon), Some(Int(..)))`. Off: for each member whose path the tree holds, `edit_prop(card, PROPS, labels_path(m), None)`.
  - `rename_label`: the uniqueness check passes `Some(self.doc.mirror.canonical_label(id))` as `except`; then `for m in label_members(canonical_label(id)) { edit_field(m, field::NAME, Str(name.clone())) }`.
  - `recolor_label`: the same loop over the group for `field::COLOR`.
  - `delete_label`: take `label_members(canonical_label(id))` first, then `forget_stored` and `queue_delete` each (collect the members before forgetting, since forgetting regroups).
  - Update the four doc comments to say "every label with the same name".

- [ ] **Step 4: Run** `cargo test -j 8 -p manifold label collide 2>&1 | grep -E "^error|test result|FAILED"`. Expected: pass.
- [ ] **Step 5: Commit** `cargo fmt --all && git add crates && git commit -m "labels: edits and unlabel address every same-named label"`.

### Task 3: Import Group, docs, gate

**Files:**
- Test: `crates/manifold/src/session_sync/tests/labels.rs`
- Modify: `PARKED.md`, `ROADMAP.md`, `README.md` (the **Labels** paragraph), spec and plan front matter, `docs/runs.md`

- [ ] **Step 1: Test** that Import Group reuses the canonical label when the target holds duplicates: use `collided(&mut b)`, then `b.import_group_json(&labelled_group_export(&mut a, "urgent", RED))`; assert `label_defs()` still holds one label and the imported note's `card_labels` equals `[a_canonical]`. Run it; it should pass without code changes (`label_keys` reads `label_defs`, now canonical). If it fails, fix at the cause.
- [ ] **Step 2: Docs.** Delete the PARKED.md "Label name collisions from import and sync" entry. ROADMAP v0.10 label line: drop "name collisions from import and sync". README **Labels** paragraph: add one sentence that same-named labels (from two replicas) read as one label, the lowest id, and edits apply to all of them. Set the spec and plan `status: done`. Grep README/PARKED/ROADMAP/docs for `collision` and `jar_fragment` and fix stale wording (use `cut -c1-200` on grep output).
- [ ] **Step 3: Full tests** `cargo test -j 8 -p manifold 2>&1 | grep -E "^error|test result|FAILED"`. Expected: all pass.
- [ ] **Step 4: Commit** the tests and docs.
- [ ] **Step 5: Gate.** `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`, then `target\debug\gate.exe all LOG light` through `~/.claude/runs/run.ps1` (`-Kind gate -Name label-collisions-light`, launch from a `.ps1` file with `Start-Process`, wait on the run record's `end` with a deadline). Set the headline in `run.json` and `docs/runs.md`, commit.
