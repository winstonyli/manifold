---
status: active
---

# Labels travel with a group export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Export Group carries the labels its cards use, and Import Group reuses a same-named label already in the target document.

**Architecture:** `subtree_to_json` appends the used `Label` nodes. `store::json::import` gains a general reuse hook (`import_with`), so identity is decided where a file id becomes a real id; a reused slot keeps its index and is flagged `Imported.reused`. `import_group_json` supplies the hook (label name key to live label) and `materialize_imported` skips reused slots.

**Tech Stack:** Rust, `crates/manifold` (`store/json.rs`, `session_files.rs`), `cargo test -p manifold`.

**Spec:** `docs/superpowers/specs/2026-10-26-label-group-export-design.md`

## Global Constraints

- Matching is `names::name_key` (rule L3); the target's label and colour win.
- `imported[i]` is `json.nodes[i]` stays true for every caller (reused slots keep their place).
- `import(json, next_id, trust)` keeps its signature; the ~30 callers do not change.
- No migration or compatibility code; no format-version constant moves (additive).
- Run `cargo fmt --all` before each commit; the pre-commit hook runs fmt and clippy with `-D warnings`, so no dead code.
- No test touches the real `%APPDATA%\manifold` (use `store::testutil::temp_dir`).

## Review Focus

1. A label no exported card uses is not exported; a deleted (non-live) label is not exported (Task 1).
2. A reused label keeps `imported[i]` aligned with `json.nodes[i]`, so `paste_copy`-style zips stay correct (Task 2).
3. Importing the same group twice into one document leaves exactly one label, not two (Task 3).
4. A file label whose name is empty once cleaned imports as a fresh label (no false match) (Task 3).
5. A file written by an older build (no label nodes) still imports, dropping memberships as today (unchanged path; the existing foreign-import tests cover it).

---

### Task 1: Export carries the labels the subtree uses

