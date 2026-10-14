# Unknown-Type Card Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A node whose type this build does not know becomes a plain live card. It has the generic fields only (pos, color, name, opened-from, group), it links and sits in groups, it closes, reopens and duplicates as a shell of its type, and its other fields stay raw in the document.

**Architecture:**
- Every node carries its stored type string end to end: `Node::ty`, the mirror's `TYPE`, `Snapshot.ty`, `Imported.ty`, `OpBody::create(.., &str, ..)` and `node_json(.., &str, ..)`. `Kind` is read from that string where a branch needs it.
- `Kind::Unknown`'s field list becomes the generic list, so every path that reads, mirrors, snapshots or copies "the kind's fields" handles it without a special case.
- One function, `card_kind::canonical_type`, maps a raw type to its row string, with a reserved sentinel row `TYPE_UNKNOWN` for unknown types. Every caps and edge lookup goes through it.
- `Runtime::Unknown` and a small header-only card paint it.

**Tech Stack:** Rust 2021, egui, the manifold store (`store::model`, `store::ops`, `store::json`, `session_sync`, `mirror`), the caps tables (`caps::rows`, `caps::edges`) and the `gate` crate (headless checks).

**Spec:** `docs/superpowers/specs/2026-10-14-unknown-type-card-design.md`

## Global Constraints

- **Branch and commits:** Work on `main`, one commit per task. Every commit message ends with the line `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- **Every task leaves the tree green:**
  - `cargo build -j 8` succeeds.
  - `cargo fmt --all` has been run, and `cargo fmt --all --check` is clean.
  - `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` passes.
- **Hooks and lints:**
  - Never `--no-verify`. The pre-commit hook runs fmt and clippy; if it fails, fix the cause.
  - Silence no lint without `#[allow(..., reason = "...")]`.
- **No shims and no migration** (memory `no-migration-until-users`; spec decision 10). The `*/*` row, `UNKNOWN_TYPE` and the stale "no unknown-type card yet" / "no menu of its own yet" comments are deleted, not kept beside the replacement. `CLOSEGROUP` is replaced by `CLOSENODE`, with no alias.
- **Replace, don't layer.** Each task deletes what it supersedes, with its tests and docs, in the same commit.
- **Profile isolation:** no test reads or writes the real `%APPDATA%\manifold`. Files go under `store::testutil::temp_dir(..)`.
- **Tests:** each piece of non-trivial logic gets one unit test. Run unit tests as `cargo test -j 8 -p manifold <filter>`.
- **No python.** Never run `python` that reads stdin (no `python -`, no heredoc into python). This plan needs no python at all.
- **Reserved names:**
  - Sentinel row string: `TYPE_UNKNOWN = "application/vnd.manifold.unknown"`. It is never stored; only `canonical_type` returns it.
  - Generic field list: `[POS, COLOR, PARENT, NAME]`, plus the group, which is always carried apart from the fields.
  - The new debug verb is `CLOSENODE id`.
  - The new gate stage is `gate unknown-type`, in `crates/gate/src/unknown_type.rs`.
- **Line numbers** are as of commit `8b90691` and were checked against the files. Earlier tasks shift later line numbers, so search for the quoted text, not the number.

## Deviations from the spec's wording (same behaviour, fewer concepts)

- **D1. `Kind::fields()` returns the generic list for `Unknown`.** The spec has a separate `Kind::generic_fields()`.
  - `Unknown` is not in `TYPES`, so `fields()` falls back to `GENERIC = [POS, COLOR, PARENT, NAME]` instead of `&[]`.
  - `fields_of`, `snapshot`, `imported_fields`, `create_ops`, `import` and the save already read `kind.fields()`, so all of them handle an unknown node with no branch (spec decision 3 and "Model").
  - Nothing else reads `fields()` for `Unknown`: before this plan it was empty and only typeless or unknown nodes had that kind.
- **D2. The type string travels as `ty: String` beside `Kind`.**
  - `Snapshot` and `Imported` replace their `kind: Kind` with `ty: String` plus a `kind()` method.
  - `Snapshot::of(doc, id, node)` is the one constructor from a document node. It replaces the four hand-built tuples and literals.
  - `RemoteChange` becomes `(NodeId, Option<Snapshot>)`; `RemoteNode` becomes `(String, Fields, Option<String>)`.
  - `OpBody::create`, `node_json` and `materialize` take `&str`.
  - `Node::ty()` is the store-side accessor the spec says is missing.
- **D3. `place_node` keeps `kind: Kind`** (spec decision 5 lists it). `materialize` seeds the mirror's `TYPE` from the stored string before it calls `insert_materialized`, and `Mirror::seed` never overwrites a `TYPE` already present (`mirror.rs:322-325`). `place_node` passes "" for `Unknown`, which `seed` skips. No caller makes a fresh unknown node.
- **D4. `named_by_name` stays as it is.** The spec (decision 6) adds `Runtime::Unknown` to it. That would make the unknown card's name its only label, so an empty rename would be refused and `accessible_name` would ignore the type. Leaving it out treats the card like every non-group card: a stored `NAME` overrides the label, and an empty one clears it (`commit_rename`, `node_menu.rs:960-972`).
  - The title bar's stored-name rule (`card.rs:2059-2062`) shows the name.
  - The filter matches it through the line that pushes the stored name (`card.rs:1274`).
  - `App::accessible_name` (`node.rs:435-444`) reads "Card: <name>".
  - Parked: matching the filter on the type string.
- **D5. `import` keeps, for every kind, only `kind.fields()` plus the group.**
  - For an unknown type this gives exactly the generic fields (spec decision 4, "Import"). For a known kind it drops a field its `Create` would never write anyway.
  - A missing, non-string or empty `type` is an error. An unrecognised one is kept.
  - If an existing test or gate seed turns out to depend on an off-list field surviving import, the implementer stops and reports it.
- **D6. EXPORTSTATE keeps `type` as the canonical row string and adds `stored_type`, the raw string.** Existing gate checks compare `type` against `state::TAB` and the like, and an `image/jpeg` node must keep reading as `image/png` there. The spec ("Model") asks that EXPORTSTATE report the real string; `stored_type` does that.
- **D7. `CLOSENODE id` replaces `CLOSEGROUP id`.** It calls `close_node_and_save` on any node. A group behaves as before, since its destroy already saves. The gate stage needs to close an unknown card by id.
- **D8. An empty `type` string counts as no type.**
  - `Node::is_live` becomes `!deleted && !ty().is_empty()`. The spec says `has_type()`; "" is not a type.
  - `snapshot` returns `None` for "".
  - `canonical_type("")` is "", which matches no row, so `worth_reopening("")` stays false.

## Leftover-reference check

Each task's check step greps for what it deletes, over all of `crates/` (tests included), and expects no hits:

```bash
git grep -nE "<names>" -- crates
```

Each task lists its `<names>`. A hit is fixed before the commit, never left for a later task. After Task 4 the whole set must be empty:

```bash
git grep -nE 'UNKNOWN_TYPE|no unknown-type card yet|no menu of its own yet|CLOSEGROUP|CloseGroup|kept, never shown|node\.kind, |root\.kind|member\.kind|snap\.kind' -- crates
git grep -n '"\*/\*"' -- crates/manifold/src/caps/rows.rs crates/manifold/src/card_kind.rs
```

The second command may only hit `rows.rs`'s own test tables (`mod tests`), never `builtin_rows`.

## Task order and batching

1. **The type string travels with the node.** Plumbing only, with no visible change. Run alone, by one Sonnet implementer.
2. **One canonical type: the sentinel row and edges.** Caps, edges, `card_kind`, `worth_reopening`, `connect`.
3. **The unknown card is live.** Liveness, `effective_group`, `Runtime::Unknown`, the card, the Recently Closed row, comments, and the sync and copy tests.
4. **`CLOSENODE`, `stored_type`, the gate stage, docs and the full check.**

