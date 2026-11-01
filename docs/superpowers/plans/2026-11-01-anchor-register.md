---
status: draft
---

# Anchor Register (Slice 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `PINNED` bool (field 7) with one composite last-writer-wins register `ANCHOR` (field 27) that holds a card at a point: written explicitly by Pin and implicitly by a drag release, released by Unpin (explicit only), Tidy, Arrange and a local move (implicit only).

**Architecture:** One `store::model::Anchor { pos, group, explicit }` value, TLV-encoded like `Pos` and `Held`. One read predicate, `Mirror::held(id) -> Option<egui::Vec2>` (set and (explicit or written in the card's current group)), drives the settle's fixed set, its seeds and the `SettleKey`; `Mirror::pinned(id)` becomes the explicit bit only. Writers go through three shared functions: `App::edit_anchor` (the one writer), `App::clear_anchors(ids, explicit)` (Unpin, Tidy, Arrange, fold) and `App::drop_at(id)` (drag release, `MOVENODE`); `App::edit_group` releases an implicit anchor itself, so Move, Join, GroupWith, Eject, fold, put-back and undo replay all get the rule from one place. Undo records carry anchor pairs (`Moved::anchor`, and a new `Folded` record for folds).

**Tech Stack:** Rust (workspace crates `manifold`, `gate`), egui, the in-repo journal store (`crates/manifold/src/store`).

**Spec:** `docs/superpowers/specs/2026-11-01-scopes-nested-layout-anchor-design.md`, section "Slice 1: the anchor register replaces `PINNED`", plus "Behaviour changes", "Tests", "Architecture check" and "Docs to update". Executors read both.

## Global Constraints

- Field 27, `ANCHOR`, value `{1: x, 2: y, 3: group, 4: explicit}`; a set anchor is always written, even at the top-level origin with no explicit bit; a clear is the empty encoding.
- Field 7 goes into `RESERVED` (`[u32; 3] = [7, 16, 17]`); `FIELDS` ends at 19 entries.
- No `FORMAT_VERSION` bump (stays 4, `store/journal.rs:26`); `PROTOCOL_VERSION` untouched.
- No migration code: old `PINNED` values are ignored (memory "No migration until users"; spec "Behaviour changes", **Breaking:**).
- `store/json.rs` drops anchors on export and ignores them on import; an old export's `"pinned"` is dropped silently.
- `Mirror::pinned` (the explicit bit) alone drives the pin mark, the menu rows, Pin All and the idle-sleep exemption (`tabs/sleep.rs:136`, unchanged).
- Pin, Unpin and a plain drag release stay off the undo history; Move, Join, fold, put-back, Tidy and Arrange record anchor pairs.
- `cargo fmt --all` before each commit; the pre-commit hook runs `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p manifold-hosts -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`; never `--no-verify`; no lint silenced without `#[allow(..., reason = "...")]`.
- No test reads or writes the real `%APPDATA%\manifold`: use `store::testutil::temp_dir` or `App::default()`.
- One cargo per target dir; pass `-j 8`. Python only as `python -P f.py < /dev/null`.
- Commits end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never push, tag or bump.
- Gate tier for the finished slice: `full` (store, mirror and session-sync changes, CLAUDE.md "Gate"); run only on request.

## Review Focus

1. A remote `ANCHOR` write arriving while the user drags that card must not snap it (the drag wins, as for `POS`) — test in Task 3 `a_remote_anchor_does_not_snap_a_card_being_dragged`.
2. A peer that writes a non-finite anchor point (a NaN from a buggy build) must decode as no anchor, never a hold at NaN — test in Task 1 `anchor_round_trips_and_a_non_finite_point_is_absent`.
3. A remote clear of an anchor must snap the card to its `POS` (else it stays drawn at the dead anchor point until the next settle) — test in Task 3 `a_remote_anchor_snaps_the_layout_and_a_remote_clear_falls_back_to_pos`.
4. Undo of a drop whose card was dropped again since must skip that card (its anchor is no longer the one the step left) — test in Task 5 `an_undo_skips_a_card_dropped_again_since`.
5. Tidy on a level with nothing to release must record no undo step and write nothing — test in Task 6 `tidy_with_nothing_to_release_records_nothing`.

## Rulings on spec gaps (made while planning; listed for review)

- **R1 `held` returns the point.** `Mirror::held(id) -> Option<egui::Vec2>` instead of a bool: the settle needs the point to seed from, and `held(id).is_some()` is the predicate. One function, as the spec's Architecture check asks.
- **R2 `edit_group` is the one clearing point for moves.** The spec lists Move, Join, fold and put-back separately; clearing an implicit anchor inside `edit_group` (after the group actually changes, past the Default-jar refusal) covers those plus GroupWith, Eject, Remove from Group, `MOVETOGROUP`, `follow_remote_group`'s local re-issue and undo replay. Remote moves bypass it (they write the mirror directly), which is exactly how an inert anchor arises.
- **R3 Folds get their own anchor record.** The spec says fold uses `Moved`, but a fold records `UndoKind::Fold`, not `Revert`. `UndoKind::Fold` becomes `Fold(Vec<Folded>)` with `Folded { group, dest, anchors: Vec<(NodeId, Anchor)> }`; `fold_group` clears and returns the members' implicit anchors, and undo restores each one whose card is back in the group with no anchor. A Recently Closed reopen (not through Undo) does not restore anchors.
- **R4 Gate `doc_key` becomes an `Option`.** Instead of a separate mirror-only wait helper, `Field::doc_key` returns `Option<&'static str>` (`None` for `Anchor`), a new `Field::word` names the field in messages, and `field_agree` compares A and B only when there is no document key, so `wait_field` is reused unchanged.
- **R5 JSON import skips `anchor` by name.** The import filter at `json.rs:271` excludes `field::ANCHOR` with `TYPE`, `CLOSED` and `HELD`, so a hand-written `"anchor"` key never reaches the typed match (which would error).
- **R6 The drag-release test changes meaning.** `session_sync/tests/edits.rs` `a_drag_release_saves_the_dropped_position` asserted the dropped card was pushed by the next settle; it now asserts the card stays and its neighbour moves (spec "Behaviour changes", first line).
- **R7 Tidy's surfaces.** Label "Tidy", tooltip "Tidy: let dropped cards move again; pins stay", icon `egui_phosphor::light::BROOM` (present in egui-phosphor 0.14.0 `codepoints.rs:244`). The toolbar chip shows only while the level has an implicit anchor (`App::level_has_drops`); the omnibar command and headless `TIDY` are always available (a no-op when nothing is held). No visual cue for drop-anchored cards (spec open question: no evidence yet).
- **R8 Changelog at the bump.** VERSIONING.md forbids an Unreleased section, so this slice writes no CHANGELOG entry; the `**Breaking:**` line stays in the spec's "Behaviour changes" (its one home) and the docs commit names it for the next bump.
- **R9 Anchor group inside a `Create` is a plain tag.** `Anchor::group` is not a link and is not remapped by `resolve_id` or JSON import (anchors are never imported).

## File Structure

| File | Change |
|---|---|
| `crates/manifold/src/store/model.rs` | `Anchor`, `FieldType::Anchor`, `FieldValue::Anchor`, `field::ANCHOR`, encode/decode, `as_anchor`; `PINNED` deleted, `RESERVED` grows |
| `crates/manifold/src/store/json.rs` | `Anchor` has no JSON; import skips it |
| `crates/manifold/src/store/journal/tests.rs` | a `Create` with an unlisted field and field 27 survives compaction |
| `crates/manifold/src/mirror.rs` | `Mirror::anchor`, `Mirror::held`, `Mirror::pinned` (explicit bit); `Carried::anchor`, `Carried::tab(.., anchor, ..)` |
| `crates/manifold/src/session_sync/edits.rs` | `edit_anchor`, `clear_anchors`; `edit_group` releases an implicit anchor |
| `crates/manifold/src/shortcuts.rs` | `set_pinned` writes the anchor |
| `crates/manifold/src/graph_nav.rs` | `drop_at`; `release_drag`, `drag_origin`, `membership_snapshot`, `moved_since`; settle seeds and holds from `held` |
| `crates/manifold/src/level.rs` | `SettleKey::holds` |
| `crates/manifold/src/session_sync/remote.rs` | a remote `ANCHOR` snaps the layout |
| `crates/manifold/src/undo.rs`, `undo/tests.rs` | `Moved::anchor`, `Folded`, replay |
| `crates/manifold/src/groups.rs` | `fold_groups` / `fold_group` carry cleared anchors |
| `crates/manifold/src/search/arrange.rs` | `tidy_level`, `level_has_drops`; Arrange releases implicit anchors |
| `crates/manifold/src/graph_ui/toolbar.rs`, `omnibar/rows.rs`, `debug_control.rs` | Tidy surfaces; `debug_move_node` through `drop_at` |
| `crates/manifold/src/tabs.rs`, `tabs/spawn_result.rs`, `node_ops.rs`, `session_sync.rs` | respawn carries the anchor |
| `crates/manifold/src/debug_export.rs` | `mirror_anchor` |
| `crates/gate/src/session.rs`, `crates/gate/src/two/groups.rs` | `Field::Anchor`; s12 checks the anchor |
| `crates/manifold/src/session_sync/tests.rs` and `tests/{edits,misc,remote,spawn,writes}.rs`, `duplicate.rs` | retargeted and new tests |
| `README.md`, `PARKED.md`, `docs/superpowers/specs/2026-10-04-node-model-design.md` | docs |

## Deleted

- `field::PINNED` (keep the comment "Never reuse 7 (the retired `pinned` bool, replaced by `ANCHOR`)") and its `FieldDef` (`model.rs:268`, `:310`).
- `PINNED` in every `TYPES` list and in `GENERIC` (`model.rs:183-198`).
- `mirror.rs` `pinned_in` (`:437-439`) and `Carried::pinned` (`:506-508`); `Carried::tab`'s `pinned: bool` parameter.
- `TabCarry::pinned` (`tabs.rs:209`) and every `pinned` read of it (`tabs.rs:237`, `:256`, `tabs/spawn_result.rs:18`, `:46`, `:105`, `:178`, `:242`, `node_ops.rs:114-124`, `session_sync.rs:270`).
- The `PINNED` write in `set_pinned` (`shortcuts.rs:271`).
- Test `pinned_reads_a_bool_and_defaults_to_false` (`mirror.rs:759-765`), replaced by `pinned_is_the_explicit_bit_and_held_is_group_scoped`.

---

### Task 1: The anchor value and its encoding

**Files:**
- Modify: `crates/manifold/src/store/model.rs` (`FieldType` `:231-250`, `mod field` `:260-301`, `FIELDS` `:303-325`, `FieldValue` `:340-355`, `absent` `:359-370`, accessors `:372-410`, `encode_field_into` `:420-460`, `decode_field` `:489-527`, tests)
- Modify: `crates/manifold/src/store/json.rs:45-63` (`value_to_json`), `:270-272` (import filter)
- Modify: `crates/manifold/src/mirror.rs:771-781` (test sample match)
- Test: `crates/manifold/src/store/model.rs` `mod tests`, `crates/manifold/src/store/journal/tests.rs`, `crates/manifold/src/store/json.rs` `mod tests`

**Interfaces:**
- Consumes: `id_message(NodeId) -> Writer`, `decode_id(&[u8]) -> Result<NodeId, TlvError>` (`model.rs:462`, `:473`).
- Produces: `pub struct Anchor { pub pos: [f32; 2], pub group: Option<NodeId>, pub explicit: bool }` (`Debug, Clone, Copy, PartialEq`); `FieldType::Anchor`; `FieldValue::Anchor(Option<Anchor>)`; `pub const field::ANCHOR: u32 = 27`; `FieldValue::as_anchor(&self) -> Option<Anchor>`. During this task `FIELDS` holds 20 entries (both `PINNED` and `ANCHOR`); Task 2 removes `PINNED`.

- [ ] **Step 1: Write the failing tests**

In `crates/manifold/src/store/model.rs` `mod tests`, after `held_round_trips_and_empty_is_absent`:

```rust
    #[test]
    fn anchor_round_trips_and_a_non_finite_point_is_absent() {
        let def = field_def(field::ANCHOR).unwrap();
        assert_eq!((def.name, def.ty), ("anchor", FieldType::Anchor));
        let id = NodeId { actor: ActorId(7), counter: 3 };
        for anchor in [
            Anchor { pos: [1.5, -2.0], group: Some(id), explicit: true },
            Anchor { pos: [0.0, 4.0], group: None, explicit: false },
            // The top-level origin with no bit is still a set anchor: never the empty encoding.
            Anchor { pos: [0.0, 0.0], group: None, explicit: false },
        ] {
            let enc = encode_field(field::ANCHOR, &FieldValue::Anchor(Some(anchor)));
            assert!(!enc.is_empty(), "{anchor:?}");
            assert_eq!(decode_field(def, &enc), Ok(FieldValue::Anchor(Some(anchor))));
            assert_eq!(FieldValue::Anchor(Some(anchor)).as_anchor(), Some(anchor));
        }
        assert!(encode_field(field::ANCHOR, &FieldValue::Anchor(None)).is_empty(), "a clear writes nothing");
        assert_eq!(FieldValue::absent(FieldType::Anchor), FieldValue::Anchor(None));
        for bad in [f32::NAN, f32::INFINITY] {
            let enc = encode_field(field::ANCHOR, &FieldValue::Anchor(Some(Anchor { pos: [bad, 1.0], group: None, explicit: true })));
            assert_eq!(decode_field(def, &enc), Ok(FieldValue::Anchor(None)), "{bad} is no point");
        }
    }
```

In `crates/manifold/src/store/journal/tests.rs`, add `Anchor` to the `use crate::store::model::{...}` line, then after `a_newer_builds_frames_and_fields_survive`:

```rust
/// A `Create` carrying a field this build doesn't list survives a snapshot and a compaction: what an older build does
/// with a newer build's `ANCHOR` (field 27) in a new node's `Create` (spec Slice 1, "No FORMAT_VERSION bump"). The
/// anchor itself round-trips through the same path in this build.
#[test]
fn a_creates_unlisted_field_and_its_anchor_survive_compaction() {
    let dir = temp_dir("create-unlisted");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let node = txn.next_id();
    let mut future = Writer::new();
    future.str(99, "from the future");
    let future = future.into_bytes();
    let anchor = FieldValue::Anchor(Some(Anchor { pos: [3.0, -4.0], group: None, explicit: true }));
    let mut fields = encode_field(field::URL, &FieldValue::Str("https://e.example/".into()));
    fields.extend(&future);
    fields.extend(encode_field(field::ANCHOR, &anchor));
    txn.commit(vec![OpBody::create(node, Kind::Tab.type_str().unwrap(), fields)]).unwrap();
    journal.close().unwrap(); // a snapshot, compacted
    let mut journal = Journal::open(&path, ActorId(2)).unwrap();
    journal.close().unwrap(); // reopened and closed by another actor

    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(doc.nodes[&node].fields[&RegKey::of(99)].enc, future, "the unlisted field is written back");
    assert_eq!(doc.nodes[&node].get(field::ANCHOR), anchor);
}
```

In `crates/manifold/src/store/json.rs` `mod tests`, after `node_json_matches_a_document_nodes_json`:

```rust
    #[test]
    fn an_anchor_is_neither_exported_nor_imported() {
        use crate::store::model::Anchor;
        let mut fields = BTreeMap::new();
        fields.insert(field::URL, FieldValue::Str("https://a.example/".into()));
        fields.insert(field::ANCHOR, FieldValue::Anchor(Some(Anchor { pos: [1.0, 2.0], group: None, explicit: true })));
        let json = node_json("n1", crate::store::model::TYPE_TAB, &fields, &Trees::new(), None);
        assert!(!json.fields.contains_key("anchor"), "the hold is not exported");
        let doc = doc_from(
            r##"{"nodes": [{"id": "t", "type": "application/vnd.manifold.tab", "url": "https://a.example/", "pos": [1, 2], "anchor": {"pos": [1, 2]}}]}"##,
            1,
        );
        let (_, tab) = doc.live().next().unwrap();
        assert_eq!(tab.get(field::POS), FieldValue::Pos(Some((1.0, 2.0))), "the position is kept");
        assert_eq!(tab.get(field::ANCHOR), FieldValue::Anchor(None), "an anchor key is skipped, not an error");
    }
```

`doc_from(json: &str, actor: u64) -> Doc` is the existing test helper at `json.rs:409`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold anchor`
Expected: FAIL to compile with "cannot find type `Anchor`" / "no associated item named `ANCHOR`".

- [ ] **Step 3: Implement**

`model.rs`, after `FieldType::Held` (`:249`):

```rust
    /// `field::ANCHOR`: a card held at a point, `{1: x, 2: y, 3: group id, 4: explicit}` (`Anchor`); absent is none.
    Anchor,
```

After the `FieldDef` struct (`:256`):

```rust
/// `field::ANCHOR`'s value: a card held at `pos`, in its group's local coordinates (as `POS`), written while the card
/// was in `group`. `explicit` is a user Pin, which only Unpin releases; otherwise it is a drop, which Tidy, Arrange
/// and a local move release, and which holds only while the card is still in `group` (`Mirror::held`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub pos: [f32; 2],
    pub group: Option<NodeId>,
    pub explicit: bool,
}
```

In `mod field`, after `BODY` (`:300`):

```rust
    /// A card's hold (`Anchor`): an explicit Pin or a drop. Replaces the retired `pinned` bool (7).
    pub const ANCHOR: u32 = 27;
```

In `FIELDS` grow the array type to `[FieldDef; 20]` and append after the `held` entry:

```rust
    FieldDef { no: field::ANCHOR, name: "anchor", ty: FieldType::Anchor },
```

In `FieldValue`, after `Held { .. }`:

```rust
    /// A card's hold; `None`: not held.
    Anchor(Option<Anchor>),
```

In `absent`: `FieldType::Anchor => FieldValue::Anchor(None),`

After `as_view_code`:

```rust
    /// A set `Anchor`; `None` for none or any other variant.
    pub fn as_anchor(&self) -> Option<Anchor> {
        match self {
            FieldValue::Anchor(anchor) => *anchor,
            _ => None,
        }
    }
```

In `encode_field_into`, before the "empty or zero value" comment:

```rust
        // A set anchor is written even at the top-level origin with no bit (an empty message), so it never reads as
        // absent.
        FieldValue::Anchor(Some(a)) => {
            let mut m = Writer::new();
            if a.pos[0] != 0.0 {
                m.f32(1, a.pos[0]);
            }
            if a.pos[1] != 0.0 {
                m.f32(2, a.pos[1]);
            }
            if let Some(group) = a.group {
                m.message(3, &id_message(group));
            }
            if a.explicit {
                m.varint(4, 1);
            }
            w.message(no, &m);
        }
```

and add `| FieldValue::Anchor(None)` to the no-write arm.

In `decode_field`, after the `FieldType::Held` arm:

```rust
            FieldType::Anchor => {
                let (mut x, mut y, mut group, mut explicit) = (0.0f32, 0.0f32, None, false);
                for f in Reader::new(v.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => x = f.value.f32()?,
                        2 => y = f.value.f32()?,
                        3 => group = Some(decode_id(f.value.bytes()?)?),
                        4 => explicit = f.value.bool()?,
                        _ => {}
                    }
                }
                FieldValue::Anchor((x.is_finite() && y.is_finite()).then_some(Anchor { pos: [x, y], group, explicit }))
            }
