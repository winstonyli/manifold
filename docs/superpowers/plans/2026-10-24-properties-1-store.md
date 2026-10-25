---
status: active
---

# Properties 1: path-keyed registers in the store — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A node's registers are addressed by `(field, path)`; the journal stores, syncs, snapshots and compacts them; two replicas applying the same ops in any order converge. Nothing in the app reads a tree yet.

**Architecture:** New `store/tree.rs` holds `TreePath` (order-preserving bytes), `Leaf` (value codec) and the visibility pass. `Node.fields` is re-keyed by `RegKey { field, path }`; every existing field is a root-path key, so behaviour is unchanged until a path op arrives. `OpBody::Set` and the snapshot register gain an optional path (wire field 4, free today). Journal `FORMAT_VERSION` 3 → 4.

**Tech Stack:** Rust, the in-house TLV (`store/tlv.rs`), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-24-properties-design.md` §1 (store), §5 phase 1, §6 tests, §8 D1. Plans 2–4 (App/mirror/undo/export; labels; inline Json) are written after this lands. Decision record: `docs/decisions/2026-10-24-properties-tags-json.md`.

## Global Constraints

- `kind: app`. Do not bump the version, edit `CHANGELOG.md`, tag, or push. No migration code: a v3 journal is refused and set aside (D1).
- The local store `FORMAT_VERSION` (`store/local.rs`) and `PROTOCOL_VERSION` do not change. Only the journal's `FORMAT_VERSION` moves, 3 → 4.
- Wire: Set body `{1 node, 2 field, 3 enc}` gains optional `4 path`; snapshot register `{1 field, 2 stamp, 3 enc}` gains optional `4 path`. A root path is never written. `Create`'s embedded field map stays path-less.
- Field numbers `25` (PROPS) and `26` (BODY) are the tree fields. They get no `FieldDef`, so `Doc::apply` treats them like unknown fields and keeps them raw, paths included.
- Merge rule: a register at A with stamp s hides every proper-descendant register with a lower stamp. Tombstones (empty `enc`) are kept. Sentinels `EmptyObj`/`EmptyArr` are used only in BODY (plan 4), never PROPS.
- `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass; `.githooks/pre-commit` runs both; never `--no-verify`. Cargo `-j 8`, one cargo per target dir. Files are LF.
- Tests never touch the real `%APPDATA%\manifold` (use `store::testutil::temp_dir`).
- No gate run in this plan (the store change needs `gate all full` before the user tags).
- Python, if any, only as `python -P f.py < /dev/null`.
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Order and duplicates:** the same op set applied in any order, with duplicates, gives equal nodes, equal `tree()`, equal snapshot bytes (T5).
2. **Late older write:** a write older than a tombstone or a replaced scalar at an ancestor stays hidden, also after a snapshot round trip (T4, T5).
3. **Forward/backward files:** an unknown field with a path survives compaction; a v3 file is refused and set aside; a v5 file opens read-only (T3).
4. **Awkward keys:** empty, NUL-containing and non-ASCII keys keep their order and round-trip; a subtree stays one key range (T1).
5. **Delete:** a deleted node's tree is cleared and later path Sets are ignored; pruning the version table keeps the actor table complete (T3, T5).

## Files

- Create `crates/manifold/src/store/tree.rs`: `TreePath`, `Seg`, `Leaf`, `visible`, `Node::visible_regs`, `Node::tree`, with tests.
- Modify `store/mod.rs` (declare module; testutil), `store/model.rs` (`RegKey`, `Node.fields`, `field::PROPS/BODY`), `store/ops.rs` (`OpBody::Set.path`, `apply`), `store/journal.rs` (wire, snapshot, `FORMAT_VERSION`), `store/journal/tests.rs`, `closed.rs`, `session_sync/**` tests (compile fixes only).
- Docs at the end: spec wording, decision-record Open item, "format 3" wording.

A scratch crate compiled and passed the T1 and T4 code and tests below (20 tests with the copied tlv/varint tests); the repo build is the authority.

---

### Task 1: `TreePath`, `Seg`, `Leaf`

**Files:** Create `crates/manifold/src/store/tree.rs`. Modify `store/mod.rs` (add `mod tree;` beside `mod tlv;`), `store/model.rs` (`id_message` and `decode_id` become `pub(super)` if they are not).

**Interfaces:**
- Consumes: `NodeId { actor: ActorId(u64), counter: u64 }`; `tlv::{Reader, Writer, TlvError}`; `varint::{zigzag, unzigzag}`; `testutil::XorShift` (`.below(n)`).
- Produces: `Seg::{Key(String), Id(NodeId)}`; `TreePath` with `of(&[Seg])`, `child(&Seg)`, `parent()`, `segments()`, `is_root()`, `as_bytes()`, `from_bytes(&[u8])`, `is_ancestor_of(&TreePath)`, `subtree_end()`, `Default` = root, `Ord`; `Leaf::{Null, Bool, Int, Float, Num, Str, Ref, EmptyObj, EmptyArr}` with `encode() -> Vec<u8>` and `decode(&[u8]) -> Result<Option<Leaf>, TlvError>`.

- [ ] **Step 1:** In `store/model.rs` make `fn id_message(id: NodeId) -> Writer` and `fn decode_id(buf: &[u8]) -> Result<NodeId, TlvError>` (both private today, lines ~410 and ~421) `pub(super)`; tree.rs calls them with exactly those signatures. `Writer` has `varint`, `fixed64`, `str`, `message`, `into_bytes`; the reader value has `bool`, `u64`, `fixed64`, `str`, `bytes` (checked against `store/tlv.rs`).
- [ ] **Step 2:** Create `store/tree.rs` with this content (the test module is part of the file). Add `mod tree;` to `store/mod.rs`.