**Batching:**
- Tasks 2 and 3 go to one Sonnet implementer. Task 2 is ~60 lines and only Task 3 makes its rows reachable from a live card.
- Tasks 1 and 4 each go to their own Sonnet implementer. Each has open details (exact borrow shapes in `restore_session`; the gate stage's waits), so none is Haiku-shaped.
- Review once at the end: a Sonnet reviewer for Tasks 1 to 3, and Opus for the final whole-branch review. The branch exceeds ~300 changed lines.

The closing gate run comes after Task 4 and belongs to the controller (see "Closing step").

---

### Task 1: The type string travels with the node

No behaviour changes in this task. An unknown node is still not live (`Node::is_live` unchanged), and `snapshot` still returns `None` for one. Only the string is carried instead of being rebuilt from `Kind`.

**Files:**
- Modify `crates/manifold/src/store/model.rs`:
  - Kind doc at 139-140.
  - `TYPES` doc at 144-148.
  - `fields()` at 171-175.
  - `kind()` at 432-442.
  - Tests at 650-694.
- Modify `crates/manifold/src/store/ops.rs`:
  - Imports at 7-9.
  - `create` at 32-38.
  - Test helper at 204-206; new tests after 232.
- Modify `crates/manifold/src/store/json.rs`:
  - `node_json` at 62-81 and `node_to_json` at 84-92.
  - `Imported` at 146-152; the `Trust` doc at 154-166.
  - `import` at 168-237.
  - `is_half_made_link`, `create_op` and `creates` at 239-257.
  - Tests at 386-430 and 438.
- Modify `crates/manifold/src/session_sync.rs`:
  - `Snapshot` at 59-72.
  - `create_ops` at 167.
  - `RemoteChange` at 193-195; `RemoteNode` at 205-206.
  - `spawn_snapshot` and `tab_fields` at 275-306.
  - `snapshot` at 378-411.
  - `materialize` at 468-511.
  - `remote_changes` and `apply_remote` at 725-756.
  - `land_remote_in_flight` at 830-836.
  - `restore_session` at 1461-1508.
  - `materialize_imported` at 1924-2029 (lines 1935, 1945, 1949, 1968, 1976, 2021, 2029).
- Modify `crates/manifold/src/closed.rs`: lines 164, 182, 200 and 303.
- Test `crates/manifold/src/session_sync/tests.rs`:
  - Line 18.
  - Helpers at 65-87 and 495-500.
  - Lines 538-539, 589, 599, 711, 1504, 2054, 2233, 2289, 2839 and 4907.
- Test `crates/manifold/src/store/journal/tests.rs`: lines 8, 157, 325, 472, 474, 488, 762, 1373 and 1384.
- Test `crates/manifold/src/view_install/tests.rs:999` and `crates/manifold/src/json_level/tests.rs:205`.

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `store::model::GENERIC: &[u32]`: `[field::POS, field::COLOR, field::PARENT, field::NAME]`, `pub(crate)`.
  - `Kind::fields(self) -> &'static [u32]` returns `GENERIC` for `Kind::Unknown`.
  - `Node::ty(&self) -> &str`: the stored `type` string, "" before the `Create` arrives.
  - `OpBody::create(node: NodeId, ty: &str, fields: Vec<u8>) -> OpBody`.
  - `store::json::node_json(id: &str, ty: &str, fields: &BTreeMap<u32, FieldValue>, group: Option<NodeId>) -> JsonNode`.
  - `store::json::Imported { pub id: NodeId, pub ty: String, pub fields: BTreeMap<u32, FieldValue> }` with `pub fn kind(&self) -> Kind`.
  - `session_sync::Snapshot { pub ty: String, pub group: Option<NodeId>, pub fields: Fields }`, with `pub(crate) fn of(doc: &Doc, id: NodeId, node: &Node) -> Snapshot` and `pub(crate) fn kind(&self) -> Kind`.
  - `session_sync::RemoteChange = (NodeId, Option<Snapshot>)`.
  - `session_sync::RemoteNode = (String, Fields, Option<String>)`.
  - `App::materialize(&mut self, id: String, ty: &str, fields: &Fields, group: Option<String>, local: Option<&LocalNode>)`.

- [ ] **Step 1: Write the failing store tests**

In `crates/manifold/src/store/model.rs` `mod tests`, add after `a_node_reads_its_kind_from_the_last_type_write` (ends at line 694):

```rust
    /// A type this build doesn't know carries the generic fields (spec decision 3): what the app reads, mirrors and
    /// copies of it. Its type string is kept as written.
    #[test]
    fn an_unknown_kind_carries_the_generic_fields() {
        use field::*;
        assert_eq!(Kind::Unknown.fields(), &[POS, COLOR, PARENT, NAME]);
        assert_eq!(Node::with_type("application/x-from-the-future").ty(), "application/x-from-the-future");
        assert_eq!(Node::with_type("Text/Markdown").ty(), "Text/Markdown", "kept as written, case and all");
        assert_eq!(Node::placeholder().ty(), "");
    }
```

In `crates/manifold/src/store/ops.rs` `mod tests`, add after `prune_drops_a_typeless_placeholder_but_keeps_a_type_this_build_does_not_know` (ends at line 232):

```rust
    #[test]
    fn a_create_writes_the_type_string_it_is_given() {
        let t = id(1, 0);
        let mut doc = Doc::new(DocId([0; 16]));
        doc.apply(&op(1, 0, 1, OpBody::create(t, "application/x-from-the-future", Vec::new())));
        assert_eq!(doc.nodes[&t].ty(), "application/x-from-the-future");
        assert_eq!(doc.nodes[&t].kind(), Kind::Unknown);
    }

    /// A `Set` of a generic field on a node of an unknown type leaves a newer client's field byte-identical (spec
    /// "Testing", 2b-5 Ruling 2).
    #[test]
    fn a_set_on_an_unknown_node_leaves_its_raw_fields_alone() {
        let t = id(1, 0);
        let mut fields = encode_field(40, &FieldValue::Str("theirs".into()));
        fields.extend(encode_field(field::GROUP, &FieldValue::Id(None)));
        let mut doc = Doc::new(DocId([0; 16]));
        doc.apply(&op(1, 0, 1, OpBody::create(t, "application/x-from-the-future", fields)));
        let raw = doc.nodes[&t].fields[&40].enc.clone();
        doc.apply(&op(1, 1, 2, name(t, "renamed")));
        let pos = encode_field(field::POS, &FieldValue::Pos(5.0, 6.0));
        doc.apply(&op(1, 2, 3, OpBody::Set { node: t, field: field::POS, enc: pos }));
        assert_eq!(doc.nodes[&t].fields[&40].enc, raw);
        assert_eq!(doc.nodes[&t].get(field::NAME), FieldValue::Str("renamed".into()));
        assert_eq!(doc.nodes[&t].get(field::POS), FieldValue::Pos(5.0, 6.0));
    }
```

In `crates/manifold/src/store/json.rs` `mod tests`:

- Rename `import_rejects_a_missing_or_unknown_type` (lines 425-430) to `import_rejects_a_missing_or_empty_type`, with this body:

```rust
    #[test]
    fn import_rejects_a_missing_or_empty_type() {
        let bad = |json: &str| import(&serde_json::from_str(json).unwrap(), ids(1), Trust::Own).unwrap_err();
        bad(r#"{"nodes": [{"id": "a"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "type": 7}]}"#);
        bad(r#"{"nodes": [{"id": "a", "type": ""}]}"#);
    }

    /// A type this build doesn't know is kept with its string and the generic fields only: a newer build's field,
    /// an `Id` among them, can't be remapped, so it is never copied (spec decision 4). A known kind keeps only its own
    /// fields too.
    #[test]
    fn import_keeps_an_unknown_type_with_the_generic_fields_only() {
        let json = doc(serde_json::json!([
            {"id": "u", "type": "application/x-from-the-future", "name": "Old", "pos": [1, 2], "color": "#ff0000",
             "url": "https://x/", "jar": "g", "text": "dropped", "future_field": 7, "group": "g"},
            {"id": "g", "type": "application/vnd.manifold.group", "name": "G", "parent": "u"},
        ]));
        let imported = import(&json, ids(1), Trust::Own).unwrap();
        assert_eq!(imported[0].ty, "application/x-from-the-future");
        assert_eq!(imported[0].kind(), Kind::Unknown);
        let kept: Vec<u32> = imported[0].fields.keys().copied().collect();
        assert_eq!(kept, [field::POS, field::COLOR, field::GROUP, field::NAME]);
        assert_eq!(imported[0].fields[&field::GROUP], FieldValue::Id(Some(imported[1].id)));
        assert!(!imported[1].fields.contains_key(&field::PARENT), "a group has no `parent`");
        let made = creates(&imported);
        let mut d = Doc::new(DocId([0; 16]));
        let mut op_ids = ids(3);
        for (i, body) in made.into_iter().enumerate() {
            d.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
        }
        assert_eq!(d.nodes[&imported[0].id].ty(), "application/x-from-the-future", "creates() writes the string back");
        let uid = imported[0].id;
        let out = node_to_json(uid, &d.nodes[&uid], None);
        assert_eq!(out.fields["type"], "application/x-from-the-future", "export writes the real string");
    }
```

`doc(..)` is the test helper at line 457. The export assertion calls `node_to_json(id, &Node, group)` (private, reachable from `mod tests`) directly, because `doc_to_json` and `doc_to_json_all` both list `doc.live()` nodes and an unknown node is not live until Task 3. Task 3 may add a `doc_to_json` assertion.

- In `bad_input_is_an_error` (386-395), delete the line-389 case `bad(r#"{"nodes": [{"id": "a", "type": "application/x-spaceship"}]}"#);`. An unrecognised type is no longer an error.
- In `export_writes_type_as_a_field_and_import_reads_it_back`:
  - Line 413 becomes `assert_eq!(imported.iter().map(Imported::kind).collect::<Vec<_>>(), [Kind::Group, Kind::Tab, Kind::Tab]);`.
  - Line 414's message becomes `"the type is carried separately"`.
- Line 438 becomes `let json = node_json("n1", crate::store::model::TYPE_TAB, &fields, None);`.

In `crates/manifold/src/session_sync/tests.rs`:

- Line 18 becomes `assert_eq!(Kind::Unknown.fields(), &[POS, COLOR, PARENT, NAME]);`.
- Add this test after `a_remote_view_change_updates_the_node_in_place` (ends at line 603):

```rust
/// `materialize` seeds the mirror's `type` with the stored string as written, and a snapshot carries it back to the
/// store unchanged; the row it reads as is the canonical one.
#[test]
fn materialize_keeps_the_stored_type_string() {
    let mut app = App::default();
    let node = id(1).to_string();
    let fields = shared(Kind::Note, None, &[(field::TEXT, FieldValue::Str("n".into()))]).fields;
    app.materialize(node.clone(), "Text/Markdown", &fields, None, None);
    assert_eq!(app.mirror.ty(&node), "Text/Markdown");
    assert_eq!(app.card_kind(&node).unwrap().ty, crate::store::model::TYPE_NOTE);
    assert_eq!(app.snapshot(id(1)).unwrap().ty, "Text/Markdown");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold --no-run`
Expected: compile errors. `no method named 'ty'` on `Node` and `Imported`; `OpBody::create` and `node_json` expect `Kind`, found `&str`; `materialize` expects `Kind`.

- [ ] **Step 3: The store: `GENERIC`, `Node::ty`, `OpBody::create(&str)`**

`crates/manifold/src/store/model.rs`:

- Replace the `Unknown` doc (lines 139-140):

```rust
    /// No `type` yet (an op named the node before its `Create` arrived), or a type a newer build wrote. The node
    /// keeps its type string (`Node::ty`); the app reads only its generic fields (`GENERIC`).
    Unknown,
```

- Below `TYPES` (after line 163) add:

```rust
/// The fields every card carries whatever its type: all the app reads, mirrors and copies of a node of a type this
/// build doesn't know (`Kind::Unknown`, spec decision 3). Its other fields stay raw in the document.
pub(crate) const GENERIC: &[u32] = &[field::POS, field::COLOR, field::PARENT, field::NAME];
```

- Replace `fields()` (lines 171-175):

```rust
    /// The fields a node of this kind carries, `group` aside (`TYPES`): what a `Create` writes, what another
    /// replica's change hands the app (2b-5 Ruling 11), and what a snapshot of the node holds. `GENERIC` for
    /// `Unknown`.
    pub fn fields(self) -> &'static [u32] {
        TYPES.iter().find(|t| t.0 == self).map_or(GENERIC, |t| t.2)
    }
```

- Replace `kind()` (lines 432-442) with:

```rust
    /// Its stored `type` string as written ("" before its `Create` arrives): the last occurrence in the register,
    /// as `decode_field` reads it.
    pub fn ty(&self) -> &str {
        let Some(reg) = self.fields.get(&field::TYPE) else { return "" };
        let mut ty = "";
        for f in Reader::new(&reg.enc).map_while(Result::ok) {
            if let Ok(s) = f.value.str() {
                ty = s;
            }
        }
        ty
    }

    /// What kind of node this is, read from its `type`.
    pub fn kind(&self) -> Kind {
        Kind::from_type(self.ty())
    }
```

`crates/manifold/src/store/ops.rs`:

- Replace `create` (lines 32-38):

```rust
impl OpBody {
    /// A `Create` whose node message carries the type string `ty` after `fields`. `ty` is the node's own string
    /// (`Node::ty`, `Snapshot::ty`), a type this build doesn't know included.
    pub fn create(node: NodeId, ty: &str, mut fields: Vec<u8>) -> OpBody {
        debug_assert!(!ty.is_empty(), "a created node has a type");
        fields.extend(encode_field(field::TYPE, &FieldValue::Str(ty.to_owned())));
        OpBody::Create { node, fields }
    }
}
```

- Drop `Kind` from the `use super::model::{..}` list at 7-9 (no other non-test use). In `mod tests`, add `use crate::store::model::Kind;` beside line 192.
- The test helper at 204-206 becomes:

```rust
    fn create(node: NodeId, kind: Kind, group: Option<NodeId>) -> OpBody {
        OpBody::create(node, kind.type_str().unwrap(), encode_field(field::GROUP, &FieldValue::Id(group)))
    }
```

`crates/manifold/src/store/journal/tests.rs`: each `OpBody::create(x, Kind::K, f)` at lines 8, 157, 325, 472, 474, 488, 762, 1373 and 1384 becomes `OpBody::create(x, Kind::K.type_str().unwrap(), f)`.

`crates/manifold/src/session_sync/tests.rs:1504`: `OpBody::create(node, Kind::Download, fields)` becomes `OpBody::create(node, crate::store::model::TYPE_DOWNLOAD, fields)`.

- [ ] **Step 4: JSON: `node_json(&str)`, `Imported.ty`, `import` keeps the string**

`crates/manifold/src/store/json.rs`:

- `node_json` (lines 62-81): the doc's "`type` from `kind`" becomes "`type` from `ty`", the parameter `kind: Kind` becomes `ty: &str`, and line 79 becomes `out.insert("type".into(), Value::String(ty.to_owned()));`.
- `node_to_json` line 91 becomes `node_json(&id.to_string(), node.ty(), &fields, group)`.
- Replace `Imported` (146-152):

```rust
/// A node read from JSON, with its fresh id, its type string as the file wrote it and its field values.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub id: NodeId,
    pub ty: String,
    pub fields: BTreeMap<u32, FieldValue>,
}

impl Imported {
    /// The kind its type string means (`Kind::from_type`).
    pub fn kind(&self) -> Kind {
        Kind::from_type(&self.ty)
    }
}
```

- In the `Trust::Own` doc (157-159), replace "every field is kept." with "every field its kind carries is kept.".
- In `import`'s doc (168-178), replace "Unknown field names are ignored (a newer build's export); a bad value is an error." with:

  "A node keeps its `type` string, one this build doesn't know included, and only the fields its kind carries (`Kind::fields`, the generic ones for an unknown type) plus its group; unknown field names are ignored (a newer build's export). A missing or empty `type`, or a bad value, is an error."

- In `import`, replace line 189 and the start of the field loop (lines 189-193) with:

```rust
        let ty = node
            .fields
            .get("type")
            .and_then(Value::as_str)
            .filter(|ty| !ty.is_empty())
            .ok_or_else(|| format!("node {:?}: missing type", node.id))?;
        let kind = Kind::from_type(ty);
        let mut fields = BTreeMap::new();
        for (name, value) in &node.fields {
            // The type is carried separately; `creates` writes it.
            let Some(def) = field_by_name(name).filter(|def| def.no != field::TYPE) else { continue };
            // Only what its kind carries, and its group: a newer build's field on an unknown type can't be remapped,
            // so it is never copied (spec decision 4).
            if def.no != field::GROUP && !kind.fields().contains(&def.no) {
                continue;
            }
```

- Line 234 becomes `out.push(Imported { id: fresh[node.id.as_str()], ty: ty.to_owned(), fields });`.
- `is_half_made_link` line 243: `node.kind == Kind::Link` becomes `node.kind() == Kind::Link`.
- `create_op` line 248 becomes `OpBody::create(node.id, &node.ty, fields)`.
- `creates` line 255: `n.kind != Kind::Link` becomes `n.kind() != Kind::Link`.