```

Update the `decode_field` doc comment: "A non-finite position or anchor point is not one: it decodes as absent."

`json.rs` `value_to_json`: add `| FieldValue::Anchor(_)` to the `return None` arm. Import filter (`:271`):

```rust
                field_by_name(name).filter(|def| !matches!(def.no, field::TYPE | field::CLOSED | field::HELD | field::ANCHOR))
```

and extend its comment: "`anchor` is this document's layout hold, never a file's (spec Slice 1, Export)."

`mirror.rs` test `carried_holds_every_tab_field`, in `sample`, add `use crate::store::model::Anchor;` and:

```rust
            FieldType::Anchor => FieldValue::Anchor(Some(Anchor { pos: [1.5, -2.0], group: None, explicit: true })),
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold anchor` then `cargo test -j 8 -p manifold store::`
Expected: PASS, including `field_numbers_are_unique_and_never_reserved` and `values_round_trip_and_defaults_encode_to_nothing` (the latter now also checks `ANCHOR`'s absent value encodes to nothing). If `a_creates_unlisted_field_and_its_anchor_survive_compaction` fails on the field-99 assert, stop and report: it means a `Create`'s unlisted fields are lost and the spec's no-bump premise is false.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/store/model.rs crates/manifold/src/store/json.rs crates/manifold/src/store/journal/tests.rs crates/manifold/src/mirror.rs
git commit -m "Anchor register: the value, field 27 and its encoding

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: ANCHOR replaces PINNED (lists, mirror readers, Pin and Unpin, respawn)

**Files:**
- Modify: `crates/manifold/src/store/model.rs:180-198` (`TYPES`, `GENERIC`), `:268` (`PINNED`), `:303-330` (`FIELDS`, `RESERVED`), tests `:850`, `:941-959`
- Modify: `crates/manifold/src/mirror.rs:355-362` (`pinned`), `:437-439`, `:445-508` (`Carried`), tests `:579-601`, `:759-765`
- Modify: `crates/manifold/src/session_sync/edits.rs` (new `edit_anchor`, `clear_anchors` after `edit_pos` `:165-167`)
- Modify: `crates/manifold/src/shortcuts.rs:264-272` (`set_pinned`)
- Modify: `crates/manifold/src/tabs.rs:207-217`, `:237`, `:255-256`; `crates/manifold/src/tabs/spawn_result.rs:18`, `:42-60`, `:105`, `:178`, `:242`; `crates/manifold/src/node_ops.rs:112-131`; `crates/manifold/src/session_sync.rs:259-275`
- Modify (tests): `crates/manifold/src/session_sync/tests.rs` (helper `a_pin`, `start_respawn` `:338`), `tests/edits.rs:212-229`, `tests/misc.rs:8-13`, `tests/remote.rs:253`, `:586`, `:1024`, `:1065`, `:1100`, `tests/spawn.rs:460`, `:472`, `:482`, `tests/writes.rs:453`, `:467`, `:595`, `:854`, `:879`; `crates/manifold/src/store/json.rs` tests `:469`, `:610-621`; `crates/manifold/src/store/journal/tests.rs:472`; `crates/manifold/src/duplicate.rs` tests

**Interfaces:**
- Consumes: Task 1's `Anchor`, `FieldValue::Anchor`, `field::ANCHOR`, `as_anchor`.
- Produces:
  - `Mirror::anchor(&self, id: NodeId) -> Option<Anchor>`
  - `Mirror::held(&self, id: NodeId) -> Option<egui::Vec2>`: the anchor point when set and (`explicit` or `anchor.group == self.group(id)`)
  - `Mirror::pinned(&self, id: NodeId) -> bool`: `anchor(id).is_some_and(|a| a.explicit)`
  - `Carried::anchor(&self) -> Option<Anchor>`; `Carried::tab(pos: Option<egui::Vec2>, anchor: Option<Anchor>, (parent, group), url: String, view_kind: u64, jar: Option<NodeId>) -> Carried`
  - `App::edit_anchor(&mut self, id: NodeId, anchor: Option<Anchor>)`
  - `App::clear_anchors(&mut self, ids: &[NodeId], explicit: bool) -> Vec<(NodeId, Anchor)>`: clears each anchor whose bit equals `explicit`, returns what it cleared
  - `TabCarry { anchor: Option<Anchor>, zoom_factor: f64, title: String, muted: bool }`
  - test helper `pub(crate) fn a_pin() -> FieldValue` in `session_sync/tests.rs`

- [ ] **Step 1: Write the failing tests**

`crates/manifold/src/session_sync/tests.rs`, after `stored` (`:147-149`):

```rust
/// An explicit pin at the top-level origin, as a test or another replica writes one directly.
pub(crate) fn a_pin() -> FieldValue {
    FieldValue::Anchor(Some(crate::store::model::Anchor { pos: [0.0, 0.0], group: None, explicit: true }))
}
```

`crates/manifold/src/mirror.rs`: replace `pinned_reads_a_bool_and_defaults_to_false` with:

```rust
    #[test]
    fn pinned_is_the_explicit_bit_and_held_is_group_scoped() {
        use crate::store::model::Anchor;
        let mut mirror = Mirror::default();
        assert!(!mirror.pinned(node(7)) && mirror.held(node(7)).is_none(), "no anchor: neither");
        let drop_in = |group| FieldValue::Anchor(Some(Anchor { pos: [5.0, 6.0], group, explicit: false }));
        mirror.set(node(7), field::ANCHOR, drop_in(None));
        assert_eq!(mirror.held(node(7)), Some(egui::vec2(5.0, 6.0)), "a drop holds in the group it was written in");
        assert!(!mirror.pinned(node(7)), "a drop is not a pin");
        mirror.set_group(node(7), Some(node(4)));
        assert_eq!(mirror.held(node(7)), None, "inert in another group");
        mirror.set(node(7), field::ANCHOR, FieldValue::Anchor(Some(Anchor { pos: [5.0, 6.0], group: None, explicit: true })));
        assert_eq!(mirror.held(node(7)), Some(egui::vec2(5.0, 6.0)), "a pin holds in any group");
        assert!(mirror.pinned(node(7)));
    }