```rust
//! Tree values (spec 2026-10-24-properties-design.md, sections 1.1 and 1.2): the address of a register inside a
//! node's tree, and the leaf values a register holds.

#![allow(dead_code, reason = "the app reads trees from plan 2 on (mirror trees); remove this line there")]

use super::model::{decode_id, id_message, ActorId, NodeId};
use super::tlv::{Reader, TlvError, Writer};
use super::varint;

const TAG_KEY: u8 = 1;
const TAG_ID: u8 = 2;
/// Greater than every tag: a path followed by `AFTER` is above the path's whole subtree.
const AFTER: u8 = 3;
/// A key segment ends with `0x00 END`; a `0x00` inside the text is written `0x00 ESC`.
const END: u8 = 1;
const ESC: u8 = 0xFF;

/// One step of a path: an object key, or the id of an element of an id-keyed map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Key(String),
    Id(NodeId),
}

/// The address of a register inside a field's tree, in an order-preserving byte encoding: a parent sorts before its
/// children, a subtree is one key range, and no key is a byte prefix of another. The root (no segments) is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TreePath(Box<[u8]>);

impl TreePath {
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Reads a path written by `as_bytes`; refuses bytes that are not a whole number of segments.
    pub fn from_bytes(bytes: &[u8]) -> Result<TreePath, TlvError> {
        parse(bytes).ok_or(TlvError::BadValue)?;
        Ok(TreePath(bytes.into()))
    }

    pub fn segments(&self) -> Vec<Seg> {
        parse(&self.0).expect("a path built or checked by this module")
    }

    pub fn of(segments: &[Seg]) -> TreePath {
        segments.iter().fold(TreePath::default(), |path, seg| path.child(seg))
    }

    pub fn child(&self, seg: &Seg) -> TreePath {
        let mut bytes = self.0.to_vec();
        match seg {
            Seg::Key(text) => {
                bytes.push(TAG_KEY);
                for &c in text.as_bytes() {
                    if c == 0 {
                        bytes.extend([0, ESC]);
                    } else {
                        bytes.push(c);
                    }
                }
                bytes.extend([0, END]);
            }
            Seg::Id(id) => {
                bytes.push(TAG_ID);
                bytes.extend(id.actor.0.to_be_bytes());
                bytes.extend(id.counter.to_be_bytes());
            }
        }
        TreePath(bytes.into())
    }

    /// The path without its last segment; `None` for the root.
    pub fn parent(&self) -> Option<TreePath> {
        let mut segments = self.segments();
        segments.pop()?;
        Some(TreePath::of(&segments))
    }

    /// Whether `other` lies strictly below this path.
    pub fn is_ancestor_of(&self, other: &TreePath) -> bool {
        other.0.len() > self.0.len() && other.0.starts_with(&self.0)
    }

    /// An exclusive upper bound of this path's subtree: every path below it sorts between the path and this.
    pub fn subtree_end(&self) -> TreePath {
        let mut bytes = self.0.to_vec();
        bytes.push(AFTER);
        TreePath(bytes.into())
    }
}

fn parse(mut b: &[u8]) -> Option<Vec<Seg>> {
    let mut out = Vec::new();
    while let Some((&tag, rest)) = b.split_first() {
        b = rest;
        match tag {
            TAG_KEY => {
                let mut text = Vec::new();
                loop {
                    let (&c, rest) = b.split_first()?;
                    b = rest;
                    if c != 0 {
                        text.push(c);
                        continue;
                    }
                    let (&next, rest) = b.split_first()?;
                    b = rest;
                    match next {
                        END => break,
                        ESC => text.push(0),
                        _ => return None,
                    }
                }
                out.push(Seg::Key(String::from_utf8(text).ok()?));
            }
            TAG_ID => {
                let (head, rest) = b.split_at_checked(16)?;
                b = rest;
                let actor = u64::from_be_bytes(head[..8].try_into().ok()?);
                let counter = u64::from_be_bytes(head[8..].try_into().ok()?);
                out.push(Seg::Id(NodeId { actor: ActorId(actor), counter }));
            }
            _ => return None,
        }
    }
    Some(out)
}

// The value tags, also the TLV field numbers of a value's encoding.
const T_NULL: u32 = 1;
const T_BOOL: u32 = 2;
const T_INT: u32 = 3;
const T_FLOAT: u32 = 4;
const T_NUM: u32 = 5;
const T_STR: u32 = 6;
const T_REF: u32 = 7;
const T_OBJ: u32 = 8;
const T_ARR: u32 = 9;

/// What a register at a path holds. An empty encoding is not a leaf: it is a tombstone (`Leaf::decode` gives `None`).
#[derive(Debug, Clone, PartialEq)]
pub enum Leaf {
    Null,
    Bool(bool),
    Int(i64),
    /// Finite only.
    Float(f64),
    /// A number that is neither an `i64` nor a finite `f64`, as its source text, so a JSON number round-trips.
    Num(String),
    Str(String),
    /// A card, by id. Does not own it.
    Ref(NodeId),
    /// `{}` and `[]`: how an empty container keeps its place (spec 1.4).
    EmptyObj,
    EmptyArr,
}

impl Leaf {
    /// One TLV field whose number is the value's tag, so every leaf, `Null` and `""` included, encodes to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Leaf::Null => w.varint(T_NULL, 0),
            Leaf::Bool(b) => w.varint(T_BOOL, u64::from(*b)),
            Leaf::Int(i) => w.varint(T_INT, varint::zigzag(*i)),
            Leaf::Float(f) => {
                debug_assert!(f.is_finite(), "a non-finite number is a `Num`");
                w.fixed64(T_FLOAT, if *f == 0.0 { 0.0f64 } else { *f }.to_bits()); // -0.0 and 0.0 are one value
            }
            Leaf::Num(text) => w.str(T_NUM, text),
            Leaf::Str(s) => w.str(T_STR, s),
            Leaf::Ref(id) => w.message(T_REF, &id_message(*id)),
            Leaf::EmptyObj => w.varint(T_OBJ, 0),
            Leaf::EmptyArr => w.varint(T_ARR, 0),
        }
        w.into_bytes()
    }

    /// `None` for an empty encoding (a tombstone); the last field wins, as for every register.
    pub fn decode(enc: &[u8]) -> Result<Option<Leaf>, TlvError> {
        let mut leaf = None;
        for f in Reader::new(enc) {
            let f = f?;
            leaf = Some(match f.no {
                T_NULL => Leaf::Null,
                T_BOOL => Leaf::Bool(f.value.bool()?),
                T_INT => Leaf::Int(varint::unzigzag(f.value.u64()?)),
                T_FLOAT => {
                    let x = f64::from_bits(f.value.fixed64()?);
                    if !x.is_finite() {
                        return Err(TlvError::BadValue);
                    }
                    Leaf::Float(x)
                }
                T_NUM => Leaf::Num(f.value.str()?.to_owned()),
                T_STR => Leaf::Str(f.value.str()?.to_owned()),
                T_REF => Leaf::Ref(decode_id(f.value.bytes()?)?),
                T_OBJ => Leaf::EmptyObj,
                T_ARR => Leaf::EmptyArr,
                _ => return Err(TlvError::BadValue),
            });
        }
        Ok(leaf)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ops::Bound::Excluded;

    use super::*;
    use crate::store::testutil::XorShift;

    fn id(actor: u64, counter: u64) -> NodeId {
        NodeId { actor: ActorId(actor), counter }
    }

    fn key(text: &str) -> Seg {
        Seg::Key(text.to_owned())
    }

    #[test]
    fn a_path_reads_back_as_the_segments_it_was_built_from() {
        let segments = [key("a"), key(""), key("a\0b"), key("héllo"), Seg::Id(id(7, 9)), key("\0")];
        assert_eq!(TreePath::of(&segments).segments(), segments);
        assert_eq!(TreePath::of(&[]).segments(), []);
        let path = TreePath::of(&segments);
        assert_eq!(TreePath::from_bytes(path.as_bytes()), Ok(path.clone()));
        assert_eq!(path.parent().unwrap().segments(), segments[..5]);
        assert_eq!(TreePath::default().parent(), None);
    }

    #[test]
    fn bytes_that_are_not_whole_segments_are_refused() {
        let path = TreePath::of(&[key("ab"), Seg::Id(id(1, 2))]);
        let bytes = path.as_bytes();
        for cut in 1..bytes.len() {
            let whole = TreePath::from_bytes(&bytes[..cut]).is_ok();
            // Only the cut between the two segments is a whole path.
            assert_eq!(whole, cut == 5, "cut at {cut}");
        }
        assert!(TreePath::from_bytes(&[9]).is_err(), "an unknown tag");
        assert!(TreePath::from_bytes(&[TAG_KEY, b'a', 0, 7]).is_err(), "an unknown escape");
        assert!(TreePath::from_bytes(&[TAG_KEY, 0xFF, 0, END]).is_err(), "not UTF-8");
    }

    fn random_path(rng: &mut XorShift) -> TreePath {
        let keys = ["a", "ab", "a\0", "", "b", "\u{1}"];
        let segments: Vec<Seg> = (0..rng.below(4))
            .map(|_| if rng.below(4) == 0 { Seg::Id(id(1, rng.below(3))) } else { key(keys[rng.below(6) as usize]) })
            .collect();
        TreePath::of(&segments)
    }

    #[test]
    fn a_subtree_is_one_range_and_parents_sort_before_children() {
        let mut rng = XorShift(0x7EA5_0001);
        for _ in 0..200 {
            let paths: BTreeSet<TreePath> = (0..14).map(|_| random_path(&mut rng)).collect();
            for p in &paths {
                let below: Vec<&TreePath> = paths.range((Excluded(p.clone()), Excluded(p.subtree_end()))).collect();
                let expected: Vec<&TreePath> = paths.iter().filter(|q| p.is_ancestor_of(q)).collect();
                assert_eq!(below, expected, "the range below {:?}", p.segments());
                assert!(expected.iter().all(|q| *q > p), "a child sorts after its parent");
            }
        }
    }

    #[test]
    fn leaves_round_trip() {
        let long = "1".repeat(30);
        for leaf in [
            Leaf::Null,
            Leaf::Bool(true),
            Leaf::Bool(false),
            Leaf::Int(0),
            Leaf::Int(i64::MIN),
            Leaf::Int(i64::MAX),
            Leaf::Float(1.5),
            Leaf::Float(f64::MAX),
            Leaf::Float(5e-324),
            Leaf::Num(long),
            Leaf::Str(String::new()),
            Leaf::Str("héllo \0 x".into()),
            Leaf::Ref(id(0xAB, 17)),
            Leaf::EmptyObj,
            Leaf::EmptyArr,
        ] {
            let enc = leaf.encode();
            assert!(!enc.is_empty(), "{leaf:?} must not look like a tombstone");
            assert_eq!(Leaf::decode(&enc), Ok(Some(leaf)));
        }
        assert_eq!(Leaf::decode(&[]), Ok(None), "an empty encoding is a tombstone");
    }

    #[test]
    fn equal_values_encode_equally_and_odd_encodings_are_refused() {
        assert_eq!(Leaf::Float(-0.0).encode(), Leaf::Float(0.0).encode());
        let mut nan = Writer::new();
        nan.fixed64(T_FLOAT, f64::NAN.to_bits());
        assert_eq!(Leaf::decode(&nan.into_bytes()), Err(TlvError::BadValue));
        let mut future = Writer::new();
        future.varint(99, 1);
        assert_eq!(Leaf::decode(&future.into_bytes()), Err(TlvError::BadValue));
        let mut two = Leaf::Int(1).encode();
        two.extend(Leaf::Str("x".into()).encode());
        assert_eq!(Leaf::decode(&two), Ok(Some(Leaf::Str("x".into()))), "the last field wins");
    }
}
```

