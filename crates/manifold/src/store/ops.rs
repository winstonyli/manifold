//! Operations and their merge rules (spec "Operations and merge"). Any
//! set of ops applied in any order, any number of times, gives the same
//! document.

use std::collections::BTreeSet;

use super::model::{
    decode_field, encode_field, field, field_def, Doc, FieldValue, Hlc, Node, NodeId, Reg, RegKey, Stamp,
};
use super::tlv::Reader;
use super::tree::TreePath;

#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    /// `(actor, counter)` from the writer's shared counter. A `Create`'s op
    /// id is its own: the node's id was taken when the node was made.
    pub id: NodeId,
    pub hlc: Hlc,
    pub body: OpBody,
}

#[derive(Debug, Clone, PartialEq)]
#[rustfmt::skip]
pub enum OpBody {
    /// `fields` is a node message (`Node::message`), `group` and `type` included.
    Create { node: NodeId, fields: Vec<u8> },
    /// `enc` is `encode_field(field, value)`; empty resets the field. `path` addresses a register inside a tree field;
    /// it is root for every scalar field, and `apply` ignores a non-root path on a field with a `FieldDef`.
    Set { node: NodeId, field: u32, path: TreePath, enc: Vec<u8> },
    Move { node: NodeId, group: Option<NodeId> },
    Delete { node: NodeId },
}

impl OpBody {
    /// A `Create` whose node message carries the type string `ty` after `fields`. `ty` is the node's own string
    /// (`Node::ty`, `Snapshot::ty`), a type this build doesn't know included.
    pub fn create(node: NodeId, ty: &str, mut fields: Vec<u8>) -> OpBody {
        debug_assert!(!ty.is_empty(), "a created node has a type");
        fields.extend(encode_field(field::TYPE, &FieldValue::Str(ty.to_owned())));
        OpBody::Create { node, fields }
    }

    /// A `Set` of a whole field (the root path): every scalar field's write.
    pub fn set(node: NodeId, field: u32, enc: Vec<u8>) -> OpBody {
        OpBody::Set { node, field, path: TreePath::default(), enc }
    }
}

impl Op {
    pub fn stamp(&self) -> Stamp {
        Stamp { hlc: self.hlc, actor: self.id.actor }
    }
}

/// One entry of the move log (Kleppmann et al., "A highly-available move
/// operation for replicated trees").
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoveRec {
    stamp: Stamp,
    node: NodeId,
    new: Option<NodeId>,
    /// The node's group before this move, recorded when (re)applied.
    old: Option<NodeId>,
    /// `false` when applying it would have made a cycle.
    applied: bool,
}

impl Doc {
    /// Applies `op` and returns the nodes it changed. Nodes shown inside
    /// them may move too: see `Doc::with_descendants`.
    pub fn apply(&mut self, op: &Op) -> Vec<NodeId> {
        let stamp = op.stamp();
        let next = self.version.entry(op.id.actor).or_insert(0);
        *next = (*next).max(op.id.counter + 1);
        self.clock = self.clock.max(op.hlc);
        match &op.body {
            OpBody::Create { node, fields } => {
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                let mut group = None;
                for field in Reader::new(fields).map_while(Result::ok) {
                    if field.no == field::GROUP {
                        let def = field_def(field::GROUP).expect("known");
                        if let Ok(FieldValue::Id(g)) = decode_field(def, field.raw) {
                            group = g;
                        }
                    } else if !entry.deleted {
                        set_reg(entry, RegKey::of(field.no), stamp, field.raw.to_vec());
                    }
                }
                // The initial group is a move at the create's own time.
                let mut touched = vec![*node];
                touched.extend(self.apply_move(stamp, *node, group));
                touched
            }
            OpBody::Set { node, field: no, path, enc } => {
                if *no == field::GROUP || (!path.is_root() && field_def(*no).is_some()) {
                    return Vec::new(); // the tree edge changes only by `Move`; a scalar field has no tree
                }
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                if entry.deleted {
                    return Vec::new();
                }
                set_reg(entry, RegKey { field: *no, path: path.clone() }, stamp, enc.clone());
                vec![*node]
            }
            OpBody::Move { node, group } => {
                self.nodes.entry(*node).or_insert_with(Node::placeholder);
                self.apply_move(stamp, *node, *group)
            }
            OpBody::Delete { node } => {
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                if entry.deleted {
                    return Vec::new();
                }
                // Delete wins: its fields are gone and later `Set`s are
                // ignored. It keeps its tree edge; see `apply_move`.
                entry.deleted = true;
                entry.fields.clear();
                vec![*node]
            }
        }
    }

