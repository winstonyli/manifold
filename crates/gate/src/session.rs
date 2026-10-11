//! The two-instance test's JSON asserts (were scripts/compare_sessions.py and scripts/session_asserts.py), over
//! EXPORTSESSION documents (`{"nodes": [...], "local": ...}`) and EXPORTSTATE files (`mirror_groups`, `mirror_names`,
//! `mirror_texts`, `mirror_pos`: what that instance shows, by node id). Node ids are strings (the store's model); a
//! node without one is ignored by the id-based asserts.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::Value;

pub fn nodes(doc: &Value) -> &[Value] {
    doc.get("nodes").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn id(n: &Value) -> Option<&str> {
    n.get("id").and_then(Value::as_str)
}

/// A comparison key with python's `json.dumps(v, sort_keys=True)` equality: object keys sorted at every depth
/// (code-point order, which is UTF-8 byte order), scalars in serde's form, so `1` and `1.0` and `-0.0` stay distinct
/// as in python. Independent of serde_json's `preserve_order`.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner: Vec<String> =
                keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical(&m[k.as_str()]))).collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// compare_sessions.py: the two documents' shared nodes are the same multiset (`local` is personal and ignored).
/// Ok(node count) or Err(what differs, for the failure line).
pub fn same_nodes(a: &Value, b: &Value) -> Result<usize, String> {
    let (Some(na), Some(nb)) = (a.get("nodes").and_then(Value::as_array), b.get("nodes").and_then(Value::as_array))
    else {
        return Err("an export has no nodes array".to_string());
    };
    let keys = |n: &[Value]| {
        let mut k: Vec<String> = n.iter().map(canonical).collect();
        k.sort();
        k
    };
    if keys(na.as_slice()) == keys(nb.as_slice()) {
        return Ok(na.len());
    }
    // Only for the failure line: which ids differ (a duplicated id shows as a count difference).
    let by_id = |n: &[Value]| -> BTreeMap<String, String> {
        n.iter().map(|x| (id(x).unwrap_or("<no id>").to_string(), canonical(x))).collect()
    };
    let (ia, ib) = (by_id(na.as_slice()), by_id(nb.as_slice()));
    let differ: BTreeSet<&str> =
        ia.keys().chain(ib.keys()).filter(|k| ia.get(*k) != ib.get(*k)).map(String::as_str).take(5).collect();
    Err(format!("sessions differ: A has {} nodes, B has {}; differing ids: {differ:?}", na.len(), nb.len()))
}

/// session_asserts.py count_kind (has_kinds is `count_kind(..) >= want`).
pub fn count_kind(doc: &Value, kind: &str) -> usize {
    nodes(doc).iter().filter(|n| n.get("type").and_then(Value::as_str) == Some(kind)).count()
}

/// session_asserts.py new_id: the id of the one node in `after` and not in `before`.
pub fn new_id(before: &Value, after: &Value) -> Result<String, String> {
    let had: HashSet<&str> = nodes(before).iter().filter_map(id).collect();
    let new: Vec<&str> = nodes(after).iter().filter_map(id).filter(|i| !had.contains(i)).collect();
    match new.as_slice() {
        [one] => Ok((*one).to_string()),
        _ => Err(format!("expected exactly 1 new node, got {}: {new:?}", new.len())),
    }
}

pub fn has_id(doc: &Value, wanted: &str) -> bool {
    nodes(doc).iter().any(|n| id(n) == Some(wanted))
}

/// session_asserts.py group_of: None if the id is missing, Some(None) at the top level (python prints "null" for a
/// missing, null or empty group), Some(Some(group)) otherwise. The last node with the id wins, as in python's dict.
pub fn group_of(doc: &Value, wanted: &str) -> Option<Option<String>> {
    nodes(doc)
        .iter()
        .rev()
        .find(|n| id(n) == Some(wanted))
        .map(|n| n.get("group").and_then(Value::as_str).filter(|g| !g.is_empty()).map(str::to_string))
}

/// The script's way of printing a group_of result.
pub fn show_group(g: &Option<Option<String>>) -> String {
    match g {
        None => "<absent>".to_string(),
        Some(None) => "null".to_string(),
        Some(Some(g)) => g.clone(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Group,
    Name,
    Text,
    Pos,
}

impl Field {
    /// The EXPORTSTATE map.
    pub fn state_key(self) -> &'static str {
        match self {
            Field::Group => "mirror_groups",
            Field::Name => "mirror_names",
            Field::Text => "mirror_texts",
            Field::Pos => "mirror_pos",
        }
    }
    /// The EXPORTSESSION node key, and the word in messages.
    pub fn doc_key(self) -> &'static str {
        match self {
            Field::Group => "group",
            Field::Name => "name",
            Field::Text => "text",
            Field::Pos => "pos",
        }
    }
}

