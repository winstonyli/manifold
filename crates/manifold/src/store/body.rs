//! The BODY tree: an inline Json card's document (spec 2026-10-24-properties-design.md section 4).
//!
//! The document sits at `[Key("v")]`. A scalar is a leaf; an object has `Key` children; an empty object or array is
//! `Leaf::EmptyObj` / `Leaf::EmptyArr`; a non-empty array has `Id(e)` children, each with `Key("o")` = `Leaf::Str(order)`
//! and `Key("v")` = the element. A container with any `Id` child reads as an array (its `Key` children are ignored);
//! elements sort by `(o, id)`; an element with no `v` is skipped and one with no `o` sorts first.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::model::{ActorId, NodeId};
use super::tree::{resolved, Leaf, Seg, Tree, TreePath};
use super::tree_json::{leaf_json, number_leaf, MAX_DEPTH};
use crate::json_view::{child_pointer, pointer_tokens};

pub const INLINE_MAX_LEAVES: usize = 10_000;

/// The order key of element `i` of a fresh array: fixed-width decimal, so byte order is numeric order, with room
/// between neighbours for later inserts.
pub fn initial_order(i: usize) -> String {
    format!("{:08}", (i + 1) * 1000)
}

/// An order key strictly between `a` and `b` (`None`: no bound on that side), or `None` when none fits. Keys are
/// digit strings. A neighbour that is an 8-digit multiple of 1000 (what `initial_order` writes) gets the next step of
/// 1000 on the open side, so appending or prepending never lengthens keys; anything else gets the midpoint digit at
/// the first position where the bounds leave room, which never ends in `'0'` (so a lower key always exists).
pub fn between(a: Option<&str>, b: Option<&str>) -> Option<String> {
    let step =
        |s: &str| (s.len() == 8 && s.bytes().all(|c| c.is_ascii_digit())).then(|| s.parse::<u64>().ok()).flatten();
    match (a, b) {
        (None, None) => return Some(initial_order(0)),
        (Some(a), None) => {
            if let Some(n) = step(a).filter(|n| n % 1000 == 0 && n + 1000 < 100_000_000) {
                return Some(format!("{:08}", n + 1000));
            }
        }
        (None, Some(b)) => {
            if let Some(n) = step(b).filter(|n| n % 1000 == 0 && *n >= 2000) {
                return Some(format!("{:08}", n - 1000));
            }
        }
        (Some(a), Some(b)) if a >= b => return None,
        _ => {}
    }
    let (a, b) = (a.unwrap_or("").as_bytes(), b.map(str::as_bytes));
    let digit = |c: Option<&u8>| c.map_or(0, |c| i32::from(c.wrapping_sub(b'0')));
    let (mut out, mut open) = (Vec::new(), b.is_none());
    for i in 0..a.len().max(b.map_or(0, <[u8]>::len)) + 2 {
        let lo = digit(a.get(i));
        let hi = if open { 10 } else { digit(b.and_then(|b| b.get(i))) };
        if hi - lo > 1 {
            out.push(b'0' + ((lo + hi) / 2) as u8);
            return String::from_utf8(out).ok();
        }
        out.push(b'0' + lo as u8);
        open |= hi > lo;
    }
    None
}

fn key(name: &str) -> Seg {
    Seg::Key(name.to_owned())
}

/// The path of the document itself.
pub fn root() -> TreePath {
    TreePath::default().child(&key("v"))
}

type Entries<'a> = [(Vec<Seg>, &'a Leaf)];

/// The document `tree` holds; `Null` for an empty tree. A `Ref` leaf reads as `{"$ref": "<id>"}`.
pub fn tree_to_value(tree: &Tree) -> Value {
    document(tree, &mut None)
}

/// The document as a level shows it: a `Ref` leaf is its id as a string and its pointer is recorded in the map, so a
/// typed `{"$ref": "5"}` object (no `Ref` leaf) stays plain data.
pub fn tree_to_value_refs(tree: &Tree) -> (Value, BTreeMap<String, NodeId>) {
    let mut refs = Some(BTreeMap::new());
    let value = document(tree, &mut refs);
    (value, refs.unwrap_or_default())
}

fn document(tree: &Tree, refs: &mut Option<BTreeMap<String, NodeId>>) -> Value {
    let entries: Vec<(Vec<Seg>, &Leaf)> = resolved(tree)
        .map(|(p, l)| (p.segments(), l))
        .filter(|(segs, _)| matches!(segs.first(), Some(Seg::Key(k)) if k == "v"))
        .collect();
    if entries.is_empty() {
        Value::Null
    } else {
        build(&entries, 1, "", refs)
    }
}

