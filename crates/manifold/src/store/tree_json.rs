//! JSON for a node's tree fields (spec 2026-10-24-properties-design.md section 2.3). Dialect of PROPS; plan 4 adds
//! BODY's (empty containers and arrays are values there, and `$ref` is plain data).

use serde_json::{Map, Number, Value};

use super::model::NodeId;
use super::tree::{resolved, Leaf, Seg, Tree, TreePath};

/// Nesting deeper than this, or more leaves than this, is not a tree the app wrote.
pub const MAX_DEPTH: usize = 64;
pub const MAX_LEAVES: usize = 10_000;

/// `$ref`, `$$ref`, ...: keys that would read as a Ref object, so export adds a `$` and import takes one off.
fn is_ref_like(key: &str) -> bool {
    key.starts_with('$') && key.trim_start_matches('$') == "ref"
}

fn seg_text(seg: &Seg) -> String {
    match seg {
        Seg::Key(k) if k.starts_with('@') || is_ref_like(k) => format!("{}{k}", &k[..1]),
        Seg::Key(k) => k.clone(),
        Seg::Id(id) => format!("@{id}"),
    }
}

fn leaf_json(leaf: &Leaf) -> Value {
    match leaf {
        Leaf::Null => Value::Null,
        Leaf::Bool(b) => Value::Bool(*b),
        Leaf::Int(i) => Value::from(*i),
        Leaf::Float(f) => Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Leaf::Num(text) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_number)
            .unwrap_or_else(|| Value::String(text.clone())),
        Leaf::Str(s) => Value::String(s.clone()),
        Leaf::Ref(id) => serde_json::json!({ "$ref": id.to_string() }),
        Leaf::EmptyObj => Value::Object(Map::new()),
        Leaf::EmptyArr => Value::Array(Vec::new()),
    }
}

/// The resolved leaves as nested objects. A leaf nested deeper than `MAX_DEPTH` is left out: import refuses it,
/// and the nested `Value` could overflow the stack when dropped.
pub fn tree_to_json(tree: &Tree) -> Value {
    let mut root = Map::new();
    for (path, leaf) in resolved(tree) {
        let segs = path.segments();
        if segs.len() > MAX_DEPTH {
            continue;
        }
        let Some((last, parents)) = segs.split_last() else { continue };
        let mut at = &mut root;
        for seg in parents {
            let slot = at.entry(seg_text(seg)).or_insert_with(|| Value::Object(Map::new()));
            at = slot.as_object_mut().expect("`resolved` leaves no leaf above another");
        }
        at.insert(seg_text(last), leaf_json(leaf));
    }
    Value::Object(root)
}

/// `value` as a tree. `resolve` maps an id text to this document's id, or `None` to drop what names it. Empty
/// containers are dropped and a non-empty array is an error (PROPS holds neither).
pub fn tree_from_json(value: &Value, resolve: &dyn Fn(&str) -> Option<NodeId>) -> Result<Tree, String> {
    let Value::Object(map) = value else { return Err("not an object".to_owned()) };
    let mut tree = Tree::new();
    walk(map, &TreePath::default(), resolve, &mut tree, 1)?;
    Ok(tree)
}