- [ ] **Step 5: Sync: `Snapshot.ty`, `materialize(&str)` and the tuples**

`crates/manifold/src/session_sync.rs`:

- Replace `Snapshot` and its doc (lines 59-67):

```rust
/// A node as the store holds it, or is about to (`App::snapshot`): its type string as stored, its group and its
/// fields of that type's kind (`Kind::fields`, every one present). A `Create` writes one, Recently Closed records
/// one, and a tab's spawn starts from one (`App::spawning`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Snapshot {
    /// The stored `type` (`Node::ty`, `Mirror::ty`), kept as written: a type this build doesn't know included.
    pub ty: String,
    pub group: Option<NodeId>,
    pub fields: Fields,
}

impl Snapshot {
    /// What the document shows for its node `id`: its type, its effective group and its fields (`fields_of`).
    pub(crate) fn of(doc: &Doc, id: NodeId, node: &Node) -> Snapshot {
        Snapshot { ty: node.ty().to_owned(), group: doc.effective_group(id), fields: fields_of(node) }
    }

    /// The kind its type means (`Kind::from_type`).
    pub(crate) fn kind(&self) -> Kind {
        Kind::from_type(&self.ty)
    }
}
```

- Add `TYPE_LINK, TYPE_TAB` to the `crate::store::model::{..}` import at 18-20.
- `create_ops` line 167 becomes `ops.push(OpBody::create(id, &node.ty, enc));`.
- Line 193-195: the doc reads "One node another replica changed: what the document shows for it (`Snapshot::of`), or `None` when it is no longer live (deleted or pruned)." The alias becomes `pub(crate) type RemoteChange = (NodeId, Option<Snapshot>);`.
- Line 205-206: the doc reads "A node's type string, fields and group, as `materialize` takes them." The alias becomes `pub(crate) type RemoteNode = (String, Fields, Option<String>);`.
- `spawn_snapshot` line 286-287 becomes:

```rust
    let ty = tab_fields(&mut fields, url, view_kind, pinned, parent, jar);
    Snapshot { ty: ty.to_owned(), group: parse_id(&spawn.group), fields }
```

- `tab_fields` returns `&'static str` (signature line 299, `) -> &'static str {`), and its last line 305 becomes `TYPE_TAB`.
- `snapshot` (378-411): in the doc, replace "Its kind is its type (`Mirror::ty`, the one source of a node's type)." with "Its type is the mirror's string (`Mirror::ty`, the one source of a node's type), kept as written.". Replace lines 392-404 with:

```rust
            let ty = self.mirror.ty(&key);
            let kind = Kind::from_type(ty);
            if kind == Kind::Unknown {
                return None;
            }
            let fields = kind
                .fields()
                .iter()
                .map(|&no| match no {
                    field::PARENT => (no, FieldValue::Id(link_id(self.mirror.parent(&key)))),
                    _ => (no, self.mirror.get(&key, no)),
                })
                .collect();
            return Some(Snapshot { ty: ty.to_owned(), group: link_id(self.mirror.group(&key)), fields });
```

  Line 408 becomes `return Some(Snapshot { ty: TYPE_LINK.to_owned(), group: None, fields });`.

  (The `kind == Kind::Unknown` early return goes in Task 3.)

- `materialize` (468-511):
  - The doc paragraph at 468-470 becomes: "A type mismatch between the live node and `ty` is ignored (the store never changes a node's type, so the mirror's first string stands). A file card whose file can't be loaded is kept, showing its error: the node stays in the store."
  - The parameter `kind: Kind` (line 479) becomes `ty: &str`, and the body starts with `let kind = Kind::from_type(ty);` before line 484's comment.
  - Line 498 becomes `self.spawns.remote_in_flight.insert(id, Some((ty.to_owned(), fields.clone(), group)));`.
  - Lines 502-505 become:

```rust
        // The store never changes a node's type, so the first one seen stands (2b-5 Ruling 10). Kept as written.
        if !ty.is_empty() {
            self.mirror.init_field(&id, field::TYPE, FieldValue::Str(ty.to_owned()));
        }
```

- `remote_changes` line 730 becomes `(id, live.map(|n| Snapshot::of(doc, id, n)))`.
- `apply_remote`:
  - Lines 746-749 become:

```rust
        changes.sort_by_key(|(_, c)| match c {
            Some(s) if s.kind() == Kind::Group => 0,
            _ => 1,
        });
```

  - Line 756 becomes `Some(s) => self.materialize(key, &s.ty, &s.fields, s.group.map(|g| g.to_string()), None),`.

- `land_remote_in_flight` line 832 becomes `Some(Some((ty, fields, group))) => self.materialize(id.to_string(), &ty, &fields, group, None),`.
- `restore_session`: replace lines 1464-1465 with:

```rust
        let mut nodes: Vec<(NodeId, Snapshot)> = doc.live().map(|(id, n)| (id, Snapshot::of(doc, id, n))).collect();
```

  and lines 1480-1502 with:

```rust
        // Links ride along: stored, never graph nodes.
        for (id, s) in nodes.iter().filter(|(_, s)| s.kind() == Kind::Link) {
            self.materialize(id.to_string(), &s.ty, &s.fields, None, None);
        }
        nodes.retain(|(_, s)| s.kind() != Kind::Link);
        // Only jars (the Default at least): a fresh start, which keeps them where they are.
        if nodes.iter().all(|(_, s)| s.kind() == Kind::Jar) {
            for (id, s) in &nodes {
                self.materialize(id.to_string(), &s.ty, &s.fields, s.group.map(|g| g.to_string()), None);
            }
            self.restore_closed(local.closed);
            self.arm_local(unread, true);
            return false;
        }
        nodes.sort_by_key(|(_, s)| s.kind() != Kind::Group);
        let pass = self.begin_content_pass();
        for (id, s) in &nodes {
            let group = s.group.map(|g| g.to_string());
            // The store's own `pos` still reaches the mirror inside
            // `materialize`; only the live placement is overridden.
            let entry = local.nodes.get(&id.to_string());
            self.materialize(id.to_string(), &s.ty, &s.fields, group, entry);
        }
```

  and line 1508 with:

```rust
        self.focused = nodes.iter().find(|(_, s)| s.kind() == Kind::Tab).map(|(id, _)| id.to_string());
```

- `materialize_imported`:
  - Lines 1935, 1945, 1949, 2021 and 2029: `i.kind` becomes `i.kind()`. At 1949, `match i.kind {` becomes `match i.kind() {`.
  - Line 1968 becomes `let mut fields = imported_fields(node.kind(), &node.fields);`.
  - Line 1976 becomes `self.materialize(node.id.to_string(), &node.ty, &fields, group, None);`.

`crates/manifold/src/closed.rs`:
- Line 164: `node_json(id, &root.ty, &root_fields, None)`.
- Line 182: `node_json(&label, &member.ty, &fields, member.group)`.
- Line 200: `node_json(&label, &snap.ty, &snap.fields, None)`.
- Line 303: `new.kind != Kind::Jar` becomes `new.kind() != Kind::Jar`.

- [ ] **Step 6: Test helpers follow the new shapes**

`crates/manifold/src/session_sync/tests.rs`:

- `shared` (65-72): the last line becomes `Snapshot { ty: kind.type_str().unwrap().to_owned(), group, fields }`.
- `shown` (83-87) becomes:

```rust
fn shown(doc: &Doc) -> BTreeMap<NodeId, Snapshot> {
    doc.live().map(|(id, node)| (id, Snapshot::of(doc, id, node))).collect()
}
```

- Line 2839 becomes `let want = Snapshot::of(doc, id, node);`.
- `app_with` line 497 becomes `let changes = nodes.iter().map(|(&id, s)| (id, Some(s.clone()))).collect();`.
- Add this helper after `group` (line 80):

```rust
/// A remote change's snapshot of a `kind` node: its type string, `fields` and `group`.
fn snap(kind: Kind, fields: Fields, group: Option<NodeId>) -> Snapshot {
    Snapshot { ty: kind.type_str().unwrap().to_owned(), group, fields }
}
```

- Lines 538, 539, 711, 2054, 2233 and 2289: each `Some((Kind::K, fields, group))` becomes `Some(snap(Kind::K, fields, group))`. Line 2233 is inside the `toggled` closure at 2225-2234.
- Lines 589 and 599: `materialize(node.clone(), Kind::Json, ..)` becomes `materialize(node.clone(), crate::store::model::TYPE_JSON, ..)`.
- Line 4907: `ty(node.kind)` becomes `ty(node.kind())`, twice on the line (the message too).

Other test files:
- `crates/manifold/src/view_install/tests.rs:999` becomes `crate::session_sync::Snapshot { ty: crate::store::model::TYPE_TAB.into(), group: None, fields: Default::default() };`.
- `crates/manifold/src/json_level/tests.rs:205` becomes `(id, s.kind(), s.group, s.fields)`.

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test -j 8 -p manifold an_unknown_kind_carries_the_generic_fields
cargo test -j 8 -p manifold a_create_writes_the_type_string_it_is_given
cargo test -j 8 -p manifold a_set_on_an_unknown_node_leaves_its_raw_fields_alone
cargo test -j 8 -p manifold import_
cargo test -j 8 -p manifold materialize_keeps_the_stored_type_string
cargo test -j 8 -p manifold
```

Expected: PASS for all, including `jar_fields_belong_to_tabs_groups_and_jars`, `import_keeps_an_unknown_type_with_the_generic_fields_only`, `import_rejects_a_missing_or_empty_type`, and the whole crate.

- If an existing test fails because `import` now drops an off-list field (D5), stop and report the test and field. Do not widen the filter.
- If `random_edits_converge_in_one_save` or another sync test fails, it is a tuple rewrite gone wrong: check that each `Snapshot` carries the string the document holds.

- [ ] **Step 8: Leftover check, format, lint, commit**

```bash
git grep -nE 'kind: Kind,$|node\.kind, |root\.kind|member\.kind|snap\.kind|Some\(\(Kind::|kept, never shown|expect\("a known kind"\)' -- crates
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo build -j 8
git add -A crates
git commit -m "Store: every node carries its type string (Snapshot/Imported ty, OpBody::create(&str), Kind::Unknown has the generic fields)

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected grep hits: none, except `kind: Kind,` in signatures that still take a kind: `place_node`, `insert_placeholder_node`, `insert_materialized`, `imported_fields`, the ops test helper and `random_edits_converge_in_one_save`'s model. Each remaining hit must be a `Kind` parameter, not a struct field.

---

### Task 2: One canonical type: the sentinel row and edges

**Files:**
- Modify `crates/manifold/src/store/model.rs`: the TYPE consts at 116-125.
- Modify `crates/manifold/src/card_kind.rs`:
  - Lines 7-12, and `card_kind` at 44-51.
  - Fixtures at 79-93.
  - Tests at 106-119, 180-190 and 192-210.
- Modify `crates/manifold/src/caps/rows.rs`: `builtin_rows` at 93-111.
- Modify `crates/manifold/src/caps/edges.rs`: the import at 6, `CARD_TYPES` at 38-48, plus a new test.
- Modify `crates/manifold/src/closed.rs`: `worth_reopening` at 34-46, `subtree_entry` at 171, and tests at 534-539.
- Modify `crates/manifold/src/links.rs:116`.

**Interfaces:**
- Consumes (from Task 1): nothing beyond the existing `Kind::from_type`.
- Produces:
  - `store::model::TYPE_UNKNOWN: &str = "application/vnd.manifold.unknown"`, never stored.
  - `card_kind::canonical_type(raw: &str) -> &'static str`: a known kind's row string, `TYPE_UNKNOWN` for any other non-empty string, and "" for "".
  - `App::card_kind(id)` reports `TYPE_UNKNOWN` for an unknown stored type (it was `"*/*"`).
  - `caps::registry().caps(TYPE_UNKNOWN, ..)` is `restorable: true, draw_layer: 3`.
  - `caps::edges()`: `contains` and `link` accept `TYPE_UNKNOWN`; `opened-from` and `jar` refuse it.

- [ ] **Step 1: Write the failing tests**

`crates/manifold/src/card_kind.rs` `mod tests`. Replace `an_unknown_stored_type_falls_back_to_the_catch_all_row` (180-190) with:

```rust
    /// A type this build doesn't know reads as the sentinel row: a restorable card drawn with notes and jars, not a
    /// container.
    #[test]
    fn an_unknown_stored_type_reads_as_the_unknown_row() {
        use crate::store::model::{field, FieldValue, TYPE_UNKNOWN};
        let mut app = App::default();
        app.test_node("u", Kind::Unknown, Runtime::None);
        app.mirror.init_field("u", field::TYPE, FieldValue::Str("application/x-from-the-future".into()));
        let kind = app.card_kind("u").unwrap();
        assert_eq!(kind.ty, TYPE_UNKNOWN);
        assert!(kind.caps().restorable);
        assert_eq!(kind.caps().draw_layer, 3);
        assert!(!is_container(kind.ty));
        assert!(!app.is_container("u") && !app.is_container("missing"));
    }

    /// One mapping from a stored type to its row (spec decision 7): a known kind's own string (any image, any case),
    /// the sentinel for anything else, and "" (no row) for no type.
    #[test]
    fn canonical_type_maps_known_kinds_to_their_row_and_the_rest_to_the_sentinel() {
        use crate::store::model::{TYPE_LINK, TYPE_UNKNOWN};
        assert_eq!(canonical_type("Text/Markdown"), TYPE_NOTE);
        assert_eq!(canonical_type("image/jpeg"), TYPE_IMAGE);
        assert_eq!(canonical_type(TYPE_TAB), TYPE_TAB);
        assert_eq!(canonical_type(TYPE_LINK), TYPE_LINK);
        assert_eq!(canonical_type("application/x-from-the-future"), TYPE_UNKNOWN);
        assert_eq!(canonical_type("application/vnd.manifold.json-leaf"), TYPE_UNKNOWN, "a retired type");
        assert_eq!(canonical_type(TYPE_UNKNOWN), TYPE_UNKNOWN);
        assert_eq!(canonical_type(""), "");
    }
```

Task 3 Step 1 switches `Runtime::None` here to `Runtime::Unknown`.

In `draw_layer_follows_the_graph_draw_order` (194-210), add `(plain(crate::store::model::TYPE_UNKNOWN), 3),` after the `TYPE_JAR` row.

`crates/manifold/src/caps/edges.rs` `mod tests`: add after `builtin_contains_takes_every_card_a_jar_included_into_a_group_only` (ends at 238):

```rust
    /// The unknown-type card's row (spec decision 8): it sits in a group and links to any card both ways, but it is
    /// no container, opens nothing, is opened from nothing and takes no jar.
    #[test]
    fn the_unknown_row_links_and_sits_in_a_group_but_opens_nothing_and_takes_no_jar() {
        use crate::store::model::{TYPE_GROUP, TYPE_JAR, TYPE_NOTE, TYPE_TAB, TYPE_UNKNOWN};
        let t = edges();
        assert!(t.can_connect(CONTAINS, TYPE_UNKNOWN, TYPE_GROUP));
        assert!(!t.target_accepts(CONTAINS, TYPE_UNKNOWN), "not a container");
        for other in [TYPE_TAB, TYPE_GROUP, TYPE_NOTE, TYPE_JAR, TYPE_UNKNOWN] {
            assert!(t.can_connect(LINK, TYPE_UNKNOWN, other) && t.can_connect(LINK, other, TYPE_UNKNOWN), "{other}");
        }
        assert!(!t.can_connect(OPENED_FROM, TYPE_UNKNOWN, TYPE_TAB) && !t.can_connect(OPENED_FROM, TYPE_TAB, TYPE_UNKNOWN));
        assert!(!t.can_connect(JAR_EDGE, TYPE_UNKNOWN, TYPE_JAR) && !t.can_connect(JAR_EDGE, TYPE_TAB, TYPE_UNKNOWN));
        // Unchanged: a group is opened from nothing, and a jar opens nothing.
        assert!(!t.can_connect(OPENED_FROM, TYPE_GROUP, TYPE_TAB) && !t.can_connect(OPENED_FROM, TYPE_TAB, TYPE_JAR));
    }
```

`crates/manifold/src/closed.rs` `mod tests`. Replace `an_unknown_type_is_not_worth_reopening` (534-539) with:

```rust
    /// A type this build doesn't know comes back as a shell of its type (spec decision 4), whatever its case; no type
    /// at all (`Mirror::ty` of a node the mirror lacks) does not; a blank note is not, whatever the case of its type.
    #[test]
    fn an_unknown_type_is_worth_reopening_as_a_shell() {
        assert!(worth_reopening("application/x-future", &Default::default()));
        assert!(worth_reopening("application/vnd.manifold.json-leaf", &Default::default()), "a retired type");
        assert!(!worth_reopening("", &Default::default()));
        assert!(!worth_reopening("Text/Markdown", &Default::default()), "a blank note, its type in another case");
        assert!(!worth_reopening(store::model::TYPE_DOWNLOAD, &Default::default()));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold --no-run`
Expected: compile errors: `cannot find value 'TYPE_UNKNOWN'` and `cannot find function 'canonical_type'`.

- [ ] **Step 3: The sentinel, `canonical_type` and its callers**

`crates/manifold/src/store/model.rs`, after `TYPE_DOWNLOAD` (line 125):

```rust
/// Never stored: the row and edge string a type this build doesn't know reads as (`card_kind::canonical_type`).
pub const TYPE_UNKNOWN: &str = "application/vnd.manifold.unknown";
```

`crates/manifold/src/card_kind.rs`. Replace lines 7-12 with:

```rust
use crate::store::model::{Kind, TYPE_UNKNOWN};
#[cfg(test)]
use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP, TYPE_IMAGE, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB};

/// The row string a stored type reads as, for `caps::registry` and `caps::edges` (spec decision 7): a known kind's
/// own type string (case ignored; any `image/*` is the image row), `TYPE_UNKNOWN` for a type this build doesn't
/// know, and "" for no type (an id the mirror holds no `type` for), which no row matches.
pub(crate) fn canonical_type(raw: &str) -> &'static str {
    if raw.is_empty() {
        return "";
    }
    Kind::from_type(raw).type_str().unwrap_or(TYPE_UNKNOWN)
}
```

Then:

- The `card_kind` doc (44-45) becomes: "The identity of card `id`: its stored type as its row reads it (`canonical_type`), plus `Tag::Loaded` while a tab has an engine. `None` when `id` has no node."
- Line 48 becomes `let ty = canonical_type(self.mirror.ty(id));`.
- Fixture `kinds()` (79-93): the return type becomes `[CardKind; 10]`, and `plain(TYPE_UNKNOWN),` goes after `plain(TYPE_JAR),`. The doc's first sentence becomes "Every card kind, in draw order: each `TYPES` entry a node can show and the unknown-type row (`kinds_cover_every_node_type` pins that), plus the loaded tab, which shares the sleeping tab's type."
- `kinds_cover_every_node_type` (106-119): line 110 becomes:

```rust
        let mut types: Vec<&str> = TYPES.iter().map(|t| t.1).filter(|&ty| ty != TYPE_LINK).collect();
        types.push(TYPE_UNKNOWN);
```

`crates/manifold/src/caps/rows.rs` `builtin_rows` (93-111):

- The doc becomes: "What `worth_reopening` decides per type: only a completed download and a blank note are not recorded when closed (a type this build doesn't know comes back as a shell, spec decision 4). Also the draw layer of each type (`draw_layer`). A string no row matches gets the `Caps` default; `card_kind::canonical_type` maps every stored type to a row first."
- The import becomes `use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP, TYPE_JAR, TYPE_NOTE, TYPE_TAB, TYPE_UNKNOWN};`.
- Line 98 `Row::new("*/*").restorable(false).draw_layer(0),` is deleted.
- After the `TYPE_JAR` row (107), add `Row::new(TYPE_UNKNOWN).restorable(true).draw_layer(3), // a type this build doesn't know, drawn with notes`.
- Line 109 becomes `Row::new(TYPE_DOWNLOAD).restorable(false).draw_layer(2), // a download`.

`crates/manifold/src/caps/edges.rs`:

- Line 6 becomes `use crate::store::model::{field, TYPE_GROUP, TYPE_JAR, TYPE_LINK, TYPE_NOTE, TYPE_TAB, TYPE_UNKNOWN};`.
- `CARD_TYPES` (38-48) becomes:

```rust
/// Every card type that may sit inside a group and be linked; `TYPE_UNKNOWN` is a type this build doesn't know
/// (`card_kind::canonical_type`), which opens nothing and takes no jar.
const CARD_TYPES: [&str; 9] = [
    TYPE_TAB,
    TYPE_GROUP,
    TYPE_NOTE,
    TYPE_JAR,
    "image/*",
    "application/pdf",
    "application/json",
    "application/octet-stream",
    TYPE_UNKNOWN,
];
```

`crates/manifold/src/closed.rs`:

- `worth_reopening` (34-46) becomes:

```rust
/// Whether a node of stored type `ty` (`Mirror::ty`) with `fields` (its snapshot's) is worth keeping in a
/// `ClosedEntry`. `false` for a completed download (it has no closed state -- starting it over isn't "reopening"), a
/// blank note (nothing to restore; the `Blank` tag) and no type at all; everything else -- tabs, groups, images,
/// PDFs, JSON files, jars and a type this build doesn't know (a shell, spec decision 4) -- is. The capability rows
/// in `caps/rows.rs` decide, by the row the type reads as (`card_kind::canonical_type`).
pub(crate) fn worth_reopening(ty: &str, fields: &session_sync::Fields) -> bool {
    use crate::caps::{Tag, TagSet};
    let ty = crate::card_kind::canonical_type(ty);
    let text = fields.get(&field::TEXT);
    let blank = !matches!(text, Some(store::model::FieldValue::Str(text)) if !text.trim().is_empty());
    let tags = if ty == store::model::TYPE_NOTE && blank { TagSet::of(&[Tag::Blank]) } else { TagSet::EMPTY };
    crate::caps::registry().caps(ty, tags).restorable
}
```

- `subtree_entry` line 171 becomes `|id| app.is_container(&id.to_string()),`. `App::is_container` goes through `card_kind`, so it is canonical.

`crates/manifold/src/links.rs:116` becomes:

```rust
        let ty = |id: &str| crate::card_kind::canonical_type(self.mirror.ty(id));
        if from == to || !crate::caps::edges().can_connect(edge, ty(from), ty(to)) {
```

Keep the rest of the `if` as it is.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -j 8 -p manifold card_kind
cargo test -j 8 -p manifold caps
cargo test -j 8 -p manifold closed
cargo test -j 8 -p manifold
```

Expected: PASS, including:
- `canonical_type_maps_known_kinds_to_their_row_and_the_rest_to_the_sentinel`
- `an_unknown_stored_type_reads_as_the_unknown_row`
- `kinds_cover_every_node_type`
- `draw_layer_follows_the_graph_draw_order`
- `every_card_kind_can_go_in_a_group_and_nothing_goes_in_anything_else`
- `the_unknown_row_links_and_sits_in_a_group_but_opens_nothing_and_takes_no_jar`
- `an_unknown_type_is_worth_reopening_as_a_shell`
- `worth_reopening_is_what_it_was_for_every_kind`

`card/tests.rs` and `node_menu/tests.rs` iterate `kinds()` (`card_controls_list_each_cards_buttons_in_pointer_order`, `drop_action_matches_the_drop_table`, `del_hint_names_what_del_does_on_the_card`, `card_del_hint_matches_the_menu_row_hinted_del`) and must pass with the tenth kind. Its menu is the `_` arm's: Delete hinted Del, verb "delete". If one fails, report it rather than special-casing the sentinel.

- [ ] **Step 5: Leftover check, format, lint, commit**

```bash
git grep -nE 'UNKNOWN_TYPE|catch_all_row' -- crates
git grep -n '"\*/\*"' -- crates/manifold/src/caps/rows.rs crates/manifold/src/card_kind.rs
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo build -j 8
git add -A crates
git commit -m "Caps: one canonical type; an unknown type reads as the restorable sentinel row that links and sits in groups

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: the first grep has no hits. The second hits only lines inside `rows.rs`'s `mod tests`.

---

### Task 3: The unknown card is live

**Files:**
- Modify `crates/manifold/src/store/model.rs`:
  - `is_live` at 444-447 and the `is_group` it gains.
  - `effective_group` at 515-530.
  - Tests at 680, 692-693 and 708-733.