- [ ] **Step 3:** `cargo test -j 8 -p manifold --bin manifold store::tree` — Expected: 5 tests pass (`a_path_reads_back…`, `bytes_that_are_not_whole_segments…`, `a_subtree_is_one_range…`, `leaves_round_trip`, `equal_values_encode_equally…`). Mutation check: temporarily set `AFTER` to `0`; `a_subtree_is_one_range…` must fail; restore.
- [ ] **Step 4:** `cargo fmt --all` and clippy (Global Constraints). Commit: `store: TreePath and Leaf, the address and value of a tree register` + trailer.

---

### Task 2: Re-key `Node.fields` by `RegKey` (no behaviour change)

**Files:** Modify `store/model.rs`, `store/ops.rs`, `store/journal.rs`, `store/mod.rs` (testutil `shown`), `closed.rs`, and tests in `store/model.rs`, `store/ops.rs`, `store/journal/tests.rs`, `session_sync/tests.rs`, `session_sync/tests/remote.rs`, `session_sync/tests/writes.rs`.

**Interfaces:**
- Produces: `RegKey { field: u32, path: TreePath }` (`Debug, Clone, PartialEq, Eq, Ord, Hash`), `RegKey::of(no) -> RegKey` (root path), `Node.fields: BTreeMap<RegKey, Reg>`, `Node::reg(&self, no: u32) -> Option<&Reg>` (the root-path register), `field::PROPS = 25`, `field::BODY = 26`.

