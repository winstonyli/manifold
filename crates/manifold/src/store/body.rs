//! The BODY tree: an inline Json card's document (spec 2026-10-24-properties-design.md section 4).
//!
//! The document sits at `[Key("v")]`. A scalar is a leaf; an object has `Key` children; an empty object or array is
//! `Leaf::EmptyObj` / `Leaf::EmptyArr`; a non-empty array has `Id(e)` children, each with `Key("o")` = `Leaf::Str(order)`
//! and `Key("v")` = the element. A container with any `Id` child reads as an array (its `Key` children are ignored);
//! elements sort by `(o, id)`; an element with no `v` is skipped and one with no `o` sorts first.

use serde_json::{Map, Value};

use super::model::NodeId;
use super::tree::{resolved, Leaf, Seg, Tree, TreePath};
use super::tree_json::{leaf_json, number_leaf, MAX_DEPTH};

pub const INLINE_MAX_LEAVES: usize = 10_000;

/// The order key of element `i` of a fresh array: fixed-width decimal, so byte order is numeric order, with room
/// between neighbours for later inserts.
pub fn initial_order(i: usize) -> String {
    format!("{:08}", (i + 1) * 1000)
}

fn key(name: &str) -> Seg {
    Seg::Key(name.to_owned())
}

fn root() -> TreePath {
    TreePath::default().child(&key("v"))
}

type Entries<'a> = [(Vec<Seg>, &'a Leaf)];

/// The document `tree` holds; `Null` for an empty tree.
pub fn tree_to_value(tree: &Tree) -> Value {
    let entries: Vec<(Vec<Seg>, &Leaf)> = resolved(tree)
        .map(|(p, l)| (p.segments(), l))
        .filter(|(segs, _)| matches!(segs.first(), Some(Seg::Key(k)) if k == "v"))
        .collect();
    if entries.is_empty() {
        Value::Null
    } else {
        build(&entries, 1)
    }
}

/// The run of `entries` whose segment at `depth` is `Key(name)`.
fn child_run<'a>(entries: &'a Entries<'a>, depth: usize, name: &str) -> &'a Entries<'a> {
    let is = |s: &Vec<Seg>| matches!(s.get(depth), Some(Seg::Key(k)) if k == name);
    let start = entries.iter().position(|(s, _)| is(s)).unwrap_or(entries.len());
    let len = entries[start..].iter().take_while(|(s, _)| is(s)).count();
    &entries[start..start + len]
}

/// `entries` all share their first `depth` segments and are in path order, which groups children together.
fn build(entries: &Entries, depth: usize) -> Value {
    // Another replica could send a tree nested past what `value_to_tree` writes; stop before the stack does.
    if depth > 2 * MAX_DEPTH + 2 {
        return Value::Null;
    }
    if let [(segs, leaf)] = entries {
        if segs.len() == depth {
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
        let mut elements: Vec<(String, NodeId, Value)> = Vec::new();
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
            elements.push((order, *id, build(value, depth + 2)));
        }
        elements.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        return Value::Array(elements.into_iter().map(|(_, _, v)| v).collect());
    }
    let mut map = Map::new();
    for (seg, group) in groups {
        if let Seg::Key(k) = seg {
            map.insert(k.clone(), build(group, depth + 1));
        }
    }
    Value::Object(map)
}

/// `value` as a BODY tree, minting an id per array element. Refused past `MAX_DEPTH` or `INLINE_MAX_LEAVES`.
pub fn value_to_tree(value: &Value, mint: &mut dyn FnMut() -> NodeId) -> Result<Tree, String> {
    let mut tree = Tree::new();
    put(value, &root(), 1, mint, &mut tree)?;
    Ok(tree)
}

fn put(
    value: &Value,
    at: &TreePath,
    depth: usize,
    mint: &mut dyn FnMut() -> NodeId,
    tree: &mut Tree,
) -> Result<(), String> {
    if depth > MAX_DEPTH + 1 {
        return Err(format!("nested deeper than {MAX_DEPTH}"));
    }
    match value {
        Value::Object(m) if !m.is_empty() => {
            for (k, v) in m {
                put(v, &at.child(&key(k)), depth + 1, mint, tree)?;
            }
        }
        Value::Array(a) if !a.is_empty() => {
            for (i, v) in a.iter().enumerate() {
                let el = at.child(&Seg::Id(mint()));
                tree.insert(el.child(&key("o")), Leaf::Str(initial_order(i)));
                put(v, &el.child(&key("v")), depth + 1, mint, tree)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tree::{Leaf, Seg, TreePath};
    use crate::store::tree_json::MAX_DEPTH;
    use serde_json::json;

    fn id(n: u64) -> NodeId {
        NodeId { actor: crate::store::model::ActorId(1), counter: n }
    }

    fn mint_from(start: u64) -> impl FnMut() -> NodeId {
        let mut n = start;
        move || {
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
}