- Modify `crates/manifold/src/store/journal/tests.rs`: lines 283-284 and 312-313.
- Modify `crates/manifold/src/node.rs`:
  - The `Runtime` enum at 15-27.
  - Lines 84-90, 103 and 112-119.
  - `accessible_name` at 395-424; the test at 536.
- Modify `crates/manifold/src/session_sync.rs`:
  - `runtime_has_type` at 88-105.
  - The `snapshot` doc and body at 378-395.
  - `update_live_node` at 572-580.
  - `insert_materialized` at 715.
- Modify `crates/manifold/src/reveal.rs:49` and `crates/manifold/src/graph_nav.rs` (lines 291 and 656).
- Modify `crates/manifold/src/card.rs`:
  - Size consts at 26-30.
  - `node_card_size` at 898-916; `card_filter_text` at 923-939.
  - `paint_card_body` at 1306-1307; the new `paint_unknown_body` after 1541.
- Modify `crates/manifold/src/node_menu.rs:650` (comment only).
- Modify `crates/manifold/src/popups.rs`: `closed_entry_label` at 714-732.
- Modify `crates/manifold/src/card_kind.rs`: `payloads()` at 153-163, and Task 2's unknown-row test.
- Test `crates/manifold/src/session_sync/tests.rs` (new tests) and `crates/manifold/src/card/tests.rs` (lines 413, 634 and 689).

**Interfaces:**
- Consumes:
  - From Task 1: `Snapshot { ty, .. }`, `Snapshot::of`, `Node::ty`, `OpBody::create(.., &str, ..)` and `App::materialize(.., &str, ..)`.
  - From Task 2: `canonical_type`, `TYPE_UNKNOWN`, and the sentinel row and edges.
- Produces:
  - `Node::is_live(&self) -> bool`: `!deleted && !ty().is_empty()`.
  - `Node::is_group(&self) -> bool`.
  - `Runtime::Unknown` (unit variant).
  - `card::UNKNOWN_NODE_SIZE: egui::Vec2`.
  - `App::paint_unknown_body(..) -> CardBody`, with the same parameters as `paint_jar_body`.

- [ ] **Step 1: Write the failing tests**

`crates/manifold/src/store/model.rs` `mod tests`:

- Line 680's comment becomes `// The retired JSON leaf: an old document's leaf is now a type this build doesn't know (an unknown-type card).`
- Lines 692-693 become:

```rust
        node.fields.insert(field::TYPE, reg(2, "application/x-from-the-future"));
        assert_eq!((node.kind(), node.has_type(), node.is_live()), (Kind::Unknown, true, true), "a card of its own");
        node.fields.insert(field::TYPE, reg(3, ""));
        assert!(node.has_type() && !node.is_live(), "an empty type is no type");
```

- Rename `nodes_show_at_their_nearest_live_group` (708-733) to `nodes_show_at_their_nearest_live_group_never_inside_another_card`:
  - Line 722's comment becomes `// a newer build's container: live, not a group`.
  - After line 726 (`id(9)`), add `doc.nodes.insert(id(10), node(Kind::Note, Some(3), false)); // in a tab: shown in 1`.
  - After line 730, add `assert_eq!(doc.effective_group(id(10)), Some(id(1)), "a tab is no group");`.
  - Line 731's expected set becomes `BTreeSet::from([id(2), id(3), id(10)])`.
  - Line 732's becomes `BTreeSet::from([1, 2, 3, 5, 6, 10].map(id))`.
  - Keep the line-729 expectation `effective_group(id(6)) == Some(id(1))`. It now holds because 5 is not a group; before, 5 was not live.

`crates/manifold/src/store/journal/tests.rs`:
- Line 284 becomes `assert!(doc.nodes[&node].is_live(), "a type this build doesn't know is a card");`.
- Line 313 becomes `assert!(doc.nodes[&leaf].is_live(), "an unknown type is a card now");`.
- The doc comment at 287-288 ends "...reads as unknown (a card) and keeps every byte through a snapshot and compaction.".

`crates/manifold/src/card_kind.rs`:
- In `an_unknown_stored_type_reads_as_the_unknown_row`, change `Runtime::None` to `Runtime::Unknown`.
- In `payloads()`, add `(Kind::Unknown, Runtime::Unknown, plain(crate::store::model::TYPE_UNKNOWN)),` before the Group row. `card_kind_reads_the_type_and_tags_loaded_for_every_constructible_payload` would then see "" as the stored type (`place_node` seeds none for `Unknown`). So in that test, after `app.test_node(&id, kind, payload);`, add:

```rust
            if kind == Kind::Unknown {
                let future = FieldValue::Str("application/x-from-the-future".into());
                app.mirror.init_field(&id, crate::store::model::field::TYPE, future);
            }
```

and add `use crate::store::model::FieldValue;` to that test module's imports.

`crates/manifold/src/card/tests.rs`:
- Line 413's list gains `"jar", "unknown"`.
- In `node_card_size_matches_each_kinds_card`, after line 634 add `assert_eq!(node_card_size(&Runtime::Unknown), UNKNOWN_NODE_SIZE);`.
- In `card_filter_text_matches_each_kinds_filter_input`, after line 689 add `assert!(filter(&Runtime::Unknown).is_empty(), "matched by its stored name, which the caller pushes");`.

`crates/manifold/src/session_sync/tests.rs`: add near the end of the file. The module already has `id`, `app_on`, `add`, `add_group`, `ops_of`, `stored`, `temp_dir`, `Journal` and `ActorId`.

```rust
/// A newer build's type: the unknown-type card (spec 2026-10-14).
const FUTURE: &str = "application/x-from-the-future";

/// A session another replica (actor 2) wrote holding one node of type `FUTURE`, named "Old" at (5, 5), with a newer
/// build's raw field 40 and two known fields its kind doesn't carry (`url`, an `Id`-typed `jar`), restored by actor 1.
fn restored_unknown() -> (App, NodeId, std::path::PathBuf) {
    let path = temp_dir("unknown-card").join("session.mnf");
    let node = {
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        let mut txn = other.begin().unwrap();
        let node = txn.next_id();
        let mut fields = encode_field(field::NAME, &FieldValue::Str("Old".into()));
        fields.extend(encode_field(field::POS, &FieldValue::Pos(5.0, 5.0)));
        fields.extend(encode_field(field::URL, &FieldValue::Str("https://x/".into())));
        fields.extend(encode_field(field::JAR, &FieldValue::Id(Some(node))));
        fields.extend(encode_field(40, &FieldValue::Str("theirs".into())));
        txn.commit(vec![OpBody::create(node, FUTURE, fields)]).unwrap();
        node
    };
    let mut app = app_on(&path, 1);
    assert!(app.restore_session(), "a lone unknown card is real content, not a fresh start");
    (app, node, path)
}

/// The fields `node` holds in `app`'s document, by number.
fn doc_fields(app: &App, node: NodeId) -> Vec<u32> {
    app.journal.as_ref().unwrap().doc().nodes[&node].fields.keys().copied().collect()
}

#[test]
fn an_unknown_type_is_a_card_from_a_restore_and_from_a_remote_create() {
    let (mut app, node, path) = restored_unknown();
    let key = node.to_string();
    assert!(matches!(app.nodes[&key].runtime, Runtime::Unknown));
    assert_eq!(app.mirror.ty(&key), FUTURE);
    assert_eq!(app.card_kind(&key).unwrap().ty, crate::store::model::TYPE_UNKNOWN);
    assert_eq!(app.mirror.name(&key), "Old");
    assert_eq!(app.mirror.pos(&key), Some(egui::vec2(5.0, 5.0)));
    assert_eq!(app.accessible_name(&key).as_deref(), Some("Card: Old"));
    let mut third = Journal::open(&path, ActorId(3)).unwrap();
    let mut txn = third.begin().unwrap();
    let second = txn.next_id();
    txn.commit(vec![OpBody::create(second, FUTURE, encode_field(field::NAME, &FieldValue::Str("New".into())))])
        .unwrap();
    assert!(app.save_session());
    assert!(matches!(app.nodes[&second.to_string()].runtime, Runtime::Unknown), "another replica's shows too");
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(shown(doc), snapshots(&app));
}

#[test]
fn moving_an_unknown_card_writes_one_set_and_keeps_its_raw_fields() {
    let (mut app, node, _path) = restored_unknown();
    let key = node.to_string();
    assert!(app.save_session());
    let raw = app.journal.as_ref().unwrap().doc().nodes[&node].fields[&40].enc.clone();
    let before = ops_of(&app);
    app.edit_pos(&key, Some(egui::vec2(50.0, 60.0)));
    assert!(app.save_session());
    assert_eq!(ops_of(&app), before + 1, "one Set");
    assert_eq!(stored(&app, &key, field::POS), FieldValue::Pos(50.0, 60.0));
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.nodes[&node].fields[&40].enc, raw, "a newer build's field is untouched");
    assert_eq!(doc.nodes[&node].get(field::URL), FieldValue::Str("https://x/".into()), "so is an off-list one");
}

#[test]
fn a_remote_delete_removes_an_unknown_card() {
    let (mut app, node, path) = restored_unknown();
    let mut other = Journal::open(&path, ActorId(3)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node }]).unwrap();
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&node.to_string()));
}

/// A node an op named before its `Create` arrived stays hidden; once its type lands it is a card.
#[test]
fn a_typeless_placeholder_gets_its_card_when_its_type_arrives() {
    let node = id(5);
    let mut doc = Doc::new(crate::store::model::DocId([0; 16]));
    doc.nodes.insert(node, Node::placeholder());
    assert_eq!(App::remote_changes(&doc, &[node]), vec![(node, None)]);
    doc.nodes.insert(node, Node::with_type(FUTURE));
    let changes = App::remote_changes(&doc, &[node]);
    assert_eq!(changes[0].1.as_ref().map(|s| s.ty.as_str()), Some(FUTURE));
    let mut app = App::default();
    app.apply_remote(changes);
    assert!(matches!(app.nodes[&node.to_string()].runtime, Runtime::Unknown));
}

/// Duplicate, Close then Reopen, and a JSON round trip each make a shell of the type: the type string and the
/// generic fields, never a newer build's field or a known one its kind doesn't carry (spec decision 4).
#[test]
fn an_unknown_card_copies_as_a_shell_of_its_type() {
    use crate::store::model::GENERIC;
    let (mut app, node, _path) = restored_unknown();
    let key = node.to_string();
    app.edit_name(&key, "Mine");
    assert!(app.save_session());
    assert_eq!(app.accessible_name(&key).as_deref(), Some("Card: Mine"));
    let shell = |app: &App, id: NodeId| {
        let doc = app.journal.as_ref().unwrap().doc();
        assert_eq!(doc.nodes[&id].ty(), FUTURE);
        doc_fields(app, id).iter().all(|no| *no == field::TYPE || *no == field::GROUP || GENERIC.contains(no))
    };

    app.duplicate_nodes(std::slice::from_ref(&key));
    let copies: Vec<String> =
        app.nodes.iter().filter(|(k, n)| **k != key && matches!(n.runtime, Runtime::Unknown)).map(|(k, _)| k.clone()).collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(app.mirror.name(&copies[0]), "Mine");
    assert!(shell(&app, copies[0].parse().unwrap()), "{:?}", doc_fields(&app, copies[0].parse().unwrap()));

    app.close_node_and_save(&key);
    assert!(!app.nodes.contains_key(&key));
    assert_eq!(app.closed.last().unwrap().doc.nodes[0].fields["type"], FUTURE, "the closed entry holds the real string");
    app.reopen_closed_shortcut();
    let reopened: Vec<NodeId> = app
        .nodes
        .iter()
        .filter(|(k, n)| **k != copies[0] && matches!(n.runtime, Runtime::Unknown))
        .map(|(k, _)| k.parse().unwrap())
        .collect();
    assert_eq!(reopened.len(), 1, "a fresh shell, not the tombstone");
    assert!(shell(&app, reopened[0]));

    let json = crate::store::json::doc_to_json(app.journal.as_ref().unwrap().doc());
    let mut counter = 0;
    let next = || {
        counter += 1;
        NodeId { actor: ActorId(9), counter }
    };
    let imported = crate::store::json::import(&json, next, crate::store::json::Trust::Foreign).unwrap();
    let shells: Vec<_> = imported.iter().filter(|i| i.ty == FUTURE).collect();
    assert_eq!(shells.len(), 2);
    for s in shells {
        assert!(s.fields.keys().all(|no| *no == field::GROUP || GENERIC.contains(no)), "{:?}", s.fields);
    }
}

#[test]
fn a_link_and_a_group_take_an_unknown_card() {
    let (mut app, node, _path) = restored_unknown();
    let key = node.to_string();
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    let g = add_group(&mut app, "G");
    assert!(app.save_session());
    assert!(app.connect(crate::caps::LINK, &key, &note).is_ok());
    assert!(app.connect(crate::caps::OPENED_FROM, &key, &note).is_err(), "it opens nothing");
    assert!(app.save_session());
    let links = |app: &App| app.journal.as_ref().unwrap().doc().live().filter(|(_, n)| n.kind() == Kind::Link).count();
    assert_eq!(links(&app), 1);
    app.edit_group(&key, Some(g.clone()));
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(node), Some(g.parse().unwrap()));
    assert_eq!(app.mirror.group(&key), Some(g.as_str()));
}
```

