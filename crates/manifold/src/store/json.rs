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
    encode_field, field, field_by_name, field_def, Doc, FieldType, FieldValue, Kind, Node, NodeId, VIEW_KINDS,
};
use super::ops::OpBody;
use super::tree::{resolved, Seg, Trees, TREE_FIELDS};
use super::tree_json::{field_from_json, field_to_json};

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
        FieldValue::Pos(Some((x, y))) => serde_json::json!([x, y]),
        FieldValue::Color(Some(rgb)) => Value::String(format!("#{rgb:06x}")),
        FieldValue::Id(Some(id)) => Value::String(id.to_string()),
        FieldValue::Str(s) if !s.is_empty() => Value::String(s.clone()),
        FieldValue::Bool(true) => Value::Bool(true),
        FieldValue::ViewCode(code) if *code != 0 => Value::String(VIEW_KINDS.get(*code as usize - 1)?.to_string()),
        // The empty or zero value of each type is "absent", so it has no JSON.
        FieldValue::Pos(None)
        | FieldValue::Color(None)
        | FieldValue::Id(None)
        | FieldValue::Str(_)
        | FieldValue::Bool(false)
        | FieldValue::ViewCode(_)
        | FieldValue::Replica(_)
        | FieldValue::Held { .. }
        | FieldValue::Anchor(_) => return None,
    })
}

/// A node as JSON from its field values, `group` given separately and
/// `type` from `ty`: what `doc_to_json` writes for a document node, for
/// callers holding values rather than a `Node` (Recently Closed,
/// `session_sync`).
pub fn node_json(
    id: &str,
    ty: &str,
    fields: &BTreeMap<u32, FieldValue>,
    trees: &Trees,
    group: Option<NodeId>,
) -> JsonNode {
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
    for &(no, name) in &TREE_FIELDS {
        if let Some(tree) = trees.get(&no).filter(|tree| resolved(tree).next().is_some()) {
            out.insert(name.into(), field_to_json(no, tree));
        }
    }
    out.insert("type".into(), Value::String(ty.to_owned()));
    JsonNode { id: id.to_owned(), fields: out }
}

/// `node` as JSON, its group given separately (the effective one).
fn node_to_json(id: NodeId, node: &Node, group: Option<NodeId>) -> JsonNode {
    let fields = node
        .fields
        .keys()
        .filter(|k| k.field != field::TYPE && field_def(k.field).is_some())
        .map(|k| (k.field, node.get(k.field)))
        .collect();
    node_json(&id.to_string(), node.ty(), &fields, &node.trees(), group)
}

/// Every shown node, in id order, each under its effective group.
/// A link whose end is not a shown card is left out (an export drops links whose far end is not in the file).
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
        .filter(|&(id, node)| {
            all_links
                || (doc.is_shown(id)
                    && (node.kind() != Kind::Link || link_ends_among(node, |id| is_shown_card(doc, id))))
        })
        .map(|(id, node)| node_to_json(id, node, doc.effective_group(id)))
        .collect();
    JsonDoc { nodes, local: None }
}

/// Whether `node` is a Link whose `from` and `to` both satisfy `ok`.
fn link_ends_among(node: &Node, ok: impl Fn(NodeId) -> bool) -> bool {
    [field::FROM, field::TO].iter().all(|&no| node.get(no).as_id().is_some_and(&ok))
}

/// Whether `id` is a shown node that a link can join (any card, a jar included; not a link).
fn is_shown_card(doc: &Doc, id: NodeId) -> bool {
    doc.nodes.get(&id).is_some_and(|n| n.kind().card()) && doc.is_shown(id)
}

/// `root` and its shown subtree, root first (a group export). The root
/// is written without a group, since its group isn't exported. A link is
/// included when both its ends are in the export, and so is every live label a card in it carries.
pub fn subtree_to_json(doc: &Doc, root: NodeId) -> JsonDoc {
    let mut nodes = Vec::new();
    if let Some(node) = doc.nodes.get(&root).filter(|_| doc.is_shown(root)) {
        nodes.push(node_to_json(root, node, None));
    }
    for (id, node) in doc.shown() {
        if id != root && doc.is_ancestor(root, id) {
            nodes.push(node_to_json(id, node, doc.effective_group(id)));
        }
    }
    let inside: std::collections::BTreeSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
    for (id, node) in doc.shown() {
        if node.kind() == Kind::Link && link_ends_among(node, |end| inside.contains(&end.to_string())) {
            nodes.push(node_to_json(id, node, None));
        }
    }
    // The labels the exported cards carry (`PROPS/labels/@<id>`), so a membership names a node in the file. Any
    // `PROPS/<key>/@<id>` is taken: only a label's id reaches the file through the `Kind::Label` filter below.
    let mut used = std::collections::BTreeSet::new();
    for exported in &nodes {
        let Some(node) = exported.id.parse::<NodeId>().ok().and_then(|id| doc.nodes.get(&id)) else { continue };
        let trees = node.trees();
        for (path, _) in trees.get(&field::PROPS).into_iter().flat_map(|tree| tree.iter()) {
            if let [Seg::Key(_), Seg::Id(label)] = path.segments().as_slice() {
                used.insert(*label);
            }
        }
    }
    for id in used {
        if let Some(node) = doc.nodes.get(&id).filter(|n| !n.deleted && n.kind() == Kind::Label) {
            nodes.push(node_to_json(id, node, None));
        }
    }
    JsonDoc { nodes, local: None }
}

