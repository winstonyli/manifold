# Node model, plan 2a: the type string replaces the numeric kind

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A node's type is an ordinary string field (`type`, a MIME type) in the journal, the snapshot and the JSON export; `Create` loses its `kind` slot and the numeric `KINDS` table goes. The app's live model and the `Kind` enum stay as they are.

**Architecture:** `store::model::Kind` stays as the app-facing enum, now derived from the node's `type` register (`Node::kind()`), with one table of type strings (`Kind::type_str` / `Kind::from_type`) replacing `KINDS` and the numeric codes. `OpBody::create(node, kind, fields)` appends the type string to the node message. JSON export and import carry `type` like any other field. Unknown types stay hidden in this plan (see Rulings); plan 2c shows them as generic cards.

**Tech Stack:** Rust (egui app, crate `manifold`), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-04-node-model-design.md` (section 6 items 1 and 9; "Rename plan and order", plan 2a). Plan 1 is `docs/superpowers/plans/2026-10-04-node-model-1-tables.md` (done).

## Global Constraints

- Work on `dev`, never `main`. No new worktrees. Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` (your own model's name if different). Do not push, merge or tag.
- `cargo fmt --all` before each commit. `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass at every commit; `.githooks/pre-commit` runs both when Rust is staged. Never `--no-verify`. No `#[allow(dead_code)]`; fix or justify a lint with a comment.
- Always `-j 8`. Tests: `cargo test -j 8 -p manifold --bins` (baseline 716 passed, 4 ignored) and `cargo test -j 8 -p ipc-protocol` (43 passed). C: is nearly full: no extra target dirs.
- No test may read or write the real `%APPDATA%\manifold`; use `store::testutil::temp_dir`.
- Behavior-neutral except where the Rulings say otherwise. Do not edit an existing assertion to make it pass; an assertion changes only where its subject changed (a kind code, a `kind` JSON key), and the diff shows why.
- Edit scripts go in the session scratchpad, never `%TEMP%`. Python runs as `python -P script < /dev/null`.

## Rulings (spec gaps decided here)

- **Type strings** (one table, in `store/model.rs`): Tab `application/vnd.manifold.tab`; Group `.group`; Jar `.jar`; JsonLeaf `application/vnd.manifold.json-leaf`; Note `text/markdown`; Pdf `application/pdf`; Json `application/json`; Image `image/png`; Download `application/octet-stream`. `Kind::from_type` maps **any** `image/*` to Image and compares the rest case-insensitively. Until 2c an image's subtype is not kept: a created Image is stored as `image/png`, and nothing reads the difference.
- **`type` is field 19**, `FieldType::Str`, not `explicit`, in the `FIELDS` table. It is not in `kind_fields`, so `Shared.fields` and `diff` never touch it after create: a node's type does not change by sync.
- **Unknown types stay hidden.** `Node::is_live` is still `!deleted && kind() != Kind::Unknown`; the spec's behavior change (item 9: unknown types shown as a generic card) needs the generic card and lands in 2c. "Created" is now "has a `type` register": `Node::placeholder()` has none, and `prune` drops a typeless placeholder but keeps a node whose type string this build does not know, as it kept `Kind::Unknown(40)`.
- **`Kind::Unknown` is a unit variant** (it carried the numeric code). `Kind::type_str(self) -> Option<&'static str>` is `None` for it.
- **`FORMAT_VERSION` becomes 2.** A version-1 file would read as nodes with no type and be compacted into an empty session. `load` refuses a file with `header.version < FORMAT_VERSION` as `JournalError::NotAJournal`; its callers already handle that as an unreadable file (no migration, per the spec's decision). Task 2 Step 8 verifies what the callers do and stops if they would overwrite the file in place.
- **JSON key**: the node's `kind` member is replaced by the ordinary field `type`. `JsonNode` loses its `kind: String` member; `JsonNode::kind(&self) -> Option<Kind>` reads it. `LocalState.closed` entries saved before this change no longer reopen (no migration).
- The MIME constants plan 1 left in `caps/bridge.rs` move to `store/model.rs` as `TYPE_*` (plan 1 carry-over 1); `Kind::type_str` and the caps rows share them.

## File structure

- Modify `crates/manifold/src/store/model.rs`: `field::TYPE`, `TYPE_*` constants, `Kind` table, `Node::kind()`, `Node::placeholder`, test constructors.
- Modify `crates/manifold/src/store/ops.rs`: `OpBody::Create` without `kind`, `OpBody::create`, `prune`.
- Modify `crates/manifold/src/store/journal.rs`: snapshot node and `Create` encodings, `FORMAT_VERSION`, `load`.
- Modify `crates/manifold/src/store/json.rs`: `type` as a field.
- Modify `crates/manifold/src/session_sync.rs`, `session_sync/tests.rs`, `closed.rs`, `jar_defs.rs`, `popups.rs`, `cli.rs`, `caps/bridge.rs`, `caps/rows.rs`, `caps/edges.rs`.
- Modify `scripts/*` and docs that spell `"kind"` in a session JSON (Task 3).

---

### Task 1: `Node::kind()` as a method

Pure refactor: every reader goes through a method, so Task 2 changes one place.

**Files:**
- Modify: `crates/manifold/src/store/model.rs` (`impl Node`), and every reader of a store `Node`'s `kind` field outside `model.rs`: `store/ops.rs:158`, `store/journal.rs:436-437`, `store/json.rs:78,244,282`, `jar_defs.rs:467`, `session_sync.rs:86,165,778,1048,1080`, `session_sync/tests.rs:92,1272,1703,1845,1944`. (`Shared.kind`, `Imported.kind` and `JsonNode.kind` are different fields: leave them.)

**Interfaces:**
- Produces: `Node::kind(&self) -> Kind` (a copy of the field for now).

- [ ] **Step 1: Add the method and use it**

In `impl Node` (`store/model.rs`):

```rust
    /// What kind of node this is.
    pub fn kind(&self) -> Kind {
        self.kind
    }
```

Replace each reader of a store `Node`'s `.kind` field (list above; find the rest with `grep -rn "\.kind\b" crates/manifold/src` and judge each receiver's type) with `.kind()`. Do not change constructors (`Node { kind: ..., }`) or `entry.kind = ...` writes yet.

- [ ] **Step 2: Gate and commit**

Run `cargo fmt --all`, the clippy command, `cargo test -j 8 -p manifold --bins` (716 passed). Commit: `Node::kind() method`.

---

### Task 2: The type string in the store, the journal and the JSON

One commit: the crate does not compile half-switched.

**Files:** see "File structure". Start with `store/model.rs`, then `ops.rs`, `journal.rs`, `json.rs`, then the compile errors lead through `session_sync.rs`, `popups.rs`, `closed.rs`, `caps/*`, `jar_defs.rs`, `cli.rs` and the tests.

**Interfaces:**
- Produces: `field::TYPE: u32 = 19`; `store::model::{TYPE_TAB, TYPE_GROUP, TYPE_JAR, TYPE_JSON_LEAF, TYPE_NOTE, TYPE_PDF, TYPE_JSON, TYPE_IMAGE, TYPE_DOWNLOAD}: &str`; `Kind::type_str(self) -> Option<&'static str>`; `Kind::from_type(&str) -> Kind`; `Kind::Unknown` (unit); `Node::kind(&self) -> Kind` read from the register; `OpBody::create(node: NodeId, kind: Kind, fields: Vec<u8>) -> OpBody`; `OpBody::Create { node, fields }`; `JsonNode::kind(&self) -> Option<Kind>`.
- Removes: `KINDS`, `Kind::code`, `Kind::from_code`, `Kind::name`, `Kind::from_name`, the `kind` member of `Node`, `OpBody::Create`'s `kind`, `JsonNode`'s `kind` member.

- [ ] **Step 1: Tests first (store/model.rs, replacing `kinds_have_codes_and_names` and `jar_is_kind_nine_and_live`)**

```rust
    #[test]
    fn every_known_kind_has_a_type_string_that_maps_back() {
        for kind in [
            Kind::Tab, Kind::Group, Kind::Note, Kind::Image, Kind::Pdf, Kind::Json, Kind::JsonLeaf, Kind::Download,
            Kind::Jar,
        ] {
            let ty = kind.type_str().expect("a known kind has a type");
            assert_eq!(Kind::from_type(ty), kind, "{ty}");
        }
        assert_eq!(Kind::Unknown.type_str(), None);
    }

    #[test]
    fn from_type_is_case_insensitive_takes_any_image_and_rejects_the_rest() {
        assert_eq!(Kind::from_type("Application/VND.manifold.TAB"), Kind::Tab);
        assert_eq!(Kind::from_type("image/jpeg"), Kind::Image);
        assert_eq!(Kind::from_type("IMAGE/webp"), Kind::Image);
        assert_eq!(Kind::from_type("video/mp4"), Kind::Unknown);
        assert_eq!(Kind::from_type(""), Kind::Unknown);
    }

    #[test]
    fn a_node_reads_its_kind_from_the_last_type_write() {
        let mut node = Node::placeholder();
        assert_eq!(node.kind(), Kind::Unknown, "no type yet");
        assert!(!node.has_type() && !node.is_live());
        node.fields.insert(field::TYPE, reg(1, "application/vnd.manifold.jar"));
        assert_eq!((node.kind(), node.has_type(), node.is_live()), (Kind::Jar, true, true));
        node.fields.insert(field::TYPE, reg(2, "application/x-from-the-future"));
        assert_eq!((node.kind(), node.has_type(), node.is_live()), (Kind::Unknown, true, false));
    }
```

with a test helper `fn reg(counter: u64, ty: &str) -> Reg` building `Reg { stamp, enc: encode_field(field::TYPE, &FieldValue::Str(ty.into())) }` (copy how other tests in this file build a `Stamp`), and a `#[cfg(test)] impl Node { pub fn typed(kind: Kind) -> Node; pub fn with_type(ty: &str) -> Node }` constructing a live, groupless node whose `TYPE` register holds the given string with a zero stamp. Adapt the existing `doc.nodes.insert(id(n), node(Kind::X, group, deleted))` test around line 634 to build nodes with `Node::typed` (set `group` and `deleted` after), and its "a newer build's group" node (`Kind::Unknown(40)`) with `Node::with_type("application/x-future")`.

- [ ] **Step 2: Run to see them fail** (compile errors are the expected failure): `cargo test -j 8 -p manifold --bins store::model`.

- [ ] **Step 3: Implement `store/model.rs`**

```rust
pub const TYPE_TAB: &str = "application/vnd.manifold.tab";
pub const TYPE_GROUP: &str = "application/vnd.manifold.group";
pub const TYPE_JAR: &str = "application/vnd.manifold.jar";
pub const TYPE_JSON_LEAF: &str = "application/vnd.manifold.json-leaf";
pub const TYPE_NOTE: &str = "text/markdown";
pub const TYPE_PDF: &str = "application/pdf";
pub const TYPE_JSON: &str = "application/json";
/// What a created image is stored as until sniffed types arrive (plan 2c); any `image/*` reads back as an image.
pub const TYPE_IMAGE: &str = "image/png";
pub const TYPE_DOWNLOAD: &str = "application/octet-stream";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Tab, Group, Note, Image, Pdf, Json, JsonLeaf, Download, Jar,
    /// No `type` yet (an op named the node before its `Create` arrived), or a type a newer build wrote
    /// (kept, never shown).
    Unknown,
}

const TYPES: [(Kind, &str); 9] = [
    (Kind::Tab, TYPE_TAB), (Kind::Group, TYPE_GROUP), (Kind::Note, TYPE_NOTE), (Kind::Image, TYPE_IMAGE),
    (Kind::Pdf, TYPE_PDF), (Kind::Json, TYPE_JSON), (Kind::JsonLeaf, TYPE_JSON_LEAF),
    (Kind::Download, TYPE_DOWNLOAD), (Kind::Jar, TYPE_JAR),
];

impl Kind {
    /// The type string a created node of this kind is stored under.
    pub fn type_str(self) -> Option<&'static str> {
        TYPES.iter().find(|t| t.0 == self).map(|t| t.1)
    }

    /// The kind a stored type string means; any `image/*` is an image.
    pub fn from_type(ty: &str) -> Kind {
        if ty.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("image/")) {
            return Kind::Image;
        }
        TYPES.iter().find(|t| t.1.eq_ignore_ascii_case(ty)).map_or(Kind::Unknown, |t| t.0)
    }
}
```

Add `pub const TYPE: u32 = 19;` to `mod field` (doc: "A node's MIME type, written by `Create`."), `FieldDef { no: field::TYPE, name: "type", ty: FieldType::Str, explicit: false }` to `FIELDS` (array length 19), and change the `field::JAR` doc's `Kind::Jar` mention if it names the code. Remove `KINDS`, `code`, `from_code`, `name`, `from_name`. In `Node`: delete the `kind` field; `placeholder()` is `Node { fields: BTreeMap::new(), group: None, deleted: false }`; and

```rust
    /// Created: an op wrote its `type`.
    pub fn has_type(&self) -> bool {
        self.fields.contains_key(&field::TYPE)
    }

    pub fn kind(&self) -> Kind {
        let Some(reg) = self.fields.get(&field::TYPE) else { return Kind::Unknown };
        let mut ty = "";
        for f in Reader::new(&reg.enc).map_while(Result::ok) {
            if let Ok(s) = f.value.str() {
                ty = s; // the last occurrence wins, as `decode_field` reads it
            }
        }
        Kind::from_type(ty)
    }

    pub fn is_live(&self) -> bool {
        !self.deleted && self.kind() != Kind::Unknown
    }
```

(`Node::message` under `cfg(test)` needs no change: `type` is an ordinary register.)

- [ ] **Step 4: Ops tests, then `store/ops.rs`**

Replace the test helper

```rust
    fn create(node: NodeId, kind: Kind, group: Option<NodeId>) -> OpBody {
        OpBody::create(node, kind, encode_field(field::GROUP, &FieldValue::Id(group)))
    }
```

and add

```rust
    #[test]
    fn a_create_carries_its_type_and_a_set_before_it_shows_once_it_arrives() {
        let t = id(1, 0);
        let mut doc = Doc::new(DocId([0; 16]));
        doc.apply(&op(1, 1, 2, OpBody::Set { node: t, field: field::NAME, enc: encode_field(field::NAME, &FieldValue::Str("x".into())) }));
        assert!(!doc.nodes[&t].is_live(), "named by an op, not created");
        doc.apply(&op(1, 0, 1, create(t, Kind::Tab, None)));
        assert_eq!(doc.nodes[&t].kind(), Kind::Tab);
        assert!(doc.nodes[&t].is_live());
    }

    #[test]
    fn prune_drops_a_typeless_placeholder_but_keeps_a_type_this_build_does_not_know() {
        let (ghost, future) = (id(1, 0), id(1, 1));
        let mut doc = Doc::new(DocId([0; 16]));
        doc.apply(&op(1, 2, 1, OpBody::Set { node: ghost, field: field::NAME, enc: encode_field(field::NAME, &FieldValue::Str("g".into())) }));
        let mut fields = encode_field(field::TYPE, &FieldValue::Str("application/x-from-the-future".into()));
        fields.extend(encode_field(field::GROUP, &FieldValue::Id(None)));
        doc.apply(&op(1, 3, 1, OpBody::Create { node: future, fields }));
        doc.forget_moves();
        doc.prune();
        assert!(!doc.nodes.contains_key(&ghost));
        assert!(doc.nodes.contains_key(&future));
    }
```

(`id`, `op` are the existing helpers in this test module; adjust counters if they collide.) Implement: `OpBody::Create { node: NodeId, fields: Vec<u8> }` (doc: "`fields` is a node message (`Node::message`), `group` and `type` included."); in `apply`, drop `entry.kind = *kind;` and the `kind` binding; `prune`'s retain becomes `named.contains(id) || !(n.deleted || !n.has_type())`; add

```rust
impl OpBody {
    /// A `Create` whose node message carries `kind`'s type string after `fields`.
    pub fn create(node: NodeId, kind: Kind, mut fields: Vec<u8>) -> OpBody {
        let ty = kind.type_str().expect("a created node has a known kind");
        fields.extend(encode_field(field::TYPE, &FieldValue::Str(ty.to_owned())));
        OpBody::Create { node, fields }
    }
}
```

Every other test or production `OpBody::Create { node, kind, fields }` that names a kind (`grep -rn "OpBody::Create" crates`) becomes `OpBody::create(node, kind, fields)`; a pattern match on `Create { node, kind, fields }` drops `kind`.

- [ ] **Step 5: Journal tests, then `store/journal.rs`**

Add tests next to the existing round-trip tests: (a) a document holding a node whose `type` is `application/x-from-the-future` snapshots and re-reads equal, and compacting it keeps the node's raw `type` register; (b) a `Create` op frame round-trips through `ops_message`/`decode_ops` with its fields, with no `kind` slot (compare against `OpBody::create(..)`); (c) a file with `header.version == 1` is refused: build a valid journal file, rewrite its header version varint to 1 (copy how `a_newer_format_version_opens_read_only` builds its file), and assert `load`/`open` returns `JournalError::NotAJournal`.

Implement: `FORMAT_VERSION: u64 = 2`; in `load` (after `read_header`) `if header.version < FORMAT_VERSION { return Err(JournalError::NotAJournal); }`; snapshot node encoding drops field 2 (`kind`): the doc comment becomes `A node is {1: id, 3: deleted, 4: group, 5: register*}` (keep field numbers; 2 is retired, write nothing for it), delete the `node.kind.code()` write and the `2 => node.kind = ...` decode arm (leave the number unused; unknown fields are skipped by the `_ => {}` arm); `Create` encodes `{1: node, 3: fields}` (drop `b.varint(2, kind.code())`; update the op doc comment) and decodes without `num` for `OP_CREATE`: keep `(OP_SET, 2) => num` and make the `OP_CREATE` arm `OpBody::Create { node, fields: blob }`. Update `Node::placeholder()` users and every `Kind::from_code` call.

- [ ] **Step 6: JSON tests, then `store/json.rs`**

Update the module doc example to `{"nodes": [{"id": "…", "type": "application/vnd.manifold.tab", "url": "…", "pos": [x, y]}, …], …}`. Rewrite the literals in this file's tests (`"kind": "tab"` and so on) as `"type": "<string>"` with the table in the Rulings (`group`→`application/vnd.manifold.group`, `note`→`text/markdown`, `tab`→`application/vnd.manifold.tab`, `jar`→`application/vnd.manifold.jar`, `download`→`application/octet-stream`), and `bad(r#"... "kind": "spaceship" ..."#)` as `"type": "application/x-spaceship"`. Add:

```rust
    #[test]
    fn export_writes_type_as_a_field_and_import_reads_it_back() {
        // build a doc with a group and a tab (reuse this module's existing fixture), export, import:
        // assert export.nodes[i].kind() == Some(Kind::Group) / Some(Kind::Tab), that the serialized JSON has a
        // "type" key and no "kind" key, and that import's `Imported.kind` and `creates()` round-trip the type.
    }

    #[test]
    fn import_rejects_a_missing_or_unknown_type() {
        bad(r#"{"nodes": [{"id": "a"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "type": "application/x-spaceship"}]}"#);
    }
```

(fill the first test's body from the module's existing fixture; it must assert the three things in its comment). Implement: `JsonNode { id, #[serde(flatten)] fields }` (delete `kind: String`); `node_json` inserts `"type"` (via the normal `put(field::TYPE, ..)` path when `fields` holds it, otherwise `out.insert("type", kind.type_str()...)`: callers of `node_json` pass a field map without `TYPE` and a `kind`, so insert it explicitly: `out.insert("type".into(), Value::String(kind.type_str().expect("a known kind").into()))`); `node_to_json` must not also emit a stale duplicate: skip `field::TYPE` in the field loop (`no != field::GROUP && no != field::TYPE`) and in `node_to_json`'s key filter; `impl JsonNode { pub fn kind(&self) -> Option<Kind> }` returning `self.fields.get("type").and_then(Value::as_str).map(Kind::from_type).filter(|k| *k != Kind::Unknown)`; `import` takes `kind` from `node.kind()` (`ok_or_else(|| format!("node {:?}: missing or unknown type", node.id))`), and when it parses fields skips `field::TYPE` (the kind is carried separately); `creates()` becomes `OpBody::create(node.id, node.kind, fields)`.

- [ ] **Step 7: Everything else**

`cargo clippy -j 8 -p manifold --all-targets -- -D warnings` leads through the remaining sites; the rules:
- `session_sync.rs`: `ops.push(OpBody::create(id, node.kind, fields))` at the create site; `kind_fields`' `Kind::Unknown(_)` arm is `Kind::Unknown`; `fields_of(node)` uses `node.kind()`; do not add `TYPE` to any `kind_fields` list.
- `caps/bridge.rs`: `store_node`'s kind arms use `Kind::type_str` for known kinds (`kind.type_str().unwrap_or("application/x-manifold-unknown")`, keeping the blank-note tag logic) and the file's `TAB/GROUP/JSON_LEAF/TEXT_MARKDOWN` constants are deleted; `caps/rows.rs` and `caps/edges.rs` import `TYPE_TAB`, `TYPE_GROUP`, `TYPE_JSON_LEAF`, `TYPE_NOTE` from `store::model`; the bridge's `Kind::Unknown(7)` test uses `Kind::Unknown`. `item_node` keeps its other literals but uses the `TYPE_*` constants for Tab, Group, Note, Pdf, Json, Download.
- `closed.rs`: `Kind::Unknown(99)` becomes `Kind::Unknown`; the assertion `entry.doc.nodes[0].kind == "group"` becomes `entry.doc.nodes[0].kind() == Some(Kind::Group)`.
- `popups.rs:596-610,1573`: `node.kind == "group"` and so on become `node.kind() == Some(Kind::Group)` / `Some(Kind::Note)` / `Some(Kind::Tab)`; the `match node.kind.as_str()` at 610 becomes a `match node.kind()` over the same arms.
- `session_sync/tests.rs:954`: `json.nodes[0].kind == "group"` becomes `json.nodes[0].kind() == Some(Kind::Group)`.
- `cli.rs:98`: the `--import-json` test literal uses `"type"` and the table's strings.
- A `Reader` import in `model.rs` if it is not already in scope.

- [ ] **Step 8: Check what the callers do with `NotAJournal`**

Run `grep -rn "NotAJournal" crates/manifold/src` and read each non-test caller (`Journal::open`, `session_sync` startup, `cli.rs`). Expected: an unreadable file is moved aside or reported and the session starts fresh. If any path would overwrite the old file in place or crash, **stop and report** (status BLOCKED) with what you found; do not improvise. Otherwise add the finding (which callers, what they do) to your report.

- [ ] **Step 9: Gate and commit**

`cargo fmt --all`; clippy (all three crates, `-D warnings`); `cargo test -j 8 -p manifold --bins` (716 plus the new tests, 4 ignored; no old test removed except the two replaced in Step 1); `cargo test -j 8 -p ipc-protocol` (43). Commit: `The type string replaces the numeric node kind`.

---

### Task 3: Scripts, docs and the spec

**Files:**
- Modify: the scripts and docs that spell `"kind"` in a session JSON: `grep -rlE '"kind"' scripts README.md docs --include=* | grep -v docs/superpowers/plans`, expected `scripts/chrome-shots.sh`, `headless-smoke-test.sh`, `jar-probe.sh`, `json-level-probe.py`, `page-kind-probe.sh`, `scale-test.sh`, `session_asserts.py`, `sleep-probe.sh`, `two-instance-test.sh`. Judge each hit: only a session or group JSON node's `"kind": "<name>"` member changes; other `kind` words (engine kind, `JsonCard`, page kind) do not. Old plans under `docs/superpowers/plans/` are history: leave them.
- Modify: `docs/superpowers/specs/2026-10-04-node-model-design.md`.

- [ ] **Step 1: Rewrite the session-JSON spellings**

`"kind": "tab"` becomes `"type": "application/vnd.manifold.tab"`, and likewise `group`→`application/vnd.manifold.group`, `note`→`text/markdown`, `image`→`image/png`, `pdf`→`application/pdf`, `json`→`application/json`, `json_leaf`→`application/vnd.manifold.json-leaf`, `download`→`application/octet-stream`, `jar`→`application/vnd.manifold.jar`. Python readers (`session_asserts.py`, `json-level-probe.py`) that read `node["kind"]` read `node["type"]` and compare against the same strings. Verify with `grep -rnE '"kind"' scripts` (only non-session uses remain) and `bash -n` / `python -m py_compile` on each edited script. Run `scripts/headless-smoke-test.sh` only if the memory note "Build before probes" is satisfied (rebuild the exe first); otherwise report it as not run.

- [ ] **Step 2: Spec**

In section 6 item 1 append `Done in plan 2a: …` with the Rulings' key points (type is field 19; Image stored as `image/png` and Download as `application/octet-stream` until 2c; `FORMAT_VERSION` 2 refuses version-1 files). In item 9 add `The create-without-kind half is done (2a); showing unknown types is 2c.` Under "Rename plan and order" add `Plan 2a done: <commit hashes>.` and delete carry-over 1 (constants moved). Commit both docs and scripts: `Scripts and docs spell the node type`.

- [ ] **Step 3: Final gate**: fmt check, clippy, both test suites, as in Global Constraints.

## Self-review

- **Spec coverage (2a scope):** item 1 (type string in the journal and the JSON, numeric table gone) in Task 2; item 9's `Create` without a `kind` slot in Task 2 (unknown types shown: deferred to 2c, in Rulings); fixtures that spell the JSON in Task 3. Items 2-8 and 10 belong to 2b and 2c.
- **Placeholders:** the two body fills left to the executor (the JSON round-trip test, the exact counters in the ops tests) are bounded by stated assertions; mechanical migrations are given as rules with the full site lists.
- **Type consistency:** `OpBody::create` (Task 2 Step 4) is the one constructor used in Steps 5 to 7; `Kind::Unknown` is a unit variant everywhere; `Node::kind()` (Task 1) becomes register-backed in Task 2 without changing callers; `JsonNode::kind()` returns `Option<Kind>`, never the string.
- **Known risks:** Task 2 is large by necessity (the crate does not compile half-switched); the version-1 file handling depends on the callers' `NotAJournal` handling, checked in Step 8; local-state `closed` entries from before the change stop reopening (no migration).
