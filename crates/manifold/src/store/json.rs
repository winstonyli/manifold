//! JSON snapshots (spec "JSON snapshots"): the document's live nodes
//! through serde, with no history. Session and group export, probes'
//! `EXPORTSESSION`, and `--import-json` all use this one shape:
//!
//! `{"nodes": [{"id": "…", "type": "application/vnd.manifold.tab", "url": "…", "pos": [x, y]}, …],
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
    /// Fields by name, `type` included; defaults are left out.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl JsonNode {
    /// The kind its `type` names; `None` when it has none, or one this build doesn't know.
    pub fn kind(&self) -> Option<Kind> {
        self.fields.get("type").and_then(Value::as_str).map(Kind::from_type).filter(|k| *k != Kind::Unknown)
    }
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

/// A node as JSON from its field values, `group` given separately and
/// `type` from `kind`: what `doc_to_json` writes for a document node, for
/// callers holding values rather than a `Node` (Recently Closed,
/// `session_sync`).
pub fn node_json(id: &str, kind: Kind, fields: &BTreeMap<u32, FieldValue>, group: Option<NodeId>) -> JsonNode {
    let mut out = Map::new();
    let mut put = |no: u32, value: &FieldValue| {
        if let (Some(def), Some(json)) = (field_def(no), value_to_json(value)) {
            out.insert(def.name.to_owned(), json);
        }
    };
    for (&no, value) in fields {
        if no != field::GROUP && no != field::TYPE {
            put(no, value);
        }
    }
    put(field::GROUP, &FieldValue::Id(group));
    out.insert("type".into(), Value::String(kind.type_str().expect("a known kind").into()));
    JsonNode { id: id.to_owned(), fields: out }
}

/// `node` as JSON, its group given separately (the effective one).
fn node_to_json(id: NodeId, node: &Node, group: Option<NodeId>) -> JsonNode {
    let fields = node
        .fields
        .keys()
        .filter(|&&no| no != field::TYPE && field_def(no).is_some())
        .map(|&no| (no, node.get(no)))
        .collect();
    node_json(&id.to_string(), node.kind(), &fields, group)
}

/// Every live node, in id order, each under its effective group.
/// A link whose end is not a live card is left out (an export drops links whose far end is not in the file).
pub fn doc_to_json(doc: &Doc) -> JsonDoc {
    doc_to_json_with(doc, false)
}

/// `doc_to_json` with every stored link, dangling ones included (`--dump`, a forensic view).
pub fn doc_to_json_all(doc: &Doc) -> JsonDoc {
    doc_to_json_with(doc, true)
}

fn doc_to_json_with(doc: &Doc, all_links: bool) -> JsonDoc {
    let nodes = doc
        .live()
        .filter(|(_, node)| all_links || node.kind() != Kind::Link || link_ends_among(node, |id| is_live_card(doc, id)))
        .map(|(id, node)| node_to_json(id, node, doc.effective_group(id)))
        .collect();
    JsonDoc { nodes, local: None }
}

/// Whether `node` is a Link whose `from` and `to` both satisfy `ok`.
fn link_ends_among(node: &Node, ok: impl Fn(NodeId) -> bool) -> bool {
    [field::FROM, field::TO].iter().all(|&no| matches!(node.get(no), FieldValue::Id(Some(end)) if ok(end)))
}

/// Whether `id` is a live node that a link can join (not a jar, not a link).
fn is_live_card(doc: &Doc, id: NodeId) -> bool {
    doc.nodes.get(&id).is_some_and(|n| n.is_live() && !matches!(n.kind(), Kind::Jar | Kind::Link))
}