/// A node read from JSON, with its fresh id, its type string as the file wrote it and its field values.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub id: NodeId,
    pub ty: String,
    pub fields: BTreeMap<u32, FieldValue>,
    pub trees: Trees,
    /// The node is an existing one `import_with`'s `reuse` named: it keeps its slot but is not created.
    pub reused: bool,
}

impl Imported {
    /// The kind its type string means (`Kind::from_type`).
    pub fn kind(&self) -> Kind {
        Kind::from_type(&self.ty)
    }
}

/// How far an import trusts the file's machine-local fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// A file this app wrote for this profile (Recently Closed, a named
    /// session): every field its kind carries is kept.
    Own,
    /// Anything else (Import Session/Group, `--import`): a `path` of any kind
    /// is dropped (a Download's would point "Show in folder" anywhere; a file
    /// card's would make restore open a UNC share the file's author chose), and a `jar` is kept only when it names a node in the same file
    /// (an export re-imported on its source machine must not re-link tabs to
    /// that machine's logins); so are a link's `from` and `to` (it must not
    /// join the file's cards to this session's).
    Foreign,
}

/// Reads `json`'s nodes, giving each a fresh id from `next_id`. Ids
/// naming a node in the file are rewritten to its fresh id; others are
/// kept when they are real node ids and dropped otherwise. A node keeps its
/// `type` string, one this build doesn't know included, and only the fields
/// its kind carries (`Kind::fields`, the generic ones for an unknown type)
/// plus its group; unknown field names are ignored (a newer build's export).
/// A missing or empty `type`, or a bad value, is an error.
/// Under `Trust::Foreign` the machine-local fields are filtered first (see
/// `Trust`): every `path` is dropped, and a `jar`/
/// link `from`/`to` naming no node in the file is dropped rather than kept
/// as a real id. Under either trust a link end naming a jar, a link or
/// the link itself is dropped.
/// The result is in `json.nodes` order, so `imported[i]` is `json.nodes[i]`
/// (callers map labels to fresh ids by index).
pub fn import(json: &JsonDoc, next_id: impl FnMut() -> NodeId, trust: Trust) -> Result<Vec<Imported>, String> {
    import_with(json, next_id, trust, |_| None)
}