```

In `seed_then_carried_round_trips_and_never_overwrites` (`:585`) replace the `PINNED` line with:

```rust
            .with(field::ANCHOR, FieldValue::Anchor(Some(crate::store::model::Anchor { pos: [3.0, -4.0], group: Some(node(4)), explicit: true })))
```

`crates/manifold/src/store/model.rs` tests:
- `values_round_trip_and_defaults_encode_to_nothing` (`:850`): replace `(field::PINNED, FieldValue::Bool(true)),` with `(field::ANCHOR, FieldValue::Anchor(Some(Anchor { pos: [1.0, 2.0], group: Some(id), explicit: true }))),`.
- `every_card_kind_carries_the_pin` (`:941-948`): rename to `every_card_kind_carries_the_anchor`, doc "Any card can be pinned or dropped, so every card kind stores the anchor", assert `kind.fields().contains(&field::ANCHOR)`.
- `an_unknown_kind_carries_the_generic_fields` (`:955`): `&[POS, ANCHOR, PARENT, NAME, CLOSED]`.
- `field_numbers_are_unique_and_never_reserved`: add `assert!(field_def(7).is_none() && field_by_name("pinned").is_none(), "pinned is retired");`.

`crates/manifold/src/session_sync/tests/misc.rs:8-13`: replace each `PINNED` with `ANCHOR` (Tab, Group, Jar, Unknown lines).

New tests in `crates/manifold/src/session_sync/tests/edits.rs` (add `use crate::store::model::Anchor;` under `use super::*;`), after `a_pinned_card_stays_where_it_was_shown`:

```rust
/// Pin writes the drawn position and an explicit anchor there in one step; Unpin clears the anchor and leaves `POS`.
#[test]
fn pin_writes_pos_and_an_explicit_anchor_and_unpin_leaves_pos() {
    let mut app = App::default();
    let note = a_note_at(&mut app, egui::vec2(10.0, 10.0));
    a_note_at(&mut app, egui::vec2(52.0, 10.0));
    app.settle_current_level();
    let shown = app.layout_pos(note);
    app.toggle_pin(note);
    assert_eq!(app.doc.mirror.pos(note), Some(shown));
    assert_eq!(app.doc.mirror.anchor(note), Some(Anchor { pos: [shown.x, shown.y], group: None, explicit: true }));
    app.toggle_pin(note);
    assert_eq!(app.doc.mirror.anchor(note), None, "Unpin clears the pin");
    assert_eq!(app.doc.mirror.pos(note), Some(shown), "and leaves the card where it is");
}

/// Pinning a card held by a drop at its drawn point only sets the bit; an inert drop (written in another group) is
/// replaced by the drawn point in the current group, never promoted.
#[test]
fn pin_keeps_a_held_drop_and_replaces_an_inert_one() {
    let mut app = App::default();
    let g = add_group(&mut app, "g");
    let note = a_note_at(&mut app, egui::vec2(10.0, 10.0));
    app.snap_layout(note, egui::vec2(40.0, 20.0));
    app.edit_anchor(note, Some(Anchor { pos: [40.0, 20.0], group: None, explicit: false }));
    app.toggle_pin(note);
    assert_eq!(app.doc.mirror.anchor(note), Some(Anchor { pos: [40.0, 20.0], group: None, explicit: true }));
    app.toggle_pin(note);
    // An inert drop: written in `g`, while the mirror shows the card at the top level (a remote move that raced it,
    // applied to the mirror without the local release, as `remote.rs` does).
    app.edit_anchor(note, Some(Anchor { pos: [999.0, 999.0], group: Some(g), explicit: false }));
    assert_eq!(app.doc.mirror.held(note), None);
    app.toggle_pin(note);
    assert_eq!(app.doc.mirror.anchor(note), Some(Anchor { pos: [40.0, 20.0], group: None, explicit: true }));
}

/// Unpin All clears pins only: a tab held by a drop stays held.
#[test]
fn unpin_all_keeps_a_drop_anchor() {
    let mut app = App::default();
    let (dropped, pinned) = (
        add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0)),
        add(&mut app, Kind::Tab, failed_tab("https://u1/", 1.0)),
    );
    let drop = Anchor { pos: [7.0, 8.0], group: None, explicit: false };
    app.edit_anchor(dropped, Some(drop));
    app.toggle_pin(pinned);
    app.set_all_pinned(false);
    assert_eq!(app.doc.mirror.anchor(dropped), Some(drop));
    assert_eq!(app.doc.mirror.anchor(pinned), None);
}

/// A document from an older build: its `pinned` (field 7) is ignored, so the card is unpinned (spec, **Breaking:**).
#[test]
fn an_old_pinned_value_is_ignored() {
    let path = temp_dir("old-pinned").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::set(tab, 7, encode_field(7, &FieldValue::Bool(true)))]).unwrap();
    assert!(app.save_session());
    assert!(!app.doc.mirror.pinned(tab));
    assert_eq!(app.doc.mirror.anchor(tab), None);
}
```

Retarget `a_tab_pin_is_committed_and_a_remote_pin_reaches_the_mirror` (`edits.rs:212-229`):

```rust
    app.edit_field(tab, field::ANCHOR, a_pin());
    assert!(app.doc.mirror.pinned(tab));
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::ANCHOR), a_pin());

    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::ANCHOR, FieldValue::Anchor(None));
