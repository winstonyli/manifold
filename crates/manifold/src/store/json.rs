//! JSON snapshots (spec "JSON snapshots"): the document's live nodes
//! through serde, with no history. Session and group export, probes'
//! `EXPORTSESSION`, and `--import-json` all use this one shape:
//!
//! `{"nodes": [{"id": "…", "kind": "tab", "url": "…", "pos": [x, y]}, …],
//!   "local": {…}}`
//!
//! Importing gives every node a fresh id, so a file can be imported twice.
//! Ids in the file are labels: any string, resolved within the file.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::model::{
    encode_field, field, field_by_name, field_def, Doc, FieldType, FieldValue, Kind, Node, NodeId, ENGINES,
};
use super::ops::OpBody;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JsonDoc {
    pub nodes: Vec<JsonNode>,
    /// Personal state, when the export includes it (spec "Personal state").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonNode {
    pub id: String,
    pub kind: String,
    /// Fields by name; defaults are left out.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

fn value_to_json(value: &FieldValue) -> Option<Value> {
    Some(match value {
        FieldValue::Pos(x, y) if (*x, *y) != (0.0, 0.0) => serde_json::json!([x, y]),
        FieldValue::Color(Some(rgb)) => Value::String(format!("#{rgb:06x}")),
        FieldValue::Id(Some(id)) => Value::String(id.to_string()),
        FieldValue::Str(s) if !s.is_empty() => Value::String(s.clone()),
        FieldValue::Bool(true) => Value::Bool(true),
        FieldValue::Engine(code) if *code != 0 => Value::String(ENGINES.get(*code as usize - 1)?.to_string()),
        // The empty or zero value of each type is "absent", so it has no JSON.
        FieldValue::Pos(..)
        | FieldValue::Color(None)
        | FieldValue::Id(None)
        | FieldValue::Str(_)
        | FieldValue::Bool(false)
        | FieldValue::Engine(_) => return None,
    })
}

/// A node as JSON from its field values, `group` given separately: what
/// `doc_to_json` writes for a document node, for callers holding values
/// rather than a `Node` (Recently Closed, `session_sync`).
pub fn node_json(id: &str, kind: Kind, fields: &BTreeMap<u32, FieldValue>, group: Option<NodeId>) -> JsonNode {
    let mut out = Map::new();
    let mut put = |no: u32, value: &FieldValue| {
        if let (Some(def), Some(json)) = (field_def(no), value_to_json(value)) {
            out.insert(def.name.to_owned(), json);
        }
    };
    for (&no, value) in fields {
        if no != field::GROUP {
            put(no, value);
        }
    }
    put(field::GROUP, &FieldValue::Id(group));
    JsonNode { id: id.to_owned(), kind: kind.name().expect("a known kind").to_owned(), fields: out }
}

/// `node` as JSON, its group given separately (the effective one).
fn node_to_json(id: NodeId, node: &Node, group: Option<NodeId>) -> JsonNode {
    let fields = node.fields.keys().filter(|&&no| field_def(no).is_some()).map(|&no| (no, node.get(no))).collect();
    node_json(&id.to_string(), node.kind, &fields, group)
}

/// Every live node, in id order, each under its effective group.
pub fn doc_to_json(doc: &Doc) -> JsonDoc {
    let nodes = doc.live().map(|(id, node)| node_to_json(id, node, doc.effective_group(id))).collect();
    JsonDoc { nodes, local: None }
}

/// `root` and its live subtree, root first (a group export). The root
/// is written without a group, since its group isn't exported.
pub fn subtree_to_json(doc: &Doc, root: NodeId) -> JsonDoc {
    let mut nodes = Vec::new();
    if let Some(node) = doc.nodes.get(&root).filter(|n| n.is_live()) {
        nodes.push(node_to_json(root, node, None));
    }
    for (id, node) in doc.live() {
        if id != root && doc.is_ancestor(root, id) {
            nodes.push(node_to_json(id, node, doc.effective_group(id)));
        }
    }
    JsonDoc { nodes, local: None }
}

/// A node read from JSON, with its fresh id and field values.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub id: NodeId,
    pub kind: Kind,
    pub fields: BTreeMap<u32, FieldValue>,
}