**Files:**
- Modify: `crates/manifold/src/store/json.rs` (`subtree_to_json`, ~line 146; tests in the same file's `mod tests`)

**Interfaces:**
- Consumes: `Doc.nodes`, `Node::trees()` (a map from field number to tree; a tree's `.iter()` yields `(TreePath, Leaf)` pairs, as `Mirror::card_labels` and the test `a_labelled_note_and_its_label_node_round_trip_...` use it), `crate::labels::LABELS_KEY`, `Seg::{Key, Id}`, `node_to_json`.
- Produces: `subtree_to_json` output with the used live `Label` nodes appended after the links.

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `store/json.rs`, next to `a_labelled_note_and_its_label_node_round_trip_with_the_membership_following_the_new_id`)

```rust
    #[test]
    fn a_group_export_carries_the_labels_its_cards_use_and_no_others() {
        let json = r##"{"nodes": [
            {"id": "g", "type": "application/vnd.manifold.group", "name": "G"},
            {"id": "a", "type": "text/markdown", "text": "a", "group": "g", "props": {"labels": {"@used": 5}}},
            {"id": "used", "type": "application/vnd.manifold.label", "name": "urgent"},
            {"id": "other", "type": "application/vnd.manifold.label", "name": "unused"}
        ]}"##;
        let doc = doc_from(json, 1);
        let g = doc.live().find(|(_, n)| n.kind() == Kind::Group).map(|(id, _)| id).unwrap();
        let export = subtree_to_json(&doc, g);
        let names: Vec<_> = export
            .nodes
            .iter()
            .filter(|n| n.kind() == Some(Kind::Label))
            .map(|n| n.fields["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["urgent"], "only the label a card in the subtree carries");
        assert_eq!(export.nodes.last().unwrap().kind(), Some(Kind::Label), "labels come after the subtree");
        let back = import(&export, ids(2), Trust::Foreign).unwrap();
        let note = back.iter().find(|n| n.fields.get(&field::TEXT) == Some(&FieldValue::Str("a".into()))).unwrap();
        assert_eq!(note.trees[&field::PROPS].len(), 1, "the membership survives a foreign import");
    }

    #[test]
    fn a_deleted_label_is_not_exported() {
        let json = r##"{"nodes": [
            {"id": "g", "type": "application/vnd.manifold.group", "name": "G"},
            {"id": "a", "type": "text/markdown", "text": "a", "group": "g", "props": {"labels": {"@l": 5}}},
            {"id": "l", "type": "application/vnd.manifold.label", "name": "gone"}
        ]}"##;
        let mut doc = doc_from(json, 1);
        let g = doc.live().find(|(_, n)| n.kind() == Kind::Group).map(|(id, _)| id).unwrap();
        let l = doc.live().find(|(_, n)| n.kind() == Kind::Label).map(|(id, _)| id).unwrap();
        doc.nodes.get_mut(&l).unwrap().deleted = true;
        let export = subtree_to_json(&doc, g);
        assert!(export.nodes.iter().all(|n| n.kind() != Some(Kind::Label)));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j 8 -p manifold group_export_carries_the_labels 2>&1 | tail -15`
Expected: `a_group_export_carries_the_labels_its_cards_use_and_no_others` FAILS (`names` is empty). `a_deleted_label_is_not_exported` passes already (nothing is exported yet). If a test fails to compile on a helper name, fix the test, not the code.

- [ ] **Step 3: Implement** — in `subtree_to_json`, after the links loop and before the final `JsonDoc { nodes, local: None }`:

```rust
    // The labels the exported cards carry (`PROPS/labels/@<id>`), so a membership names a node in the file.
    let mut used = std::collections::BTreeSet::new();
    for exported in &nodes {
        let Some(node) = exported.id.parse::<NodeId>().ok().and_then(|id| doc.nodes.get(&id)) else { continue };
        let trees = node.trees();
        for (path, _) in trees.get(&field::PROPS).into_iter().flat_map(|tree| tree.iter()) {
            if let [Seg::Key(k), Seg::Id(label)] = path.segments().as_slice() {
                if k == crate::labels::LABELS_KEY {
                    used.insert(*label);
                }
            }
        }
    }
    for id in used {
        if let Some(node) = doc.nodes.get(&id).filter(|n| !n.deleted && n.kind() == Kind::Label) {
            nodes.push(node_to_json(id, node, None));
        }
    }
```

If `tree.iter()` yields a different item shape than `(path, leaf)`, adapt the destructuring to it. Update the doc comment of `subtree_to_json`: "A link is included when both its ends are in the export, and so is every live label a card in it carries."

- [ ] **Step 4: Run to verify they pass, and the file's other tests**

Run: `cargo test -j 8 -p manifold store::json 2>&1 | tail -8`
Expected: all pass, including the test that a closed group exports nothing (`nodes.is_empty()`): with no cards there are no labels either.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add crates/manifold/src/store/json.rs
git commit -m "json: a group export carries the labels its cards use"
```

---

### Task 2: `import_with`, the reuse hook

**Files:**
- Modify: `crates/manifold/src/store/json.rs` (`Imported`, `import`, `creates`; tests in the same file)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn import_with(json: &JsonDoc, next_id: impl FnMut() -> NodeId, trust: Trust, reuse: impl Fn(&JsonNode) -> Option<NodeId>) -> Result<Vec<Imported>, String>`. `import(..)` calls it with `|_| None`. `Imported` gains `pub reused: bool`. `creates(&[Imported])` skips reused slots.

- [ ] **Step 1: Write the failing test** (in `mod tests`)

```rust
    #[test]
    fn a_reused_node_takes_the_existing_id_and_keeps_its_slot() {
        let json = r##"{"nodes": [
            {"id": "a", "type": "text/markdown", "text": "a", "props": {"labels": {"@l": 5}}},
            {"id": "l", "type": "application/vnd.manifold.label", "name": "urgent"}
        ]}"##;
        let parsed: JsonDoc = serde_json::from_str(json).unwrap();
        let existing = id(9, 9);
        let imported = import_with(&parsed, ids(1), Trust::Foreign, |n| (n.id == "l").then_some(existing)).unwrap();
        assert_eq!(imported.len(), parsed.nodes.len(), "one slot per file node");
        assert!(!imported[0].reused && imported[1].reused);
        assert_eq!(imported[1].id, existing);
        let (path, _) = imported[0].trees[&field::PROPS].iter().next().unwrap();
        assert_eq!(path.segments().last(), Some(&Seg::Id(existing)), "the membership names the reused id");
        assert_eq!(creates(&imported).len(), 1, "a reused slot creates nothing");
    }
```

(`id(actor, counter)` is this module's helper; if the module lacks it, use `NodeId { actor: ActorId(9), counter: 9 }`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j 8 -p manifold a_reused_node_takes 2>&1 | tail -8`
Expected: compile error, `import_with` not found.

- [ ] **Step 3: Implement.**

1. `Imported`: add `pub reused: bool,` after `trees`. The one construction site (`out.push(Imported { .. })` at the end of the node loop) sets `reused: reused.contains(node.id.as_str())`.
2. Rename `pub fn import` to `pub fn import_with` with the final parameter `reuse: impl Fn(&JsonNode) -> Option<NodeId>`, and add above it:

```rust
pub fn import(json: &JsonDoc, next_id: impl FnMut() -> NodeId, trust: Trust) -> Result<Vec<Imported>, String> {
    import_with(json, next_id, trust, |_| None)
}
```

3. In `import_with`, replace the `fresh` loop with:

```rust
    let mut fresh = HashMap::new();
    let mut reused = std::collections::HashSet::new();
    for node in &json.nodes {
        let id = match reuse(node) {
            Some(existing) => {
                reused.insert(node.id.as_str());
                existing
            }
            None => next_id(),
        };
        if fresh.insert(node.id.as_str(), id).is_some() {
            return Err(format!("node id {:?} appears twice", node.id));
        }
    }
```

4. Add to the doc comment: "`reuse` may name an existing node for a file node: its slot is flagged `reused`, keeps its place, and every reference to it resolves to that id. `import` reuses none."
5. `creates` (~line 318): skip reused slots by filtering `imported.iter().filter(|i| !i.reused)`; read its body first and keep its shape.

- [ ] **Step 4: Run to verify it passes, then the file and a crate build**

Run: `cargo test -j 8 -p manifold store::json 2>&1 | tail -6` then `cargo build -j 8 -p manifold 2>&1 | grep -E "^error|Finished"`
Expected: all pass and the crate builds. If another file constructs `Imported {..}`, add `reused: false` there.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add crates/manifold/src/store/json.rs
git commit -m "json: import_with, a hook that reuses an existing node for a file node"
```

---

### Task 3: Import Group reuses a same-named label

**Files:**
- Modify: `crates/manifold/src/session_files.rs` (`import_group_json` ~line 276, `materialize_imported` ~line 308)
- Test: `crates/manifold/src/session_sync/tests/labels.rs`

**Interfaces:**
- Consumes: `import_with` and `Imported.reused` (Task 2); `App::label_defs()` (id to `LabelDef { name, color }`); `names::name_key`; Task 1's export.
- Produces: `import_group_json` merges file labels into live ones by name key.

- [ ] **Step 1: Write the failing tests** (append to `session_sync/tests/labels.rs`)

```rust
/// A group with one labelled note in `app`, exported.
fn labelled_group_export(app: &mut App, label: &str, color: egui::Color32) -> crate::store::json::JsonDoc {
    let g = add_group(app, "G");
    let n = add_note(app, "n");
    app.edit_group(n, Some(g));
    let l = app.create_label(label, Some(color)).unwrap();
    app.set_label(&[n], l, true);
    assert!(app.save_session());
    app.export_group_json(g).unwrap()
}

fn label_names(app: &App) -> Vec<String> {
    let mut names: Vec<_> = app.label_defs().values().map(|d| d.name.clone()).collect();
    names.sort();
    names
}

#[test]
fn a_group_export_brings_its_label_to_a_document_without_it() {
    let mut a = app_on(&temp_dir("label-export-a").join("session.mnf"), 1);
    let json = labelled_group_export(&mut a, "urgent", egui::Color32::RED);
    let mut b = app_on(&temp_dir("label-export-b").join("session.mnf"), 2);
    b.import_group_json(&json);
    assert_eq!(label_names(&b), ["urgent"]);
    let (id, def) = b.label_defs().into_iter().next().unwrap();
    assert_eq!(def.color, Some(egui::Color32::RED));
    assert!(b.nodes.keys().any(|n| b.doc.mirror.card_labels(*n) == [id]), "the imported note carries the label");
}

#[test]
fn a_group_export_reuses_a_same_named_label_and_keeps_the_targets_colour() {
    let mut a = app_on(&temp_dir("label-merge-a").join("session.mnf"), 1);
    let json = labelled_group_export(&mut a, "Urgent", egui::Color32::RED);
    let mut b = app_on(&temp_dir("label-merge-b").join("session.mnf"), 2);
    let mine = b.create_label("urgent", Some(egui::Color32::BLUE)).unwrap();
    b.import_group_json(&json);
    b.import_group_json(&json);
    assert_eq!(label_names(&b), ["urgent"], "one label after two imports, named as the target has it");
    assert_eq!(b.doc.mirror.label_def(mine).color, Some(egui::Color32::BLUE));
    let carriers = b.nodes.keys().filter(|n| b.doc.mirror.card_labels(**n) == [mine]).count();
    assert_eq!(carriers, 2, "both imported notes carry the target's label");
}

#[test]
fn a_file_label_with_an_unusable_name_imports_as_its_own_label() {
    let mut b = app_on(&temp_dir("label-blank").join("session.mnf"), 2);
    b.create_label("x", None).unwrap();
    let json: crate::store::json::JsonDoc = serde_json::from_str(
        r#"{"nodes": [{"id": "g", "type": "application/vnd.manifold.group", "name": "G"},
            {"id": "l", "type": "application/vnd.manifold.label", "name": "​"}]}"#,
    )
    .unwrap();
    b.import_group_json(&json);
    assert!(b.label_defs().len() <= 2, "no false match with the live label");
}
```

Adapt to what the file already imports (`add_group`, `add_note`, `app_on`, `temp_dir` come from `super::*`). If `label_defs()` returns pairs rather than a map, use `.map(|(_, d)| ..)`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j 8 -p manifold group_export_ 2>&1 | tail -12`
Expected: `brings_its_label` and `reuses_a_same_named_label` FAIL (no label, or two labels); the third passes vacuously until implemented.

