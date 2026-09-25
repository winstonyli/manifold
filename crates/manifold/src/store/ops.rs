//! Operations and their merge rules (spec "Operations and merge"). Any
//! set of ops applied in any order, any number of times, gives the same
//! document.

use super::model::{decode_field, field, field_def, Doc, FieldValue, Hlc, Kind, Node, NodeId, Reg, Stamp};
use super::tlv::Reader;

#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    /// `(actor, counter)` from the writer's shared counter. A `Create`'s op
    /// id is its own: the node's id was taken when the node was made.
    pub id: NodeId,
    pub hlc: Hlc,
    pub body: OpBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpBody {
    /// `fields` is a node message (`Node::message`), `group` included.
    Create { node: NodeId, kind: Kind, fields: Vec<u8> },
    /// `enc` is `encode_field(field, value)`; empty resets the field.
    Set { node: NodeId, field: u32, enc: Vec<u8> },
    Move { node: NodeId, group: Option<NodeId> },
    Delete { node: NodeId },
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
    /// Applies `op` and returns the nodes whose shown state may have changed.
    pub fn apply(&mut self, op: &Op) -> Vec<NodeId> {
        let stamp = op.stamp();
        let next = self.version.entry(op.id.actor).or_insert(0);
        *next = (*next).max(op.id.counter + 1);
        self.clock = self.clock.max(op.hlc);
        match &op.body {
            OpBody::Create { node, kind, fields } => {
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                entry.kind = *kind;
                let mut group = None;
                for field in Reader::new(fields).map_while(Result::ok) {
                    if field.no == field::GROUP {
                        let def = field_def(field::GROUP).expect("known");
                        if let Ok(FieldValue::Id(g)) = decode_field(def, field.raw) {
                            group = g;
                        }
                    } else if !entry.deleted {
                        set_reg(entry, field.no, stamp, field.raw.to_vec());
                    }
                }
                // The initial group is a move at the create's own time.
                let mut touched = vec![*node];
                touched.extend(self.apply_move(stamp, *node, group));
                touched
            }
            OpBody::Set { node, field: no, enc } => {
                if *no == field::GROUP {
                    return Vec::new(); // the tree edge changes only by `Move`
                }
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                if entry.deleted {
                    return Vec::new();
                }
                set_reg(entry, *no, stamp, enc.clone());
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
                // Its members now show at its nearest live ancestor.
                let mut touched = vec![*node];
                touched.extend(self.nodes.iter().filter(|(_, n)| n.group == Some(*node)).map(|(id, _)| *id));
                touched
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

    /// Drops the move log. Compaction does this: every op written after it
    /// is stamped later than everything in the file, so none is undone.
    pub fn forget_moves(&mut self) {
        self.moves.clear();
    }
}

fn set_reg(node: &mut Node, no: u32, stamp: Stamp, enc: Vec<u8>) {
    if node.fields.get(&no).is_none_or(|reg| reg.stamp < stamp) {
        node.fields.insert(no, Reg { stamp, enc });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{encode_field, ActorId, DocId};
    use crate::store::testutil::XorShift;

    fn id(actor: u64, counter: u64) -> NodeId {
        NodeId { actor: ActorId(actor), counter }
    }

    fn op(actor: u64, counter: u64, ms: u64, body: OpBody) -> Op {
        // Real stamps are unique per op (`Clock::tick`); `logical` keeps them so.
        Op { id: id(actor, counter), hlc: Hlc { ms, logical: counter as u32 }, body }
    }

    fn create(node: NodeId, kind: Kind, group: Option<NodeId>) -> OpBody {
        OpBody::Create { node, kind, fields: encode_field(field::GROUP, &FieldValue::Id(group)) }
    }

    fn name(node: NodeId, text: &str) -> OpBody {
        OpBody::Set { node, field: field::NAME, enc: encode_field(field::NAME, &FieldValue::Str(text.into())) }
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
        let ops = [op(1, 0, 1, create(g, Kind::Group, None)), op(1, 1, 5, name(g, "late")), op(2, 0, 3, name(g, "early"))];
        for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
            let doc = apply_all(&order.map(|i| ops[i].clone()));
            assert_eq!(doc.nodes[&g].get(field::NAME), FieldValue::Str("late".into()));
        }
    }

    #[test]
    fn delete_wins_over_a_concurrent_set() {
        let t = id(1, 0);
        let doc = apply_all(&[op(1, 0, 1, create(t, Kind::Tab, None)), op(1, 1, 2, OpBody::Delete { node: t }), op(2, 0, 9, name(t, "x"))]);
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