/// The run of `entries` whose segment at `depth` is `Key(name)`.
fn child_run<'a>(entries: &'a Entries<'a>, depth: usize, name: &str) -> &'a Entries<'a> {
    let is = |s: &Vec<Seg>| matches!(s.get(depth), Some(Seg::Key(k)) if k == name);
    let start = entries.iter().position(|(s, _)| is(s)).unwrap_or(entries.len());
    let len = entries[start..].iter().take_while(|(s, _)| is(s)).count();
    &entries[start..start + len]
}

/// `entries` all share their first `depth` segments and are in path order, which groups children together. `pointer`
/// is where they sit in the document; with `refs` a `Ref` leaf is recorded there and read as its id string.
fn build(entries: &Entries, depth: usize, pointer: &str, refs: &mut Option<BTreeMap<String, NodeId>>) -> Value {
    // Another replica could send a tree nested past what `value_to_tree` writes; stop before the stack does.
    if depth > 2 * MAX_DEPTH + 2 {
        return Value::Null;
    }
    if let [(segs, leaf)] = entries {
        if segs.len() == depth {
            if let (Leaf::Ref(id), Some(refs)) = (leaf, refs.as_mut()) {
                refs.insert(pointer.to_owned(), *id);
                return Value::String(id.to_string());
            }
            return leaf_json(leaf);
        }
    }
    let mut groups: Vec<(&Seg, &Entries)> = Vec::new();
    let mut start = 0;
    while start < entries.len() {
        let Some(seg) = entries[start].0.get(depth) else {
            start += 1; // a scalar beside children cannot happen after `resolved`; skip rather than panic
            continue;
        };
        let len = entries[start..].iter().take_while(|(s, _)| s.get(depth) == Some(seg)).count();
        groups.push((seg, &entries[start..start + len]));
        start += len;
    }
    if groups.iter().any(|(seg, _)| matches!(seg, Seg::Id(_))) {
        let mut elements: Vec<(String, NodeId, &Entries)> = Vec::new();
        for (seg, group) in &groups {
            let Seg::Id(id) = seg else { continue };
            let value = child_run(group, depth + 1, "v");
            if value.is_empty() {
                continue;
            }
            let order = match child_run(group, depth + 1, "o").first() {
                Some((_, Leaf::Str(s))) => s.clone(),
                _ => String::new(),
            };
            elements.push((order, *id, value));
        }
        elements.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        return Value::Array(
            elements
                .into_iter()
                .enumerate()
                .map(|(i, (_, _, run))| build(run, depth + 2, &child_pointer(pointer, &i.to_string()), refs))
                .collect(),
        );
    }
    let mut map = Map::new();
    for (seg, group) in groups {
        if let Seg::Key(k) = seg {
            map.insert(k.clone(), build(group, depth + 1, &child_pointer(pointer, k), refs));
        }
    }
    Value::Object(map)
}

/// `value` as a BODY tree, minting an id per array element; `mint` gets the element's JSON pointer. Refused past
/// `MAX_DEPTH` or `INLINE_MAX_LEAVES`.
pub fn value_to_tree(value: &Value, mint: &mut dyn FnMut(&str) -> NodeId) -> Result<Tree, String> {
    value_to_tree_at(value, &root(), "", 1, mint)
}

/// `value` as the subtree at `at`, which is the entry at `pointer`, `base_depth` levels below the document root (the
/// pointer's token count plus 1): `MAX_DEPTH` counts from the root.
pub fn value_to_tree_at(
    value: &Value,
    at: &TreePath,
    pointer: &str,
    base_depth: usize,
    mint: &mut dyn FnMut(&str) -> NodeId,
) -> Result<Tree, String> {
    let mut tree = Tree::new();
    put(value, at, pointer, base_depth, mint, &mut tree)?;
    Ok(tree)
}

