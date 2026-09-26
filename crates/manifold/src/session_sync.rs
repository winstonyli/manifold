//! The live graph and the session store (spec "Local edits become ops").
//! This half is pure: each live node's shared fields (`Shared`), and the
//! diff that turns local edits into ops. The `impl App` half (restore,
//! saving, other replicas' ops) follows it.

// Wired into `App` by the next task; this attribute goes then.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};

use crate::process_manager::EngineKind;
use crate::store::model::{encode_field, field, field_def, Doc, FieldValue, Kind, Node, NodeId};
use crate::store::ops::OpBody;

/// A node's shared fields by number: every field of its kind but `group`.
pub(crate) type Fields = BTreeMap<u32, FieldValue>;

/// The fields each kind carries, `group` aside (spec, "The shared document").
pub(crate) fn kind_fields(kind: Kind) -> &'static [u32] {
    use field::*;
    match kind {
        Kind::Tab => &[POS, COLOR, PARENT, URL, ENGINE, PINNED],
        Kind::Group => &[POS, COLOR, NAME, FROM_JSON],
        Kind::Note => &[POS, COLOR, TEXT],
        Kind::Image | Kind::Pdf | Kind::Json | Kind::Download => &[POS, COLOR, PATH],
        Kind::JsonLeaf => &[POS, COLOR, PATH, KEY, VALUE],
        Kind::Unknown(_) => &[],
    }
}

/// One live node as the store sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shared {
    pub kind: Kind,
    pub group: Option<NodeId>,
    /// Exactly `kind_fields(kind)`, absent values included, so a field
    /// cleared live reads as a change.
    pub fields: Fields,
}

/// A document node's fields for its kind, shaped like `Shared::fields`.
pub(crate) fn fields_of(node: &Node) -> Fields {
    kind_fields(node.kind).iter().map(|&no| (no, node.get(no))).collect()
}

pub(crate) fn color_value(color: Option<egui::Color32>) -> FieldValue {
    FieldValue::Color(color.map(|c| u32::from_be_bytes([0, c.r(), c.g(), c.b()])))
}

pub(crate) fn color_of(value: &FieldValue) -> Option<egui::Color32> {
    match value {
        FieldValue::Color(Some(rgb)) => {
            let [_, r, g, b] = rgb.to_be_bytes();
            Some(egui::Color32::from_rgb(r, g, b))
        }
        _ => None,
    }
}

/// `store::model::ENGINES` order: code `n` is `ENGINES[n - 1]`.
pub(crate) fn engine_code(kind: EngineKind) -> u64 {
    match kind {
        EngineKind::Servo => 1,
        EngineKind::Cef => 2,
        EngineKind::Webkit => 3,
        EngineKind::Webview2 => 4,
    }
}

/// Unknown codes fall back to Servo, as `engine_kind_serde` does for names.
pub(crate) fn engine_of(code: u64) -> EngineKind {
    match code {
        2 => EngineKind::Cef,
        3 => EngineKind::Webkit,
        4 => EngineKind::Webview2,
        _ => EngineKind::Servo,
    }
}

