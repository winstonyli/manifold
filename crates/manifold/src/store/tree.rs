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