    /// Kleppmann's move: undo the logged moves that sort after this one,
    /// apply it, and redo them, each checked again for cycles. Deleted
    /// nodes move too, so every replica walks the same tree whatever it
    /// has deleted. In order (single-player), nothing is undone.
    fn apply_move(&mut self, stamp: Stamp, node: NodeId, group: Option<NodeId>) -> Vec<NodeId> {
        let at = self.moves.partition_point(|m| m.stamp < stamp);
        if self.moves.get(at).is_some_and(|m| m.stamp == stamp) {
            return Vec::new(); // already applied
        }
        for m in self.moves[at..].iter().rev() {
            if m.applied {
                if let Some(n) = self.nodes.get_mut(&m.node) {
                    n.group = m.old;
                }
            }
        }
        self.moves.insert(at, MoveRec { stamp, node, new: group, old: None, applied: false });
        let mut touched = Vec::new();
        for i in at..self.moves.len() {
            let MoveRec { node, new, .. } = self.moves[i];
            let old = self.nodes.get(&node).and_then(|n| n.group);
            let applied = new.is_none_or(|g| g != node && !self.is_ancestor(node, g));
            if applied {
                self.nodes.entry(node).or_insert_with(Node::placeholder).group = new;
            }
            self.moves[i].old = old;
            self.moves[i].applied = applied;
            touched.push(node);
        }
        touched
    }

    /// Drops the move log. Only sound when every op applied after this is
    /// stamped later than every op applied before it, so none would be
    /// undone: the journal compacts under its lock after catching up, and
    /// stamps its own ops after that. Sync will need the Move-log tail.
    pub fn forget_moves(&mut self) {
        self.moves.clear();
    }

    /// Drops what no later op can need while the document has never been
    /// shared (spec "Compact"). First, deleted nodes and placeholders that
    /// no group edge names; this repeats, since dropping a child can free
    /// its group. Then registers a newer ancestor hides (`Node::visible_regs`): their effect is already in the
    /// hider, so a late older write stays hidden without them. Then the version entries of actors nothing names any
    /// more. What is shown doesn't change; a later op naming a dropped
    /// node makes a placeholder, which isn't shown. Call after
    /// `forget_moves`: the move log names nodes too.
    pub fn prune(&mut self) {
        debug_assert!(self.moves.is_empty(), "prune after forget_moves");
        loop {
            let named: BTreeSet<NodeId> = self.nodes.values().filter_map(|n| n.group).collect();
            let before = self.nodes.len();
            self.nodes.retain(|id, n| named.contains(id) || !(n.deleted || !n.has_type()));
            if self.nodes.len() == before {
                break;
            }
        }
        for node in self.nodes.values_mut() {
            if node.visible_regs().count() < node.fields.len() {
                let keep: BTreeSet<RegKey> = node.visible_regs().map(|(key, _)| key.clone()).collect();
                node.fields.retain(|key, _| keep.contains(key));
            }
        }
        let mut used = BTreeSet::new();
        for (id, node) in &self.nodes {
            used.insert(id.actor);
            used.extend(node.group.map(|g| g.actor));
            used.extend(node.fields.values().map(|reg| reg.stamp.actor));
        }
        self.version.retain(|actor, _| used.contains(actor));
    }
}