- [ ] **Step 1:** Baseline: `cargo test -j 8 -p manifold --bin manifold`, note the pass count. Then in `store/model.rs` add above `Node` (with `use super::tree::TreePath;`):

```rust
/// Where a register lives in a node: a field and a path inside that field's tree. Every scalar field sits at the
/// root path; only tree fields (`field::PROPS`, `field::BODY`, and numbers this build doesn't know) have deeper ones.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegKey {
    pub field: u32,
    pub path: TreePath,
}

impl RegKey {
    pub fn of(no: u32) -> RegKey {
        RegKey { field: no, path: TreePath::default() }
    }
}
```

  Change `Node.fields` to `BTreeMap<RegKey, Reg>` and its doc comment to "by field and path". Add to `impl Node`: `pub fn reg(&self, no: u32) -> Option<&Reg> { self.fields.get(&RegKey::of(no)) }`.
- [ ] **Step 2:** Add to `mod field`:

```rust
    /// Tree fields (no `FieldDef`: `Doc::apply` keeps them raw, paths included). PROPS: a card's properties; BODY: an
    /// inline Json card's document.
    #[allow(dead_code, reason = "read by the app in plan 2; remove this attribute there")]
    pub const PROPS: u32 = 25;
    #[allow(dead_code, reason = "read by the app in plan 4; remove this attribute there")]
    pub const BODY: u32 = 26;
```

- [ ] **Step 3:** Fix `model.rs` users: `has_type` → `self.reg(field::TYPE).is_some()`; `ty` → `self.reg(field::TYPE)`; `get` → `self.reg(no)`; `with_type` → `BTreeMap::from([(RegKey::of(field::TYPE), reg)])`; `closed_roots` → `n.reg(field::CLOSED)?.stamp`; `message` (cfg test) → iterate `self.fields.iter().filter(|(k, _)| k.path.is_root())` and compare `k.field` in the group-insertion test. In the tests (lines ~843–847, 913, 988) use `RegKey::of(…)` for keys.
- [ ] **Step 4:** `ops.rs`: change `set_reg` to take the key:

```rust
fn set_reg(node: &mut Node, key: RegKey, stamp: Stamp, enc: Vec<u8>) {
    if node.fields.get(&key).is_none_or(|reg| reg.stamp < stamp) {
        node.fields.insert(key, Reg { stamp, enc });
    }
}
```

  The Create loop passes `RegKey::of(field.no)`; the Set arm passes `RegKey::of(*no)`. `prune`'s `node.fields.values()` is unchanged. Tests at ops.rs ~251/255: `fields[&40]` → `fields[&RegKey::of(40)]`.
- [ ] **Step 5:** `journal.rs`: snapshot loop `for (key, reg) in &node.fields { … r.varint(1, u64::from(key.field)); …`; decode inserts `RegKey::of(no)`. `store/mod.rs` testutil `shown` returns `Vec<(NodeId, BTreeMap<RegKey, Reg>, Option<NodeId>)>`.
- [ ] **Step 6:** `closed.rs:140`: `n.fields.get(&field::CLOSED)?.stamp` → `n.reg(field::CLOSED)?.stamp`. Test sites: `journal/tests.rs` ~249 and ~282, `session_sync/tests.rs:478` (`.fields.keys().map(|k| k.field).collect()`), `session_sync/tests/remote.rs` ~157/432/1018, `session_sync/tests/writes.rs` ~1186/1193. Then `cargo check -j 8 -p manifold --all-targets` and fix any further `Node.fields` use the compiler reports (the other `.fields` maps — `Snapshot`, `Mirror`, `json` — are different types and stay).
- [ ] **Step 7:** Compare with the baseline taken before Step 1 (run `cargo test -j 8 -p manifold --bin manifold` on the clean tree first and note the pass count). Run it again — Expected: the same count, all pass. Clippy, fmt, commit: `store: Node.fields keyed by (field, path); every field is a root path` + trailer.