fn put(
    value: &Value,
    at: &TreePath,
    pointer: &str,
    depth: usize,
    mint: &mut dyn FnMut(&str) -> NodeId,
    tree: &mut Tree,
) -> Result<(), String> {
    if depth > MAX_DEPTH + 1 {
        return Err(format!("nested deeper than {MAX_DEPTH}"));
    }
    match value {
        Value::Object(m) if !m.is_empty() => {
            for (k, v) in m {
                put(v, &at.child(&key(k)), &child_pointer(pointer, k), depth + 1, mint, tree)?;
            }
        }
        Value::Array(a) if !a.is_empty() => {
            for (i, v) in a.iter().enumerate() {
                let child = child_pointer(pointer, &i.to_string());
                let el = at.child(&Seg::Id(mint(&child)));
                tree.insert(el.child(&key("o")), Leaf::Str(initial_order(i)));
                put(v, &el.child(&key("v")), &child, depth + 1, mint, tree)?;
            }
        }
        scalar => {
            let leaf = match scalar {
                Value::Null => Leaf::Null,
                Value::Bool(b) => Leaf::Bool(*b),
                Value::Number(n) => number_leaf(n),
                Value::String(s) => Leaf::Str(s.clone()),
                Value::Object(_) => Leaf::EmptyObj,
                Value::Array(_) => Leaf::EmptyArr,
            };
            tree.insert(at.clone(), leaf);
        }
    }
    if tree.len() >= INLINE_MAX_LEAVES {
        return Err(format!("more than {INLINE_MAX_LEAVES} leaves"));
    }
    Ok(())
}

/// An id that depends only on the element's JSON pointer, so two replicas detaching one file write the same
/// registers and an array does not double (actor 0 is never a launch's actor).
pub fn det_id(pointer: &str) -> NodeId {
    let hash =
        pointer.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    NodeId { actor: ActorId(0), counter: hash }
}

/// One element of a BODY array: its id, order key and the `Id(e)` path.
pub struct Element {
    pub id: NodeId,
    pub order: String,
    pub at: TreePath,
}

/// The elements of the array at `array` that have a value, in the order `tree_to_value` reads them.
pub fn elements(tree: &Tree, array: &TreePath) -> Vec<Element> {
    let depth = array.segments().len();
    let mut found: std::collections::BTreeMap<NodeId, (String, bool)> = std::collections::BTreeMap::new();
    for (path, leaf) in tree.range(array.clone()..array.subtree_end()) {
        let segs = path.segments();
        let (Some(Seg::Id(e)), Some(Seg::Key(k))) = (segs.get(depth), segs.get(depth + 1)) else { continue };
        let slot = found.entry(*e).or_default();
        match (k.as_str(), leaf) {
            ("o", Leaf::Str(order)) if segs.len() == depth + 2 => slot.0.clone_from(order),
            ("v", _) => slot.1 = true,
            _ => {}
        }
    }
    let mut out: Vec<Element> = found
        .into_iter()
        .filter(|(_, (_, has_value))| *has_value)
        .map(|(id, (order, _))| Element { id, order, at: array.child(&Seg::Id(id)) })
        .collect();
    out.sort_by(|a, b| (&a.order, a.id).cmp(&(&b.order, b.id)));
    out
}