/// session_asserts.py same_value: all equal; lists elementwise within 1e-3 (f32 through JSON).
fn same_value(values: &[&Value]) -> bool {
    match values[0] {
        Value::Array(first) => values.iter().all(|v| {
            v.as_array().is_some_and(|v| {
                v.len() == first.len()
                    && v.iter().zip(first).all(|(a, b)| match (a.as_f64(), b.as_f64()) {
                        (Some(a), Some(b)) => (a - b).abs() <= 1e-3,
                        _ => false,
                    })
            })
        }),
        first => values.iter().all(|v| *v == first),
    }
}

fn show(v: Option<&Value>) -> String {
    v.map_or_else(|| "<absent>".to_string(), Value::to_string)
}

/// session_asserts.py field_agree: each id's field is present and the same in A's and B's EXPORTSTATE map and in the
/// document (a node without the key counts as null there). Ok or Err carries one line per id:
/// "<id> <field>: A shows <json>, B shows <json>, document <json>".
pub fn field_agree(field: Field, a: &Value, b: &Value, doc: &Value, ids: &[&str]) -> Result<Vec<String>, Vec<String>> {
    let map = |s: &Value| s.get(field.state_key()).and_then(Value::as_object).cloned().unwrap_or_default();
    let (ma, mb) = (map(a), map(b));
    let null = Value::Null;
    let mut d: HashMap<&str, &Value> = HashMap::new();
    for n in nodes(doc) {
        if let Some(i) = id(n) {
            d.insert(i, n.get(field.doc_key()).unwrap_or(&null));
        }
    }
    let mut ok = true;
    let mut lines = Vec::new();
    for i in ids {
        let seen = [ma.get(*i), mb.get(*i), d.get(i).copied()];
        lines.push(format!(
            "{i} {}: A shows {}, B shows {}, document {}",
            field.doc_key(),
            show(seen[0]),
            show(seen[1]),
            show(seen[2])
        ));
        match seen {
            [Some(x), Some(y), Some(z)] if same_value(&[x, y, z]) => {}
            _ => ok = false,
        }
    }
    if ok {
        Ok(lines)
    } else {
        Err(lines)
    }
}

/// session_asserts.py field_of: the id's value in that EXPORTSTATE map (None if absent).
pub fn field_of(state: &Value, field: Field, wanted: &str) -> Option<Value> {
    state.get(field.state_key())?.get(wanted).cloned()
}

/// session_asserts.py no_cycle: following `group` from any node never loops.
pub fn no_cycle(doc: &Value) -> Result<(), String> {
    let groups: BTreeMap<&str, Option<&str>> =
        nodes(doc).iter().filter_map(|n| Some((id(n)?, n.get("group").and_then(Value::as_str)))).collect();
    for &start in groups.keys() {
        let mut seen = HashSet::new();
        let mut cur = Some(start);
        while let Some(c) = cur {
            if !seen.insert(c) {
                return Err(format!("a group cycle through {c:?} (from {start:?})"));
            }
            cur = groups.get(c).copied().flatten();
        }
    }
    Ok(())
}