If `GENERIC` is not reachable from the test module (it is `pub(crate)` in `store::model`), the `use crate::store::model::GENERIC;` line covers it. `App::remote_changes` is a private associated fn of `App` in `session_sync.rs`, so it is visible to the child `tests` module.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j 8 -p manifold --no-run`
Expected: compile error, `no variant named 'Unknown'` on `Runtime`, and `UNKNOWN_NODE_SIZE` not found.

- [ ] **Step 3: Liveness and the effective group**

`crates/manifold/src/store/model.rs`:

- Replace `is_live` (444-447):

```rust
    /// Shown in the graph: created with a type (any, one this build doesn't know included: an unknown-type card),
    /// and not deleted. An empty `type` is no type.
    pub fn is_live(&self) -> bool {
        !self.deleted && !self.ty().is_empty()
    }

    /// A group: the only kind other nodes are shown inside (`Doc::effective_group`).
    pub fn is_group(&self) -> bool {
        self.kind() == Kind::Group
    }
```

- `effective_group` (515-530): the doc becomes:

```rust
    /// The group `id` is shown in: its own, or, when that isn't a live group (deleted, not created yet, or another
    /// card, a type this build doesn't know included), its nearest live group ancestor (spec "Semantics"; unknown-type
    /// card decision 2). The document keeps the raw `group`. `None` is the top level, also for a group missing from
    /// the document or a cycle, which only a damaged snapshot can hold.