```

Retarget the remaining sites mechanically:
- `remote_set(&mut other, tab, field::PINNED, FieldValue::Bool(true))` → `remote_set(&mut other, tab, field::ANCHOR, a_pin())` (`remote.rs:253`, `:1024`, `:1065`, `:1100`, `writes.rs:595`).
- `app.edit_field(tab, field::PINNED, FieldValue::Bool(true))` → `app.edit_field(tab, field::ANCHOR, a_pin())` (`spawn.rs:460`, `writes.rs:453`, `:854`, `:879`).
- `stored(&app, tab, field::PINNED), FieldValue::Bool(true)` → `stored(&app, tab, field::ANCHOR), a_pin()` (`spawn.rs:482`, `writes.rs:467`).
- `spawn.carry.pinned = app.doc.mirror.pinned(tab);` → `spawn.carry.anchor = app.doc.mirror.anchor(tab);` (`spawn.rs:472`); `spawn.carry.pinned = carried.pinned();` → `spawn.carry.anchor = carried.anchor();` (`tests.rs:338`).
- `remote.rs:586` message "the url changed; pinned, engine and the rest did not" → "the url changed; the anchor, engine and the rest did not".
- `journal/tests.rs:472`: `set(tab, field::ANCHOR, FieldValue::Anchor(Some(Anchor { pos: [0.0, 0.0], group: None, explicit: true }))),` and its later assert on that field, if any, to match.
- `json.rs` `HAND_WRITTEN` keeps `"pinned": true` (an old export); `:469` becomes `assert_eq!(tab1.get(field::ANCHOR), FieldValue::Anchor(None), "an old export's pinned is dropped");`.
- `json.rs` `node_json_matches_a_document_nodes_json` (`:610-621`): replace the `PINNED` insert with `fields.insert(field::NAME, FieldValue::Str("Mine".into()));` and the `"pinned"` assert with `assert_eq!(json.fields.get("name"), Some(&Value::String("Mine".into())));`.

`crates/manifold/src/duplicate.rs` `mod tests`, new test:

```rust
    /// A pinned card's copy keeps where it was drawn but arrives unpinned: the export drops anchors (spec, Behaviour
    /// changes).
    #[test]
    fn a_duplicated_pinned_note_keeps_its_place_but_not_its_pin() {
        let path = store::testutil::temp_dir("duplicate-pinned").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = add(&mut app, Kind::Note, Runtime::Note(crate::node::NotePayload { text: "n".into(), editing: false }));
        app.snap_layout(note, egui::vec2(10.0, 20.0));
        app.toggle_pin(note);
        assert!(app.save_session());
        app.duplicate_nodes(&[note]);
        let copy = *app.nodes.keys().find(|&&id| id != note).unwrap();
        assert_eq!(app.doc.mirror.pos(copy), Some(egui::vec2(10.0 + DUPLICATE_OFFSET, 20.0 + DUPLICATE_OFFSET)));
        assert!(!app.doc.mirror.pinned(copy) && app.doc.mirror.anchor(copy).is_none());
        assert!(app.doc.mirror.pinned(note), "the original stays pinned");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold pin`
Expected: FAIL to compile ("no method named `anchor`", "no method named `edit_anchor`", "no field `anchor` on `TabCarry`").

- [ ] **Step 3: Implement**

`model.rs`:
- In `TYPES` replace `PINNED` with `ANCHOR` in place on the Tab, Group, Note, Image, Pdf, Json, Download and Jar lines; `GENERIC` becomes `&[field::POS, field::ANCHOR, field::PARENT, field::NAME, field::CLOSED]`.
- Replace `pub const PINNED: u32 = 7;` with the comment line `// Never reuse 7 (the retired \`pinned\` bool, replaced by \`ANCHOR\`).`.
- Delete the `pinned` `FieldDef`; `FIELDS` back to `[FieldDef; 19]`.
- `RESERVED`: `pub const RESERVED: [u32; 3] = [7, 16, 17];` and its doc: "Retired: 7, the `pinned` bool (replaced by `ANCHOR`); 16 and 17, a tab's `jar_set` and a group's `default_jar` (replaced by the `jar` edge). Never reused; old documents keep them raw (checked by a test)."

`mirror.rs` — add `Anchor` to the `crate::store::model` import, then replace `pinned` (`:355-362`) with:

```rust
    /// The node's anchor (`field::ANCHOR`), held or not.
    pub(crate) fn anchor(&self, id: NodeId) -> Option<Anchor> {
        self.nodes.get(&id).and_then(anchor_in)
    }

    /// Where a settle holds card `id`, if it does: its anchor point when the anchor is a pin (any group) or was
    /// written in the group the card is in now. Every reader of a hold reads this one predicate (spec Slice 1,
    /// "Meaning"); an implicit anchor from another group is inert. It reads the document only, so replicas agree.
    pub(crate) fn held(&self, id: NodeId) -> Option<egui::Vec2> {
        let a = self.anchor(id)?;
        (a.explicit || a.group == self.group(id)).then(|| egui::vec2(a.pos[0], a.pos[1]))
    }

    /// The user's pin: an explicit anchor, in any group. The pin mark, the menu rows, Pin All and the idle-sleep
    /// exemption read it; a drop's anchor never counts.
    pub(crate) fn pinned(&self, id: NodeId) -> bool {
        self.anchor(id).is_some_and(|a| a.explicit)
    }
```

Replace `pinned_in` with:

```rust
fn anchor_in(fields: &Fields) -> Option<Anchor> {
    fields.get(&field::ANCHOR).and_then(FieldValue::as_anchor)
}
```

`Carried`: doc comment `:447` "Read the fields a respawn needs through `pos`, `anchor` and `jar`."; `tab` takes `anchor: Option<Anchor>` in place of `pinned: bool` and writes `.with(field::ANCHOR, FieldValue::Anchor(anchor))`; replace `Carried::pinned` with:

```rust
    pub(crate) fn anchor(&self) -> Option<Anchor> {
        anchor_in(&self.fields)
    }
```

`session_sync/edits.rs`, after `edit_pos` (add `Anchor` to the file's `crate::store::model` import, or write `crate::store::model::Anchor`):

```rust
    /// The one writer of a node's anchor (spec Slice 1): Pin, a drop (`drop_at`), a release (`clear_anchors`) and
    /// undo replay come here. A non-finite point writes the absent value, as `edit_pos` does.
    pub(crate) fn edit_anchor(&mut self, id: NodeId, anchor: Option<Anchor>) {
        let anchor = anchor.filter(|a| a.pos.iter().all(|c| c.is_finite()));
        self.edit_field(id, field::ANCHOR, FieldValue::Anchor(anchor));
    }

    /// Releases the anchor of each of `ids` whose explicit bit is `explicit`, and returns what it released, for an
    /// undo record. The three ways a hold is let go share it: Unpin (`explicit`), Tidy and Arrange (not), and a local
    /// move or fold (not; `edit_group`, `fold_group`).
    pub(crate) fn clear_anchors(&mut self, ids: &[NodeId], explicit: bool) -> Vec<(NodeId, Anchor)> {
        let mut cleared = Vec::new();
        for &id in ids {
            if let Some(a) = self.doc.mirror.anchor(id).filter(|a| a.explicit == explicit) {
                self.edit_anchor(id, None);
                cleared.push((id, a));
            }
        }
        cleared
    }
```

`shortcuts.rs` `set_pinned` (`:264-272`):

```rust
    /// Every pin and unpin goes through here. Pin holds the card where it is drawn: `POS` and an explicit anchor at
    /// the drawn point in its current group, except that a card already held there (a drop) only gains the bit. An
    /// inert anchor (written in another group) is replaced, never promoted. Unpin releases a pin only, so a drop's
    /// anchor stays (Unpin All), and leaves `POS`: the card stays until the next settle seeds from it.
    pub(crate) fn set_pinned(&mut self, id: NodeId, on: bool) {
        if !on {
            self.clear_anchors(&[id], true);
            return;
        }
        let shown = self.layout_pos(id);
        let anchor = match self.doc.mirror.anchor(id) {
            Some(a) if self.doc.mirror.held(id) == Some(shown) => Anchor { explicit: true, ..a },
            _ => {
                self.edit_pos(id, Some(shown));
                Anchor { pos: [shown.x, shown.y], group: self.doc.mirror.group(id), explicit: true }
            }
        };
        self.edit_anchor(id, Some(anchor));
    }
```

Also update `toggle_pin`'s doc (`:256`): "Pins or unpins node `id` of any kind: a pinned card is held where it is drawn (`set_pinned`)."

Respawn carry:
- `tabs.rs:207-217`: `/// See \`Mirror::anchor\`; seeds the mirror when the tab's id has no entry yet.` `pub(crate) anchor: Option<crate::store::model::Anchor>,`; `TabSpawn::new` (`:237`) `carry: TabCarry { anchor: None, zoom_factor, title: String::new(), muted: false }`; `respawn` (`:256`) `anchor: carried.anchor(),`.
- `node_ops.rs:114`: `let (pos, anchor, jar) = (carried.pos(), carried.anchor(), carried.jar());` and `:124` `let carry = TabCarry { anchor, zoom_factor, title, muted };`.
- `tabs/spawn_result.rs`: `:18` `let TabCarry { anchor, zoom_factor, title, muted } = carry;`; the wake branch `:42-60` binds `let anchor = carried.anchor();` and returns `(parent, group, anchor, tab.muted)`, the tuple pattern becomes `let (parent, group, anchor, muted) = ...` and the else arm `(parent, group, anchor, muted)`; `:105` and `:242` `Carried::tab(Some(pos), anchor, (parent, group), ...)`; `:178` `TabCarry { anchor, zoom_factor, title, muted }`.
- `session_sync.rs:270`: `(field::ANCHOR, FieldValue::Anchor(spawn.carry.anchor)),`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold`
Expected: PASS (whole crate: this task touches every former `PINNED` reader). Then
Run: `git grep -n "PINNED\|carry\.pinned\|carried\.pinned\|pinned_in" -- crates`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/manifold
git diff --cached --stat
git commit -m "Anchor register replaces PINNED: lists, mirror readers, Pin and Unpin, respawn

Field 7 is reserved; old pins are ignored (no migration).

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The settle, the settle key and remote snaps read `held`

**Files:**
- Modify: `crates/manifold/src/graph_nav.rs:554-600` (`settle_current_level`)
- Modify: `crates/manifold/src/level.rs:95-121` (`SettleKey`, `settle_key`), tests after `:382`
- Modify: `crates/manifold/src/session_sync/remote.rs:108-113` (`update_live_node`)
- Test: `crates/manifold/src/session_sync/tests/edits.rs`, `tests/remote.rs`, `level.rs` `mod tests`

**Interfaces:**
- Consumes: `Mirror::held`, `App::edit_anchor` (Task 2).
- Produces: `SettleKey { pub(crate) count: usize, sizes: u64, holds: u64 }`; the settle seeds a held card from its anchor point and holds it fixed; a remote `POS` or `ANCHOR` change snaps a card not being dragged to `held(id).unwrap_or(pos)`.

- [ ] **Step 1: Write the failing tests**

`tests/edits.rs`, after `a_settle_ends_where_the_document_says_whatever_came_before`:

```rust
/// A held card is seeded from its anchor point, not its `POS` (they differ after a remote write), and stays there.
#[test]
fn a_held_card_is_placed_at_its_anchor_not_its_pos() {
    let mut app = App::default();
    let note = a_note_at(&mut app, egui::vec2(10.0, 10.0));
    a_note_at(&mut app, egui::vec2(52.0, 10.0));
    app.edit_anchor(note, Some(Anchor { pos: [200.0, 0.0], group: None, explicit: false }));
    app.settle_current_level();
    assert_eq!(app.layout_pos(note), egui::vec2(200.0, 0.0));
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(10.0, 10.0)), "the settle writes no document pos");
}

/// An inert anchor (written in another group) holds nothing: the settle moves the card as if it had none.
#[test]
fn an_inert_anchor_holds_nothing_in_the_settle() {
    let mut app = App::default();
    let g = add_group(&mut app, "g");
    let note = a_note_at(&mut app, egui::vec2(10.0, 10.0));
    a_note_at(&mut app, egui::vec2(12.0, 10.0));
    app.edit_anchor(note, Some(Anchor { pos: [10.0, 10.0], group: Some(g), explicit: false }));
    app.settle_current_level();
    assert_ne!(app.layout_pos(note), egui::vec2(10.0, 10.0), "pushed apart from its overlapping neighbour");
}
```

`tests/remote.rs` (add `use crate::store::model::Anchor;`), after `a_remote_pos_snaps_the_layout`:

```rust
/// Another replica's anchor reaches the mirror and snaps the layout to its point; a later remote clear snaps the card
/// back to its `POS`.
#[test]
fn a_remote_anchor_snaps_the_layout_and_a_remote_clear_falls_back_to_pos() {
    let path = temp_dir("anchor-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let theirs = Anchor { pos: [5.0, 6.0], group: None, explicit: false };
    remote_set(&mut other, note, field::ANCHOR, FieldValue::Anchor(Some(theirs)));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.anchor(note), Some(theirs));
    assert_eq!(app.layout_pos(note), egui::vec2(5.0, 6.0));
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(10.0, 20.0)));
    remote_set(&mut other, note, field::ANCHOR, FieldValue::Anchor(None));
    assert!(app.save_session());
    assert_eq!(app.layout_pos(note), egui::vec2(10.0, 20.0), "no hold: drawn at its document pos");
}

/// A card the user is dragging is not snapped by a remote anchor either.
#[test]
fn a_remote_anchor_does_not_snap_a_card_being_dragged() {
    let path = temp_dir("anchor-remote-drag").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    drag(&mut app, &[note], egui::vec2(0.0, 50.0));
    remote_set(&mut other, note, field::ANCHOR, FieldValue::Anchor(Some(Anchor { pos: [5.0, 6.0], group: None, explicit: false })));
    assert!(app.save_session());
    assert_eq!(app.layout_pos(note), egui::vec2(10.0, 70.0), "the dragged card stays under the pointer");
}
```

`level.rs` `mod tests`, after `moving_a_card_does_not_change_the_settle_key`:

```rust
    /// The held set with its anchor points is part of the key: an anchor write (a drop, a remote Pin) re-settles the
    /// level, a `POS` write alone does not, and an inert anchor counts as no hold.
    #[test]
    fn an_anchor_write_changes_the_settle_key_and_a_pos_write_does_not() {
        use crate::store::model::Anchor;
        let mut app = App::default();
        app.insert_placeholder_node(D1, egui::Vec2::ZERO, Kind::Download, download("d.bin"));
        let entry = CardEntry {
            id: CardId::from(D1),
            kind: crate::card_kind::fixtures::plain(crate::store::model::TYPE_DOWNLOAD),
            origin: Origin::Stored,
            rect: node_card_rect(egui::Vec2::ZERO, &download("d.bin")),
        };
        let first = app.settle_key(std::slice::from_ref(&entry));
        app.edit_pos(D1, Some(egui::vec2(30.0, 40.0)));
        assert_eq!(app.settle_key(std::slice::from_ref(&entry)), first, "a pos write");
        app.edit_anchor(D1, Some(Anchor { pos: [30.0, 40.0], group: Some(OTHER), explicit: false }));
        assert_eq!(app.settle_key(std::slice::from_ref(&entry)), first, "an inert anchor holds nothing");
        app.edit_anchor(D1, Some(Anchor { pos: [30.0, 40.0], group: None, explicit: false }));
        let held = app.settle_key(std::slice::from_ref(&entry));
        assert_ne!(held, first, "a drop");
        app.edit_anchor(D1, Some(Anchor { pos: [31.0, 40.0], group: None, explicit: false }));
        assert_ne!(app.settle_key(std::slice::from_ref(&entry)), held, "the anchor point moved");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold anchor`
Expected: FAIL: `a_held_card_is_placed_at_its_anchor_not_its_pos` (layout at the settled `POS`, not (200, 0)), `a_remote_anchor_snaps_the_layout...` (layout not (5, 6)), `an_anchor_write_changes_the_settle_key...` ("a drop" assert_ne fails).

- [ ] **Step 3: Implement**

`graph_nav.rs` `settle_current_level`: replace the `start` closure and the `pinned` set:

```rust
        // Starts from the document, at rest: a held card from its anchor point (`Mirror::held`), any other from its
        // document pos, so the result depends only on the document and the cards' sizes, not on earlier settles; an
        // unplaced node starts where it is drawn. Writes the layout only (`App::layout`): physics never writes the
        // document.
        let at_level = || self.nodes.keys().filter(|id| self.in_level(**id));
        let start = |id: NodeId| {
            self.doc.mirror.held(id).or_else(|| self.doc.mirror.pos(id)).unwrap_or_else(|| self.layout_pos(id))
        };
        let mut positions: HashMap<NodeId, egui::Vec2> = at_level().map(|id| (*id, start(*id))).collect();
        let mut velocities: HashMap<NodeId, egui::Vec2> = at_level().map(|id| (*id, egui::Vec2::ZERO)).collect();
        let edges = self.spring_edges();
        // Held cards stay where they start and still repel.
        let held: HashSet<NodeId> = at_level().filter(|id| self.doc.mirror.held(**id).is_some()).copied().collect();
```

and pass `&held` in place of `&pinned` to `step_sized`. Remove the now-replaced comment lines that described starting from the document pos (the new comment above covers them). Update the doc comment above the fn if it says "pinned": "a held card (`Mirror::held`) never moves".

`level.rs`:

```rust
/// What a level's layout was computed from, besides its group (`GraphUi::settled_key`): how many of its cards are
/// not virtual (a source's entry count can change with each navigation, and that must not re-settle (move) the
/// stored cards), the stored cards' sizes, the layout's other input, so a card that changes size (an image
/// decoding) re-lays-out the level as a new card does, and the held cards with their anchor points
/// (`Mirror::held`), so a drop, a Pin or an Unpin, local or remote, re-lays-out the level around them. Sizes and
/// holds are summed, so drawing order doesn't change the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SettleKey {
    pub(crate) count: usize,
    sizes: u64,
    holds: u64,
}
```

and in `settle_key`, after `sizes`:

```rust
        let holds = items
            .iter()
            .filter(|item| item.origin == Origin::Stored)
            .filter_map(|item| {
                let id = item.id.node()?;
                let at = self.doc.mirror.held(id)?;
                Some(hasher.hash_one((id, at.x.to_bits(), at.y.to_bits())))
            })
            .fold(0, u64::wrapping_add);