---

### Task 3: Path on `Set`, the wire, `FORMAT_VERSION` 4

**Files:** Modify `store/ops.rs`, `store/journal.rs`, `store/mod.rs` (testutil), `store/journal/tests.rs`, `cli.rs` (one test caller of `version_one_file`), and every `OpBody::Set` construction site (`grep -rn "OpBody::Set" crates --include=*.rs`; 16 today).

**Interfaces:**
- Consumes: `TreePath::{from_bytes, as_bytes, is_root}`, `RegKey`, `Node::tree` (T4 lands later: this task's tests read `fields` directly, or move that assertion to T5).
- Produces: `OpBody::Set { node, field, path: TreePath, enc }`; `OpBody::set(node, field, enc) -> OpBody` (root path); `testutil::file_with_version(path: &Path, version: u8) -> Vec<u8>` (replaces `version_one_file`).

- [ ] **Step 1 (tests first).** In `store/journal/tests.rs` (imports: `crate::store::tree::{Leaf, Seg, TreePath}`, `RegKey`) add:

```rust
fn props_path(keys: &[&str]) -> TreePath {
    TreePath::of(&keys.iter().map(|k| Seg::Key((*k).to_owned())).collect::<Vec<_>>())
}

fn set_at(node: NodeId, path: TreePath, leaf: Leaf) -> OpBody {
    OpBody::Set { node, field: field::PROPS, path, enc: leaf.encode() }
}

fn props(doc: &Doc, node: NodeId) -> Vec<(TreePath, Leaf)> {
    doc.nodes[&node]
        .fields
        .iter()
        .filter(|(k, r)| k.field == field::PROPS && !r.enc.is_empty())
        .map(|(k, r)| (k.path.clone(), Leaf::decode(&r.enc).unwrap().unwrap()))
        .collect()
}

#[test]
fn a_path_set_survives_the_log_a_snapshot_and_a_reopen() {
    let dir = temp_dir("path-set");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://p.example/");
    txn.commit(vec![
        create,
        set_at(tab, props_path(&["a"]), Leaf::Str("x".into())),
        set_at(tab, props_path(&["a", "b\0é"]), Leaf::Int(7)),
        set_at(tab, props_path(&["gone"]), Leaf::Null),
        OpBody::Set { node: tab, field: field::PROPS, path: props_path(&["gone"]), enc: Vec::new() },
    ])
    .unwrap();
    let live = Journal::read(&std::fs::read(&path).unwrap()).unwrap(); // from the log
    journal.close().unwrap(); // compacts: from a snapshot
    let snap = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    for doc in [&live, &snap] {
        assert_eq!(
            props(doc, tab),
            vec![(props_path(&["a"]), Leaf::Str("x".into())), (props_path(&["a", "b\0é"]), Leaf::Int(7))]
        );
    }
    assert_eq!(live.nodes[&tab], snap.nodes[&tab], "the tombstone at /gone is kept by the snapshot");
}

#[test]
fn a_path_on_a_known_scalar_field_is_ignored() {
    let dir = temp_dir("path-scalar");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://s.example/");
    let changed = encode_field(field::URL, &FieldValue::Str("https://changed.example/".into()));
    txn.commit(vec![create, OpBody::Set { node: tab, field: field::URL, path: props_path(&["x"]), enc: changed }])
        .unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(url(&doc, tab), FieldValue::Str("https://s.example/".into()));
    assert!(doc.nodes[&tab].fields.keys().all(|k| k.path.is_root()));
}

#[test]
fn a_delete_clears_the_tree_and_later_path_sets_are_ignored() {
    let dir = temp_dir("path-delete");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://d.example/");
    txn.commit(vec![create, set_at(tab, props_path(&["a"]), Leaf::Int(1)), OpBody::Delete { node: tab }]).unwrap();
    let mut txn = journal.begin().unwrap();
    txn.commit(vec![set_at(tab, props_path(&["b"]), Leaf::Int(2))]).unwrap();
    journal.close().unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert!(doc.nodes[&tab].deleted && doc.nodes[&tab].fields.is_empty());
}

#[test]
fn an_unknown_field_with_a_path_survives_compaction() {
    let dir = temp_dir("path-future");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://f.example/");
    let mut future = Writer::new();
    future.str(99, "from the future");
    let key = RegKey { field: 99, path: props_path(&["k"]) };
    txn.commit(vec![create, OpBody::Set { node: tab, field: 99, path: key.path.clone(), enc: future.into_bytes() }])
        .unwrap();
    journal.close().unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert!(doc.nodes[&tab].fields.contains_key(&key));
}

#[test]
fn a_version_three_file_is_refused_and_set_aside() {
    let dir = temp_dir("version-three");
    let path = dir.join("session.mnf");
    let bytes = file_with_version(&path, 3);
    assert!(matches!(Journal::read(&bytes), Err(JournalError::NotAJournal)));
    let (_journal, aside) = Journal::open_or_set_aside(&path, ActorId(2)).unwrap();
    assert_eq!(std::fs::read(aside.expect("moved aside")).unwrap(), bytes);
}
```

  In `store/mod.rs` testutil rename `version_one_file` to `file_with_version(path: &Path, version: u8)` (doc: "an empty journal whose header says format `version`", `bytes[4] = version`); replace its three calls in `journal/tests.rs` (and the import) and the one in `cli.rs` with `file_with_version(&path, 1)`.
- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold store::journal` — Expected: FAIL (compile: `path` is not a field of `OpBody::Set`).
- [ ] **Step 3:** `ops.rs`:

```rust
    /// `enc` is `encode_field(field, value)`; empty resets the field. `path` addresses a register inside a tree field;
    /// it is root for every scalar field, and `apply` ignores a non-root path on a field with a `FieldDef`.
    Set { node: NodeId, field: u32, path: TreePath, enc: Vec<u8> },
```

  and in `impl OpBody`:

```rust
    /// A `Set` of a whole field (the root path): every scalar field's write.
    pub fn set(node: NodeId, field: u32, enc: Vec<u8>) -> OpBody {
        OpBody::Set { node, field, path: TreePath::default(), enc }
    }
```

  The Set arm of `apply`:

```rust
            OpBody::Set { node, field: no, path, enc } => {
                if *no == field::GROUP || (!path.is_root() && field_def(*no).is_some()) {
                    return Vec::new(); // the tree edge changes only by `Move`; a scalar field has no tree
                }
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                if entry.deleted {
                    return Vec::new();
                }
                set_reg(entry, RegKey { field: *no, path: path.clone() }, stamp, enc.clone());
                vec![*node]
            }
```

  Update every construction site to `OpBody::set(node, field, enc)`; patterns naming `field`/`enc` without `..` need `path` or `..` (`op_actors` already uses `..`).
- [ ] **Step 4:** `journal.rs` wire. Snapshot register: after the `3` write, `if !key.path.is_root() { r.bytes(4, key.path.as_bytes()); }`; decode: a `path` local starting `TreePath::default()`, arm `4 => path = TreePath::from_bytes(r.value.bytes()?)?` (map the error the way neighbouring arms map `TlvError`), insert `RegKey { field: no, path }`. Ops: `ops_message`'s Set arm writes `if !path.is_root() { b.bytes(4, path.as_bytes()); }` after the `3` write; `decode_ops` adds a `path` local, arm `(OP_SET, 4) => path = TreePath::from_bytes(f.value.bytes()?)?`, and builds `OpBody::Set { node, field, path, enc: blob }`. Update the op-message doc comment (`5 Set {1: node, 2: field, 3: value encoding, 4: path}`).
- [ ] **Step 5:** `pub const FORMAT_VERSION: u64 = 4;` and extend its doc comment: "4: registers carry a path (`Set` field 4, snapshot register field 4). 3: …". Keep `read_readable_header`'s `header.version < FORMAT_VERSION` (D1 refuses v3 although it holds no paths); update its doc comment, which names versions 1 and 2, to say any older file is refused.
- [ ] **Step 6:** `cargo test -j 8 -p manifold --bin manifold` — Expected: all pass, including the 5 new tests; the newer-version test (`FORMAT_VERSION + 1`) still passes.
- [ ] **Step 7:** fmt, clippy, commit: `store: Set and snapshot registers carry a path; journal format 4` + trailer.

---

### Task 4: Visibility

**Files:** Modify `store/tree.rs`.

**Interfaces:**
- Consumes: `RegKey`, `Reg`, `Stamp`, `Node` from `store::model`.
- Produces: `visible(iter) -> iter`; `Node::visible_regs(&self) -> impl Iterator<Item = (&RegKey, &Reg)>` (not hidden; tombstones included); `Node::tree(&self, no: u32) -> Vec<(TreePath, Leaf, Stamp)>` (live leaves of field `no` in path order).

- [ ] **Step 1:** Change the `use super::model::…` line of `tree.rs` to `use super::model::{decode_id, id_message, ActorId, Node, NodeId, Reg, RegKey, Stamp};`; in its `tests` module add `use crate::store::model::Hlc;`. Append these to the `tests` module:

```rust
    fn stamp(n: u64) -> Stamp {
        Stamp { hlc: Hlc { ms: n, logical: 0 }, actor: ActorId(1) }
    }

    fn reg(n: u64, leaf: Option<Leaf>) -> Reg {
        Reg { stamp: stamp(n), enc: leaf.map(|l| l.encode()).unwrap_or_default() }
    }

    fn at(field: u32, segs: &[&str]) -> RegKey {
        let segs: Vec<Seg> = segs.iter().map(|s| Seg::Key((*s).to_owned())).collect();
        RegKey { field, path: TreePath::of(&segs) }
    }

    fn brute(node: &Node) -> Vec<RegKey> {
        node.fields
            .iter()
            .filter(|(k, r)| {
                !node.fields.iter().any(|(a, ar)| a.field == k.field && a.path.is_ancestor_of(&k.path) && ar.stamp > r.stamp)
            })
            .map(|(k, _)| k.clone())
            .collect()
    }

    #[test]
    fn visibility_matches_the_definition_on_random_trees() {
        let mut rng = XorShift(0xA11CE);
        let names = ["a", "b", "c"];
        for _ in 0..500 {
            let mut node = Node::placeholder();
            for _ in 0..rng.below(14) {
                let depth = rng.below(4) as usize;
                let segs: Vec<&str> = (0..depth).map(|_| names[rng.below(3) as usize]).collect();
                let leaf = if rng.below(4) == 0 { None } else { Some(Leaf::Int(rng.below(9) as i64)) };
                node.fields.insert(at(1 + rng.below(2) as u32, &segs), reg(1 + rng.below(6), leaf));
            }
            let got: Vec<RegKey> = node.visible_regs().map(|(k, _)| k.clone()).collect();
            assert_eq!(got, brute(&node), "{:?}", node.fields);
        }
    }

    #[test]
    fn a_newer_ancestor_hides_and_an_older_one_does_not() {
        let mut node = Node::placeholder();
        node.fields.insert(at(1, &["a"]), reg(5, None)); // tombstone
        node.fields.insert(at(1, &["a", "b"]), reg(3, Some(Leaf::Int(1)))); // older: hidden
        node.fields.insert(at(1, &["a", "c"]), reg(7, Some(Leaf::Int(2)))); // newer: shows
        node.fields.insert(at(1, &["a", "b", "d"]), reg(6, Some(Leaf::Int(3)))); // under a hidden one, newer than /a
        node.fields.insert(at(1, &["z"]), reg(1, Some(Leaf::Str("s".into()))));
        let tree = node.tree(1);
        let shown: Vec<(Vec<Seg>, Leaf)> = tree.into_iter().map(|(p, l, _)| (p.segments(), l)).collect();
        assert_eq!(
            shown,
            vec![
                (vec![Seg::Key("a".into()), Seg::Key("b".into()), Seg::Key("d".into())], Leaf::Int(3)),
                (vec![Seg::Key("a".into()), Seg::Key("c".into())], Leaf::Int(2)),
                (vec![Seg::Key("z".into())], Leaf::Str("s".into())),
            ]
        );
        assert_eq!(node.visible_regs().count(), 4, "the hidden /a/b is dropped, the /a tombstone stays");
    }

    #[test]
    fn a_root_tombstone_hides_older_registers_of_its_field_only() {
        let mut node = Node::placeholder();
        node.fields.insert(at(1, &[]), reg(9, None));
        node.fields.insert(at(1, &["a"]), reg(4, Some(Leaf::Int(1))));
        node.fields.insert(at(2, &["a"]), reg(4, Some(Leaf::Int(2))));
        assert!(node.tree(1).is_empty());
        assert_eq!(node.tree(2).len(), 1);
    }
```

- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold store::tree` — Expected: FAIL (compile: `visible_regs`/`tree` not found).
- [ ] **Step 3:** Add this above the `tests` module in `tree.rs` (the module-level `allow(dead_code)` covers it):

```rust
/// The registers no ancestor register overrides, in key order (tombstones included). A register is hidden when a
/// register at a proper-prefix path has a higher stamp; hidden registers still hide, but whatever they hide their hider
/// hides too, so a stack of cumulative newest stamps decides each one in a single pass.
pub fn visible<'a>(
    regs: impl Iterator<Item = (&'a RegKey, &'a Reg)>,
) -> impl Iterator<Item = (&'a RegKey, &'a Reg)> {
    let mut field = None;
    let mut above: Vec<(&'a TreePath, Stamp)> = Vec::new();
    regs.filter(move |(key, reg)| {
        if field != Some(key.field) {
            field = Some(key.field);
            above.clear();
        }
        while above.last().is_some_and(|(p, _)| !p.is_ancestor_of(&key.path)) {
            above.pop();
        }
        let newest = above.last().map(|(_, s)| *s);
        above.push((&key.path, newest.map_or(reg.stamp, |s| s.max(reg.stamp))));
        newest.is_none_or(|s| s <= reg.stamp)
    })
}

impl Node {
    /// The registers of a field's tree that count (see `visible`), tombstones included.
    pub fn visible_regs(&self) -> impl Iterator<Item = (&RegKey, &Reg)> {
        visible(self.fields.iter())
    }

    /// A tree field's live leaves in path order with their stamps: visible, not a tombstone.
    pub fn tree(&self, no: u32) -> Vec<(TreePath, Leaf, Stamp)> {
        let own = self.fields.range(RegKey::of(no)..).take_while(|(k, _)| k.field == no);
        visible(own)
            .filter_map(|(k, r)| Some((k.path.clone(), Leaf::decode(&r.enc).ok()??, r.stamp)))
            .collect()
    }
}

```

- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold store::tree` — Expected: 8 pass.
- [ ] **Step 5:** fmt, clippy, commit: `store: which registers of a tree count (a newer ancestor hides)` + trailer.

---

### Task 5: Snapshots omit hidden registers; convergence test

**Files:** Modify `store/journal.rs` (snapshot loop, `doc_actors`), `store/journal/tests.rs`.

**Interfaces:**
- Consumes: `Node::visible_regs`, `Node::tree`, `Journal::read`, compaction via `Journal::close`, the snapshot-writing function `compact` calls (grep `fn snapshot_message` or the nearest equivalent in `journal.rs`).

- [ ] **Step 1 (tests first).** In `journal/tests.rs` add:

```rust
/// A fresh doc with `ops` applied in the given order.
fn apply_all(ops: &[Op]) -> Doc {
    let mut doc = Doc::new(DocId([0; 16]));
    for op in ops {
        doc.apply(op);
    }
    doc
}

#[test]
fn replicas_converge_whatever_the_order_and_duplicates() {
    let node = NodeId { actor: ActorId(1), counter: 0 };
    let mut rng = XorShift(0xC0DE_0001);
    let keys = ["a", "b", "c"];
    for round in 0..200 {
        let mut ops = vec![Op {
            id: node,
            hlc: Hlc { ms: 1, logical: 0 },
            body: OpBody::create(node, "application/x-from-the-future", Vec::new()),
        }];
        for i in 0..rng.below(14) + 1 {
            let depth = rng.below(4) as usize;
            let segs: Vec<Seg> = (0..depth)
                .map(|_| match rng.below(5) {
                    3 => Seg::Id(NodeId { actor: ActorId(9), counter: 1 }),
                    4 => Seg::Id(NodeId { actor: ActorId(9), counter: 2 }),
                    n => Seg::Key(keys[n as usize].to_owned()),
                })
                .collect();
            let enc = if rng.below(4) == 0 { Vec::new() } else { Leaf::Int(rng.below(100) as i64).encode() };
            ops.push(Op {
                id: NodeId { actor: ActorId(1 + rng.below(3)), counter: 100 + i },
                hlc: Hlc { ms: 2 + rng.below(6), logical: u32::try_from(i).unwrap() }, // distinct stamps: equal ones tie by arrival
                body: OpBody::Set { node, field: field::PROPS, path: TreePath::of(&segs), enc },
            });
        }
        let reference = apply_all(&ops);
        for _ in 0..4 {
            let mut shuffled = ops.clone();
            for i in (1..shuffled.len()).rev() {
                shuffled.swap(i, rng.below(i as u64 + 1) as usize);
            }
            let dup = shuffled[rng.below(shuffled.len() as u64) as usize].clone();
            shuffled.push(dup);
            let doc = apply_all(&shuffled);
            assert_eq!(doc.nodes, reference.nodes, "round {round}");
            assert_eq!(doc.nodes[&node].tree(field::PROPS), reference.nodes[&node].tree(field::PROPS));
            assert_eq!(snapshot_message(&doc), snapshot_message(&reference), "snapshot bytes, round {round}");
        }
    }
}

#[test]
fn a_hidden_register_stays_hidden_across_a_snapshot() {
    let dir = temp_dir("hidden");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://h.example/");
    txn.commit(vec![create, set_at(tab, props_path(&["a", "b"]), Leaf::Int(1))]).unwrap();
    let mut txn = journal.begin().unwrap();
    txn.commit(vec![OpBody::Set { node: tab, field: field::PROPS, path: props_path(&["a"]), enc: Vec::new() }])
        .unwrap();
    journal.close().unwrap(); // the snapshot drops the hidden /a/b
    let mut doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    let props_regs = |doc: &Doc| doc.nodes[&tab].fields.keys().filter(|k| k.field == field::PROPS).count();
    assert_eq!(props_regs(&doc), 1, "only the /a tombstone");
    // A replica that still holds an older /a/b applies it late: it must stay hidden.
    let older = Op {
        id: NodeId { actor: ActorId(7), counter: 1 },
        hlc: Hlc { ms: 1, logical: 0 },
        body: set_at(tab, props_path(&["a", "b"]), Leaf::Int(2)),
    };
    doc.apply(&older);
    assert!(doc.nodes[&tab].tree(field::PROPS).is_empty());
}
```

  Adjust while writing: `Op`'s fields (`id`, `hlc`, `body`); `Doc::new(DocId)` and `snapshot_message(&Doc) -> Vec<u8>` (private in `journal.rs`, reachable from `journal/tests.rs`) exist. A duplicate op must be idempotent (equal stamp: `set_reg` keeps the first), which the convergence test covers.
- [ ] **Step 2:** `cargo test -j 8 -p manifold --bin manifold store::journal` — Expected: `a_hidden_register_stays_hidden…` FAILS (the snapshot still keeps `/a/b`). If the convergence test fails, record why before changing anything; it must pass after Step 3.
- [ ] **Step 3:** `journal.rs`: the snapshot loop iterates `node.visible_regs()` (`for (key, reg) in node.visible_regs()`), and `doc_actors` uses `node.visible_regs().map(|(_, reg)| reg.stamp.actor)` so the actor table matches what the snapshot writes. Leave `Doc::prune`'s `node.fields.values()` on *all* registers: the version table then keeps actors of hidden registers, which is conservative and correct.
- [ ] **Step 4:** `cargo test -j 8 -p manifold --bin manifold` — Expected: all pass. Then `cargo fmt --all`, clippy, and the suite again.
- [ ] **Step 5 (docs).** (a) Spec `docs/superpowers/specs/2026-10-24-properties-design.md`: replace `Value`/`Box<[u8]>` wording with `Leaf`/`TreePath`; say plan 1 is the store only and `Kind::card()` (D7) moves to plan 3; front matter `status: active`. (b) `docs/decisions/2026-10-24-properties-tags-json.md` "Open": the path-key wire encoding is resolved as inline per op, no interned table. (c) `grep -rn "format 3\|version 3\|FORMAT_VERSION" README.md docs PARKED.md ROADMAP.md CLAUDE.md` and fix wording that says the journal format is 3. (d) Set this plan's `status: done`. Name the stale files found in the commit message.
- [ ] **Step 6:** Commit: `store: snapshots drop hidden registers; convergence tests; docs` + trailer.

## Self-review

- **Spec coverage:** §1.1 path and leaf (T1), §1.2 visibility and snapshot (T4, T5), §1.3 wire and version (T3), §6 store tests (T1, T3, T4, T5), D1 (T3), §1.5 delete (T3). Phases 2–4 are plans 2–4.
- **Type consistency:** `RegKey`, `TreePath`, `Leaf`, `Seg`, `OpBody::set`, `Node::reg`/`visible_regs`/`tree` are named identically in every task.
- **Checked against the repo while writing:** `id_message`/`decode_id` signatures and visibility, `Doc::new(DocId)`, `snapshot_message(&Doc)`, `Journal::close` compacts, `Journal::read(bytes) -> Doc`, `Node::placeholder()` (no `Default`), `Stamp = (hlc, actor)` so test ops need distinct stamps.