/// session_asserts.py kept_ids, inverted: the ids in `before` missing from `after`, sorted (empty = kept).
pub fn lost_ids(before: &Value, after: &Value) -> Vec<String> {
    let have: HashSet<&str> = nodes(after).iter().filter_map(id).collect();
    let mut lost: Vec<String> =
        nodes(before).iter().filter_map(id).filter(|i| !have.contains(i)).map(str::to_string).collect();
    lost.sort();
    lost.dedup();
    lost
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn canonical_sorts_keys_at_every_depth() {
        let a = canonical(&json!({"b": 1, "a": [{"d": 2, "c": 3}]}));
        let b = canonical(&json!({"a": [{"c": 3, "d": 2}], "b": 1}));
        assert_eq!(a, b);
        assert_eq!(a, r#"{"a":[{"c":3,"d":2}],"b":1}"#);
        let x: Value = serde_json::from_str(r#"{"b":1,"a":[{"d":2,"c":3}]}"#).unwrap();
        let y: Value = serde_json::from_str(r#"{"a":[{"c":3,"d":2}],"b":1}"#).unwrap();
        assert_eq!(canonical(&x), canonical(&y));
        assert_ne!(canonical(&json!(1)), canonical(&json!(1.0)));
        assert_ne!(canonical(&json!(-0.0)), canonical(&json!(0.0)));
    }

    fn doc(nodes: Value) -> Value {
        json!({"nodes": nodes, "local": {}})
    }

    #[test]
    fn same_nodes_is_a_multiset_compare() {
        let a = doc(json!([{"id": "1", "x": 1}, {"id": "2"}]));
        let b = doc(json!([{"id": "2"}, {"x": 1, "id": "1"}]));
        assert_eq!(same_nodes(&a, &b), Ok(2));
        let dup = doc(json!([{"id": "1", "x": 1}, {"id": "2"}, {"id": "2"}]));
        assert!(same_nodes(&dup, &a).is_err());
        let mut local = a.clone();
        local["local"] = json!({"selected": "1"});
        assert_eq!(same_nodes(&a, &local), Ok(2));
        let changed = doc(json!([{"id": "1", "x": 2}, {"id": "2"}]));
        assert!(same_nodes(&a, &changed).unwrap_err().contains("\"1\""));
        assert!(same_nodes(&a, &json!({"local": 1})).is_err());
    }

    #[test]
    fn new_id_wants_exactly_one() {
        let before = doc(json!([{"id": "1"}]));
        assert!(new_id(&before, &before).is_err());
        assert_eq!(new_id(&before, &doc(json!([{"id": "1"}, {"id": "2"}]))), Ok("2".to_string()));
        assert!(new_id(&before, &doc(json!([{"id": "1"}, {"id": "2"}, {"id": "3"}]))).is_err());
    }

    #[test]
    fn group_of_and_show_group() {
        let d = doc(json!([
            {"id": "a", "group": "G"}, {"id": "b", "group": null}, {"id": "c", "group": ""}, {"id": "d"}
        ]));
        assert_eq!(group_of(&d, "a"), Some(Some("G".to_string())));
        for i in ["b", "c", "d"] {
            assert_eq!(group_of(&d, i), Some(None), "{i}");
        }
        assert_eq!(group_of(&d, "zz"), None);
        assert_eq!(show_group(&group_of(&d, "a")), "G");
        assert_eq!(show_group(&group_of(&d, "b")), "null");
        assert_eq!(show_group(&group_of(&d, "zz")), "<absent>");
        assert!(has_id(&d, "a") && !has_id(&d, "zz"));
    }

    fn state(field: Field, v: Value) -> Value {
        json!({ field.state_key(): v })
    }

    #[test]
    fn field_agree_cases() {
        let f = Field::Name;
        let d = doc(json!([{"id": "n", "name": "x"}]));
        let s = state(f, json!({"n": "x"}));
        let lines = field_agree(f, &s, &s, &d, &["n"]).unwrap();
        assert_eq!(lines, [r#"n name: A shows "x", B shows "x", document "x""#]);
        let other = state(f, json!({"n": "y"}));
        assert!(field_agree(f, &s, &other, &d, &["n"]).is_err());
        assert!(field_agree(f, &s, &state(f, json!({})), &d, &["n"]).unwrap_err()[0].contains("B shows <absent>"));
        assert!(field_agree(f, &s, &s, &doc(json!([])), &["n"]).unwrap_err()[0].contains("document <absent>"));
        let nokey = doc(json!([{"id": "n"}]));
        let nulls = state(f, json!({"n": null}));
        assert!(field_agree(f, &nulls, &nulls, &nokey, &["n"]).is_ok());
    }

    #[test]
    fn field_agree_pos_tolerance() {
        let f = Field::Pos;
        let a = state(f, json!({"n": [1.0, 2.0]}));
        let d = |p: Value| doc(json!([{"id": "n", "pos": p}]));
        let b = |p: Value| state(f, json!({ "n": p }));
        assert!(field_agree(f, &a, &b(json!([1.0005, 2.0])), &d(json!([1.0, 2.0])), &["n"]).is_ok());
        assert!(field_agree(f, &a, &b(json!([1.002, 2.0])), &d(json!([1.0, 2.0])), &["n"]).is_err());
        assert!(field_agree(f, &a, &b(Value::Null), &d(json!([1.0, 2.0])), &["n"]).is_err());
    }

    #[test]
    fn field_of_cases() {
        let s = state(Field::Text, json!({"a": "t", "b": null}));
        assert_eq!(field_of(&s, Field::Text, "a"), Some(json!("t")));
        assert_eq!(field_of(&s, Field::Text, "zz"), None);
        assert_eq!(field_of(&s, Field::Text, "b"), Some(Value::Null));
    }

    #[test]
    fn no_cycle_cases() {
        let chain = doc(json!([{"id": "a", "group": "b"}, {"id": "b", "group": "c"}, {"id": "c"}]));
        assert!(no_cycle(&chain).is_ok());
        let two = doc(json!([{"id": "G", "group": "H"}, {"id": "H", "group": "G"}]));
        assert!(no_cycle(&two).is_err());
        assert!(no_cycle(&doc(json!([{"id": "s", "group": "s"}]))).is_err());
        assert!(no_cycle(&doc(json!([{"id": "a", "group": "missing"}]))).is_ok());
    }

    #[test]
    fn lost_ids_and_count_kind() {
        let before = doc(json!([{"id": "1", "type": "t"}, {"id": "2", "type": "u"}, {"id": "3", "type": "t"}]));
        assert!(lost_ids(&before, &before).is_empty());
        assert_eq!(lost_ids(&before, &doc(json!([{"id": "1"}, {"id": "3"}]))), ["2"]);
        assert_eq!(count_kind(&before, "t"), 2);
        assert_eq!(count_kind(&before, "zz"), 0);
    }
}