```

with `holds,` added to the returned `SettleKey`.

`session_sync/remote.rs` `update_live_node` (`:111`):

```rust
        // A held card is drawn at its anchor point, any other at its document pos (`settle_current_level` seeds the
        // same way), so a remote anchor write or clear snaps it like a remote pos.
        if (changed(field::POS) || changed(field::ANCHOR)) && !self.is_being_dragged(CardId::from(id)) {
            let at = self.doc.mirror.held(id).unwrap_or(pos);
            self.snap_layout(id, at);
        }
```

and in the fn's doc comment: "A new pos or anchor snaps the layout to where the card is held, else to `pos`, at rest, except for a card being dragged".

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold anchor` then `cargo test -j 8 -p manifold settle` then `cargo test -j 8 -p manifold session_sync::tests::remote`
Expected: PASS (including `a_card_pinned_on_one_replica_stays_where_it_was_shown_on_the_other` and `a_pinned_card_stays_where_it_was_shown`).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/graph_nav.rs crates/manifold/src/level.rs crates/manifold/src/session_sync/remote.rs crates/manifold/src/session_sync/tests/edits.rs crates/manifold/src/session_sync/tests/remote.rs
git commit -m "Anchor register: the settle, its key and remote snaps read held

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: A drop writes the anchor; a local move releases a drop's

**Files:**
- Modify: `crates/manifold/src/graph_nav.rs:206-222` (`release_drag`), new `drop_at` after it
- Modify: `crates/manifold/src/session_sync/edits.rs:188-198` (`edit_group`)
- Modify: `crates/manifold/src/debug_control.rs:1207-1217` (`debug_move_node`), test `:1571-1574`
- Test: `crates/manifold/src/session_sync/tests/edits.rs` (rewrite `:88-116`, new tests)

**Interfaces:**
- Consumes: `edit_anchor`, `clear_anchors`, `Mirror::held`, `Mirror::pinned` (Task 2); the held settle (Task 3).
- Produces: `App::drop_at(&mut self, id: NodeId)`: writes `POS` = `layout_pos(id)` and `Anchor { pos: layout_pos, group: mirror.group(id), explicit: mirror.pinned(id) }`. `edit_group` calls `clear_anchors(&[id], false)` after a real group change.

- [ ] **Step 1: Write the failing tests**

Replace `a_drag_release_saves_the_dropped_position` (`tests/edits.rs:88-116`):

```rust
/// A drag release writes the document pos and a drop anchor there; the next settle holds the card where it was
/// dropped and moves its neighbour around it. Physics never writes the document.
#[test]
fn a_drag_release_saves_the_dropped_position() {
    let path = temp_dir("pos-drag").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 10.0));
    let other = a_note_at(&mut app, egui::vec2(52.0, 10.0));
    assert!(app.save_session());
    drag(&mut app, &[note], egui::vec2(40.0, 0.0));
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(Some((10.0, 10.0))), "a drag in progress is not saved");
    app.release_drag();
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(50.0, 10.0)), "shown at once");
    assert_eq!(app.doc.mirror.anchor(note), Some(Anchor { pos: [50.0, 10.0], group: None, explicit: false }));
    assert!(!app.doc.mirror.pinned(note), "a drop is not a pin");
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(Some((50.0, 10.0))));

    app.settle_current_level();
    assert_eq!(app.layout_pos(note), egui::vec2(50.0, 10.0), "the dropped card stays where it was dropped");
    assert_ne!(app.layout_pos(other), egui::vec2(52.0, 10.0), "its overlapping neighbour moved around it");
    assert!(app.save_session());
    assert!(!app.doc.writes.has_fields());
    assert_eq!(stored(&app, other, field::POS), FieldValue::Pos(Some((52.0, 10.0))), "physics never writes the document");
}
```

New tests after it:

```rust
/// A local move releases a drop's anchor and keeps a pin, which then holds at the same (x, y) in the new group; a
/// fold (Dissolve) and its put-back keep the pin too.
#[test]
fn a_move_releases_a_drop_and_keeps_a_pin() {
    let path = temp_dir("anchor-move").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "g");
    let (dropped, pinned) = (a_note_at(&mut app, egui::vec2(10.0, 10.0)), a_note_at(&mut app, egui::vec2(90.0, 10.0)));
    app.snap_layout(dropped, egui::vec2(30.0, 30.0));
    app.drop_at(dropped);
    app.snap_layout(pinned, egui::vec2(70.0, 70.0));
    app.toggle_pin(pinned);
    app.edit_group(dropped, Some(g));
    app.edit_group(pinned, Some(g));
    assert_eq!(app.doc.mirror.anchor(dropped), None, "a move releases a drop");
    assert_eq!(app.doc.mirror.held(pinned), Some(egui::vec2(70.0, 70.0)), "a pin holds at its (x, y) in the new group");
    app.ungroup(&[g]);
    assert_eq!(app.doc.mirror.group(pinned), None);
    assert!(app.doc.mirror.pinned(pinned), "a fold keeps a pin");
    app.reopen_node(g);
    assert_eq!(app.doc.mirror.group(pinned), Some(g), "put back");
    assert_eq!(app.doc.mirror.held(pinned), Some(egui::vec2(70.0, 70.0)), "a put-back keeps a pin");
}

/// Dropping a pinned card keeps the bit, in any group: the drop rewrites the pin at the new point and group.
#[test]
fn a_drop_of_a_pin_in_another_group_keeps_the_bit() {
    let mut app = App::default();
    let g = add_group(&mut app, "g");
    let note = a_note_at(&mut app, egui::vec2(10.0, 10.0));
    app.toggle_pin(note);
    app.edit_group(note, Some(g));
    app.snap_layout(note, egui::vec2(-20.0, 5.0));
    app.drop_at(note);
    assert_eq!(app.doc.mirror.anchor(note), Some(Anchor { pos: [-20.0, 5.0], group: Some(g), explicit: true }));
    assert_eq!(app.doc.mirror.pos(note), Some(egui::vec2(-20.0, 5.0)));
}

/// A drop on a tab holds it without pinning it, so the idle-sleep exemption (which reads `pinned`,
/// `tabs/sleep.rs:136`) does not apply. A live tab can't be built in a unit test (no fake `ViewTab`), so this checks
/// the predicate the exemption reads.
#[test]
fn a_drop_does_not_pin_a_tab() {
    let mut app = App::default();
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    app.snap_layout(tab, egui::vec2(4.0, 4.0));
    app.drop_at(tab);
    assert!(app.doc.mirror.held(tab).is_some() && !app.doc.mirror.pinned(tab));
}
```

`debug_control.rs` test (`:1571-1574`), after the existing `layout_pos` assert add:

```rust
        let anchor = crate::store::model::Anchor { pos: [12.0, -34.0], group: app.doc.mirror.group(note), explicit: false };
        assert_eq!(app.doc.mirror.anchor(note), Some(anchor), "MOVENODE drops as a drag release does");
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold drop` then `cargo test -j 8 -p manifold name_text_and_pos_commands_write_the_document`
Expected: FAIL to compile ("no method named `drop_at`").

- [ ] **Step 3: Implement**

`graph_nav.rs`, replace the loop body in `release_drag` and add `drop_at`:

```rust
        for moved_id in moved {
            let Some(key) = moved_id.node() else { continue };
            if self.nodes.contains_key(&key) {
                self.interaction.drag_origin.push((moved_id, self.doc.mirror.pos(key)));
                self.drop_at(key);
            }
        }
    }

    /// Card `id` released where it is drawn: its document pos and an anchor there, in the group it is in, keeping
    /// its pin bit (so dragging a pin keeps it a pin, in any group). A drag release and headless `MOVENODE` both come
    /// here.
    pub(crate) fn drop_at(&mut self, id: NodeId) {
        let at = self.layout_pos(id);
        self.edit_pos(id, Some(at));
        let explicit = self.doc.mirror.pinned(id);
        self.edit_anchor(id, Some(Anchor { pos: [at.x, at.y], group: self.doc.mirror.group(id), explicit }));
    }
```

(Task 5 changes the `drag_origin.push` line; leave it as is here.) Update `release_drag`'s doc: "each card it moved (`drag_moved`) is dropped where it is drawn (`drop_at`: its pos and anchor)". Import `crate::store::model::Anchor` in `graph_nav.rs` if `super::*` doesn't bring it.

`session_sync/edits.rs` `edit_group`, after `self.doc.mirror.set_group(id, group);`:

```rust
        // A drop's anchor holds only in the group it was written in: a local move releases it, a pin stays (spec
        // Slice 1). Move, Join, GroupWith, Eject, fold, put-back and undo replay all come through here; a remote move
        // writes the mirror directly, which is how an inert anchor arises.
        self.clear_anchors(&[id], false);
```

and its doc: "The one writer of a node's group: shown at once, committed as a `Move` by the next save; releases an implicit anchor."

`debug_control.rs` `debug_move_node`:

```rust
    /// `MOVENODE`: card `id` dragged to world point `pos` and released. The drag moves the layout (`card.rs`), the
    /// release drops it there (`drop_at`: its document pos and anchor, as `release_drag` writes them), and the drop
    /// saves (`apply_drop` with no target). `false` when there is no such node.
    fn debug_move_node(&mut self, id: NodeId, pos: egui::Vec2) -> bool {
        if !self.nodes.contains_key(&id) {
            return false;
        }
        self.snap_layout(id, pos);
        self.drop_at(id);
        self.save_session();
        true
    }
```

