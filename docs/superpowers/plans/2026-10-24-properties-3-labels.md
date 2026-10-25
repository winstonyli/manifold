---
status: done
---

# Properties, plan 3: labels

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (the user's default) or superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Cards carry labels (global, named, coloured) that show as the card border and header chips, are found by the filter box and the omnibar (`#label`), are applied to the selection from the bar in one undo step, and can be applied from a suggestion. They replace the per-card `color` field.

**Architecture:** A label is a new stored-only node kind `Kind::Label` (fields `NAME`, `COLOR`), registered in the mirror beside links. A card's membership is `PROPS/labels/Id(label)` written through plan 2's `App::edit_prop`; everything else (display, find, act) reads `Mirror::card_labels`. Per-card colour code is deleted, not kept beside labels.

**Tech Stack:** Rust, egui, the existing store/mirror/undo; gate stage `labels` in `crates/gate`.

**Spec:** `docs/superpowers/specs/2026-10-24-properties-design.md` (section 3, D3, D5, D7); decision record `docs/decisions/2026-10-24-properties-tags-json.md`. Plans 1 and 2 are done (`docs/superpowers/plans/2026-10-24-properties-{1-store,2-app}.md`).

## Global Constraints

- `kind: app`. Work on `main`; run `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass (the pre-commit hook checks both). Never `--no-verify`.
- Tests never touch `%APPDATA%\manifold` (`profile::dir()` is `None` under `cfg(test)`).
- Bin-crate dead code: an unused `pub` item trips `-D warnings`; a temporary `#[allow(dead_code, reason = "...")]` is removed in the task that first uses the item.
- Every stored name reaches the screen through a name rule (`names::tests::every_stored_name_reaches_the_screen_through_a_name_rule`): write with `names::clean_name`, draw with `names::display_name(name, UNTITLED)`, or mark the line `// name rule: <why>`.
- No migration code: old per-card `color` values are ignored, not converted (`no-migration-until-users`). A store `FORMAT_VERSION`/`PROTOCOL_VERSION` change is not part of this plan.
- Never push, tag or bump. Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Gate: finish with `gate all LOG full` (store/session/mirror changes) through `~/.claude/runs/run.ps1` (CLAUDE.md "Gate"), only when the user asks.

## Rulings (proposed; the user confirms before execution starts)

- **L1 One stored-only registry.** The mirror's link registry (`link_nodes`, `add_link`, `forget_link`, `is_link`, `link_ids`) becomes a registry of stored-only nodes keyed by id, with `Kind` read from the node's type; links and labels are views onto it. Adding `Label` beside `Link` as a second parallel set would be the third copy of one shape (WORKING_STYLE tripwire). Cost: touches `mirror.rs:40-169,234`, `links.rs` callers, `session_sync/writes.rs:386,408`.
- **L2 Membership order is stored in the value.** Spec 3 says order comes from the register stamp, and 2.1 says readers never read the document; the mirror does not hold stamps, so those two cannot both hold. Ruling: the membership leaf is `Int(added_at_ms)` instead of `Bool(true)`; order is `(added_at_ms, label id)`. Re-adding writes a fresh value, so the label goes last (D3 holds). Cost: the spec's "plain `Bool(true)`" wording changes (Task 9 fixes it); two replicas with skewed clocks may order concurrent adds oddly, which is harmless. Rejected: stamps in the mirror (a `Leaf` wrapper touching every tree reader).
- **L3 Label names are unique among live labels** by `names::name_key`: creating a name that exists selects the existing label. Cost: two labels cannot share a name; the menu is unambiguous.
- **L4 Deleting a label** is `queue_delete` of the node (not restorable, no undo step); memberships stay in the cards' `PROPS` and are hidden by "a label that is not live is not shown" (spec 3). Cost: dead entries remain in `PROPS` until a card's labels are edited; they cost bytes only.
- **L5 Removing per-card colour.** `COLOR` leaves the `TYPES` rows of Tab, Group, Note, Image, Pdf, Json and Download (Jar keeps it; Label gets it). Old values stay raw in the document. The card `Swatch` control, `NodeAction::CycleColor`, `next_tag_color`, `Carried.color` and `TabCarry.color` are deleted. `NodeAction::SetColor` stays for jars only.
- **L6 Export.** A whole-document export includes label nodes (they are nodes); the PROPS `@<id>` segments name them. A subtree export (copying a selection) leaves the label nodes out, so under `Trust::Foreign` the memberships drop (unknown id) and under `Trust::Own` they are kept. Cost: copying cards to another document loses their labels; parked, not built (see Task 9).

## Review Focus

1. **A deleted label** is gone everywhere at once: no chip, no border colour, no filter or `#label` hit, not counted in "+N"; re-applying a new label of the same name works (Task 4, 6, 7).
2. **Two replicas** adding the same label to a card converge to one entry; add on one side and remove on the other resolve by the store's merge (Task 4).
3. **Rename or recolour a label** updates every labelled card's chip and border and reindexes their search text without touching the cards (Task 6, 7).
4. **A labelled card** survives close and reopen, duplicate (tab and note), whole-document export then import, and restart; the memberships stay attached to the same labels (Task 4, 8).
5. **An old session with per-card colours** opens, shows no stray colour, and a jar keeps its colour (Task 5).
6. **Dense view, long names and many labels**: no chips above `LOD_SIMPLIFY_COUNT`; chip text is clipped, "+N" counts the rest; a card with 50 labels costs one capped loop per frame (Task 6).
7. **Empty, whitespace-only, duplicate and very long label names** are refused or cleaned as `clean_name` does (Task 3).

---

### Task 1: `Kind::card()` (D7)

**Files:** Modify `store/model.rs` (add `Kind::card`), `session_sync.rs:354`, `session_sync/remote.rs:309`, `store/json.rs:140,320`; Test in `store/model.rs` tests.

**Interfaces:**
- Produces: `impl Kind { pub(crate) fn card(self) -> bool }`: false for `Link`, true for every other kind (Task 2 adds `Label` to the false arm).

- [ ] **Step 1 (test first).** In `store/model.rs` tests add:

```rust
#[test]
fn only_stored_only_kinds_are_not_cards() {
    for kind in [Kind::Tab, Kind::Group, Kind::Note, Kind::Image, Kind::Pdf, Kind::Json, Kind::Download, Kind::Jar, Kind::Unknown] {
        assert!(kind.card(), "{kind:?}");
    }
    assert!(!Kind::Link.card());
}
```

- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold only_stored_only` — Expected: compile error, no `card`.
- [ ] **Step 3:** Add `pub(crate) fn card(self) -> bool { !matches!(self, Kind::Link) }` beside `Kind::fields`. Replace the "not a card" tests only: `session_sync.rs:354` `nodes.retain(|(_, s)| s.kind().card())`; `session_sync/remote.rs:309` `n.kind().card()`; `store/json.rs:140` (`is_shown_card`) and `:320` (`creates`). Leave `session_sync.rs:351`, `store/json.rs:122-134,158,253,308`, `links.rs` and the `EdgeKind::Link` sites (they mean a link's own ends). Leave `remote.rs:44,276` and `model.rs:686` for Task 2 (they need the new variant to be correct).
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold` — Expected: all pass (behaviour unchanged). fmt, clippy.
- [ ] **Step 5:** Commit `store: Kind::card() names the stored-only kinds` + trailer.

---

### Task 2: `Kind::Label` and the stored-only registry (L1)

**Files:** Modify `store/model.rs` (`Kind`, `TYPE_LABEL`, `TYPES` length 9 -> 10, `type_str`, `fields`, `from_type`, `card`, `Doc::is_shown` at ~686), `mirror.rs` (registry), `session_sync.rs:351-356`, `session_sync/remote.rs:44,276`, `session_sync/writes.rs:386-388,408-420`, `session_files.rs:355`, `caps/rows.rs` (a Label row, not restorable), `closed.rs:312` test row, `card_kind.rs` payload table, `store/json.rs` (a Label passes export and import like a jar's node, not like a link's ends). Tests: `store/model.rs`, `mirror.rs`, `session_sync/tests/labels.rs` (new, add `mod labels;` to `session_sync/tests.rs`).

**Interfaces:**
- Consumes: `Kind::card` (Task 1).
- Produces: `Kind::Label` (type `application/vnd.manifold.label`, fields `[NAME, COLOR, CLOSED]`); `Mirror::add_stored(id)` (renames `add_link`), `Mirror::forget_stored(id)`, `Mirror::is_stored(id) -> bool`, `Mirror::stored_ids(kind: Kind) -> impl Iterator<Item = NodeId>` (replaces `link_ids`), `Mirror::label_def(id) -> LabelDef { name: String, color: Option<egui::Color32> }`.

- [ ] **Step 1 (tests first).**
  - `store/model.rs`: extend the type-table test (~`model.rs:834-878`) so `Kind::Label.type_str() == Some("application/vnd.manifold.label")`, `Kind::from_type` round-trips it, `Kind::Label.fields()` is `[NAME, COLOR, CLOSED]`, and `!Kind::Label.card()`.
  - `session_sync/tests/labels.rs` (new): a label created in one app appears in another that reads the same file, and is not a graph node:

```rust
#[test]
fn a_label_syncs_as_a_stored_node_and_is_never_a_card() {
    let path = temp_dir("label-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let label = app.test_stored_label("red things"); // helper: queue_create + add_stored + edit_field(NAME), as links.rs does
    assert!(app.save_session());
    assert!(app.doc.mirror.is_stored(label) && !app.nodes.contains_key(&label), "mirror only");
    let mut other = app_on(&path, 2);
    assert!(other.save_session()); // reads the document
    assert_eq!(other.doc.mirror.label_def(label).name, "red things");
    assert!(!other.nodes.contains_key(&label));
}
```

  (`test_stored_label` is a `#[cfg(test)]` helper in `session_sync/tests/labels.rs` that mirrors `session_sync/tests/links.rs`; Task 3 replaces it with `create_label`.)
- [ ] **Step 2:** Run `cargo test -j 8 -p manifold --bin manifold labels model::` — Expected: FAIL (compile errors / missing variant).
- [ ] **Step 3:** Implement. Rename the mirror link registry to the stored-only registry (L1): `link_nodes` -> `stored_nodes`, `add_link` -> `add_stored` (seeds `TYPE` from `Kind`), keep `Mirror::link_ids()` as `stored_ids(Kind::Link)`. Update every caller (grep `add_link\|forget_link\|is_link\|link_ids`: `links.rs`, `session_sync/remote.rs`, `session_sync/writes.rs`, `session_sync.rs`, tests). `remote.rs:44` becomes `if !kind.card() { mirror.add_stored(id, kind); queue_create; mirror_remote; return }`; `remote.rs:276` the `Kind::Link` arm becomes `Kind::Link | Kind::Label`. `writes.rs:386` snapshot arm: `if mirror.is_stored(id) { kind.fields() ... }`. `Doc::is_shown` (`model.rs:686`): `!node.kind().card() || (...)`. `session_files.rs:355` sorts `Link | Label` after the cards. `session_sync.rs:351` materialises every stored-only node (`!card()`) instead of only links. Add a `caps` row so `worth_reopening` is false for Label and add a `closed.rs:312` row for it. `card_kind.rs:178` payload table: add the variant if the table enumerates constructible kinds. `store/json.rs`: confirm a Label node exports and imports like any node (its fields are generic); a Label has no ends, so `is_half_made_link` stays link-only.
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold` — Expected: all pass. fmt, clippy.
- [ ] **Step 5:** Commit `store: Kind::Label; the mirror's stored-only registry` + trailer.

---

### Task 3: Label lifecycle

**Files:** Create `labels.rs` (module, `impl App`: create, rename, recolour, delete, `label_defs`); Modify `main.rs` (`mod labels;`), `theme.rs` (palette helper, see below), `session_sync/tests/labels.rs`.

**Interfaces:**
- Consumes: `Kind::Label`, `Mirror::add_stored/forget_stored/label_def` (Task 2); `queue_create`, `queue_delete`, `edit_field`, `alloc_node_id` (as `links.rs::create_link` uses them).
- Produces:
  - `App::create_label(&mut self, name: &str, color: Option<egui::Color32>) -> Option<NodeId>`: `names::clean_name`; `None` for an empty name; returns the existing live label with the same `name_key` (L3); otherwise allocates, registers, queues the `Create`, writes `NAME` and `COLOR` (the first palette colour no live label uses, else the palette cycles).
  - `App::rename_label(id, &str) -> bool`, `App::recolor_label(id, Option<egui::Color32>)`, `App::delete_label(id)` (`mirror.forget_stored`, `queue_delete`, no undo step: L4).
  - `App::label_defs(&self) -> Vec<(NodeId, LabelDef)>` live labels sorted by `name_key` (stable for menus).

- [ ] **Step 1 (tests first)** in `session_sync/tests/labels.rs`:

```rust
#[test]
fn a_label_name_is_cleaned_and_unique() {
    let path = temp_dir("label-names").join("session.mnf");
    let mut app = app_on(&path, 1);
    assert_eq!(app.create_label("   ", None), None, "an empty name makes nothing");
    let a = app.create_label("  Urgent \n", None).unwrap();
    assert_eq!(app.doc.mirror.label_def(a).name, "Urgent");
    assert_eq!(app.create_label("urgent", None), Some(a), "same name key: the existing label");
    assert_eq!(app.label_defs().len(), 1);
}

#[test]
fn rename_recolor_and_delete_reach_the_document() {
    let path = temp_dir("label-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = app.create_label("a", None).unwrap();
    assert!(app.rename_label(a, "b") && app.save_session());
    app.recolor_label(a, Some(egui::Color32::BLUE));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.label_def(a).color, Some(egui::Color32::BLUE));
    app.delete_label(a);
    assert!(app.save_session());
    assert!(app.label_defs().is_empty());
    assert!(!app.doc.journal.as_ref().unwrap().doc().nodes[&a].is_live());
}
```

- [ ] **Step 2:** Run `... labels::` — Expected: FAIL (no `create_label`).
- [ ] **Step 3:** Implement `labels.rs` after `links.rs::create_link`/`delete_link` (links.rs:405,415). Palette: reuse the Okabe-Ito entries from `theme.rs` (`TAG_COLOR_PALETTE` near `theme.rs:330`); add `theme::label_color(used: &[Option<Color32>]) -> Color32` there, since Task 5 deletes `next_tag_color` and this keeps the palette's one user. Call `clean_name` at write; every read of a name in this file carries `// name rule:` or goes through `display_name`.
- [ ] **Step 4:** Move the Task 2 test's creation onto `create_label` and delete `test_stored_label`. Items not yet read by UI (`label_defs`, `recolor_label`, `delete_label`, `rename_label`) carry a temporary `#[allow(dead_code, reason = "used by the selection bar menu (Task 8)")]`, removed in Task 8. `cargo test -j 8 -p manifold --bin manifold` — Expected: pass. fmt, clippy.
- [ ] **Step 5:** Commit `labels: create, rename, recolour, delete` + trailer.

---

### Task 4: Membership (L2)

**Files:** Modify `labels.rs` (membership API), `mirror.rs` (`card_labels`), `undo.rs` and `session_sync/edits.rs` (drop the `#[allow(dead_code)]` on `UndoKind::Props` and `edit_prop`, now used), tests in `session_sync/tests/labels.rs`.

**Interfaces:**
- Consumes: `App::edit_prop(node, field::PROPS, path, Option<Leaf>) -> Option<PropEdit>` (plan 2), `Mirror::tree`, `tree::resolved`, `label_def`.
- Produces:
  - `fn labels_path(label: NodeId) -> TreePath` = `PROPS/labels/Id(label)`; `labels` root key constant `const LABELS_KEY: &str = "labels"`.
  - `Mirror::card_labels(&self, card: NodeId) -> Vec<NodeId>`: the live labels of `card` in `(added_at_ms, label id)` order; skips ids that are not live labels (L4) and values that are not `Int`.
  - `App::set_label(&mut self, cards: &[NodeId], label: NodeId, on: bool) -> Vec<PropEdit>`: per card, `on` writes `Int(now_ms)` unless already present, `off` writes the tombstone if present; returns the edits that changed something. It does not record undo (the caller does, Task 8).
  - `App::record_label_undo(&mut self, message: &str, edits: Vec<PropEdit>)`: `record_undo(message, UndoKind::Props(edits))` when non-empty.

- [ ] **Step 1 (tests first):**

```rust
#[test]
fn labels_show_in_the_order_they_were_added_and_re_adding_goes_last() {
    let path = temp_dir("label-order").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let (a, b) = (app.create_label("a", None).unwrap(), app.create_label("b", None).unwrap());
    app.set_label(&[card], b, true);
    app.set_label(&[card], a, true);
    assert_eq!(app.doc.mirror.card_labels(card), vec![b, a]);
    app.set_label(&[card], b, false);
    app.set_label(&[card], b, true);
    assert_eq!(app.doc.mirror.card_labels(card), vec![a, b]);
}

#[test]
fn a_deleted_label_is_not_shown_and_a_repeat_apply_writes_nothing() {
    let path = temp_dir("label-dead").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", None).unwrap();
    assert_eq!(app.set_label(&[card], a, true).len(), 1);
    assert!(app.set_label(&[card], a, true).is_empty(), "already applied");
    app.delete_label(a);
    assert!(app.doc.mirror.card_labels(card).is_empty());
}

#[test]
fn two_replicas_adding_one_label_converge_to_one_entry() {
    let path = temp_dir("label-merge").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", None).unwrap();
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set_tree(&mut other, card, field::PROPS, labels_path(a), Some(Leaf::Int(5)));
    app.set_label(&[card], a, true);
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.card_labels(card), vec![a]);
    assert_eq!(snapshots(&app)[&card].trees, doc_trees(&app, card));
}

#[test]
fn a_labelled_card_keeps_its_labels_through_duplicate_and_a_whole_document_export() {
    let path = temp_dir("label-copy").join("session.mnf");
    let mut app = app_on(&path, 1);
    let card = add_note(&mut app, "n");
    let a = app.create_label("a", None).unwrap();
    app.set_label(&[card], a, true);
    let copy = app.duplicate_nodes(&[card])[0]; // adjust to the real duplicate entry point (plan 2: duplicate.rs)
    assert_eq!(app.doc.mirror.card_labels(copy), vec![a], "same document: same label");
    assert!(app.save_session());
    let json = app.export_session_json(); // adjust: store::json::doc_to_json_with over the whole document
    let mut fresh = app_on(&temp_dir("label-import").join("session.mnf"), 3);
    fresh.import_json(&json); // adjust: session_files import path
    let imported = fresh.label_defs();
    assert_eq!(imported.len(), 1, "the label node travels with the document");
    let note = fresh.cards_with_text("n")[0]; // adjust: find the imported note
    assert_eq!(fresh.doc.mirror.card_labels(note), vec![imported[0].0], "memberships follow the remapped id");
}

#[test]
fn a_label_edit_undoes_and_redoes_as_one_step() {
    let path = temp_dir("label-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (c1, c2) = (add_note(&mut app, "1"), add_note(&mut app, "2"));
    let a = app.create_label("a", None).unwrap();
    let edits = app.set_label(&[c1, c2], a, true);
    app.record_label_undo("Label 2 cards", edits);
    app.undo();
    assert!(app.doc.mirror.card_labels(c1).is_empty() && app.doc.mirror.card_labels(c2).is_empty());
    app.redo();
    assert_eq!(app.doc.mirror.card_labels(c2), vec![a]);
}
```

  (`remote_set_tree`, `doc_trees`, `snapshots`, `add_note` come from `session_sync/tests.rs` and `tests/props.rs`; move `doc_trees` and `key_path` to the shared test module if both files need them. The undo method names are the ones `undo/tests.rs` uses.)
- [ ] **Step 2:** Run `... labels::` — Expected: FAIL.
- [ ] **Step 3:** Implement. `now_ms` is the same clock the writer uses for ids (see `alloc_node_id`/hlc source in `session_sync`); a test seam: `labels::NOW_MS` thread-local override under `cfg(test)` so two quick adds in a test still order (add 1 ms per call in test, or take a monotonic counter `max(now, last + 1)` stored on `App`; ruling: use the monotonic counter, it is also correct in release). Remove the two `#[allow(dead_code)]` attributes; clippy must stay clean.
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold` — Expected: pass. Run once with the invariant on (debug build default). fmt, clippy.
- [ ] **Step 5:** Commit `labels: membership in PROPS; undo through Props` + trailer.

---

### Task 5: Remove per-card colour (L5)

**Files:** Modify `store/model.rs` (`TYPES` rows, `GENERIC`), `card.rs:320,348-381,628-648,916,937` (Swatch, `CardView.color` stays but is fed in Task 6), `node_menu.rs:47-49,130,201,341,451,805,986-996`, `theme.rs:330`, `mirror.rs:270,376-446`, `node_ops.rs:42,52`, `tabs.rs:209-261`, `tabs/spawn_result.rs:21,51,194`, `session_sync.rs:289`, `debug_export.rs:138`; tests listed below.

**Interfaces:**
- Removes: `CardControl::Swatch`, `NodeAction::CycleColor`, `theme::next_tag_color`, `Mirror::color` for non-jars, `Carried::color`, `TabCarry.color`.
- Keeps: `field::COLOR` on Jar and Label rows; `NodeAction::SetColor` and the picker row **for jars only**; `Mirror::jar_def`/`label_def` colour readers.

- [ ] **Step 1 (tests first):**
  - `session_sync/tests/misc.rs`-style: an old document where a note carries a `COLOR` register opens, and `card_view` colour is `None`:

```rust
#[test]
fn a_stored_card_colour_from_an_old_session_is_ignored() {
    let path = temp_dir("old-color").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add_note(&mut app, "n");
    app.edit_field(note, field::COLOR, red()); // test-only: Note no longer lists COLOR, so this writes a raw register
    assert!(app.save_session());
    assert!(app.card_color_for_test(note).is_none());
}
```

  - `store/model.rs` type-table test: `Kind::Note.fields()` no longer contains `COLOR`; `Kind::Jar` and `Kind::Label` do. A jar edited with `SetColor` still reads its colour back through `jar_def` (the jar keeps its colour).
  - `card/tests.rs:526-560`: `card_controls` expect `[Close]` (no `Swatch`) for the kinds that listed it; delete the `Swatch.action()==CycleColor` assertion.
  - Delete the tests that only exercise per-card colour: `session_sync/tests/edits.rs:175-228` (keep a jar SetColor test), `session_sync/tests/remote.rs:260-291,421,714-749`, `session_sync/tests/spawn.rs:473-480,650-659`, `fetched/tests.rs:440,472` (read each: keep what is about something else), `node_menu/tests.rs:697`, `mirror.rs:608-642` (jar parts stay).
- [ ] **Step 2:** Run the new tests — Expected: the old-colour test FAILS to compile (no seam) or fails on the colour; model test FAILS.
- [ ] **Step 3:** Implement the deletion in the order: tests first (above), then `TYPES` rows, then each reader/writer the compiler flags. `Mirror::color` is renamed `jar_color_of` or deleted if only `jar_def` uses COLOR. The `card_color_for_test` seam is `CardView.color` built by `card_view` (`card.rs:937`): set it to `None` here; Task 6 feeds the first label's colour. Drop `Swatch` from `card_controls` and the paint code (`card.rs:381,628,636-648`); remove the colour picker row from the node menu except for jars (`node_menu.rs:341,451` gated on `Runtime::Jar`). Delete `next_tag_color` and keep `TAG_COLOR_PALETTE` + `tag_shown` (labels and jars use them).
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold` — Expected: all pass; `grep -rn "CycleColor\|next_tag_color\|carried.color\|Swatch" crates/manifold/src` returns nothing outside comments. fmt, clippy.
- [ ] **Step 5:** Commit `cards: the per-card colour is gone; labels will replace it` + trailer.

---

### Task 6: Show labels (border and chips)

**Files:** Modify `card.rs` (`CardView` gains `labels: Vec<LabelChip>`; `card_view` fills it; header chips in `card/tab_body.rs:153` `paint_card_header` and the group/jar/unknown header callers at `card.rs:1381-1425`), `graph_ui.rs:51,80` (LOD), tests `card/tests.rs`, `session_sync/tests/labels.rs`.

**Interfaces:**
- Consumes: `Mirror::card_labels`, `label_def`; `Detail::content()` and `LOD_SIMPLIFY_COUNT`.
- Produces: `struct LabelChip { name: String, color: Color32 }`; `CardView.labels` holds the first `LABEL_CHIPS_SHOWN` (2) chips plus `CardView.labels_more: usize`; `CardView.color` = the first label's colour (so `card_border` is unchanged).

- [ ] **Step 1 (tests first):** in `card/tests.rs`, pure functions on a built `CardView`:

```rust
#[test]
fn the_border_takes_the_first_label_and_the_header_shows_two_chips_and_the_rest() {
    let labels = vec![chip("a", RED), chip("b", BLUE), chip("c", GREEN), chip("d", GREEN)];
    let (shown, more, border) = label_display(&labels);
    assert_eq!((shown.len(), more, border), (2, 2, Some(RED)));
    assert_eq!(label_display(&[]).2, None);
}

#[test]
fn chips_are_not_drawn_in_dense_views() {
    assert!(!chips_drawn(Detail::Simple));
    assert!(chips_drawn(Detail::Full));
}
```

  and in `session_sync/tests/labels.rs` that `card_view(id)` for a labelled note carries the first label's colour, and a deleted label's chip disappears.
- [ ] **Step 2:** Run — Expected: FAIL (no `label_display`).
- [ ] **Step 3:** Implement `label_display(&[LabelChip]) -> (&[LabelChip], usize, Option<Color32>)` and `chips_drawn(Detail)` as free functions beside `card_border`; `card_view` builds the chip list from `card_labels` capped at 64 ids per card (a loop bound; "+N" still counts the real total from the unclipped length). Chips paint in the header next to the pin mark (`paint_pinned_mark`, `card.rs:573,1193`) via the same `deferred_overlays` mechanism, clipped to `right_margin`; names drawn with `display_name`; a jar card draws no chips and takes no border from labels (D5: jars keep their own `COLOR`). Border: `CardView.color = first label colour mapped through `tag_shown` as before.
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold` — Expected: pass. Visual check in a debug run: chips on a normal card, none past 60 cards. fmt, clippy.
- [ ] **Step 5:** Commit `cards: label border and header chips` + trailer.

---

### Task 7: Find

**Files:** Modify `omnibar.rs:159` (`node_matches`), `omnibar/rows.rs:84-91` (`rows`, a `#label` token), `omnibar.rs:~100` (`AppCorpus::cards`), `search/sync.rs:39,65` (`index_print`, `index_text`), tests `omnibar` tests and `search/sync` tests.

**Interfaces:**
- Consumes: `Mirror::card_labels`, `label_def`, `Mirror::tree_version(card, field::PROPS)`.
- Produces: `fn label_names(&Mirror, card) -> Vec<String>`; the omnibar token rule: a query word starting with `#` selects cards having a live label whose `name_key` starts with the rest; other words filter as before; `#` alone lists labelled cards.

- [ ] **Step 1 (tests first):** (a) `node_matches("urgent", card)` is true for a card labelled "Urgent" and false after the label is deleted; (b) omnibar `rows` for `#urg` returns that card's row, for `#zzz` none, for `urg` (no hash) also matches via label text; (c) `index_print` of a card changes when its label is renamed and when it gains a label, and is unchanged when an unrelated label is renamed:

```rust
#[test]
fn renaming_a_label_reindexes_its_cards_only() {
    let mut app = app_on(&temp_dir("label-index").join("session.mnf"), 1);
    let (c1, c2) = (add_note(&mut app, "x"), add_note(&mut app, "y"));
    let (a, b) = (app.create_label("a", None).unwrap(), app.create_label("b", None).unwrap());
    app.set_label(&[c1], a, true);
    app.set_label(&[c2], b, true);
    let (p1, p2) = (app.index_print_for_test(c1), app.index_print_for_test(c2));
    app.rename_label(a, "renamed");
    assert_ne!(app.index_print_for_test(c1), p1);
    assert_eq!(app.index_print_for_test(c2), p2);
}
```

- [ ] **Step 2:** Run — Expected: FAIL.
- [ ] **Step 3:** Implement. `node_matches` appends `label_names` to its needles; `index_text` appends them; `index_print` hashes the resolved names (a handful) so a rename changes it (spec: "and the version counter of its PROPS tree"; the names alone are enough because `label_names` is resolved each call, so the version is not hashed unless a test shows a gap). Omnibar: parse `#` words in `rows` before `contains(text, [label, url])`; no change to command phrases.
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold` — Expected: pass. fmt, clippy.
- [ ] **Step 5:** Commit `find: label names in the filter, the index and #label` + trailer.

---

### Task 8: Act (selection bar, menu, debug commands)

**Files:** Modify `graph_ui/selection_bar.rs` (a Label button after Group, id `selection_label`), `graph_ui.rs:~115-235` (`GraphAction::LabelSelected`, `GraphAction::ToggleLabel{label, on}`, `NewLabel(name)`, `RenameLabel`, `RecolorLabel`, `DeleteLabel`; `apply_graph_action`), `menus` state (a `label_menu` open flag + draft name), a popup in `graph_ui/cards.rs` style (`menu_popup`, as `draw_pending_new_node_menu` at `cards.rs:215-240`), `debug_control.rs` (commands `LABEL <name>`, `LABELSELECTED <name> <on|off>`, `LABELRENAME`, `LABELDELETE`), `debug_export.rs` (state: `labels` list and per-card `labels`), tests.

**Interfaces:**
- Consumes: `create_label`, `rename_label`, `recolor_label`, `delete_label`, `label_defs`, `set_label`, `record_label_undo`, `card_labels`.
- Produces: a menu listing every live label with a tri-state check (all / some / none of the selection), a name field to create, per-row rename/colour/delete; one `record_label_undo` per toggle over the whole selection.

- [ ] **Step 1 (tests first):** at the `App` level, drive the actions the menu emits:

```rust
#[test]
fn applying_a_label_to_the_selection_is_one_undo_step() {
    let mut app = app_on(&temp_dir("label-bar").join("session.mnf"), 1);
    let ids: Vec<NodeId> = (0..3).map(|i| add_note(&mut app, &i.to_string())).collect();
    app.select(&ids);
    app.apply_graph_action(GraphAction::NewLabel("urgent".into()));
    let a = app.label_defs()[0].0;
    assert!(ids.iter().all(|&c| app.doc.mirror.card_labels(c) == vec![a]), "new label applies to the selection");
    app.undo();
    assert!(ids.iter().all(|&c| app.doc.mirror.card_labels(c).is_empty()), "one undo clears all three");
    app.apply_graph_action(GraphAction::ToggleLabel { label: a, on: false }); // nothing to remove
    assert_eq!(app.undo_depth(), 0, "a no-op records nothing");
}
```

  plus a test for the tri-state helper (`label_state(selection, label) -> All|Some|None`) and one that deleting a label from the menu leaves the cards without chips.
- [ ] **Step 2:** Run — Expected: FAIL.
- [ ] **Step 3:** Implement. `NewLabel(name)` = `create_label` then `set_label(selection, id, true)` then one `record_label_undo` (creation itself is not undoable, L4's mirror: the undo clears memberships and leaves the label). Button uses `chip_icon_button` (`theme.rs:719`) with `named_tip("Label")`, disabled when nothing is selected. Debug commands use the same `GraphAction`s so the gate drives real code. `debug_export` adds `labels` (name, colour present) and per-card `labels` names.
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold` — Expected: pass. fmt, clippy. A debug run: select cards, open the menu, create, toggle, rename, delete; confirm the toast text reads "Label N cards".
- [ ] **Step 5:** Commit `labels: the selection bar menu and debug commands` + trailer.

---

### Task 9: Suggest arm, gate stage, docs

**Files:** Modify `search/suggest.rs:14,87` (`Suggestion` gains how it may be applied; `accept_suggestion_as_label`), `graph_ui/suggestions.rs:20-90` (a second button "Label these cards", id `suggestion_label`), `debug_control.rs` (`ACCEPTSUGGESTIONLABEL n`), `crates/gate/src/labels.rs` (new stage), `crates/gate/src/main.rs` (`mod labels;`, `STAGES` entry, module doc list), `crates/gate/src/all.rs` (a `step("labels", false)` in the full tier next to `suggest-groups`; read `all.rs` and `tier.rs` first to also decide whether `light` includes it), docs below.

**Interfaces:**
- Consumes: `create_label`, `set_label`, `record_label_undo`.
- Produces: a suggestion can be applied as a label (`create_label(suggestion.label)` + `set_label(cards, id, true)` + one `Props` undo step) instead of a group; both buttons call the shared removal of the suggestion afterwards (`accept_suggestion` already retains the rest and saves).

- [ ] **Step 1 (tests first):** (a) `search/suggest` test: accepting a suggestion as a label labels exactly its cards, creates no group, is one undo step, and removes the suggestion; (b) the gate stage `labels` (model on `suggest_groups.rs`, 60 lines): import a few notes, `SUGGEST`, `ACCEPTSUGGESTIONLABEL 0`, `EXPORTSTATE` and check `labels` has one entry and its cards carry it; select two notes, `LABELSELECTED urgent on`, check, `UNDOKEY`, check cleared; restart the instance and check the labels persist; also the old-colour case (an imported note with a raw `color` shows no colour in `EXPORTSTATE`).
- [ ] **Step 2:** Run the unit test — Expected: FAIL. Build the stage: `cargo build -j 8 -p gate` — Expected: PASS after Step 3.
- [ ] **Step 3:** Implement the suggest arm and the stage; register it (`mod`, `STAGES`, `all.rs`). Run the stage alone: `target/debug/gate.exe labels` (see README "Headless mode" for the invocation form) — Expected: PASS.
- [ ] **Step 4 (docs).** For each claim changed, grep and fix: (a) `README.md`: the tag-colour and swatch paragraphs (`:125,129,152,160,172,206,224,256,260,267,506`; `grep -n "swatch\|tag color"`) become labels; the Links paragraph (`:190`) says labels are stored-only too; a Labels paragraph and the new debug commands under "Headless mode" (`:394`); (b) `docs/superpowers/specs/2026-09-24-tab-group-card-frame-design.md` (swatch and tag colour rows, controls table); (c) `docs/superpowers/specs/2026-10-04-node-model-design.md` (edge table: a label is not a link; "common data fields" no longer list `color` on every node); (d) the properties spec: section 3 and D3 say the membership leaf is `Int(added_at_ms)` (L2), `Bool(true)` wording removed, 2.1's reader rule and section 3's stamp order agree, the `edit_prop` signature in 2.1 reads `Option<Leaf>` (plan 2 minor M4); (e) `PARKED.md`: add "Label export with a subtree copy" (L6, trigger: copying cards between documents), "Label order register" if L2 proves wrong, and delete the entries this plan fixed ("`props` on non-cards" if `Kind::card()` closes it: check its wording); (f) this plan `status: done`. Do not edit `CHANGELOG.md` (the user writes it at the bump).
- [ ] **Step 5:** Full verification: `cargo test -j 8 -p manifold --bin manifold`, fmt, the full clippy command from CLAUDE.md. Commit `labels: suggest arm, gate stage, docs` + trailer. The `gate all LOG full` run is on the user's request.

## Self-review

- **Spec coverage:** section 3 label node (T2, T3), D7 (T1), membership and order (T4, L2), show (T5, T6), find (T7), act and undo (T8), suggested groups (T9), names (T3, Global Constraints), scope D5 (global list, all stored cards, jars get no chip: `card_labels` is read for every card; Task 6 skips jars), section 6 label tests (T4, T6, T7, T8, gate stage T9), section 7 docs (T9).
- **Type consistency:** `add_stored/forget_stored/is_stored/stored_ids/label_def` (T2) are used identically in T3 and T4; `PropEdit` and `set_label` return value flow into `record_label_undo` (T4, T8, T9); `labels_path` is the single path builder.
- **Open points to rule on while executing (ledger):** the monotonic millisecond source in T4; whether `Mirror::color` survives for jars or is replaced by `jar_def` (T5); the exact `light` tier membership of the `labels` stage (T9); the caps row shape for Label (T2).