/// The path of the entry at JSON `pointer` of the document `tree` holds, or `None` when it does not resolve. An
/// array index resolves through the element to its `v` path.
pub fn pointer_path(tree: &Tree, pointer: &str) -> Option<TreePath> {
    let mut at = root();
    for token in pointer_tokens(pointer) {
        let els = elements(tree, &at);
        at = if els.is_empty() {
            at.child(&key(&token))
        } else {
            els.get(token.parse::<usize>().ok()?)?.at.child(&key("v"))
        };
    }
    tree.range(at.clone()..at.subtree_end()).next().is_some().then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tree::{Leaf, Seg, TreePath};
    use crate::store::tree_json::MAX_DEPTH;
    use serde_json::json;

    fn id(n: u64) -> NodeId {
        NodeId { actor: crate::store::model::ActorId(1), counter: n }
    }

    fn mint_from(start: u64) -> impl FnMut(&str) -> NodeId {
        let mut n = start;
        move |_| {
            n += 1;
            id(n)
        }
    }

    fn root() -> TreePath {
        TreePath::default().child(&Seg::Key("v".to_owned()))
    }

    fn round(v: &Value) -> Value {
        tree_to_value(&value_to_tree(v, &mut mint_from(0)).unwrap())
    }

    #[test]
    fn every_root_shape_round_trips() {
        for v in [
            json!(null),
            json!(true),
            json!(3),
            json!(-2.5),
            json!("s"),
            json!([]),
            json!({}),
            json!([1, [2, [3]], {"o": 1, "v": 2}]),
            json!({"o": "x", "v": [], "$ref": "r", "@1": 2, "a": {"b": {}}}),
            json!(18446744073709551615u64),
        ] {
            assert_eq!(round(&v), v, "{v}");
        }
    }

    #[test]
    fn arrays_keep_their_order_and_get_distinct_ids() {
        let tree = value_to_tree(&json!(["a", "b", "c"]), &mut mint_from(10)).unwrap();
        let ids: std::collections::BTreeSet<_> = tree
            .keys()
            .filter_map(|p| {
                p.segments().into_iter().find_map(|s| match s {
                    Seg::Id(i) => Some(i),
                    _ => None,
                })
            })
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(tree_to_value(&tree), json!(["a", "b", "c"]));
    }

    #[test]
    fn ties_and_gaps_in_order_are_stable() {
        let v = root();
        let el = |n: u64| v.child(&Seg::Id(id(n)));
        let key = |p: &TreePath, k: &str| p.child(&Seg::Key(k.into()));
        let mut tree = Tree::new();
        // 7 and 5 tie on "b"; 9 has no order; 4 has an order but no value.
        for (n, o) in [(7, "b"), (5, "b")] {
            tree.insert(key(&el(n), "o"), Leaf::Str(o.into()));
            tree.insert(key(&el(n), "v"), Leaf::Int(n as i64));
        }
        tree.insert(key(&el(9), "v"), Leaf::Int(9));
        tree.insert(key(&el(4), "o"), Leaf::Str("a".into()));
        assert_eq!(tree_to_value(&tree), json!([9, 5, 7]), "no order first, then order, then id");
    }

    #[test]
    fn a_ref_leaf_is_an_id_string_plus_a_marked_pointer() {
        let mut tree = value_to_tree(&json!({"x": 0, "l": [0, 1]}), &mut mint_from(0)).unwrap();
        for (pointer, target) in [("/x", 5), ("/l/1", 6)] {
            tree.insert(pointer_path(&tree, pointer).unwrap(), Leaf::Ref(id(target)));
        }
        let (value, refs) = tree_to_value_refs(&tree);
        assert_eq!(value, json!({"x": id(5).to_string(), "l": [0, id(6).to_string()]}));
        assert_eq!(refs, BTreeMap::from([("/x".to_owned(), id(5)), ("/l/1".to_owned(), id(6))]));
        assert_eq!(tree_to_value(&tree)["x"], json!({"$ref": id(5).to_string()}), "the export form is unchanged");
    }

    #[test]
    fn a_typed_dollar_ref_string_is_not_a_chip() {
        let tree = value_to_tree(&json!({"x": {"$ref": "5"}}), &mut mint_from(0)).unwrap();
        let (value, refs) = tree_to_value_refs(&tree);
        assert_eq!(value, json!({"x": {"$ref": "5"}}));
        assert!(refs.is_empty());
    }

    #[test]
    fn a_scalar_with_a_child_shows_the_child() {
        let mut tree = Tree::new();
        tree.insert(root(), Leaf::Int(1));
        tree.insert(root().child(&Seg::Key("k".into())), Leaf::Int(2));
        assert_eq!(tree_to_value(&tree), json!({"k": 2}), "children win (spec 1.2)");
    }

    #[test]
    fn an_assembled_ref_is_plain_object_data() {
        let mut tree = Tree::new();
        tree.insert(root(), Leaf::Ref(id(5)));
        assert_eq!(tree_to_value(&tree), json!({"$ref": id(5).to_string()}));
        assert_eq!(round(&json!({"$ref": "5"})), json!({"$ref": "5"}), "and a typed one stays a string, not a Ref");
    }

    #[test]
    fn too_many_leaves_or_too_deep_is_refused() {
        let wide = Value::Array((0..INLINE_MAX_LEAVES).map(Value::from).collect());
        assert!(value_to_tree(&wide, &mut mint_from(0)).unwrap_err().contains("leaves"));
        let mut deep = json!(1);
        for _ in 0..MAX_DEPTH + 1 {
            deep = json!([deep]);
        }
        assert!(value_to_tree(&deep, &mut mint_from(0)).unwrap_err().contains("deeper"));
    }

    #[test]
    fn a_hostile_deep_tree_assembles_without_overflowing() {
        let mut path = root();
        for n in 0..5_000 {
            path = path.child(&Seg::Key(format!("k{n}")));
        }
        let tree = Tree::from([(path, Leaf::Int(1))]);
        let _ = tree_to_value(&tree); // must return, not overflow
    }

    #[test]
    fn initial_orders_sort_as_numbers() {
        let keys: Vec<String> = [0, 1, 9, 10, 99, 9_999].iter().map(|&i| initial_order(i)).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(keys.iter().collect::<std::collections::BTreeSet<_>>().len(), keys.len(), "all distinct");
    }

    fn ok(a: Option<&str>, b: Option<&str>) -> String {
        let k = between(a, b).expect("room");
        assert!(a.is_none_or(|a| a < k.as_str()) && b.is_none_or(|b| k.as_str() < b), "{a:?} < {k} < {b:?}");
        k
    }

    #[test]
    fn between_steps_ends_and_splits_gaps() {
        assert_eq!(ok(None, None), "00001000");
        assert_eq!(ok(Some("00003000"), None), "00004000", "an append steps by 1000");
        assert_eq!(ok(None, Some("00003000")), "00002000", "a prepend steps down");
        assert_eq!(ok(Some("00001000"), Some("00002000")), "000015");
        assert_eq!(ok(None, Some("00001000")), "000005", "no 1000 step fits below 1000");
    }

    #[test]
    fn between_refuses_when_nothing_fits() {
        assert_eq!(between(Some("b"), Some("b")), None, "equal neighbours");
        assert_eq!(between(Some("c"), Some("b")), None);
        assert_eq!(between(None, Some("000")), None, "nothing sorts below an all-zero key");
    }

    #[test]
    fn many_random_inserts_stay_strictly_ordered_and_appends_stay_short() {
        let mut keys: Vec<String> = Vec::new();
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..2_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let at = (seed >> 33) as usize % (keys.len() + 1);
            let k = ok(at.checked_sub(1).map(|i| keys[i].as_str()), keys.get(at).map(String::as_str));
            keys.insert(at, k);
            assert!(keys.windows(2).all(|w| w[0] < w[1]));
        }
        let mut tail: Vec<String> = Vec::new();
        for _ in 0..3_000 {
            let k = ok(tail.last().map(String::as_str), None);
            tail.push(k);
        }
        assert!(tail.iter().all(|k| k.len() == 8), "appends step, they don't grow");
    }

    #[test]
    fn pointer_path_resolves_keys_and_array_indexes() {
        let tree = value_to_tree(&json!({"a": [10, 20], "/x": 1}), &mut mint_from(0)).unwrap();
        let key = |k: &str| Seg::Key(k.to_owned());
        assert_eq!(pointer_path(&tree, ""), Some(root()));
        assert_eq!(pointer_path(&tree, "/a"), Some(root().child(&key("a"))));
        assert_eq!(pointer_path(&tree, "/~1x"), Some(root().child(&key("/x"))));
        let second = pointer_path(&tree, "/a/1").unwrap();
        assert_eq!(tree.get(&second), Some(&Leaf::Int(20)));
        assert_eq!(pointer_path(&tree, "/a/2"), None);
        assert_eq!(pointer_path(&tree, "/nope"), None);
    }

    #[test]
    fn elements_come_in_document_order() {
        let a = root().child(&Seg::Key("a".into()));
        let mut tree = Tree::new();
        for (n, o) in [(7, "b"), (3, "b"), (9, "a")] {
            let el = a.child(&Seg::Id(id(n)));
            tree.insert(el.child(&Seg::Key("o".into())), Leaf::Str(o.into()));
            tree.insert(el.child(&Seg::Key("v".into())), Leaf::Int(n as i64));
        }
        tree.insert(a.child(&Seg::Id(id(4))).child(&Seg::Key("o".into())), Leaf::Str("0".into())); // no value
        let order: Vec<u64> = elements(&tree, &a).iter().map(|e| e.id.counter).collect();
        assert_eq!(order, [9, 3, 7]);
        assert_eq!(tree_to_value(&tree), json!({"a": [9, 3, 7]}));
    }

    #[test]
    fn detach_ids_depend_only_on_the_pointer() {
        assert_eq!(det_id("/a/0"), det_id("/a/0"));
        assert_ne!(det_id("/a/0"), det_id("/a/1"));
    }

    #[test]
    fn mint_sees_each_elements_pointer() {
        let mut seen = Vec::new();
        value_to_tree(&json!([1, [2], {"k": [3]}]), &mut |p| {
            seen.push(p.to_owned());
            id(seen.len() as u64)
        })
        .unwrap();
        assert_eq!(seen, ["/0", "/1", "/1/0", "/2", "/2/k/0"]);
    }
}