Update the `MOVENODE` help row (`debug_control.rs:867`) text if it says "document pos": "...released: its document pos and drop anchor are written as a drag release writes them, and saved".

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold`
Expected: PASS (whole crate: `edit_group` is shared by every move path).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/graph_nav.rs crates/manifold/src/session_sync/edits.rs crates/manifold/src/debug_control.rs crates/manifold/src/session_sync/tests/edits.rs
git commit -m "Anchor register: a drop writes the anchor, a local move releases a drop's

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Undo restores anchors (drops and folds)

**Files:**
- Modify: `crates/manifold/src/undo.rs:13-19` (`Moved`), `:32-49` (`UndoKind::Fold`), `:125-127`, `:229-296` (`replay_revert`, `replay_fold`)
- Modify: `crates/manifold/src/graph_nav.rs:25-27` (`drag_origin`), `:218` (`release_drag` push), `:294-318` (`membership_snapshot`, `moved_since`)
- Modify: `crates/manifold/src/groups.rs:31-62` (`ungroup`, `fold_groups`, `fold_group`)
- Modify: `crates/manifold/src/search/arrange.rs:92-97` (the `Moved` literal), `crates/manifold/src/search/suggest.rs:90-95` (check it still compiles)
- Test: `crates/manifold/src/undo/tests.rs` (`fold` helper `:8-10`, literals `:150`, `:218`, new tests)

**Interfaces:**
- Consumes: `drop_at`, `edit_anchor`, `clear_anchors` (Tasks 2, 4).
- Produces:
  - `Moved { id, group, parent, pos, anchor: (Option<Anchor>, Option<Anchor>) }`
  - `pub(crate) struct Folded { pub(crate) group: NodeId, pub(crate) dest: Option<NodeId>, pub(crate) anchors: Vec<(NodeId, Anchor)> }`; `UndoKind::Fold(Vec<Folded>)`
  - `App::fold_group(&mut self, id: NodeId, dest: Option<NodeId>) -> Vec<(NodeId, Anchor)>` (the members' released drop anchors)
  - `Interaction::drag_origin: Vec<(CardId, Option<egui::Vec2>, Option<Anchor>)>`
  - `App::membership_snapshot(&self) -> Vec<(NodeId, Option<NodeId>, Option<NodeId>, Option<Anchor>)>`
  - `App::moved_since(&self, before: Vec<(NodeId, Option<NodeId>, Option<NodeId>, Option<Anchor>)>, origin: &[(CardId, Option<egui::Vec2>, Option<Anchor>)]) -> Vec<Moved>`

- [ ] **Step 1: Write the failing tests**

`undo/tests.rs`: add `Anchor` to the `crate::store::model` import; change the helper and the two literals:

```rust
fn fold(tag: NodeId) -> UndoKind {
    UndoKind::Fold(vec![Folded { group: tag, dest: None, anchors: Vec::new() }])
}
```

`:150` `app.interaction.drag_origin = vec![(CardId::from(n), app.doc.mirror.pos(n), None)];` and `:218` `app.interaction.drag_origin = vec![(CardId::from(a), None, None), (CardId::from(b), None, None)];`.

New tests after `a_join_is_undone_and_redone`:

```rust
/// What a drag does to `n` (the layout follows the pointer), then its release.
fn drag_and_release(app: &mut App, n: NodeId, to: egui::Vec2) {
    app.interaction.dragging = Some(CardId::from(n));
    app.interaction.drag_moved = vec![CardId::from(n)];
    app.snap_layout(n, to);
    app.release_drag();
}

/// A card dropped earlier, dragged again and released into a group: Undo puts back its pre-drag position and its
/// earlier drop anchor (not the one the release wrote), Redo the Join, whose move released the anchor.
#[test]
fn undo_of_a_drag_then_join_restores_the_pre_drag_anchor() {
    let path = store::testutil::temp_dir("join-anchor-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, _) = group_with_note(&mut app);
    let n = add_note(&mut app, "loose");
    app.snap_layout(n, egui::vec2(40.0, 50.0));
    app.drop_at(n);
    let earlier = app.doc.mirror.anchor(n);
    assert!(earlier.is_some());
    drag_and_release(&mut app, n, egui::vec2(300.0, 300.0));
    app.apply_drop(CardId::from(n), Some((DropEffect::Join, Some(CardId::from(g)))));
    assert_eq!((app.doc.mirror.group(n), app.doc.mirror.anchor(n)), (Some(g), None), "the move released the drop");
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.group(n), None);
    assert_eq!(app.doc.mirror.pos(n), Some(egui::vec2(40.0, 50.0)));
    assert_eq!(app.doc.mirror.anchor(n), earlier, "the earlier drop, not the release's");
    app.step_history(None, true);
    assert_eq!((app.doc.mirror.group(n), app.doc.mirror.anchor(n)), (Some(g), None));
}

/// Undo skips a card dropped again since the step: its anchor is no longer the one the step left.
#[test]
fn an_undo_skips_a_card_dropped_again_since() {
    let path = store::testutil::temp_dir("join-anchor-skip").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, _) = group_with_note(&mut app);
    let n = add_note(&mut app, "loose");
    drag_and_release(&mut app, n, egui::vec2(300.0, 300.0));
    app.apply_drop(CardId::from(n), Some((DropEffect::Join, Some(CardId::from(g)))));
    let pos = app.doc.mirror.pos(n);
    app.drop_at(n); // dropped again inside the group, at the same point
    assert_eq!(app.doc.mirror.pos(n), pos, "only the anchor differs");
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.group(n), Some(g), "left where the user put it");
}

/// Dissolve releases the members' drop anchors; Undo reopens the group and puts each back, Redo releases them again.
#[test]
fn a_fold_is_undone_with_its_members_anchors() {
    let path = store::testutil::temp_dir("fold-anchor-undo").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (g, inner) = group_with_note(&mut app);
    app.snap_layout(inner, egui::vec2(70.0, 80.0));
    app.drop_at(inner);
    let dropped = app.doc.mirror.anchor(inner);
    assert_eq!(dropped.map(|a| a.group), Some(Some(g)));
    app.ungroup(&[g]);
    assert_eq!((app.doc.mirror.group(inner), app.doc.mirror.anchor(inner)), (None, None));
    app.step_history(None, false);
    assert_eq!(app.doc.mirror.group(inner), Some(g));
    assert_eq!(app.doc.mirror.anchor(inner), dropped, "the drop is back with the card");
    app.step_history(None, true);
    assert_eq!((app.doc.mirror.group(inner), app.doc.mirror.anchor(inner)), (None, None));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold undo::`
Expected: FAIL to compile ("cannot find struct `Folded`", tuple arity mismatch on `drag_origin`).

- [ ] **Step 3: Implement**

`undo.rs` (import `crate::store::model::Anchor`):

```rust
/// One card a drop, Tidy or Arrange changed: each field's (before, after).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Moved {
    pub(crate) id: NodeId,
    pub(crate) group: (Option<NodeId>, Option<NodeId>),
    pub(crate) parent: (Option<NodeId>, Option<NodeId>),
    pub(crate) pos: (Option<egui::Vec2>, Option<egui::Vec2>),
    pub(crate) anchor: (Option<Anchor>, Option<Anchor>),
}

/// One group a fold closed (`App::fold_group`): where its members went, and the drop anchors the move released, so
/// Undo can put them back with the members.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Folded {
    pub(crate) group: NodeId,
    pub(crate) dest: Option<NodeId>,
    pub(crate) anchors: Vec<(NodeId, Anchor)>,
}
```

`UndoKind::Fold(Vec<Folded>)`, doc: "Dissolve, Ungroup All, Merge: each group, where `fold_group` sent its members and the drop anchors it released. Undo reopens the groups and restores those anchors, redo folds them again." Update the module doc's list of writers: "through `edit_group`, `edit_parent`, `edit_pos` and `edit_anchor`".

`replay_revert`: extend `holds` and write the anchor after the group and parent:

```rust
            let holds = self.card_in_group(m.id, side(&m.group, leaving_after))
                && self.doc.mirror.parent(m.id) == side(&m.parent, leaving_after)
                && self.doc.mirror.pos(m.id) == if leaving_after { m.pos.1 } else { m.pos.0 }
                && self.doc.mirror.anchor(m.id) == if leaving_after { m.anchor.1 } else { m.anchor.0 };
```

and after the `edit_parent` block, before the `pos` block:

```rust
            // After the group: `edit_group` releases a drop's anchor, and the arriving one is written over it.
            self.edit_anchor(m.id, if arriving_after { m.anchor.1 } else { m.anchor.0 });
```

`replay` (`:127`): `UndoKind::Fold(folds) => self.replay_fold(folds, redo),`. `replay_fold`:

```rust
    /// Undo reopens each group (`reopen_node`, which puts back the members still where the fold sent them) and
    /// restores the drop anchors the fold released to members back in it with none since; redo folds each again as
    /// the group is now, if it and its destination are shown.
    fn replay_fold(&mut self, folds: &[Folded], redo: bool) -> (usize, usize) {
        let mut done = 0;
        for f in folds {
            if redo {
                if self.nodes.contains_key(&f.group) && f.dest.as_ref().is_none_or(|d| self.nodes.contains_key(d)) {
                    self.fold_group(f.group, f.dest);
                    done += 1;
                }
            } else if !self.nodes.contains_key(&f.group) {
                // A group shown again already (reopened from Recently Closed) has nothing to undo.
                self.reopen_node(f.group);
                for &(member, anchor) in &f.anchors {
                    if self.card_in_group(member, Some(f.group)) && self.doc.mirror.anchor(member).is_none() {
                        self.edit_anchor(member, Some(anchor));
                        self.snap_layout(member, egui::vec2(anchor.pos[0], anchor.pos[1]));
                    }
                }
                done += 1;
            }
        }
        (done, folds.len())
    }
```

`groups.rs`:

```rust
        let folds = ids.iter().map(|id| (*id, self.doc.mirror.group(*id))).collect();
        self.fold_groups(folds, message);
```

stays in `ungroup`; `fold_groups`:

```rust
    pub(crate) fn fold_groups(&mut self, folds: Vec<(NodeId, Option<NodeId>)>, message: String) {
        let folds = folds
            .into_iter()
            .map(|(group, dest)| Folded { group, dest, anchors: self.fold_group(group, dest) })
            .collect();
        self.record_undo(message, UndoKind::Fold(folds));
    }
```

`fold_group` returns `Vec<(NodeId, Anchor)>`: release first so the record holds them (`edit_group` would release them anyway):

```rust
    pub(crate) fn fold_group(&mut self, id: NodeId, dest: Option<NodeId>) -> Vec<(NodeId, Anchor)> {
        let members: Vec<NodeId> = self.members_of(id).collect();
        let released = self.clear_anchors(&members, false);
        for &member in &members {
            self.edit_group(member, dest);
        }
        self.edit_field(id, field::HELD, FieldValue::Held { dest, members: members.clone() });
        self.close_node(id);
        for &member in &members {
            self.recheck_edge_scope(member);
        }
        released
    }
```

Doc addition: "Returns the drop anchors it released (`Folded::anchors`)." Import `Folded` and `Anchor` in `groups.rs`. The `fold_groups` closure borrows `self` mutably inside `map`; if the borrow checker refuses, write it as a `for` loop pushing into a `Vec<Folded>`.

`graph_nav.rs`:

```rust
    /// Where each card of the drag released last stood in the document before `release_drag` dropped it (pos and
    /// anchor), for the drop's undo (`apply_drop` takes it; a drop with no effect discards it).
    pub(crate) drag_origin: Vec<(CardId, Option<egui::Vec2>, Option<Anchor>)>,
```

`release_drag` push: `self.interaction.drag_origin.push((moved_id, self.doc.mirror.pos(key), self.doc.mirror.anchor(key)));`

```rust
    /// Every shown card's group, parent and anchor, for `moved_since`.
    pub(crate) fn membership_snapshot(&self) -> Vec<(NodeId, Option<NodeId>, Option<NodeId>, Option<Anchor>)> {
        self.nodes
            .keys()
            .map(|&w| (w, self.doc.mirror.group(w), self.doc.mirror.parent(w), self.doc.mirror.anchor(w)))
            .collect()
    }

    /// The cards whose group or parent differs from `before`, each with its before and after values. `origin` says
    /// where dragged cards started, pos and anchor (a card not in it did not move; its anchor before is the
    /// snapshot's, since the release that wrote a new one came first).
    pub(crate) fn moved_since(
        &self,
        before: Vec<(NodeId, Option<NodeId>, Option<NodeId>, Option<Anchor>)>,
        origin: &[(CardId, Option<egui::Vec2>, Option<Anchor>)],
    ) -> Vec<Moved> {
        before
            .into_iter()
            .filter_map(|(w, group, parent, anchor)| {
                let now_group = self.doc.mirror.group(w);
                let now_parent = self.doc.mirror.parent(w);
                (group != now_group || parent != now_parent).then(|| {
                    let (pos_now, anchor_now) = (self.doc.mirror.pos(w), self.doc.mirror.anchor(w));
                    let (pos_before, anchor_before) = origin
                        .iter()
                        .find(|(o, ..)| *o == CardId::from(w))
                        .map_or((pos_now, anchor), |&(_, p, a)| (p, a));
                    Moved {
                        id: w,
                        group: (group, now_group),
                        parent: (parent, now_parent),
                        pos: (pos_before, pos_now),
                        anchor: (anchor_before, anchor_now),
                    }
                })
            })
            .collect()
    }