fn walk(
    map: &Map<String, Value>,
    at: &TreePath,
    resolve: &dyn Fn(&str) -> Option<NodeId>,
    tree: &mut Tree,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("nested deeper than {MAX_DEPTH}"));
    }
    for (name, value) in map {
        let seg = match name.strip_prefix('@') {
            Some(rest) if rest.starts_with('@') => Seg::Key(rest.to_owned()),
            Some(rest) => match resolve(rest) {
                Some(id) => Seg::Id(id),
                None => continue,
            },
            None if is_ref_like(name) && name.len() > 4 => Seg::Key(name[1..].to_owned()),
            None => Seg::Key(name.clone()),
        };
        let path = at.child(&seg);
        let leaf = match value {
            Value::Null => Leaf::Null,
            Value::Bool(b) => Leaf::Bool(*b),
            Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                (Some(i), _) => Leaf::Int(i),
                (None, Some(f)) if f.is_finite() && !n.is_u64() => Leaf::Float(f),
                _ => Leaf::Num(n.to_string()),
            },
            Value::String(s) => Leaf::Str(s.clone()),
            Value::Object(m) if m.len() == 1 && m.contains_key("$ref") => match m["$ref"].as_str().and_then(resolve) {
                Some(id) => Leaf::Ref(id),
                None => continue,
            },
            Value::Object(m) if m.is_empty() => continue,
            Value::Object(m) => {
                walk(m, &path, resolve, tree, depth + 1)?;
                continue;
            }
            Value::Array(a) if a.is_empty() => continue,
            Value::Array(_) => return Err(format!("{name:?}: an array")),
        };
        tree.insert(path, leaf);
        if tree.len() > MAX_LEAVES {
            return Err(format!("more than {MAX_LEAVES} leaves"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::ActorId;

    fn key(text: &str) -> Seg {
        Seg::Key(text.to_owned())
    }
    fn at(segs: &[Seg]) -> TreePath {
        TreePath::of(segs)
    }
    fn same_ids(s: &str) -> Option<NodeId> {
        s.parse().ok()
    }

    #[test]
    fn a_tree_round_trips_through_json() {
        let mut tree = Tree::new();
        let card = NodeId { actor: ActorId(3), counter: 9 };
        for (segs, leaf) in [
            (vec![key("a"), key("b")], Leaf::Int(-4)),
            (vec![key("a"), key("c")], Leaf::Str("x".into())),
            (vec![key("f")], Leaf::Float(1.5)),
            (vec![key("big")], Leaf::Num("18446744073709551615".into())),
            (vec![key("n")], Leaf::Null),
            (vec![key("t")], Leaf::Bool(true)),
            (vec![key("r")], Leaf::Ref(card)),
            (vec![key("m"), Seg::Id(card)], Leaf::Bool(true)),
            (vec![key("@at"), key("")], Leaf::Int(1)),
            (vec![key("nul\0"), key("ünï")], Leaf::Int(2)),
        ] {
            tree.insert(at(&segs), leaf);
        }
        let json = tree_to_json(&tree);
        assert_eq!(tree_from_json(&json, &same_ids).unwrap(), tree);
    }

    #[test]
    fn export_shows_children_over_a_scalar() {
        let mut tree = Tree::new();
        tree.insert(at(&[key("a")]), Leaf::Int(1));
        tree.insert(at(&[key("a"), key("b")]), Leaf::Int(2));
        assert_eq!(tree_to_json(&tree), serde_json::json!({"a": {"b": 2}}));
    }

    #[test]
    fn import_resolves_ids_through_the_callback_and_drops_the_unknown() {
        let json = serde_json::json!({"r": {"$ref": "nope"}, "@nope": {"x": 1}, "keep": 1});
        let tree = tree_from_json(&json, &|_| None).unwrap();
        assert_eq!(tree.keys().cloned().collect::<Vec<_>>(), vec![at(&[key("keep")])]);
    }

    #[test]
    fn import_refuses_what_would_exhaust_it() {
        let mut deep = serde_json::json!(1);
        for _ in 0..MAX_DEPTH + 2 {
            deep = serde_json::json!({ "k": deep });
        }
        assert!(tree_from_json(&deep, &same_ids).is_err());
        let wide: Map<String, Value> = (0..MAX_LEAVES + 1).map(|i| (i.to_string(), serde_json::json!(i))).collect();
        assert!(tree_from_json(&Value::Object(wide), &same_ids).is_err());
        assert!(tree_from_json(&serde_json::json!({"a": [1, 2]}), &same_ids).is_err());
        assert!(tree_from_json(&serde_json::json!([1]), &same_ids).is_err());
        assert!(tree_from_json(&serde_json::json!({"a": {}, "b": []}), &same_ids).unwrap().is_empty());
    }

    #[test]
    fn a_key_named_ref_survives_a_round_trip() {
        let mut tree = Tree::new();
        for (parent, name, leaf) in [
            ("x", "$ref", Leaf::Str("hello".into())),
            ("y", "$ref", Leaf::Int(3)),
            ("z", "$$ref", Leaf::Int(3)),
            ("w", "$refx", Leaf::Int(4)),
        ] {
            tree.insert(at(&[key(parent), key(name)]), leaf);
        }
        tree.insert(at(&[key("$ref")]), Leaf::Int(5));
        let json = tree_to_json(&tree);
        assert_eq!(tree_from_json(&json, &same_ids).unwrap(), tree);
    }

    #[test]
    fn export_leaves_out_what_import_would_refuse() {
        let mut tree = Tree::new();
        let long = |tag: &str, n: usize| at(&(0..n).map(|i| key(&format!("{tag}{i}"))).collect::<Vec<_>>());
        tree.insert(long("a", MAX_DEPTH), Leaf::Int(1));
        tree.insert(long("b", MAX_DEPTH + 1), Leaf::Int(2));
        tree.insert(long("c", 5000), Leaf::Int(3));
        let json = tree_to_json(&tree);
        assert_eq!(tree_from_json(&json, &same_ids).unwrap().len(), 1, "only the 64-deep leaf is exported");
    }
}