/// Reads `json`'s nodes, giving each a fresh id from `next_id`. Ids
/// naming a node in the file are rewritten to its fresh id; others are
/// kept when they are real node ids and dropped otherwise. Unknown field
/// names are ignored (a newer build's export); a bad value is an error.
/// The result is in `json.nodes` order, so `imported[i]` is `json.nodes[i]`
/// (callers map labels to fresh ids by index).
pub fn import(json: &JsonDoc, mut next_id: impl FnMut() -> NodeId) -> Result<Vec<Imported>, String> {
    let mut fresh = HashMap::new();
    for node in &json.nodes {
        if fresh.insert(node.id.as_str(), next_id()).is_some() {
            return Err(format!("node id {:?} appears twice", node.id));
        }
    }
    let mut out = Vec::new();
    for node in &json.nodes {
        let kind = Kind::from_name(&node.kind).ok_or_else(|| format!("unknown kind {:?}", node.kind))?;
        let mut fields = BTreeMap::new();
        for (name, value) in &node.fields {
            let Some(def) = field_by_name(name) else { continue };
            let bad = || format!("node {:?}: bad {name} {value}", node.id);
            let parsed = match (def.ty, value) {
                (FieldType::Pos, Value::Array(xy)) if xy.len() == 2 => {
                    let get = |v: &Value| v.as_f64().map(|f| f as f32).filter(|f| f.is_finite());
                    FieldValue::Pos(get(&xy[0]).ok_or_else(bad)?, get(&xy[1]).ok_or_else(bad)?)
                }
                (FieldType::Color, Value::String(s)) => {
                    let hex = s.strip_prefix('#').filter(|h| h.len() == 6).ok_or_else(bad)?;
                    FieldValue::Color(Some(u32::from_str_radix(hex, 16).map_err(|_| bad())?))
                }
                (FieldType::Id, Value::String(s)) => match fresh.get(s.as_str()) {
                    Some(id) => FieldValue::Id(Some(*id)),
                    None => FieldValue::Id(s.parse().ok()),
                },
                (FieldType::Str, Value::String(s)) => FieldValue::Str(s.clone()),
                (FieldType::Bool, Value::Bool(b)) => FieldValue::Bool(*b),
                (FieldType::Engine, Value::String(s)) => {
                    let code = ENGINES.iter().position(|e| e == s).ok_or_else(bad)?;
                    FieldValue::Engine(code as u64 + 1)
                }
                _ => return Err(bad()),
            };
            fields.insert(def.no, parsed);
        }
        out.push(Imported { id: fresh[node.id.as_str()], kind, fields });
    }
    Ok(out)
}