```

Note the snapshot is taken in `apply_drop` after `release_drag` ran, so for a dragged card its snapshot anchor is the release's; the origin overrides it with the pre-drag one.

`search/arrange.rs:92-97`: the literal gains `anchor: (held, held),` where `let held = self.doc.mirror.anchor(id);` is read before the writes (Task 6 changes it to release). `suggest.rs:95` `self.moved_since(before, &[])` compiles unchanged.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold`
Expected: PASS (whole crate: `Moved` and `Fold` are shared by drops, folds, Merge, Arrange and Suggest).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/undo.rs crates/manifold/src/undo/tests.rs crates/manifold/src/graph_nav.rs crates/manifold/src/groups.rs crates/manifold/src/search/arrange.rs
git diff --cached --stat
git commit -m "Anchor register: undo restores anchors with drops and folds

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Tidy, and Arrange releases the drops it moves

**Files:**
- Modify: `crates/manifold/src/search/arrange.rs:47-105` (`arrange_by_topic`), new `tidy_level` and `level_has_drops`, tests `:161-226`
- Modify: `crates/manifold/src/graph_ui/toolbar.rs:237-243` (chip after Arrange)
- Modify: `crates/manifold/src/omnibar/rows.rs:57-62` (`TIDY`, `COMMANDS`), test `:~312`
- Modify: `crates/manifold/src/debug_control.rs:739-741` (row), `:1413` (samples)

**Interfaces:**
- Consumes: `clear_anchors`, `drop_at`, `Moved::anchor` (Tasks 2, 4, 5).
- Produces: `App::tidy_level(&mut self)`; `App::level_has_drops(&self) -> bool`; `pub(crate) static TIDY: Command`; `COMMANDS: [&Command; 3]`; headless `TIDY`.

- [ ] **Step 1: Write the failing tests**

`search/arrange.rs` `mod tests` (add `use crate::session_sync::tests::add_group;` and `use crate::store::model::Anchor;`):

```rust
    #[test]
    fn tidy_releases_drops_and_keeps_pins_in_one_undo_step() {
        let (mut app, cats, _) = spread_app();
        app.drop_at(cats[0]);
        app.toggle_pin(cats[1]);
        let (dropped, pos) = (app.doc.mirror.anchor(cats[0]), app.doc.mirror.pos(cats[0]));
        assert!(app.level_has_drops());
        app.tidy_level();
        assert_eq!(app.doc.mirror.anchor(cats[0]), None, "the drop is released");
        assert_eq!(app.doc.mirror.pos(cats[0]), pos, "and the card is not moved");
        assert!(app.doc.mirror.pinned(cats[1]), "a pin stays");
        assert!(!app.level_has_drops());
        let serial = app.toasts.undo.as_ref().map(|u| u.1);
        app.step_history(serial, false);
        assert_eq!(app.doc.mirror.anchor(cats[0]), dropped, "Undo holds it again");
        app.step_history(None, true);
        assert_eq!(app.doc.mirror.anchor(cats[0]), None);
    }

    #[test]
    fn tidy_with_nothing_to_release_records_nothing() {
        let (mut app, cats, _) = spread_app();
        app.toggle_pin(cats[1]);
        app.tidy_level();
        assert!(app.toasts.undo.is_none(), "no undo step");
        assert!(app.doc.mirror.pinned(cats[1]), "the pin is untouched");
    }

    /// Tidy releases an inert drop too (written in another group, left by a remote move), so moving the card back
    /// does not revive it.
    #[test]
    fn tidy_releases_an_inert_drop_so_moving_back_does_not_revive_it() {
        let mut app = App::default();
        let g = add_group(&mut app, "g");
        let note = add_note(&mut app, "n");
        app.edit_anchor(note, Some(Anchor { pos: [5.0, 5.0], group: Some(g), explicit: false }));
        assert_eq!(app.doc.mirror.held(note), None, "inert at the top level");
        app.tidy_level();
        assert_eq!(app.doc.mirror.anchor(note), None);
        app.doc.mirror.set_group(note, Some(g)); // a remote move back, mirror only
        assert_eq!(app.doc.mirror.held(note), None, "nothing to revive");
    }

    #[test]
    fn arrange_releases_the_drops_of_the_cards_it_moves_and_undo_restores_them() {
        let (mut app, cats, _) = spread_app();
        app.drop_at(cats[0]);
        let dropped = app.doc.mirror.anchor(cats[0]);
        app.arrange_by_topic();
        assert_eq!(app.doc.mirror.anchor(cats[0]), None, "else a later settle pulls it back to its drop point");
        let serial = app.toasts.undo.as_ref().map(|u| u.1);
        app.step_history(serial, false);
        assert_eq!(app.doc.mirror.anchor(cats[0]), dropped);
    }
```

`omnibar/rows.rs` test (`:~312`), after the `arrange` asserts:

```rust
        let tidy = |text| rows(&on, text, &[]).contains(&Row::Command(&TIDY));
        assert!(tidy("tidy") && tidy("TID") && !tidy("arr"));
```

`debug_control.rs` samples (`:1413`): add `"TIDY",` after `"ARRANGE",`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold tidy` then `cargo test -j 8 -p manifold arrange`
Expected: FAIL to compile ("no method named `tidy_level`", "cannot find value `TIDY`").

- [ ] **Step 3: Implement**

`search/arrange.rs`: import `crate::store::model::Anchor` if needed. In `arrange_by_topic`, replace the `Moved` push block:

```rust
            let after = positions[&id];
            let (group, parent) = (self.doc.mirror.group(id), self.doc.mirror.parent(id));
            // A drop's anchor would pull the card back to its drop point at the next settle; a pin never moves here.
            let released = self.clear_anchors(&[id], false).first().map(|&(_, a)| a);
            moved.push(Moved {
                id,
                group: (group, group),
                parent: (parent, parent),
                pos: (Some(self.layout_pos(id)), Some(after)),
                anchor: (released, None),
            });
```

and its doc: "A pinned card stays, and so does a group; a card it moves loses its drop anchor (one undo step)." Then add, after `arrange_by_topic`:

```rust
    /// Tidy (toolbar, omnibar, `TIDY`): Arrange without the move. Releases every drop anchor of the level's cards,
    /// inert ones included (so a stale one can't revive when a card is moved back), so the next settle moves dropped
    /// cards again; pins stay. One undo step; nothing to release records nothing.
    pub(crate) fn tidy_level(&mut self) {
        let mut level: Vec<NodeId> = self.nodes.keys().copied().filter(|&id| self.in_level(id)).collect();
        level.sort();
        let released = self.clear_anchors(&level, false);
        if released.is_empty() {
            return;
        }
        let message = match released.len() {
            1 => "Tidied 1 card".to_owned(),
            n => format!("Tidied {n} cards"),
        };
        let moved = released
            .into_iter()
            .map(|(id, anchor)| {
                let (group, parent, pos) = (self.doc.mirror.group(id), self.doc.mirror.parent(id), self.doc.mirror.pos(id));
                Moved { id, group: (group, group), parent: (parent, parent), pos: (pos, pos), anchor: (Some(anchor), None) }
            })
            .collect();
        self.record_undo(message, UndoKind::Revert { moved, created: None });
        self.save_session();
    }

    /// Whether any card at the level shown has a drop anchor (held or inert): when the toolbar offers Tidy.
    pub(crate) fn level_has_drops(&self) -> bool {
        self.nodes.keys().any(|&id| self.in_level(id) && self.doc.mirror.anchor(id).is_some_and(|a| !a.explicit))
    }
```

(`NodeId` must be `Ord` for `sort`; if it is not, drop the sort: order only affects the record's order.)

`graph_ui/toolbar.rs`, after the Arrange chip block:

```rust
            if self.level_has_drops() {
                let response = chip_icon_button(ui, egui::Id::new("icon_btn_tidy"), egui_phosphor::light::BROOM)
                    .named_tip("Tidy: let dropped cards move again; pins stay");
                grow(response.rect);
                if response.clicked() {
                    self.tidy_level();
                }
            }
```

`omnibar/rows.rs`:

```rust
pub(crate) static TIDY: Command = Command {
    title: "Tidy",
    phrases: &["tidy", "release dropped cards"],
    run: App::tidy_level,
};
pub(crate) static COMMANDS: [&Command; 3] = [&SUGGEST_GROUPS, &ARRANGE, &TIDY];
```

`debug_control.rs`, after the `ARRANGE` row:

```rust
    row("TIDY", "Toolbar's Tidy: release this level's drop anchors, pins stay (one undo step)", |_| {
        job(|app, _| app.tidy_level())
    }),
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j 8 -p manifold`
Expected: PASS (including `arrange_never_moves_a_pinned_card`, `arrange_writes_document_positions_and_one_undo_restores_them_all`, and the debug-control samples test).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/search/arrange.rs crates/manifold/src/graph_ui/toolbar.rs crates/manifold/src/omnibar/rows.rs crates/manifold/src/debug_control.rs
git commit -m "Anchor register: Tidy, and Arrange releases the drops it moves

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 7: The anchor reaches the peer (export key and gate s12)

**Files:**
- Modify: `crates/manifold/src/debug_export.rs:195-220` (`mirror_state`)
- Modify: `crates/gate/src/session.rs:98-178` (`Field`, `field_agree`), tests `:~277-306`
- Modify: `crates/gate/src/two/groups.rs:259-272` (s12)

**Interfaces:**
- Consumes: `Mirror::anchor` (Task 2); `drop_at` through `MOVENODE` (Task 4).
- Produces: `EXPORTSTATE` key `mirror_anchor`: id → `{"pos": [x, y], "group": <id or null>, "explicit": bool}` or null; gate `Field::Anchor`; `Field::doc_key(self) -> Option<&'static str>`; `Field::word(self) -> &'static str`.

- [ ] **Step 1: Write the failing tests**

`crates/gate/src/session.rs` `mod tests`, after `field_agree_pos_tolerance`:

```rust
    /// The document JSON drops anchors, so an anchor agrees when A and B show the same one, whatever the document.
    #[test]
    fn field_agree_anchor_compares_the_two_mirrors_only() {
        let f = Field::Anchor;
        let held = json!({"pos": [321.0, -123.0], "group": null, "explicit": false});
        let a = state(f, json!({"n": held}));
        let d = doc(json!([{"id": "n"}]));
        let lines = field_agree(f, &a, &a, &d, &["n"]).unwrap();
        assert!(lines[0].starts_with("n anchor: A shows"), "{lines:?}");
        let other = state(f, json!({"n": {"pos": [1.0, 2.0], "group": null, "explicit": false}}));
        assert!(field_agree(f, &a, &other, &d, &["n"]).is_err());
        assert!(field_agree(f, &a, &state(f, json!({})), &d, &["n"]).is_err(), "B shows none");
    }
```

`debug_export.rs`: if a test lists `export_state`'s keys, add `"mirror_anchor"` to it (grep `"mirror_pos"` in its tests first; none found while planning).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p gate field_agree`
Expected: FAIL to compile ("no variant named `Anchor`").

- [ ] **Step 3: Implement**

`debug_export.rs` `mirror_state`, after `mirror_pos`:

```rust
        let mirror_anchor: BTreeMap<NodeId, Option<Value>> = self
            .linked_ids()
            .map(|id| {
                let anchor = self.doc.mirror.anchor(id);
                (id, anchor.map(|a| json!({"pos": [a.pos[0], a.pos[1]], "group": a.group, "explicit": a.explicit})))
            })
            .collect();
```