/// `root` and its live subtree, root first (a group export). The root
/// is written without a group, since its group isn't exported. A link is
/// included when both its ends are in the export.
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
    let inside: std::collections::BTreeSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
    for (id, node) in doc.live() {
        if node.kind() == Kind::Link && link_ends_among(node, |end| inside.contains(&end.to_string())) {
            nodes.push(node_to_json(id, node, None));
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

/// How far an import trusts the file's machine-local fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// A file this app wrote for this profile (Recently Closed, a named
    /// session): every field is kept.
    Own,
    /// Anything else (Import Session/Group, `--import`): a Download's `path`
    /// is dropped (it would point "Show in folder" anywhere), and `jar` /
    /// `default_jar` are kept only when they name a node in the same file
    /// (an export re-imported on its source machine must not re-link tabs to
    /// that machine's logins); so are a link's `from` and `to` (it must not
    /// join the file's cards to this session's).
    Foreign,
}

/// Reads `json`'s nodes, giving each a fresh id from `next_id`. Ids
/// naming a node in the file are rewritten to its fresh id; others are
/// kept when they are real node ids and dropped otherwise. Unknown field
/// names are ignored (a newer build's export); a bad value is an error.
/// Under `Trust::Foreign` the machine-local fields are filtered first (see
/// `Trust`): a Download's `path` is dropped, and a `jar`/`default_jar`/
/// link `from`/`to` naming no node in the file is dropped rather than kept
/// as a real id. Under either trust a link end naming a jar, a link or
/// the link itself is dropped.
/// The result is in `json.nodes` order, so `imported[i]` is `json.nodes[i]`
/// (callers map labels to fresh ids by index).
pub fn import(json: &JsonDoc, mut next_id: impl FnMut() -> NodeId, trust: Trust) -> Result<Vec<Imported>, String> {
    let mut fresh = HashMap::new();
    for node in &json.nodes {
        if fresh.insert(node.id.as_str(), next_id()).is_some() {
            return Err(format!("node id {:?} appears twice", node.id));
        }
    }
    let kinds: HashMap<&str, Option<Kind>> = json.nodes.iter().map(|n| (n.id.as_str(), n.kind())).collect();
    let mut out = Vec::new();
    for node in &json.nodes {
        let kind = node.kind().ok_or_else(|| format!("node {:?}: missing or unknown type", node.id))?;
        let mut fields = BTreeMap::new();
        for (name, value) in &node.fields {
            // The kind is carried separately; `creates` writes its type.
            let Some(def) = field_by_name(name).filter(|def| def.no != field::TYPE) else { continue };
            if trust == Trust::Foreign {
                if def.no == field::PATH && kind == Kind::Download {
                    continue;
                }
                let names_in_file = value.as_str().is_some_and(|s| fresh.contains_key(s));
                if matches!(def.no, field::JAR | field::DEFAULT_JAR | field::FROM | field::TO) && !names_in_file {
                    continue;
                }
            }
            // A link joins cards: an end naming a jar, another link or itself is no end.
            if matches!(def.no, field::FROM | field::TO) {
                let label = value.as_str().unwrap_or_default();
                if label == node.id || matches!(kinds.get(label), Some(Some(Kind::Jar | Kind::Link))) {
                    continue;
                }
            }
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

/// Whether `node` is a Link with an end that is not a card (`is_card`), so it joins nothing and must not be
/// created. `import` has already dropped ends naming a jar, a link or the link itself.
pub fn is_half_made_link(node: &Imported, is_card: impl Fn(NodeId) -> bool) -> bool {
    let end = |no| matches!(node.fields.get(&no), Some(FieldValue::Id(Some(n))) if is_card(*n));
    node.kind == Kind::Link && !(end(field::FROM) && end(field::TO))
}

fn create_op(node: &Imported) -> OpBody {
    let fields = node.fields.iter().flat_map(|(no, value)| encode_field(*no, value)).collect();
    OpBody::create(node.id, node.kind, fields)
}

/// The `Create` ops that add `imported` to a document. A link whose end is not a card in `imported` is skipped
/// (the same rule `materialize_imported` applies, there also accepting live cards).
pub fn creates(imported: &[Imported]) -> Vec<OpBody> {
    let cards: std::collections::BTreeSet<NodeId> =
        imported.iter().filter(|n| !matches!(n.kind, Kind::Jar | Kind::Link)).map(|n| n.id).collect();
    imported.iter().filter(|n| !is_half_made_link(n, |id| cards.contains(&id))).map(create_op).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{ActorId, DocId, Hlc, TYPE_GROUP};
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
        for (i, body) in creates(&import(&parsed, ids(actor), Trust::Own).unwrap()).into_iter().enumerate() {
            doc.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
        }
        doc
    }

    const HAND_WRITTEN: &str = r##"{"nodes": [
        {"id": "g", "type": "application/vnd.manifold.group", "name": "Reading", "color": "#000000"},
        {"id": "t1", "type": "application/vnd.manifold.tab", "group": "g", "url": "https://a.example/", "pos": [10.5, -3], "engine": "cef", "pinned": true},
        {"id": "t2", "type": "application/vnd.manifold.tab", "group": "g", "parent": "t1", "url": "https://b.example/", "future_field": 7},
        {"id": "n", "type": "text/markdown", "text": "hi"}
    ]}"##;

    const WITH_LINKS: &str = r##"{"nodes": [
        {"id": "a", "type": "text/markdown", "text": "a"},
        {"id": "b", "type": "text/markdown", "text": "b"},
        {"id": "ok", "type": "application/vnd.manifold.link", "from": "a", "to": "b"},
        {"id": "dangling", "type": "application/vnd.manifold.link", "from": "a", "to": "424242"},
        {"id": "loop", "type": "application/vnd.manifold.link", "from": "a", "to": "loop"}
    ]}"##;

    #[test]
    fn creates_drops_a_link_with_an_end_that_is_no_card_in_the_file() {
        let parsed: JsonDoc = serde_json::from_str(WITH_LINKS).unwrap();
        let imported = import(&parsed, ids(1), Trust::Foreign).unwrap();
        let made = creates(&imported);
        assert_eq!(made.len(), 3, "two notes and the whole link");
        assert_eq!(imported.len(), 5, "import keeps index alignment with the file");
    }

    #[test]
    fn dump_shows_dangling_links_that_an_export_hides() {
        let mut doc = Doc::new(DocId([0; 16]));
        let mut op_ids = ids(200);
        let parsed: JsonDoc = serde_json::from_str(WITH_LINKS).unwrap();
        // Own trust keeps the dangling end as written, as a half-made link in a store would be.
        for (i, body) in import(&parsed, ids(1), Trust::Own).unwrap().iter().map(create_op).enumerate() {
            doc.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
        }
        let links = |j: JsonDoc| j.nodes.iter().filter(|n| n.kind() == Some(Kind::Link)).count();
        assert_eq!(links(doc_to_json(&doc)), 1);
        assert_eq!(links(doc_to_json_all(&doc)), 3);
    }

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
        let (_, group) = doc.live().find(|(_, n)| n.kind() == Kind::Group).unwrap();
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
                    format!("{} {}", label[&n.id], Value::Object(f)) // `f` holds the type
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
        let (group, _) = doc.live().find(|(_, n)| n.kind() == Kind::Group).unwrap();
        let export = subtree_to_json(&doc, group);
        assert_eq!(
            export.nodes.iter().map(JsonNode::kind).collect::<Vec<_>>(),
            [Some(Kind::Group), Some(Kind::Tab), Some(Kind::Tab)]
        );
        assert!(!export.nodes[0].fields.contains_key("group"));
    }

    #[test]
    fn bad_input_is_an_error() {
        let bad = |json: &str| import(&serde_json::from_str(json).unwrap(), ids(1), Trust::Own).unwrap_err();
        bad(r#"{"nodes": [{"id": "a", "type": "application/x-spaceship"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "type": "application/vnd.manifold.tab", "engine": "gecko"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "type": "application/vnd.manifold.tab", "color": "red"}]}"#);
        bad(
            r#"{"nodes": [{"id": "a", "type": "application/vnd.manifold.tab"}, {"id": "a", "type": "application/vnd.manifold.tab"}]}"#,
        );
    }

    #[test]
    fn export_writes_type_as_a_field_and_import_reads_it_back() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let (group, _) = doc.live().find(|(_, n)| n.kind() == Kind::Group).unwrap();
        let export = subtree_to_json(&doc, group);
        assert_eq!(export.nodes[0].kind(), Some(Kind::Group));
        assert_eq!(export.nodes[1].kind(), Some(Kind::Tab));

        let text = serde_json::to_value(&export).unwrap();
        for node in text["nodes"].as_array().unwrap() {
            assert!(node["type"].is_string(), "{node}");
            assert!(node.get("kind").is_none(), "{node}");
        }
        assert_eq!(text["nodes"][0]["type"], "application/vnd.manifold.group");

        let imported = import(&export, ids(2), Trust::Own).unwrap();
        assert_eq!(imported.iter().map(|n| n.kind).collect::<Vec<_>>(), [Kind::Group, Kind::Tab, Kind::Tab]);
        assert!(imported.iter().all(|n| !n.fields.contains_key(&field::TYPE)), "the kind is carried separately");
        let mut again = Doc::new(DocId([0; 16]));
        let mut op_ids = ids(3);
        for (i, body) in creates(&imported).into_iter().enumerate() {
            again.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
        }
        let kinds: Vec<Kind> = imported.iter().map(|n| again.nodes[&n.id].kind()).collect();
        assert_eq!(kinds, [Kind::Group, Kind::Tab, Kind::Tab], "creates() writes the type back");
        assert_eq!(again.nodes[&imported[0].id].get(field::TYPE), FieldValue::Str(TYPE_GROUP.into()));
    }

    #[test]
    fn import_rejects_a_missing_or_unknown_type() {
        let bad = |json: &str| import(&serde_json::from_str(json).unwrap(), ids(1), Trust::Own).unwrap_err();
        bad(r#"{"nodes": [{"id": "a"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "type": "application/x-spaceship"}]}"#);
    }

    #[test]
    fn node_json_matches_a_document_nodes_json() {
        let mut fields = BTreeMap::new();
        fields.insert(field::URL, FieldValue::Str("https://a.example/".into()));
        fields.insert(field::PINNED, FieldValue::Bool(true));
        fields.insert(field::COLOR, FieldValue::Color(None));
        let json = node_json("n1", Kind::Tab, &fields, None);
        assert_eq!(json.kind(), Some(Kind::Tab));
        assert_eq!(json.fields.get("url"), Some(&Value::String("https://a.example/".into())));
        assert_eq!(json.fields.get("pinned"), Some(&Value::Bool(true)));
        assert!(!json.fields.contains_key("color"), "defaults are left out");
        assert!(!json.fields.contains_key("group"));
    }

    #[test]
    fn import_keeps_file_order() {
        let json: JsonDoc =
            serde_json::from_str(r#"{"nodes":[{"id":"b","type":"text/markdown"},{"id":"a","type":"text/markdown"}]}"#)
                .unwrap();
        let mut next = ids(7);
        let first = next();
        let imported = import(&json, ids(7), Trust::Own).unwrap();
        assert_eq!(imported[0].id, first, "the first node in the file gets the first fresh id");
    }

    fn doc(nodes: Value) -> JsonDoc {
        serde_json::from_value(serde_json::json!({ "nodes": nodes })).unwrap()
    }

    #[test]
    fn a_foreign_import_drops_download_paths_and_outside_jars() {
        let outside = NodeId { actor: ActorId(9), counter: 9 }.to_string();
        // `JsonNode` flattens its fields into the node object.
        let json = doc(serde_json::json!([
            { "id": "j", "type": "application/vnd.manifold.jar" },
            { "id": "d", "type": "application/octet-stream", "path": "C:\\Windows\\x.exe" },
            { "id": "t", "type": "application/vnd.manifold.tab", "url": "https://a", "jar": "j" },
            { "id": "u", "type": "application/vnd.manifold.tab", "url": "https://b", "jar": outside },
            { "id": "g", "type": "application/vnd.manifold.group", "default_jar": outside },
        ]));
        let foreign = import(&json, ids(1), Trust::Foreign).unwrap();
        assert!(!foreign[1].fields.contains_key(&field::PATH));
        assert_eq!(foreign[2].fields.get(&field::JAR), Some(&FieldValue::Id(Some(foreign[0].id))), "in-file jar kept");
        assert!(!foreign[3].fields.contains_key(&field::JAR), "a jar outside the file is dropped");
        assert!(!foreign[4].fields.contains_key(&field::DEFAULT_JAR), "so is an outside default jar");
        let own = import(&json, ids(2), Trust::Own).unwrap();
        assert!(own[1].fields.contains_key(&field::PATH));
        assert!(own[3].fields.contains_key(&field::JAR));
        assert!(own[4].fields.contains_key(&field::DEFAULT_JAR));
    }
}