```

  and line 524 becomes `if node.is_live() && node.is_group() {`.

- [ ] **Step 4: `Runtime::Unknown` and the exhaustive matches**

`crates/manifold/src/node.rs`:

- In the `Runtime` enum (15-27), add as the last variant:

```rust
    /// A card of a type this build doesn't know: the generic fields only, all the mirror's (spec 2026-10-14).
    Unknown,
```

- `saved_tab` (84-90), `file_path` (103) and `listed_tab` (112-119): add `Runtime::Unknown` beside `Runtime::Jar` in each arm that returns `None`.
- `accessible_name` (395-424):
  - The doc's last clause becomes "...a note by the first line of `text` and a group or jar by `text`, the stored text its card shows (`App::own_text`), ignored for other kinds; a card of a type this build doesn't know is "Card", which a stored name follows (`App::accessible_name`)."
  - Line 408's comment becomes `// A group has no runtime of its own (2b-6 Ruling 8); its name is `text`.`
  - After the `Runtime::Jar` arm (413-415), add `Runtime::Unknown => ("Card", None),`.
- The test at 536-537: add `assert_eq!(Runtime::Unknown.saved_tab(), None);`.

`crates/manifold/src/reveal.rs:49`: `Runtime::Tab(_) | Runtime::Unloaded(_) | Runtime::Note(_) | Runtime::Jar | Runtime::None => None,` gains `| Runtime::Unknown`.

`crates/manifold/src/graph_nav.rs`:
- Line 291's comment becomes `// A group has no runtime of its own (2b-6 Ruling 8).`
- Line 656 becomes `Runtime::Note(_) | Runtime::Jar | Runtime::Unknown => 3,`.

`crates/manifold/src/session_sync.rs`:

- `runtime_has_type` (88-105):
  - The doc's second line becomes "A tab's runtime is one type whether it is loaded or asleep; a group has none (2b-6 Ruling 8); a type this build doesn't know reads as `TYPE_UNKNOWN` (`card_kind::canonical_type`)."
  - Add `TYPE_UNKNOWN` to its `use` list.
  - Add the arm `Runtime::Unknown => ty == TYPE_UNKNOWN,`.
- `snapshot`:
  - In the doc (383-385), replace "for a node of a type this build doesn't know (never placed today: every way in seeds a known type), and for an id that is none of these." with "for a node with no type yet, and for an id that is none of these. A type this build doesn't know is snapshotted with its string and the generic fields (`Kind::fields`)."
  - In the body, replace Task 1's

    ```rust
            let ty = self.mirror.ty(&key);
            let kind = Kind::from_type(ty);
            if kind == Kind::Unknown {
                return None;
            }
    ```

    with

    ```rust
            let ty = self.mirror.ty(&key);
            if ty.is_empty() {
                return None;
            }
            let kind = Kind::from_type(ty);
    ```

- `update_live_node` (572-580): the comment becomes `// A group's fields (it has no runtime), a jar's, an unknown-type card's (all generic), a content node's url and a Json node's view are the mirror's alone (`mirror_remote`; the level shown is relaid out below). Paths, keys and values never change after creation.` Add `| Runtime::Unknown` after `| Runtime::Jar`.
- `insert_materialized` line 715 `Kind::Link | Kind::Unknown => return,` becomes:

```rust
            Kind::Unknown => self.insert_placeholder_node(id.clone(), pos, kind, Runtime::Unknown),
            Kind::Link => return,
```

- [ ] **Step 5: The card, its size, filter and Recently Closed row**

`crates/manifold/src/card.rs`:

- After `JAR_NODE_SIZE` (line 30), add:

```rust
/// A card of a type this build doesn't know: a header and nothing under it, like a jar's.
pub(crate) const UNKNOWN_NODE_SIZE: egui::Vec2 = JAR_NODE_SIZE;
```

- `node_card_size`:
  - Line 913's comment becomes `// A group has no runtime of its own (2b-6 Ruling 8).`
  - After line 914, add `Runtime::Unknown => UNKNOWN_NODE_SIZE,`.
- `card_filter_text`:
  - The doc's last part becomes "...a group's or jar's name (`text`, `App::own_text`, for a note or a group). A card of a type this build doesn't know has nothing of its own: its stored name is pushed beside it by the caller."
  - Lines 935-937 become:

```rust
        // A group has no runtime of its own (2b-6 Ruling 8); it matches on its name, `text`, as a jar does.
        Runtime::None | Runtime::Jar => vec![text],
        Runtime::Unknown => Vec::new(),
```

- `paint_card_body` lines 1306-1307 become:

```rust
            // `TYPE_UNKNOWN` (`card_kind::canonical_type`): every other row is matched above.
            _ => self.paint_unknown_body(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale),
```

- After `paint_jar_body` (ends at 1541), add:

```rust
    /// A card of a type this build doesn't know (spec 2026-10-14 decision 9): a question-mark glyph and its type
    /// string in the header. A stored name takes the title's place (`paint_card_header`) and the type moves under it.
    /// No body.
    pub(crate) fn paint_unknown_body(
        &mut self,
        ui: &mut egui::Ui,
        deferred_overlays: &mut DeferredOverlays,
        id: &str,
        rect: egui::Rect,
        inv_zoom: f32,
        zoom_scale: f32,
    ) -> CardBody {
        let body = CardBody { interact_rect: rect, tooltip: None };
        if self.graph_lod_simplified {
            return body;
        }
        let ty = self.mirror.ty(id).to_owned();
        let named = !self.mirror.name(id).is_empty(); // name rule: only whether one is stored
        let header = CardHeader {
            glyph: Some(egui_phosphor::light::QUESTION),
            glyph_color: None,
            icon_image: None,
            source: if named { ty.clone() } else { String::new() },
            name: ty,
            user_name: None,
            source_is_path: false,
            scrim: false,
            right_margin: 0.0,
        };
        self.paint_card_header(ui, deferred_overlays, id, rect, inv_zoom, zoom_scale, header);
        body
    }
```

`crates/manifold/src/node_menu.rs:650`: the comment becomes `// A type this build doesn't know (`TYPE_UNKNOWN`): the shared rows only (Rename, Duplicate, Pin, Color, links, Move to) and Delete.` The code on line 651 is unchanged.

`crates/manifold/src/popups.rs` `closed_entry_label`:

- After the Note branch (ends at 714), add:

```rust
        // A type this build doesn't know (`JsonNode::kind` is `None`): its stored name, else its type string, as its
        // card's header shows them.
        if node.kind().is_none() {
            ui.label(egui_phosphor::light::QUESTION);
            let name = str_field("name");
            if name.is_empty() {
                return (str_field("type").into(), None);
            }
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            return (crate::names::name_job(ui, "", &segs, "").into(), crate::names::hidden_hint(&segs));
        }
```

- The `_` arm's comment (729) becomes `// Json, a jar, a download.`

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -j 8 -p manifold a_node_reads_its_kind_from_the_last_type_write
cargo test -j 8 -p manifold nodes_show_at_their_nearest_live_group_never_inside_another_card
cargo test -j 8 -p manifold unknown
cargo test -j 8 -p manifold a_retired_json_leaf_and_its_group_flag_survive_compaction
cargo test -j 8 -p manifold card_kind
cargo test -j 8 -p manifold card::
cargo test -j 8 -p manifold
```

Expected: PASS, including:
- `an_unknown_type_is_a_card_from_a_restore_and_from_a_remote_create`
- `moving_an_unknown_card_writes_one_set_and_keeps_its_raw_fields`
- `a_remote_delete_removes_an_unknown_card`
- `a_typeless_placeholder_gets_its_card_when_its_type_arrives`
- `an_unknown_card_copies_as_a_shell_of_its_type`
- `a_link_and_a_group_take_an_unknown_card`
- `card_kind_reads_the_type_and_tags_loaded_for_every_constructible_payload`
- `every_card_body_draws_a_header`
- `node_card_size_matches_each_kinds_card`
- `card_filter_text_matches_each_kinds_filter_input`

`save_session` runs `check_graph_matches_document` in debug builds, so a missed funnel fails loudly. For example, an unknown node live in the document but not placed panics with "live in the document but not shown".

- [ ] **Step 7: Leftover check, format, lint, commit**

```bash
git grep -nE 'no unknown-type card yet|no menu of its own yet|kept, never shown|nothing shows|Kind::Link \| Kind::Unknown' -- crates
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo build -j 8
git add -A crates
git commit -m "Unknown-type card: a node of a type this build doesn't know is a live card (generic fields, shell copies, links, groups)

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

Expected: no grep hits. `nothing shows` may still hit an unrelated comment. Read every hit, and fix it only if it is about unknown types.

---

### Task 4: `CLOSENODE`, `stored_type`, the gate stage, docs and the full check

**Files:**
- Modify `crates/manifold/src/debug_control.rs`:
  - The EXPORTSTATE doc at 230-241.
  - `CloseGroup` at 360-364; its references at 366, 378 and 388.
  - The help at 583, the parse at 862 and `debug_move_node` at 1056-1066.
  - The handler at 1385-1389.
  - The card JSON at 1563 and the SAMPLES at 1893.
  - The key list at 2061-2073, plus a new test.
- Modify `crates/gate/src/smoke.rs`: lines 701, 935 and 964.
- Create `crates/gate/src/unknown_type.rs`.
- Modify `crates/gate/src/main.rs`: the doc at 6-8, the mods at 54-58, the dispatch at 109-111 and the usage line at 127.
- Modify `crates/gate/src/all.rs`: the doc at 5-16 and the stages at 218-219.
- Modify `README.md`:
  - The debug list at 387-393 and the `gate all` paragraph at 405.
  - A `gate unknown-type` paragraph after the `gate link` paragraph (423).
  - A changelog entry before "Pin on every card" (710).
- Modify `docs/superpowers/specs/2026-10-04-node-model-design.md:75-76`, `docs/superpowers/specs/2026-10-04-homogeneity-decisions.md:39` and `docs/superpowers/specs/2026-10-14-unknown-type-card-design.md:3`.

**Interfaces:**
- Consumes: from Task 3, the live unknown card; from Task 2, `TYPE_UNKNOWN` as EXPORTSTATE's `type` for it.
- Produces:
  - Debug verb `CLOSENODE id` (`DebugCommand::CloseNode(String)`) and `App::debug_close_node(&mut self, id: &str) -> bool`.
  - EXPORTSTATE card key `stored_type`.
  - Gate stage `unknown-type`, run by `gate all` in every mode.

- [ ] **Step 1: Write the failing debug-command test**

`crates/manifold/src/debug_control.rs` `mod tests`, after the `MOVETOGROUP` test (around 1950-1970):

```rust
    /// `CLOSENODE id` closes any node by id, as its card's Close does, and saves.
    #[test]
    fn close_node_takes_any_node() {
        assert!(matches!(parse_line("CLOSENODE n1"), Some(DebugCommand::CloseNode(id)) if id == "n1"));
        assert!(parse_line("CLOSENODE").is_none(), "the id is required");
        let mut app = App::default();
        let (g, note) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(g.clone(), None);
        app.insert_note_at(note.clone(), None);
        assert!(app.debug_close_node(&note));
        assert!(app.debug_close_node(&g), "a group, as CLOSEGROUP did");
        assert!(!app.nodes.contains_key(&note) && !app.nodes.contains_key(&g));
        assert!(!app.debug_close_node("missing"));
    }
```

The setup copies the `MOVETOGROUP` test's pattern (`debug_control.rs` ~1958-1961): `insert_note_at(id: String, pos: Option<Vec2>)` returns `()`.

In the SAMPLES list, line 1893 `"CLOSEGROUP g1",` becomes `"CLOSENODE g1",`. In the EXPORTSTATE key test (2061-2073), add `"stored_type",` after `"is_default",`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -j 8 -p manifold close_node_takes_any_node`
Expected: compile error, `no variant named 'CloseNode'`.

- [ ] **Step 3: `CLOSENODE` and `stored_type`**

`crates/manifold/src/debug_control.rs`:

- Lines 360-364 become:

```rust
    /// `CLOSENODE id` -- closes node `id` (any kind) as its card's Close or Delete does (`close_node_and_save`), by
    /// id since a headless test can't rely on focus. An unknown id is ignored with a note on stderr.
    CloseNode(String),
```

- In the docs at 366, 378 and 388, "`CloseGroup`" becomes "`CloseNode`".
- Line 583 becomes `("CLOSENODE id", "Close node `id` (any kind), as its card's Close does, and save"),`.
- Line 862 becomes `"CLOSENODE" => non_empty(DebugCommand::CloseNode),`.
- The handler at 1385-1389 becomes:

```rust
            DebugCommand::CloseNode(id) => {
                if !self.debug_close_node(&id) {
                    eprintln!("CLOSENODE: no node {id}; ignored");
                }
            }
```

- After `debug_move_node` (ends at 1066), add:

```rust
    /// `CLOSENODE`: `close_node_and_save` on node `id`. `false` when there is no such node.
    fn debug_close_node(&mut self, id: &str) -> bool {
        if !self.nodes.contains_key(id) {
            return false;
        }
        self.close_node_and_save(id);
        true
    }
```

- The card JSON, after line 1563 `"type": kind.ty,`, gains:

```rust
                    "stored_type": self.mirror.ty(&item.id), // the raw string; `type` is its row (`canonical_type`)
```

- In the EXPORTSTATE doc (230-241), after "`store::model`'s `TYPE_*`)", insert ", `stored_type` (the type string as stored: the same but for an `image/*` other than `image/png`, a different case, or a type this build doesn't know, whose `type` is `TYPE_UNKNOWN`)".

`crates/gate/src/smoke.rs`, lines 701, 935 and 964: `CLOSEGROUP` becomes `CLOSENODE` in each `format!`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -j 8 -p manifold debug_control`
Expected: PASS (`close_node_takes_any_node`, the SAMPLES parse test and the EXPORTSTATE key test).

- [ ] **Step 5: The gate stage `unknown-type`**

Create `crates/gate/src/unknown_type.rs`:

```rust
//! `gate unknown-type`: a node of a type this build doesn't know, end to end (spec
//! docs/superpowers/specs/2026-10-14-unknown-type-card-design.md, "Testing"). Two headless runs on one fresh profile
//! seeded with a tab, a group and one `application/x-from-the-future` node. The first moves, renames, links,
//! duplicates and groups the unknown card; the second checks that all of it persisted, then closes it and reopens
//! it from Recently Closed.
//!
//! A failing check does not stop the run; a runner error (a timeout, a launch failure) prints an `ERROR:` line, counts
//! as a failure and ends the run early. Instances are killed on every exit path (`Instance`'s Drop plus the job).

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::check::{check, check_eq, exit_code, failures, note_failure, Section};
use crate::instance::{log_tail, seed_settings, Instance};
use crate::link::{bad, no_verdict_words, num};
use crate::state::{State, GROUP, TAB};
use crate::windows_main::{manifold_exe, work_dir};

/// How long an instance may take to exit after QUIT.
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);
/// The seeded node's type: one no build knows.
const FUTURE: &str = "application/x-from-the-future";
/// The row EXPORTSTATE reports it as (`store::model::TYPE_UNKNOWN`).
const SENTINEL: &str = "application/vnd.manifold.unknown";

/// The run's instances and files.
struct Run {
    exe: PathBuf,
    work: PathBuf,
    profile: PathBuf,
}

impl Run {
    /// One instance on the profile (BelowNormal, in the job), its tabs up.
    fn start(&self, log: &str) -> io::Result<Instance> {
        let mut inst = Instance::launch(&self.exe, &self.profile, &self.work, log)?;
        inst.send("WAITTABS")?;
        inst.send("WAITIDLE")?;
        Ok(inst)
    }

    fn stop(mut inst: Instance) -> io::Result<()> {
        inst.quit(QUIT_TIMEOUT).map(|_| ())
    }

    /// The profile's settings and the session built from the seed (`--import-json`, which gives fresh ids).
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, "webview2", "")?;
        let seed = self.profile.join("seed.json");
        let doc = serde_json::json!({"nodes": [
            {"id": "t", "type": TAB, "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]},
            {"id": "g", "type": GROUP, "name": "G", "pos": [300.0, 300.0]},
            {"id": "u", "type": FUTURE, "pos": [400.0, 0.0]},
        ]});
        std::fs::write(&seed, doc.to_string())?;
        let status = Command::new(&self.exe)
            .arg("--import-json")
            .arg(&seed)
            .arg("--out")
            .arg(self.profile.join("session.mnf"))
            .stdout(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("--import-json exited with {status}")))
        }
    }
}

/// The id of the one top-level card whose `type` is `ty`.
fn only(s: &State, ty: &str) -> io::Result<String> {
    let ids: Vec<&str> = s.cards().iter().filter(|c| c["type"] == ty).filter_map(|c| c["id"].as_str()).collect();
    match ids.as_slice() {
        [one] => Ok(one.to_string()),
        _ => Err(bad(format!("expected one {ty} card, found {}", ids.len()))),
    }
}

/// Whether EXPORTSTATE's `links` holds a link between `a` and `b`, either way round.
fn linked(s: &State, a: &str, b: &str) -> bool {
    s.0["links"].as_array().is_some_and(|l| {
        l.iter().any(|k| (k["from"] == a && k["to"] == b) || (k["from"] == b && k["to"] == a))
    })
}

/// The top-level cards of the unknown row named `name`.
fn named(s: &State, name: &str) -> usize {
    s.cards().iter().filter(|c| c["type"] == SENTINEL && c["name"] == name).count()
}

/// The stored nodes of type `FUTURE` named `name`, from an EXPORTSESSION.
fn stored_named(session: &Value, name: &str) -> usize {
    session["nodes"].as_array().map_or(0, |n| n.iter().filter(|n| n["type"] == FUTURE && n["name"] == name).count())
}

fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
    next("run1-edit");
    let mut i = run.start("log1")?;
    let s0 = i.export_until("s0", |s| s.cards().iter().any(|c| c["type"] == SENTINEL))?;
    let (u, t, g) = (only(&s0, SENTINEL)?, only(&s0, TAB)?, only(&s0, GROUP)?);
    let card = s0.card(&u).ok_or_else(|| bad(format!("no card {u}")))?;
    check_eq("the unknown node is a card that keeps its stored type", FUTURE.to_string(), py(&card["stored_type"]));
    i.send(&format!("MOVENODE {u} 500 50"))?;
    let at = |s: &State| {
        let pos = &s.0["mirror_pos"][u.as_str()];
        Some((num(&pos[0]).ok()?, num(&pos[1]).ok()?))
    };
    let s1 = i.export_until("s1", |s| at(s) == Some((500.0, 50.0)))?;
    check("a move writes its pos", at(&s1) == Some((500.0, 50.0)));
    i.send(&format!("RENAMEGROUP {u} Future"))?;
    i.send(&format!("LINK {u} {t}"))?;
    i.send(&format!("DUPLICATENODE {u}"))?;
    i.send("WAITIDLE")?;
    let s2 = i.export_until("s2", |s| named(s, "Future") == 2 && linked(s, &u, &t))?;
    check_eq("a rename names it", "Future".to_string(), py(&s2.card(&u).map_or(Value::Null, |c| c["name"].clone())));
    check("it links to a tab", linked(&s2, &u, &t));
    check_eq("Duplicate makes a second unknown card with its name", 2, named(&s2, "Future"));
    i.send(&format!("MOVETOGROUP {u} {g}"))?;
    let s3 = i.export_until("s3", |s| py(&s.0["mirror_groups"][u.as_str()]) == g)?;
    check_eq("it moves into a group", g.clone(), py(&s3.0["mirror_groups"][u.as_str()]));
    Run::stop(i)?;

    next("run2-restore");
    let mut i = run.start("log2")?;
    let s4 = i.export_state("s4")?;
    check_eq("its group persists", g.clone(), py(&s4.0["mirror_groups"][u.as_str()]));
    check("its link persists", linked(&s4, &u, &t));
    let session = i.export_session("session4")?;
    check_eq("the document keeps its real type and name, the copy's too", 2, stored_named(&session, "Future"));
    i.send(&format!("CLOSENODE {u}"))?;
    let s5 = i.export_until("s5", |s| s.card(&u).is_none())?;
    check("Close removes it", s5.card(&u).is_none());
    i.send("REOPENTAB")?;
    i.send("WAITIDLE")?;
    let s6 = i.export_until("s6", |s| named(s, "Future") == 2)?;
    check_eq("Reopen brings a shell back beside the copy", 2, named(&s6, "Future"));
    let session = i.export_session("session6")?;
    check_eq("both are stored with the real type", 2, stored_named(&session, "Future"));
    Run::stop(i)
}

/// A JSON value as text: a string's own text, anything else as JSON.
fn py(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

pub fn unknown_type(parent: Option<PathBuf>) -> ExitCode {
    let mut sec = Some(Section::start("setup"));
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let work = match work_dir(parent, "unknown-type") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let run = Run { exe, profile: work.join("profile"), work };
    if let Err(e) = run.setup().and_then(|()| drive(&run, &mut sec)) {
        println!("ERROR: unknown-type stopped early: {e}");
        note_failure();
    }
    if failures() > 0 {
        for l in ["log1", "log2"] {
            println!("--- {l}\n{}", no_verdict_words(&log_tail(&run.work.join(l), 20)));
        }
    }
    drop(sec);
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&run.work);
    }
    exit_code()
}
```

Notes for the implementer:
- `check_eq` is generic over `PartialEq + Display` (`check.rs:41`), so the `usize` and `String` calls above both work.
- No fixed sleeps: every send-then-look is `Instance::export_until(name, pred)` (`instance.rs:253`; polls `export_state` every 250 ms until `pred` holds or `STEP_TIMEOUT`, returning the last state either way so the `check` after it reports a state that never arrived). `WAITTABS` and `WAITIDLE` are the app's own waits. Check `num`'s return type in `link.rs`: the `at` closure assumes `io::Result<f64>`; adapt only that closure.
- `link.rs` already exports `bad`, `num` and `no_verdict_words` as `pub(crate)`.
- If `MOVETOGROUP` is refused (stderr "isn't a sibling group"), the seed's group is not a sibling at the top level. Read `log1` and report; do not change the verb.
- `state::State.0` is the raw EXPORTSTATE value, which is how `link.rs` reads `links`.

`crates/gate/src/main.rs`:

- After `mod two;` (line 58), add:

```rust
#[cfg(windows)]
mod unknown_type;
```

- After `"two" => crate::two::two(parent),` (line 111), add `"unknown-type" => crate::unknown_type::unknown_type(parent),`.
- In the usage string (127), `gate hello|launch-forms|unresponsive|layout|link|smoke|two` becomes `gate hello|launch-forms|unresponsive|layout|link|smoke|two|unknown-type`.
- In the module doc (6-8), after the `link` entry, add "`unknown-type` (a node of a type this build doesn't know, end to end, see unknown_type.rs),".

`crates/gate/src/all.rs`:

- After line 218 `g.stage("link", ..)`, add `g.stage("unknown-type", &sibling(&log, "unknown-type.log"), gate("unknown-type"));`.
- The module doc:
  - Line 5: `full` lists "`gate two` x3, `smoke`, `launch-forms`, `link`, `unknown-type`, `layout`".
  - Line 9: `ui` reads "then `link`, `unknown-type` and `layout`".
  - Line 14: the log list gains `.unknown-type.log` after `.link.log`.

- [ ] **Step 6: Docs**

`README.md`:

- After the `MOVETOGROUP` bullet (387), add:

  `- `CLOSENODE <id>` — closes node `id` (any kind) as its card's Close or Delete does, recording it in Recently Closed, and saves; an unknown id is ignored with a note on stderr. (It replaced `CLOSEGROUP`.)`

- In the paragraph that describes EXPORTSTATE's `cards`, wherever `type` is listed, add: "`stored_type` is the type string as stored. It equals `type` except for an `image/*` other than `image/png`, a different case, or a type this build doesn't know, whose `type` is `application/vnd.manifold.unknown`."
- The `gate all` paragraph (405):
  - "`gate smoke`, `gate launch-forms`, `gate link` and `gate layout` once each" becomes "`gate smoke`, `gate launch-forms`, `gate link`, `gate unknown-type` and `gate layout` once each".
  - The `ui` sentence's "`gate link` and `gate layout`" becomes "`gate link`, `gate unknown-type` and `gate layout`".
  - The stage log list gains `LOG.unknown-type.log` after `LOG.link.log`.
- After the `gate link` paragraph (423), add:

  ``gate unknown-type` (`crates/gate/src/unknown_type.rs`) checks the unknown-type card end to end (about 30 s; build first). Two headless runs at BelowNormal share a throwaway profile seeded with a tab, a group and one node of type `application/x-from-the-future`. The first run finds the card (EXPORTSTATE `type` `application/vnd.manifold.unknown`, `stored_type` the real string), moves it (`MOVENODE`, `mirror_pos`), renames it, links it to the tab, duplicates it and moves it into the group. The second run checks that the group, the link and the stored type and name of both cards persisted, then closes the card (`CLOSENODE`) and reopens it (`REOPENTAB`): a shell of the type comes back beside the copy.`

- Before the "**Pin on every card (2026-10-12, homogeneity).**" entry (710), add:

  `- **The unknown-type card (2026-10-14, homogeneity).** A node whose type this build doesn't know (a newer client's, or a retired one such as the old JSON leaf) is now a live card instead of a hidden node. It shows a question mark and its type string, or its stored name with the type under it, and it carries the generic fields only (pos, color, name, opened-from, group); every other field stays raw in the document, untouched by a move, rename or regroup. It links and sits in groups, but opens nothing and takes no jar. Duplicate, Reopen and JSON import make a shell of its type (the type string and generic fields, never the raw extras). One function, `card_kind::canonical_type`, maps every stored type to its caps row, with the sentinel row `application/vnd.manifold.unknown` (restorable, drawn with notes) in place of the old `*/*` row. `CLOSENODE` replaced `CLOSEGROUP`, EXPORTSTATE cards gained `stored_type`, and `gate unknown-type` checks it end to end. Spec `docs/superpowers/specs/2026-10-14-unknown-type-card-design.md`, plan `docs/superpowers/plans/2026-10-14-unknown-type-card.md`.`

- Leave the historical entry at 712 ("JSON leaf retired ... loads as a `Kind::Unknown` node nothing shows") as it is: it records what that change did then. The new entry says what holds now.

`docs/superpowers/specs/2026-10-04-node-model-design.md:75-76`: append to the `*/*` row's line: " (Superseded 2026-10-14: the unknown-type card replaced the `*/*` row with the sentinel `application/vnd.manifold.unknown`, restorable, layer 3; see `2026-10-14-unknown-type-card-design.md` decision 7.)"

`docs/superpowers/specs/2026-10-04-homogeneity-decisions.md:39`: item 5 becomes:

`5. **The unknown-type card.** Designed in `2026-10-14-unknown-type-card-design.md`. Done (built 2026-10-14, `plans/2026-10-14-unknown-type-card.md`): a node of a type this build doesn't know is a live card with the generic fields, shell copies, links and groups.`

`docs/superpowers/specs/2026-10-14-unknown-type-card-design.md:3`: `Status: design, approved...` becomes `Status: built 2026-10-14 (plan `../plans/2026-10-14-unknown-type-card.md`; deviations D1-D8 there). Design approved in outline by the user on 2026-10-14 ("go" to: live card; Close and Reopen work; links allowed).`, followed by the rest of the paragraph unchanged.

- [ ] **Step 7: The full check**

```bash
git grep -nE 'UNKNOWN_TYPE|no unknown-type card yet|no menu of its own yet|CLOSEGROUP|CloseGroup|kept, never shown|node\.kind, |root\.kind|member\.kind|snap\.kind' -- crates
git grep -n '"\*/\*"' -- crates/manifold/src/caps/rows.rs crates/manifold/src/card_kind.rs
cargo fmt --all
cargo fmt --all --check
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo build -j 8 -p manifold -p gate
cargo test -j 8 -p manifold -p gate
```

(`default-members` is `crates/manifold` only, so a bare `cargo build`/`cargo test` skips the gate crate; the gate run in "Closing step" also needs `-p manifold-view-webview2 -p manifold-view-pdfium`.)

Expected:
- The first grep has no hits. The second hits only `rows.rs`'s `mod tests`.
- fmt is clean, and clippy passes with no warnings.
- `cargo test -j 8 -p manifold -p gate` passes, including the gate crate's tests.
- `target/debug/gate.exe tier` should classify the new `unknown_type.rs` (advice only; if it says nothing or the wrong tier, report it, do not edit `tier`).
- Behaviour change to note in the commit body and the README entry: `canonical_type("")` is "" (no row), so `worth_reopening("")` stays false; it used to hit the `*/*` row.
- The new tests that must appear in the output:
  - Task 1:
    - `an_unknown_kind_carries_the_generic_fields`
    - `a_create_writes_the_type_string_it_is_given`
    - `a_set_on_an_unknown_node_leaves_its_raw_fields_alone`
    - `import_keeps_an_unknown_type_with_the_generic_fields_only`
    - `import_rejects_a_missing_or_empty_type`
    - `materialize_keeps_the_stored_type_string`
  - Task 2:
    - `canonical_type_maps_known_kinds_to_their_row_and_the_rest_to_the_sentinel`
    - `an_unknown_stored_type_reads_as_the_unknown_row`
    - `the_unknown_row_links_and_sits_in_a_group_but_opens_nothing_and_takes_no_jar`
    - `an_unknown_type_is_worth_reopening_as_a_shell`
  - Task 3:
    - `nodes_show_at_their_nearest_live_group_never_inside_another_card`
    - `an_unknown_type_is_a_card_from_a_restore_and_from_a_remote_create`
    - `moving_an_unknown_card_writes_one_set_and_keeps_its_raw_fields`
    - `a_remote_delete_removes_an_unknown_card`
    - `a_typeless_placeholder_gets_its_card_when_its_type_arrives`
    - `an_unknown_card_copies_as_a_shell_of_its_type`
    - `a_link_and_a_group_take_an_unknown_card`
  - Task 4: `close_node_takes_any_node`.

If the workspace test run is too slow under load, run it detached at BelowNormal (LONG_RUNS.md) and wait on its exit.

- [ ] **Step 8: Commit**

```bash
git add -A crates README.md docs/superpowers/specs docs/superpowers/plans/2026-10-14-unknown-type-card.md
git commit -m "Gate unknown-type; CLOSENODE replaces CLOSEGROUP; EXPORTSTATE stored_type; docs: the unknown-type card is built

canonical_type(\"\") is \"\" (no row); worth_reopening(\"\") stays false.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Closing step (the controller, after Task 4)

The controller runs these. They are not an implementer's steps, because another session may hold the target lock.

1. `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`. `-p manifold` alone leaves the WebView2 host and the PDF view stale.
2. Run the gate detached at BelowNormal from the repo root. The new stage's log is `LOG.unknown-type.log`.

   ```powershell
   Start-Process -FilePath target\debug\gate.exe -ArgumentList 'all','gate-unknown-type.log','full' -WindowStyle Hidden -PassThru | ForEach-Object { $_.PriorityClass = 'BelowNormal'; $_.Id }
   ```

3. Wait on the log's `GATE done` line or the pid's exit (no fixed sleeps). Then report:
   - `Get-Process manifold, gate -EA 0`, to show that none is left.
   - `(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue`
   - `(Get-MpComputerStatus).RealTimeProtectionEnabled`

   Report them with the verdict counts.

---

## Self-review log

Four rounds:

1. **While writing**, each cited site was checked against the code at `8b90691`:
   - The model, ops, json, session_sync, closed, card_kind, caps, card, node, popups, debug_control and gate files.
   - Who calls `node_json` and `OpBody::create`.
   - Every `Snapshot {` and `Imported {` literal, and every `Some((Kind::` tuple.
   - The `.kind` field reads.
   - Every exhaustive `Runtime` match (`node.rs`, `reveal.rs`, `graph_nav.rs`, `card.rs`, `session_sync.rs`).

   Caught:
   - `remote_in_flight` holds `RemoteNode` (string group), not `RemoteChange`.
   - `land_remote_in_flight` and the destructures at 804/1364/1374 are positional, so they survive the `String` swap unchanged.
   - `tab_fields` returns the kind, so it must return the type string.
   - `close_node_and_save` already saves for every kind, so `CLOSENODE` needs nothing else.
   - The jars plan's co-author line differs from this task's (this plan uses Sonnet 5.5, per the session's attribution).
2. **Spec coverage**, pass over every decision, Behaviour bullet, touch point and Testing bullet.

   Caught:
   - An empty type string would have been live under `has_type()` (D8 added; `canonical_type("")` returns "" so `worth_reopening("")` stays false).
   - `effective_group` needed a non-group live parent case, so the model test gains id 10 (a note in a tab).
   - `named_by_name` (spec decision 6) would make an empty rename a no-op and hide the type from the screen-reader label: kept unchanged (D4), and the test asserts "Card: Old" and "Card: Mine".
   - The spec's `generic_fields()` duplicated what `fields()` already routes everywhere (D1).
   - EXPORTSTATE `type` must stay canonical for the existing gate checks (D6).
   - The README's historical leaf entry is history, not current behaviour, so it stays and a new entry is added.
3. **Ordering and green builds.**
   - Flipping `is_live` before `Runtime::Unknown` exists would trip `check_graph_matches_document` ("live in the document but not shown") in every sync test that sees an unknown node. So Task 1 keeps `snapshot`'s early return and the old `is_live`, and Task 3 flips both together with the card.
   - `TYPE_UNKNOWN` is defined in Task 2, where it is first used, not Task 1: an unused `pub const` in the binary crate warns under `-D warnings`.
   - `card_kind_reads_the_type_and_tags_loaded_for_every_constructible_payload` needs a seeded type for its new `Unknown` row, because `place_node` seeds none. The row's test now seeds one.
   - The `kinds()` fixture grows to 10, and the four `card/tests.rs` and `node_menu/tests.rs` loops over it were checked against the `_` menu arm (Delete hinted Del, verb "delete") and `del_verb`.
4. **Types, names and placeholders.**
   - Every produced name was checked across tasks: `GENERIC`, `Node::ty`, `Node::is_group`, `Snapshot::of`/`kind`, `Imported::kind`, `canonical_type`, `TYPE_UNKNOWN`, `Runtime::Unknown`, `UNKNOWN_NODE_SIZE`, `paint_unknown_body`, `CloseNode`, `debug_close_node`, `stored_type` and `unknown_type`.
   - The `restore_session` rewrite was checked against the borrow of `doc` (the `Snapshot`s are owned, so `nodes` outlives the `journal` borrow exactly as the old tuples did).
   - Caught: the copy test needed `doc_fields`, and the JSON export check in Task 1 had to use `doc_to_json_all`, since the unknown node is not live until Task 3.
   - No "TBD" or "similar to" steps remain. The remaining conditionals (`num`'s return type in the gate stage) name what to adapt.