/// `import`, where `reuse` may name an existing node for a file node: its slot is flagged `reused`, keeps its place,
/// and every reference to it resolves to that id.
pub fn import_with(
    json: &JsonDoc,
    mut next_id: impl FnMut() -> NodeId,
    trust: Trust,
    reuse: impl Fn(&JsonNode) -> Option<NodeId>,
) -> Result<Vec<Imported>, String> {
    let mut fresh = HashMap::new();
    let mut reused = std::collections::HashSet::new();
    for node in &json.nodes {
        let id = match reuse(node) {
            Some(existing) => {
                reused.insert(node.id.as_str());
                existing
            }
            None => next_id(),
        };
        if fresh.insert(node.id.as_str(), id).is_some() {
            return Err(format!("node id {:?} appears twice", node.id));
        }
    }
    let kinds: HashMap<&str, Option<Kind>> = json.nodes.iter().map(|n| (n.id.as_str(), n.kind())).collect();
    let mut out = Vec::new();
    for node in &json.nodes {
        let ty = node
            .fields
            .get("type")
            .and_then(Value::as_str)
            .filter(|ty| !ty.is_empty())
            .ok_or_else(|| format!("node {:?}: missing type", node.id))?;
        let kind = Kind::from_type(ty);
        let mut fields = BTreeMap::new();
        for (name, value) in &node.fields {
            // The type is carried separately; `creates` writes it. `closed` is the document's history, never a
            // file's: an import always arrives shown. `held` names this document's nodes. `anchor` is this
            // document's layout hold, never a file's (spec Slice 1, Export).
            let Some(def) = field_by_name(name)
                .filter(|def| !matches!(def.no, field::TYPE | field::CLOSED | field::HELD | field::ANCHOR))
            else {
                continue;
            };
            // Only what its kind carries, and its group: a newer build's field on an unknown type can't be remapped,
            // so it is never copied (spec decision 4).
            if def.no != field::GROUP && !kind.fields().contains(&def.no) {
                continue;
            }
            if trust == Trust::Foreign {
                if def.no == field::PATH {
                    continue;
                }
                let names_in_file = value.as_str().is_some_and(|s| fresh.contains_key(s));
                if matches!(def.no, field::JAR | field::FROM | field::TO) && !names_in_file {
                    continue;
                }
            }
            // A link joins cards: an end naming another link or itself is no end.
            if matches!(def.no, field::FROM | field::TO) {
                let label = value.as_str().unwrap_or_default();
                if label == node.id || matches!(kinds.get(label), Some(Some(Kind::Link))) {
                    continue;
                }
            }
            let bad = || format!("node {:?}: bad {name} {value}", node.id);
            let parsed = match (def.ty, value) {
                (FieldType::Pos, Value::Array(xy)) if xy.len() == 2 => {
                    let get = |v: &Value| v.as_f64().map(|f| f as f32).filter(|f| f.is_finite());
                    FieldValue::Pos(Some((get(&xy[0]).ok_or_else(bad)?, get(&xy[1]).ok_or_else(bad)?)))
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
                (FieldType::ViewCode, Value::String(s)) => {
                    let code = VIEW_KINDS.iter().position(|e| e == s).ok_or_else(bad)?;
                    FieldValue::ViewCode(code as u64 + 1)
                }
                _ => return Err(bad()),
            };
            fields.insert(def.no, parsed);
        }
        // A type this build doesn't know takes no tree either (decision 4). An id the file names becomes its fresh id;
        // another real id stays under `Own` and is dropped under `Foreign`.
        let mut trees = Trees::new();
        if kind != Kind::Unknown {
            let resolve = |text: &str| match fresh.get(text) {
                Some(id) => Some(*id),
                None if trust == Trust::Own => text.parse().ok(),
                None => None,
            };
            for &(no, name) in &TREE_FIELDS {
                let Some(value) = node.fields.get(name) else { continue };
                let tree = field_from_json(no, value, &resolve, &mut next_id)
                    .map_err(|e| format!("node {:?}: bad {name}: {e}", node.id))?;
                if !tree.is_empty() {
                    trees.insert(no, tree);
                }
            }
        }
        out.push(Imported {
            id: fresh[node.id.as_str()],
            ty: ty.to_owned(),
            fields,
            trees,
            reused: reused.contains(node.id.as_str()),
        });
    }
    Ok(out)
}

/// Whether `node` is a Link with an end that is not a card (`is_card`), so it joins nothing and must not be
/// created. `import` has already dropped ends naming a jar, a link or the link itself.
pub fn is_half_made_link(node: &Imported, is_card: impl Fn(NodeId) -> bool) -> bool {
    let end = |no| node.fields.get(&no).and_then(FieldValue::as_id).is_some_and(&is_card);
    node.kind() == Kind::Link && !(end(field::FROM) && end(field::TO))
}

fn create_op(node: &Imported) -> OpBody {
    let fields = node.fields.iter().flat_map(|(no, value)| encode_field(*no, value)).collect();
    OpBody::create(node.id, &node.ty, fields)
}

/// The `Create` ops that add `imported` to a document. A link whose end is not a card in `imported` is skipped
/// (the same rule `materialize_imported` applies, there also accepting live cards).
pub fn creates(imported: &[Imported]) -> Vec<OpBody> {
    let cards: std::collections::BTreeSet<NodeId> = imported.iter().filter(|n| n.kind().card()).map(|n| n.id).collect();
    imported
        .iter()
        .filter(|n| !n.reused && !is_half_made_link(n, |id| cards.contains(&id)))
        .flat_map(|n| std::iter::once(create_op(n)).chain(tree_sets(n)))
        .collect()
}

/// One `Set` per leaf of `node`'s trees, ancestors first, to follow its `Create`.
fn tree_sets(node: &Imported) -> impl Iterator<Item = OpBody> + '_ {
    node.trees.iter().flat_map(move |(&field, tree)| {
        tree.iter().map(move |(path, leaf)| OpBody::Set {
            node: node.id,
            field,
            path: path.clone(),
            enc: leaf.encode(),
        })
    })
}

#[cfg(test)]
mod tests;