fn set_reg(node: &mut Node, key: RegKey, stamp: Stamp, enc: Vec<u8>) {
    if node.fields.get(&key).is_none_or(|reg| reg.stamp < stamp) {
        node.fields.insert(key, Reg { stamp, enc });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{ActorId, DocId, Kind};
    use crate::store::testutil::{shown, XorShift};

    fn id(actor: u64, counter: u64) -> NodeId {
        NodeId { actor: ActorId(actor), counter }
    }

    fn op(actor: u64, counter: u64, ms: u64, body: OpBody) -> Op {
        // Real stamps are unique per op (`Clock::tick`); `logical` keeps them so.
        Op { id: id(actor, counter), hlc: Hlc { ms, logical: counter as u32 }, body }
    }

    fn create(node: NodeId, kind: Kind, group: Option<NodeId>) -> OpBody {
        OpBody::create(node, kind.type_str().unwrap(), encode_field(field::GROUP, &FieldValue::Id(group)))
    }

    #[test]
    fn a_create_carries_its_type_and_a_set_before_it_shows_once_it_arrives() {
        let t = id(1, 0);
        let mut doc = Doc::new(DocId([0; 16]));
        doc.apply(&op(1, 1, 2, name(t, "x")));
        assert!(!doc.nodes[&t].is_live(), "named by an op, not created");
        doc.apply(&op(1, 0, 1, create(t, Kind::Tab, None)));
        assert_eq!(doc.nodes[&t].kind(), Kind::Tab);
        assert!(doc.nodes[&t].is_live());
    }

    #[test]
    fn prune_drops_a_typeless_placeholder_but_keeps_a_type_this_build_does_not_know() {
        let (ghost, future) = (id(1, 0), id(1, 1));
        let mut doc = Doc::new(DocId([0; 16]));
        doc.apply(&op(1, 2, 1, name(ghost, "g")));
        let mut fields = encode_field(field::TYPE, &FieldValue::Str("application/x-from-the-future".into()));
        fields.extend(encode_field(field::GROUP, &FieldValue::Id(None)));
        doc.apply(&op(1, 3, 1, OpBody::Create { node: future, fields }));
        doc.forget_moves();
        doc.prune();
        assert!(!doc.nodes.contains_key(&ghost));
        assert!(doc.nodes.contains_key(&future));
        assert_eq!(doc.nodes[&future].kind(), Kind::Unknown);
    }

    #[test]
    fn a_create_writes_the_type_string_it_is_given() {
        let t = id(1, 0);
        let mut doc = Doc::new(DocId([0; 16]));
        doc.apply(&op(1, 0, 1, OpBody::create(t, "application/x-from-the-future", Vec::new())));
        assert_eq!(doc.nodes[&t].ty(), "application/x-from-the-future");
        assert_eq!(doc.nodes[&t].kind(), Kind::Unknown);
    }

    /// A `Set` of a generic field on a node of an unknown type leaves a newer client's field byte-identical (spec
    /// "Testing").
    #[test]
    fn a_set_on_an_unknown_node_leaves_its_raw_fields_alone() {
        let t = id(1, 0);
        let mut fields = encode_field(40, &FieldValue::Str("theirs".into()));
        fields.extend(encode_field(field::GROUP, &FieldValue::Id(None)));
        let mut doc = Doc::new(DocId([0; 16]));
        doc.apply(&op(1, 0, 1, OpBody::create(t, "application/x-from-the-future", fields)));
        let raw = doc.nodes[&t].fields[&RegKey::of(40)].enc.clone();
        doc.apply(&op(1, 1, 2, name(t, "renamed")));
        let pos = encode_field(field::POS, &FieldValue::Pos(Some((5.0, 6.0))));
        doc.apply(&op(1, 2, 3, OpBody::set(t, field::POS, pos)));
        assert_eq!(doc.nodes[&t].fields[&RegKey::of(40)].enc, raw);
        assert_eq!(doc.nodes[&t].get(field::NAME), FieldValue::Str("renamed".into()));
        assert_eq!(doc.nodes[&t].get(field::POS), FieldValue::Pos(Some((5.0, 6.0))));
    }

    fn name(node: NodeId, text: &str) -> OpBody {
        OpBody::set(node, field::NAME, encode_field(field::NAME, &FieldValue::Str(text.into())))
    }

    fn apply_all(ops: &[Op]) -> Doc {
        let mut doc = Doc::new(DocId([0; 16]));
        for op in ops {
            doc.apply(op);
        }
        doc
    }

    #[test]
    fn the_later_write_wins_whatever_the_arrival_order() {
        let g = id(1, 0);
        let ops =
            [op(1, 0, 1, create(g, Kind::Group, None)), op(1, 1, 5, name(g, "late")), op(2, 0, 3, name(g, "early"))];
        for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
            let doc = apply_all(&order.map(|i| ops[i].clone()));
            assert_eq!(doc.nodes[&g].get(field::NAME), FieldValue::Str("late".into()));
        }
    }

    #[test]
    fn delete_wins_over_a_concurrent_set() {
        let t = id(1, 0);
        let doc = apply_all(&[
            op(1, 0, 1, create(t, Kind::Tab, None)),
            op(1, 1, 2, OpBody::Delete { node: t }),
            op(2, 0, 9, name(t, "x")),
        ]);
        assert!(doc.nodes[&t].deleted && doc.nodes[&t].fields.is_empty());
        assert_eq!(doc.live().count(), 0);
    }

    #[test]
    fn a_move_into_its_own_subtree_is_a_no_op() {
        let (a, b) = (id(1, 0), id(1, 1));
        let doc = apply_all(&[
            op(1, 0, 1, create(a, Kind::Group, None)),
            op(1, 1, 1, create(b, Kind::Group, Some(a))),
            op(1, 2, 2, OpBody::Move { node: a, group: Some(b) }),
            op(1, 3, 3, OpBody::Move { node: a, group: Some(a) }),
        ]);
        assert_eq!((doc.nodes[&a].group, doc.nodes[&b].group), (None, Some(a)));
    }

    #[test]
    fn concurrent_moves_that_would_cycle_keep_the_earlier_one() {
        // Actor 1 puts A in B while actor 2 puts B in A. The later move
        // (actor 2's, ms 3) would close a cycle, so it is skipped, on
        // either arrival order.
        let (a, b) = (id(1, 0), id(1, 1));
        let base = [op(1, 0, 1, create(a, Kind::Group, None)), op(1, 1, 1, create(b, Kind::Group, None))];
        let one = op(1, 2, 2, OpBody::Move { node: a, group: Some(b) });
        let two = op(2, 0, 3, OpBody::Move { node: b, group: Some(a) });
        for ops in [[one.clone(), two.clone()], [two.clone(), one.clone()]] {
            let doc = apply_all(&[base.as_slice(), ops.as_slice()].concat());
            assert_eq!((doc.nodes[&a].group, doc.nodes[&b].group), (Some(b), None));
        }
    }

    #[test]
    fn a_node_moved_into_a_deleted_group_shows_at_its_nearest_live_ancestor() {
        let (outer, inner, tab) = (id(1, 0), id(1, 1), id(1, 2));
        let doc = apply_all(&[
            op(1, 0, 1, create(outer, Kind::Group, None)),
            op(1, 1, 1, create(inner, Kind::Group, Some(outer))),
            op(1, 2, 1, create(tab, Kind::Tab, None)),
            op(1, 3, 2, OpBody::Delete { node: inner }),
            op(2, 0, 3, OpBody::Move { node: tab, group: Some(inner) }),
        ]);
        assert_eq!(doc.nodes[&tab].group, Some(inner));
        assert_eq!(doc.effective_group(tab), Some(outer));
    }

    /// Random concurrent ops from three actors over a few shared nodes:
    /// field writes, moves that race into cycles, and deletes racing moves.
    fn random_ops(rng: &mut XorShift) -> Vec<Op> {
        let groups: Vec<NodeId> = (0..4).map(|c| id(1, c)).collect();
        let tabs: Vec<NodeId> = (4..8).map(|c| id(1, c)).collect();
        let mut counters = [100u64; 4];
        let mut next = |actor: u64, ms: u64, body: OpBody| {
            let counter = &mut counters[actor as usize];
            *counter += 1;
            // A distinct logical per op keeps every stamp unique.
            Op { id: id(actor, *counter), hlc: Hlc { ms, logical: *counter as u32 }, body }
        };
        let mut ops: Vec<Op> = groups.iter().map(|&g| next(1, 1, create(g, Kind::Group, None))).collect();
        ops.extend(tabs.iter().map(|&t| next(1, 1, create(t, Kind::Tab, Some(groups[0])))));
        let all: Vec<NodeId> = groups.iter().chain(&tabs).copied().collect();
        for _ in 0..60 {
            let actor = 1 + rng.below(3);
            let ms = 2 + rng.below(20);
            let node = all[rng.below(all.len() as u64) as usize];
            let body = match rng.below(10) {
                0 => OpBody::Delete { node },
                1..=4 => OpBody::Move { node, group: (rng.below(5) > 0).then(|| groups[rng.below(4) as usize]) },
                _ => name(node, &format!("n{}", rng.below(5))),
            };
            ops.push(next(actor, ms, body));
        }
        ops
    }

    #[test]
    fn prune_drops_what_no_later_op_can_need() {
        let (outer, g, t, u) = (id(1, 0), id(1, 1), id(1, 2), id(1, 3));
        let (v, w) = (id(2, 0), id(4, 0));
        let mut doc = apply_all(&[
            op(1, 0, 1, create(outer, Kind::Group, None)),
            op(1, 1, 1, create(g, Kind::Group, Some(outer))),
            op(1, 2, 1, create(t, Kind::Tab, Some(g))),
            op(1, 3, 1, create(u, Kind::Tab, Some(g))),
            op(2, 0, 2, create(v, Kind::Tab, None)),
            op(3, 0, 3, name(t, "t")),
            op(4, 1, 3, name(w, "never created")),
            op(1, 4, 4, OpBody::Delete { node: u }),
            op(2, 1, 4, OpBody::Delete { node: v }),
            op(1, 5, 4, OpBody::Delete { node: g }),
        ]);
        doc.forget_moves();
        let before = shown(&doc);
        doc.prune();
        assert_eq!(shown(&doc), before);
        assert!(![u, v, w].iter().any(|n| doc.nodes.contains_key(n)));
        assert!(doc.nodes[&g].deleted); // kept: t is shown through it, at outer
        assert_eq!(doc.effective_group(t), Some(outer));
        assert_eq!(doc.version.keys().copied().collect::<Vec<_>>(), [ActorId(1), ActorId(3)]);

        // Once t goes, nothing names g.
        doc.apply(&op(1, 6, 5, OpBody::Delete { node: t }));
        doc.prune();
        assert!(!doc.nodes.contains_key(&t) && !doc.nodes.contains_key(&g));
        assert_eq!(doc.live().map(|(id, _)| id).collect::<Vec<_>>(), [outer]);
    }

    #[test]
    fn pruning_never_changes_what_is_shown() {
        let mut rng = XorShift(0x5EED_CAFE);
        for _ in 0..200 {
            let mut doc = apply_all(&random_ops(&mut rng));
            doc.forget_moves();
            let before = shown(&doc);
            doc.prune();
            assert_eq!(shown(&doc), before);
            assert!(
                doc.nodes.values().filter_map(|n| n.group).all(|g| doc.nodes.contains_key(&g)),
                "an edge lost its group"
            );
            let once = (doc.nodes.clone(), doc.version.clone());
            doc.prune();
            assert_eq!((doc.nodes, doc.version), once);
        }
    }

    #[test]
    fn replicas_converge_whatever_the_order() {
        let mut rng = XorShift(0xDEAD_BEEF);
        for _ in 0..50 {
            let ops = random_ops(&mut rng);
            let expected = apply_all(&ops);
            for id in expected.nodes.keys() {
                assert!(!expected.is_ancestor(*id, *id), "a cycle through {id}");
            }
            let expected_bytes = crate::store::journal::snapshot_bytes(&expected);
            for _ in 0..20 {
                let mut shuffled = ops.clone();
                for i in (1..shuffled.len()).rev() {
                    shuffled.swap(i, rng.below(i as u64 + 1) as usize);
                }
                // Applying some ops twice changes nothing either.
                let again: Vec<Op> = shuffled.iter().take(10).cloned().collect();
                let doc = apply_all(&[shuffled, again].concat());
                assert_eq!(doc.nodes, expected.nodes);
                assert_eq!(doc.version, expected.version);
                assert_eq!(crate::store::journal::snapshot_bytes(&doc), expected_bytes);
            }
        }
    }
}