- [ ] **Step 3: Implement.**

In `import_group_json`, replace the `let next_id = ...` line and the `import(..)` call with:

```rust
        let live: std::collections::HashMap<String, NodeId> = self
            .label_defs()
            .into_iter()
            .map(|(id, def)| (crate::names::name_key(&def.name), id))
            .filter(|(key, _)| !key.is_empty())
            .collect();
        let reuse = |node: &crate::store::json::JsonNode| {
            if node.kind() != Some(Kind::Label) {
                return None;
            }
            let key = crate::names::name_key(node.fields.get("name")?.as_str()?);
            live.get(&key).copied()
        };
        let next_id = || self.alloc_node_id();
        let imported = match crate::store::json::import_with(json, next_id, crate::store::json::Trust::Foreign, reuse) {
```

(`live` is built before `next_id` borrows `self` mutably; keep the existing `Ok(imported) => imported, Err(err) => { toast; return }` arms.) In `materialize_imported`, right after `let mut imported = imported;` add:

```rust
        // A node `import_with` matched to an existing one (a label with a live twin) is already in the document.
        imported.retain(|i| !i.reused);
```

Extend `import_group_json`'s doc comment with: "A label in the file whose name key matches a live label is not created: its cards carry the live one (`import_with`)."

- [ ] **Step 4: Run to verify they pass, then the label, files and json suites**

