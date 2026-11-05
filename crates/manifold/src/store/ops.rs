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
mod tests;