add `"mirror_anchor": mirror_anchor,` to the `json!` map and extend the doc comment: "and every node's anchor (`{pos, group, explicit}`, null when none), which EXPORTSESSION never carries". Import `serde_json::Value` if the file does not already.

`gate/src/session.rs`:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Group,
    Name,
    Text,
    Pos,
    /// The drop or pin anchor: in both mirrors, never in the document JSON.
    Anchor,
}

impl Field {
    /// The EXPORTSTATE map.
    pub fn state_key(self) -> &'static str {
        match self {
            Field::Group => "mirror_groups",
            Field::Name => "mirror_names",
            Field::Text => "mirror_texts",
            Field::Pos => "mirror_pos",
            Field::Anchor => "mirror_anchor",
        }
    }
    /// The word in messages.
    pub fn word(self) -> &'static str {
        match self {
            Field::Group => "group",
            Field::Name => "name",
            Field::Text => "text",
            Field::Pos => "pos",
            Field::Anchor => "anchor",
        }
    }
    /// The EXPORTSESSION node key; `None` for a field the document JSON drops (the anchor), which then agrees when
    /// A and B do.
    pub fn doc_key(self) -> Option<&'static str> {
        (self != Field::Anchor).then(|| self.word())
    }
}
```

`field_agree`: build `d` only when there is a key, print `word()`, and accept a missing document value for a key-less field:

```rust
    let mut d: HashMap<&str, &Value> = HashMap::new();
    if let Some(key) = field.doc_key() {
        for n in nodes(doc) {
            if let Some(i) = id(n) {
                d.insert(i, n.get(key).unwrap_or(&null));
            }
        }
    }
    let mut ok = true;
    let mut lines = Vec::new();
    for i in ids {
        let seen = [ma.get(*i), mb.get(*i), d.get(i).copied()];
        let document = if field.doc_key().is_some() { show(seen[2]) } else { "not exported".to_string() };
        lines.push(format!("{i} {}: A shows {}, B shows {}, document {document}", field.word(), show(seen[0]), show(seen[1])));
        match seen {
            [Some(x), Some(y), Some(z)] if same_value(&[x, y, z]) => {}
            [Some(x), Some(y), None] if field.doc_key().is_none() && same_value(&[x, y]) => {}
            _ => ok = false,
        }
    }
```

Update `field_agree`'s doc: "...and in the document (a node without the key counts as null there; a field with no document key, the anchor, is compared between A and B only)". Grep `doc_key()` in `crates/gate` for any other caller and switch it to `word()`.

`two/groups.rs` s12, after the "the note is where A dropped it" observe:

```rust
        let (a, _) = t.wait_field(Field::Anchor, "scenario 12: A and B agree on the anchor after A's move", "s12a", &[&n])?;
        let group = session::field_of(&a, Field::Group, &n).unwrap_or(Value::Null);
        let anchor = session::field_of(&a, Field::Anchor, &n);
        observe(
            "scenario 12: the note is held where A dropped it, as a drop, not a pin",
            anchor == Some(json!({"pos": [321.0, -123.0], "group": group, "explicit": false})),
            &format!("{n} is anchored at {}", shown(&anchor)),
        );
```

(`Value` comes from `serde_json`; add it to the file's imports if missing.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j 8 -p gate` then `cargo test -j 8 -p manifold debug_export`
Expected: PASS. The s12 scenario itself runs only in `gate two` (needs a build and two headless instances): leave it to the gate run in Task 8 Step 4.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/debug_export.rs crates/gate/src/session.rs crates/gate/src/two/groups.rs
git commit -m "Anchor register: mirror_anchor export and gate two s12 checks the peer

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Docs, and the slice's closing checks

**Files:**
- Modify: `README.md` (`:146`, `:233`-`:243` toolbar wording, `:409`, `:450`, `:453`, a new paragraph after `:409`)
- Modify: `docs/superpowers/specs/2026-10-04-node-model-design.md:43`, `:121`, `:187`, `:262`
- Modify: `PARKED.md:47`, `:50`, `:52`
- Modify: `docs/superpowers/specs/2026-10-19-move-undo-design.md` (only if it names `Moved`'s fields)
- Modify: this plan's front matter

**Interfaces:**
- Consumes: everything above.
- Produces: no code.

- [ ] **Step 1: README.** Grep `git grep -n -i "pinn\|anchor\|Tidy\|MOVENODE\|mirror_pos" README.md` and handle each hit; dated history entries (`:136`, `:156`, `:187`, `:189`, `:235`, `:238`, `:280`, `:288`, `:516`, `:719`, `:784`, `:791`, `:792`, `:805`) record what was true then and stay unchanged unless they state current behaviour in the present tense. Make these edits:
  - `:146` (Graph view overview): replace the clause "a pinned node is excluded from the layout entirely ... when one runs" with "a pinned node, and a node dropped after a drag, is held where it was put (an anchor: `field::ANCHOR`), so a settle never moves it, though it still pushes its neighbours; a drop's hold ends with Tidy, Arrange by topic or a move to another group, a pin's only with Unpin".
  - After `:409`, a new paragraph:
    `**Drop and pin anchors (2026-11-01, spec docs/superpowers/specs/2026-11-01-scopes-nested-layout-anchor-design.md, Slice 1).** One stored register, \`ANCHOR\` (field 27: point, group, explicit bit), replaces the \`pinned\` bool (field 7, now reserved; an older session's pins are ignored). Pin writes an explicit anchor where the card is drawn; a drag release writes an implicit one where it lands. A card is held when its anchor is explicit, or implicit and written in the group it is in now (\`Mirror::held\`); the settle seeds a held card from its anchor point and keeps it there, and the settle key includes the held set, so a drop re-settles the neighbours around it. Unpin releases a pin only; Tidy (toolbar broom while a drop is held, omnibar "Tidy", \`TIDY\`) releases every drop at the level, Arrange by topic the drops of the cards it moves, and a local move the card's drop; Tidy, Arrange, moves and folds are undoable with their anchors. Only the explicit bit counts as "pinned" (pin mark, menus, Pin All, idle-sleep exemption). JSON export drops anchors, so a copied or duplicated pinned card arrives unpinned.`
  - `:409`: "Pinned cards and groups stay." → "Pinned cards and groups stay; a card it moves loses its drop anchor."
  - `:450` (`MOVENODE`): "its document pos is written as a drag release writes it" → "its document pos and drop anchor are written as a drag release writes them"; append "`mirror_anchor` maps every node to its anchor (`{pos, group, explicit}`) or null; the document JSON never carries it."
  - `:453`: after the `ARRANGE` sentence add "`TIDY` — the toolbar's Tidy: releases this level's drop anchors, pins stay (one undo step)."
  - The toolbar icon list (`:233`-`:243`): add the Tidy chip (`BROOM`, shown while the level has a drop) beside Arrange by topic.

- [ ] **Step 2: Other docs.**
  - Node-model spec `:43`, `:121`, `:187`, `:262`: append to each mention of `pinned` "(*replaced 2026-11-01 by the \`anchor\` register, field 27; see \`2026-11-01-scopes-nested-layout-anchor-design.md\` Slice 1*)" once per line.
  - `PARKED.md:50` ("One hard anchor register replaces `PINNED`"): delete the entry (fixed; its rationale lives in the spec, VERSIONING "delete it when fixed"); add one `decided against`-style line only if a part was not built (none expected).
  - `PARKED.md:47`: "drag and Pin retire into the hard anchor (the entry below)" → "drag and Pin retired into the hard anchor (landed 2026-11-01, spec Slice 1)".
  - `PARKED.md:52` trigger: "the anchor register and per-level view state have landed" → "per-level view state has landed (the anchor register landed 2026-11-01)".
  - `PARKED.md:91` (pin dot): still true (the pin mark reads `pinned`); leave.
  - `git grep -n "pos: (" docs/superpowers/specs/2026-10-19-move-undo-design.md`: if it lists `Moved`'s fields, add `anchor`.
  - `git grep -n -i "anchor\|pinned" ROADMAP.md`: nothing found while planning; record the result in the closing report.
  - CHANGELOG: no edit (VERSIONING: entries are written at the bump). The `**Breaking:**` line lives in the spec's "Behaviour changes"; name it in this commit's message for the next bump.
  - This plan's front matter: `status: done`.

- [ ] **Step 3: Commit**

```bash
git add README.md PARKED.md docs/superpowers/specs/2026-10-04-node-model-design.md docs/superpowers/plans/2026-11-01-anchor-register.md
git add docs/superpowers/specs/2026-10-19-move-undo-design.md   # only if changed
git commit -m "Docs: the anchor register (Slice 1) landed

For the next release's changelog: **Breaking:** old sessions' pins (field 7) are ignored, so previously
pinned cards are unpinned (spec 2026-11-01 Behaviour changes).

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 4: Closing checks (on request only).** `cargo clippy` already ran in each commit's hook. The pre-merge gate is `full` (store, mirror and session-sync changes); run it only when the user asks, through `~/.claude/runs/run.ps1 -Kind gate -Detach` as CLAUDE.md "Gate" says. Expect, and report rather than fix silently: `gate two` s12's new anchor check; `layout` (MOVENODE-parked cards now stay held); `shots` graph views after a mouse drag (`shots.rs:862`, `:928`, `:1055`, `:1112`) may differ, because a dropped card no longer drifts and the Tidy chip appears; re-take those baselines only with the user's go-ahead.

---

## Architecture check

- Grew: Tidy is a fourth action on the Arrange surfaces (toolbar, omnibar, headless), as the spec accepted; if the chip row grows again, fold Tidy into Arrange as a "keep placement" option. Gate `Field::Anchor` is the one field with no document key (`doc_key` is `None`).
- Grew: two undo shapes now carry anchors: `Moved::anchor` and `Folded::anchors`. The unifying change is to record a fold's members as `Moved` entries (group, parent, pos and anchor pairs), so `replay_fold` reuses `replay_revert`'s per-card replay and `regroup_held`'s put-back rule; it subsumes `Folded::anchors` and the anchor-restore loop in `replay_fold`. Not done here: `regroup_held` also serves Recently Closed reopens with no undo record. Worth a PARKED entry if the user agrees.
- Removed special cases: `edit_group` is the one place a move releases a drop (subsumes per-site clearing in Move, Join, GroupWith, Eject, fold, put-back, `MOVETOGROUP` and replay); `clear_anchors` serves Unpin, Tidy, Arrange and fold; `drop_at` serves the drag release and `MOVENODE`; `held` is the one hold predicate (settle, settle key, remote snap, Pin's "already held" check).

## Self-review (rounds run while planning)

- Round 1 (spec coverage): every Slice 1 bullet maps to a task (Encoding: 1; No bump: 1; Meaning: 2; Writers: 2, 4, 6; Readers: 3; Export: 1, 2; Undo: 5; Respawn: 2; Deleted: 2) and every Slice 1 test line in the spec's Tests section has a test (the idle-sleep one is checked through `pinned`, since a live tab can't be built in a unit test). Caught: fold undo is `UndoKind::Fold`, not `Moved` (R3); the JSON import would error on an `"anchor"` key (R5).
- Round 2 (compile order): `PINNED` cannot be deleted before its readers change, so Task 1 adds `ANCHOR` alongside it (20 `FIELDS` for one task) and Task 2 does the whole switch, Pin and respawn included. Caught: `Moved` literals in `arrange.rs` must change in Task 5, not 6; `debug_move_node`'s existing test needs no change to pass, only an added assert.
- Round 3 (behaviour): caught that `a_drag_release_saves_the_dropped_position` inverts (R6); that Unpin All on a drop-anchored, then pinned, card would clear the drop (the test uses two tabs instead); that `replay_revert` must write the anchor after `edit_group`, which releases it; that `moved_since`'s snapshot runs after the release, so the origin must carry the pre-drag anchor; that `shots` baselines may move. A final pass found no further material issue.