/// The ops that make `doc` show `live` (spec "Local edits become ops").
/// - A live node the document lacks is created, each group before the
///   nodes inside it. A live id the document holds as a tombstone is
///   never recreated (ids are never reused).
/// - A node shown in another group moves. Its group is compared with
///   `effective_group`, so re-homing out of a deleted group is not a move.
/// - A diffed field that differs is `Set`. An explicit field (`pos`,
///   `url`) is `Set` only when `explicit` names it.
/// - A document node missing from `live` is deleted, unless `keep` names
///   it (a tab whose spawn is in flight).
///
/// A group or parent naming a node that is neither live in the document
/// nor created here is written as none (contract "Missing means deleted").
pub(crate) fn diff(
    doc: &Doc,
    live: &BTreeMap<NodeId, Shared>,
    explicit: &BTreeSet<(NodeId, u32)>,
    keep: &BTreeSet<NodeId>,
) -> Vec<OpBody> {
    let in_doc = |id: &NodeId| doc.nodes.get(id).is_some_and(Node::is_live);
    let new: BTreeSet<NodeId> = live.keys().filter(|id| !doc.nodes.contains_key(id)).copied().collect();
    let valid = |id: Option<NodeId>| id.filter(|id| in_doc(id) || new.contains(id));
    let mut ops = Vec::new();

    // Creates, each group's chain of new ancestors first.
    let mut order = Vec::new();
    let mut queued = BTreeSet::new();
    for &start in &new {
        let mut chain = Vec::new();
        let mut at = Some(start);
        while let Some(id) = at.filter(|id| new.contains(id) && !queued.contains(id) && !chain.contains(id)) {
            chain.push(id);
            at = live[&id].group;
        }
        for id in chain.into_iter().rev() {
            queued.insert(id);
            order.push(id);
        }
    }
    let mut created = BTreeSet::new();
    for id in order {
        let node = &live[&id];
        // A group created later in this batch (only a cycle does that)
        // would make a placeholder: create at the top level instead.
        let group = valid(node.group).filter(|g| in_doc(g) || created.contains(g));
        let mut all = node.fields.clone();
        all.insert(field::GROUP, FieldValue::Id(group));
        if let Some(FieldValue::Id(parent)) = all.get_mut(&field::PARENT) {
            *parent = valid(*parent);
        }
        let fields = all.iter().flat_map(|(&no, value)| encode_field(no, value)).collect();
        ops.push(OpBody::Create { node: id, kind: node.kind, fields });
        created.insert(id);
    }

    // Existing nodes: moves and sets.
    for (&id, node) in live {
        let Some(current) = doc.nodes.get(&id).filter(|n| n.is_live()) else { continue };
        let group = valid(node.group);
        if group != doc.effective_group(id) {
            ops.push(OpBody::Move { node: id, group });
        }
        for (&no, value) in &node.fields {
            if field_def(no).is_some_and(|def| def.explicit) && !explicit.contains(&(id, no)) {
                continue;
            }
            let value = match value {
                FieldValue::Id(parent) => FieldValue::Id(valid(*parent)),
                other => other.clone(),
            };
            let enc = encode_field(no, &value);
            if current.fields.get(&no).map_or(&[][..], |reg| &reg.enc[..]) != &enc[..] {
                ops.push(OpBody::Set { node: id, field: no, enc });
            }
        }
    }

    for (id, _) in doc.live() {
        if !live.contains_key(&id) && !keep.contains(&id) {
            ops.push(OpBody::Delete { node: id });
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{ActorId, DocId, Hlc};
    use crate::store::ops::Op;
    use crate::store::testutil::XorShift;

    const A: ActorId = ActorId(0xA);

    fn id(counter: u64) -> NodeId {
        NodeId { actor: A, counter }
    }

    /// Applies `bodies` to `doc` as actor `A`'s next ops.
    fn apply(doc: &mut Doc, bodies: Vec<OpBody>) {
        for body in bodies {
            let counter = doc.version.get(&A).copied().unwrap_or(0).max(1000);
            let hlc = Hlc { ms: doc.clock.ms + 1, logical: 0 };
            doc.apply(&Op { id: id(counter), hlc, body });
        }
    }

    fn shared(kind: Kind, group: Option<NodeId>, set: &[(u32, FieldValue)]) -> Shared {
        let mut fields: Fields =
            kind_fields(kind).iter().map(|&no| (no, FieldValue::absent(field_def(no).unwrap().ty))).collect();
        for (no, value) in set {
            fields.insert(*no, value.clone());
        }
        Shared { kind, group, fields }
    }

    fn note(text: &str, group: Option<NodeId>) -> Shared {
        shared(Kind::Note, group, &[(field::TEXT, FieldValue::Str(text.into()))])
    }

    fn group(name: &str) -> Shared {
        shared(Kind::Group, None, &[(field::NAME, FieldValue::Str(name.into()))])
    }

    /// What `doc` shows, shaped like `live`.
    fn shown(doc: &Doc) -> BTreeMap<NodeId, Shared> {
        doc.live()
            .map(|(id, node)| (id, Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) }))
            .collect()
    }

    fn all_explicit(live: &BTreeMap<NodeId, Shared>) -> BTreeSet<(NodeId, u32)> {
        live.keys().flat_map(|&id| [(id, field::POS), (id, field::URL)]).collect()
    }

    #[test]
    fn new_nodes_are_created_groups_first() {
        let doc = Doc::new(DocId([1; 16]));
        // The member's id sorts before its group's.
        let live = BTreeMap::from([(id(0), note("inside", Some(id(1)))), (id(1), group("G"))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        assert!(matches!(ops[0], OpBody::Create { node, .. } if node == id(1)));
        let mut doc = doc;
        apply(&mut doc, ops);
        assert_eq!(shown(&doc), live);
    }

    #[test]
    fn an_unchanged_graph_emits_nothing() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
    }

    #[test]
    fn explicit_fields_change_only_when_marked() {
        let mut doc = Doc::new(DocId([1; 16]));
        let mut live = BTreeMap::from([(id(0), note("a", None))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        live.get_mut(&id(0)).unwrap().fields.insert(field::POS, FieldValue::Pos(5.0, 6.0));
        assert_eq!(diff(&doc, &live, &BTreeSet::new(), &BTreeSet::new()), vec![], "physics motion is not an edit");
        let marked = BTreeSet::from([(id(0), field::POS)]);
        assert!(matches!(&diff(&doc, &live, &marked, &BTreeSet::new())[..], [OpBody::Set { field: field::POS, .. }]));
    }

    #[test]
    fn a_missing_node_is_deleted_unless_kept() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        let empty = BTreeMap::new();
        assert_eq!(diff(&doc, &empty, &BTreeSet::new(), &BTreeSet::from([id(0)])), vec![]);
        assert_eq!(diff(&doc, &empty, &BTreeSet::new(), &BTreeSet::new()), vec![OpBody::Delete { node: id(0) }]);
    }

    #[test]
    fn a_tombstone_is_never_recreated() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", None))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        apply(&mut doc, vec![OpBody::Delete { node: id(0) }]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        assert!(!ops.iter().any(|op| matches!(op, OpBody::Create { .. })), "{ops:?}");
    }

    #[test]
    fn re_homing_out_of_a_deleted_group_is_not_a_move() {
        let mut doc = Doc::new(DocId([1; 16]));
        let live = BTreeMap::from([(id(0), note("a", Some(id(1)))), (id(1), group("G"))]);
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        // Another replica deleted the group only; the note is shown at the top.
        apply(&mut doc, vec![OpBody::Delete { node: id(1) }]);
        let rehomed = BTreeMap::from([(id(0), note("a", None))]);
        assert_eq!(diff(&doc, &rehomed, &BTreeSet::new(), &BTreeSet::new()), vec![]);
    }

    #[test]
    fn a_parent_naming_a_missing_node_is_written_as_none() {
        let doc = Doc::new(DocId([1; 16]));
        let tab = shared(Kind::Tab, None, &[(field::PARENT, FieldValue::Id(Some(id(9))))]);
        let live = BTreeMap::from([(id(0), tab)]);
        let mut doc2 = doc.clone();
        apply(&mut doc2, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
        assert_eq!(doc2.nodes[&id(0)].get(field::PARENT), FieldValue::Id(None));
    }

    #[test]
    fn random_edits_converge_in_one_diff() {
        let mut rng = XorShift(0x5E55);
        let mut doc = Doc::new(DocId([1; 16]));
        let mut live: BTreeMap<NodeId, Shared> = BTreeMap::new();
        let mut next = 0;
        for _ in 0..200 {
            match rng.below(4) {
                0 | 1 => {
                    let groups: Vec<NodeId> =
                        live.iter().filter(|(_, s)| s.kind == Kind::Group).map(|(id, _)| *id).collect();
                    let group = (!groups.is_empty() && rng.below(2) == 0)
                        .then(|| groups[rng.below(groups.len() as u64) as usize]);
                    let node = if rng.below(3) == 0 { group_in("g", group) } else { note("n", group) };
                    live.insert(id(next), node);
                    next += 1;
                }
                2 if !live.is_empty() => {
                    let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                    live.get_mut(&key)
                        .unwrap()
                        .fields
                        .insert(field::COLOR, FieldValue::Color(Some(rng.below(1 << 24) as u32)));
                }
                3 if !live.is_empty() => {
                    let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                    // Deleting a group deletes what it shows, as the app does.
                    let doomed: Vec<NodeId> =
                        live.keys().filter(|&&n| n == key || is_inside(&live, n, key)).copied().collect();
                    for n in doomed {
                        live.remove(&n);
                    }
                }
                _ => {}
            }
            let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
            apply(&mut doc, ops);
            assert_eq!(shown(&doc), live);
            assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
        }
    }

    fn group_in(name: &str, parent: Option<NodeId>) -> Shared {
        Shared { group: parent, ..group(name) }
    }

    fn is_inside(live: &BTreeMap<NodeId, Shared>, node: NodeId, ancestor: NodeId) -> bool {
        let mut at = live.get(&node).and_then(|s| s.group);
        while let Some(g) = at {
            if g == ancestor {
                return true;
            }
            at = live.get(&g).and_then(|s| s.group);
        }
        false
    }

    #[test]
    fn engine_codes_follow_the_store_table() {
        for kind in [EngineKind::Servo, EngineKind::Cef, EngineKind::Webkit, EngineKind::Webview2] {
            let name = crate::store::model::ENGINES[engine_code(kind) as usize - 1];
            assert_eq!(serde_json::to_value(Wrap(kind)).unwrap(), serde_json::json!(name));
            assert_eq!(engine_of(engine_code(kind)), kind);
        }
    }

    #[derive(serde::Serialize)]
    struct Wrap(#[serde(with = "crate::process_manager::engine_kind_serde")] EngineKind);

    #[test]
    fn colors_round_trip() {
        let c = egui::Color32::from_rgb(1, 2, 3);
        assert_eq!(color_of(&color_value(Some(c))), Some(c));
        assert_eq!(color_value(Some(egui::Color32::BLACK)), FieldValue::Color(Some(0)), "black is encoded");
        assert_eq!(color_of(&color_value(None)), None);
    }
}
