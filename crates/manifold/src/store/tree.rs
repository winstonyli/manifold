//! Tree values (spec 2026-10-24-properties-design.md, sections 1.1 and 1.2): the address of a register inside a
//! node's tree, and the leaf values a register holds.

#![allow(dead_code, reason = "the app reads trees from plan 2 on (mirror trees); remove this line there")]

use std::collections::BTreeMap;

use super::model::{decode_id, field, id_message, ActorId, Node, NodeId, Reg, RegKey, Stamp};
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
                w.fixed64(T_FLOAT, if *f == 0.0 { 0.0f64 } else { *f }.to_bits());
                // -0.0 and 0.0 are one value
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

/// The registers no ancestor register overrides, in key order (tombstones included). A register is hidden when a
/// register at a proper-prefix path has a higher stamp; hidden registers still hide, but whatever they hide their hider
/// hides too, so a stack of cumulative newest stamps decides each one in a single pass.
pub fn visible<'a>(regs: impl Iterator<Item = (&'a RegKey, &'a Reg)>) -> impl Iterator<Item = (&'a RegKey, &'a Reg)> {
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

    /// The registers of a field's tree that count (see `visible`), tombstones included.
    pub fn visible_regs(&self) -> impl Iterator<Item = (&RegKey, &Reg)> {
        visible(self.fields.iter())
    }

    /// A tree field's live leaves in path order with their stamps: visible, not a tombstone.
    pub fn tree(&self, no: u32) -> Vec<(TreePath, Leaf, Stamp)> {
        let own = self.fields.range(RegKey::of(no)..).take_while(|(k, _)| k.field == no);
        visible(own).filter_map(|(k, r)| Some((k.path.clone(), Leaf::decode(&r.enc).ok()??, r.stamp))).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ops::Bound::Excluded;

    use super::*;
    use crate::store::model::Hlc;
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
                !node
                    .fields
                    .iter()
                    .any(|(a, ar)| a.field == k.field && a.path.is_ancestor_of(&k.path) && ar.stamp > r.stamp)
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
        node.fields.insert(at(crate::store::model::field::PROPS, &["a"]), reg(1, Some(Leaf::Int(1))));
        node.fields.insert(at(crate::store::model::field::PROPS, &["a", "b"]), reg(2, Some(Leaf::Int(2))));
        node.fields.insert(at(crate::store::model::field::PROPS, &["gone"]), reg(3, None)); // a tombstone is no leaf
        let trees = node.trees();
        assert_eq!(trees.len(), 1);
        assert_eq!(trees[&crate::store::model::field::PROPS].len(), 2);
        assert!(Node::placeholder().trees().is_empty());
    }
}