/// The `Create` ops that add `imported` to a document.
pub fn creates(imported: &[Imported]) -> Vec<OpBody> {
    imported
        .iter()
        .map(|node| {
            let fields = node.fields.iter().flat_map(|(no, value)| encode_field(*no, value)).collect();
            OpBody::Create { node: node.id, kind: node.kind, fields }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{ActorId, DocId, Hlc};
    use crate::store::ops::Op;

    fn ids(actor: u64) -> impl FnMut() -> NodeId {
        let mut counter = 0;
        move || {
            counter += 1;
            NodeId { actor: ActorId(actor), counter }
        }
    }

    fn doc_from(json: &str, actor: u64) -> Doc {
        let parsed: JsonDoc = serde_json::from_str(json).unwrap();
        let mut doc = Doc::new(DocId([0; 16]));
        let mut op_ids = ids(actor + 100);
        for (i, body) in creates(&import(&parsed, ids(actor)).unwrap()).into_iter().enumerate() {
            doc.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
        }
        doc
    }

    const HAND_WRITTEN: &str = r##"{"nodes": [
        {"id": "g", "kind": "group", "name": "Reading", "color": "#000000"},
        {"id": "t1", "kind": "tab", "group": "g", "url": "https://a.example/", "pos": [10.5, -3], "engine": "cef", "pinned": true},
        {"id": "t2", "kind": "tab", "group": "g", "parent": "t1", "url": "https://b.example/", "future_field": 7},
        {"id": "n", "kind": "note", "text": "hi"}
    ]}"##;

    #[test]
    fn hand_written_short_ids_resolve_within_the_file() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let find = |url: &str| doc.live().find(|(_, n)| n.get(field::URL) == FieldValue::Str(url.into())).unwrap();
        let (t1, tab1) = find("https://a.example/");
        let (_, tab2) = find("https://b.example/");
        let group = tab1.group.unwrap();
        assert_eq!(doc.nodes[&group].get(field::NAME), FieldValue::Str("Reading".into()));
        assert_eq!(tab2.group, Some(group));
        assert_eq!(tab2.get(field::PARENT), FieldValue::Id(Some(t1)));
        assert_eq!(tab1.get(field::POS), FieldValue::Pos(10.5, -3.0));
        assert_eq!(tab1.get(field::ENGINE), FieldValue::Engine(2));
        assert_eq!(tab1.get(field::PINNED), FieldValue::Bool(true));
    }

    #[test]
    fn black_is_a_color_not_the_absence_of_one() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let (_, group) = doc.live().find(|(_, n)| n.kind == Kind::Group).unwrap();
        assert_eq!(group.get(field::COLOR), FieldValue::Color(Some(0)));
        let json = serde_json::to_value(doc_to_json(&doc)).unwrap();
        assert!(json["nodes"].as_array().unwrap().iter().any(|n| n["color"] == "#000000"));
    }

    #[test]
    fn export_then_import_gives_the_same_graph_under_fresh_ids() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let exported = serde_json::to_string(&doc_to_json(&doc)).unwrap();
        let again = doc_from(&exported, 2);
        // Same nodes, relabelled: exporting both with ids hidden matches.
        let shape = |doc: &Doc| {
            let json = doc_to_json(doc);
            let label: HashMap<String, usize> = json.nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
            let mut out: Vec<String> = json
                .nodes
                .iter()
                .map(|n| {
                    let mut f = n.fields.clone();
                    for key in ["group", "parent"] {
                        if let Some(Value::String(id)) = f.get(key) {
                            f.insert(key.into(), Value::from(label[id]));
                        }
                    }
                    format!("{} {} {}", label[&n.id], n.kind, Value::Object(f))
                })
                .collect();
            out.sort();
            out
        };
        assert_eq!(shape(&doc), shape(&again));
        assert!(again.nodes.keys().all(|id| id.actor == ActorId(2)));
    }

    #[test]
    fn a_group_export_lists_the_group_first_then_its_subtree() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let (group, _) = doc.live().find(|(_, n)| n.kind == Kind::Group).unwrap();
        let export = subtree_to_json(&doc, group);
        assert_eq!(export.nodes.iter().map(|n| n.kind.as_str()).collect::<Vec<_>>(), ["group", "tab", "tab"]);
        assert!(!export.nodes[0].fields.contains_key("group"));
    }

    #[test]
    fn bad_input_is_an_error() {
        let bad = |json: &str| import(&serde_json::from_str(json).unwrap(), ids(1)).unwrap_err();
        bad(r#"{"nodes": [{"id": "a", "kind": "spaceship"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "kind": "tab", "engine": "gecko"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "kind": "tab", "color": "red"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "kind": "tab"}, {"id": "a", "kind": "tab"}]}"#);
    }

    #[test]
    fn node_json_matches_a_document_nodes_json() {
        let mut fields = BTreeMap::new();
        fields.insert(field::URL, FieldValue::Str("https://a.example/".into()));
        fields.insert(field::PINNED, FieldValue::Bool(true));
        fields.insert(field::COLOR, FieldValue::Color(None));
        let json = node_json("n1", Kind::Tab, &fields, None);
        assert_eq!(json.kind, "tab");
        assert_eq!(json.fields.get("url"), Some(&Value::String("https://a.example/".into())));
        assert_eq!(json.fields.get("pinned"), Some(&Value::Bool(true)));
        assert!(!json.fields.contains_key("color"), "defaults are left out");
        assert!(!json.fields.contains_key("group"));
    }

    #[test]
    fn import_keeps_file_order() {
        let json: JsonDoc =
            serde_json::from_str(r#"{"nodes":[{"id":"b","kind":"note"},{"id":"a","kind":"note"}]}"#).unwrap();
        let mut next = ids(7);
        let first = next();
        let imported = import(&json, ids(7)).unwrap();
        assert_eq!(imported[0].id, first, "the first node in the file gets the first fresh id");
    }
}