Run: `cargo test -j 8 -p manifold label 2>&1 | tail -6`, then `cargo test -j 8 -p manifold session_sync::tests::files 2>&1 | tail -6`, then `cargo test -j 8 -p manifold store::json 2>&1 | tail -4`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add crates/manifold/src/session_files.rs crates/manifold/src/session_sync/tests/labels.rs
git commit -m "labels: Import Group reuses a same-named label"
```

---

### Task 4: Docs, full suite, gate

**Files:**
- Modify: `README.md` (line ~194), `PARKED.md`, `ROADMAP.md`, `docs/superpowers/specs/2026-10-26-label-group-export-design.md`, this plan

- [ ] **Step 1: Docs.**
  - `README.md`, **Labels** paragraph: replace "A subtree copy drops labels (parked: PARKED.md \"Label export with a subtree copy\"); a whole-document export keeps them." with "A group export carries the labels its cards use, and Import Group reuses a live label with the same name (its colour wins); a whole-document export keeps them all."
  - `PARKED.md`: delete the "Label export with a subtree copy (2026-10-24)." entry. In "Label name collisions from import and sync" add: "`store::json::import_with`'s reuse hook (2026-10-26) is the mechanism the merge half of this needs."
  - `ROADMAP.md`: remove "label export with a subtree copy" from the v0.10 label line.
  - Spec: in its Testing section replace the gate bullet with "Gate: `light`. The App-level tests drive `export_group_json` and `import_group_json`, the functions the menu and `EXPORTGROUP`/`IMPORTGROUP` call; the gate group stages must still pass." Set the spec and this plan to `status: done`.
  - Check for stale wording: `grep -rn "drops labels\|subtree copy" README.md PARKED.md ROADMAP.md docs --include=*.md`; fix whatever is not history (plans and specs already marked done keep their text).

- [ ] **Step 2: Full suite**

Run: `cargo test -j 8 -p manifold 2>&1 | grep -E "test result|FAILED|panicked" | head`
Expected: no failures.

- [ ] **Step 3: Commit, then gate**

```bash
git add -A && git commit -m "docs: labels travel with a group export"
```

Build and run `gate all LOG light` through `~/.claude/runs/run.ps1` (RUNS.md, `-Kind gate`), after `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`. Expected: every stage exit 0. Set the headline in `docs/runs.md` and commit.
