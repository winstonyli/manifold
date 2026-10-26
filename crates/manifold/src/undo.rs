//! Session-local undo and redo of drops, folds and property edits. Docs: `docs/superpowers/specs/2026-10-19-move-undo-design.md`.
//! In memory only: not saved, not synced. A revert is an ordinary edit through `edit_group`, `edit_parent` and
//! `edit_pos`, so other devices see it as one.

use super::*;
use crate::store::model::NodeId;
use crate::store::tree::{self, Leaf, TreePath};

/// How many steps the history keeps, each way.
pub(crate) const UNDO_DEPTH: usize = 10;

/// One card a drop changed: each field's (before, after).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Moved {
    pub(crate) id: NodeId,
    pub(crate) group: (Option<NodeId>, Option<NodeId>),
    pub(crate) parent: (Option<NodeId>, Option<NodeId>),
    pub(crate) pos: (Option<egui::Vec2>, Option<egui::Vec2>),
}

/// One write to a tree field (`App::edit_prop`): `path` held `before` (every leaf at or under it) and now holds
/// `after` (`None`: deleted). Undo and redo replay it as a further edit.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PropEdit {
    pub(crate) node: NodeId,
    pub(crate) field: u32,
    pub(crate) path: TreePath,
    pub(crate) before: Vec<(TreePath, Leaf)>,
    pub(crate) after: Option<Leaf>,
}

pub(crate) enum UndoKind {
    /// A label deleted (`App::delete_label`). A store delete is permanent, so undo makes a new label from `name` and
    /// `color` and puts each of `carriers` (card, membership stamp) on it; `label` is the one it made, so redo
    /// deletes that. `None` until the first undo.
    LabelDelete { label: Option<NodeId>, name: String, color: Option<egui::Color32>, carriers: Vec<(NodeId, i64)> },
    /// A drop with an effect (Join, GroupWith, Eject, Reparent). `created` is the group a GroupWith made.
    Revert { moved: Vec<Moved>, created: Option<NodeId> },
    /// Dissolve, Ungroup All, Merge: each group and where `fold_group` sent its members. Undo reopens the groups,
    /// redo folds them again.
    Fold(Vec<(NodeId, Option<NodeId>)>),
    /// Tree edits (`App::edit_prop`): labels, properties, Json. Each part changed the leaves at `path` and under it
    /// from `before` to `after`; undo reverses them newest first, redo repeats them.
    Props(Vec<PropEdit>),
    /// A file Json card detached to inline (`App::detach_json`): `stored` was its synced `PATH`, `shown` the file this
    /// machine read, and `body` the BODY tree the detach wrote. Undo applies only while the card still holds exactly
    /// `body`; redo detaches again.
    Detach { node: NodeId, stored: String, shown: String, url: String, body: tree::Tree },
}

pub(crate) struct UndoEntry {
    pub(crate) serial: u64,
    pub(crate) message: String,
    pub(crate) kind: UndoKind,
}

#[derive(Default)]
pub(crate) struct UndoHistory {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    next: u64,
}

impl UndoHistory {
    /// A fresh step: onto `undo`, and nothing left to redo.
    pub(crate) fn record(&mut self, message: String, kind: UndoKind) -> u64 {
        self.next += 1;
        self.redo.clear();
        self.undo.push(UndoEntry { serial: self.next, message, kind });
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.next
    }

    /// Takes entry `serial` (the newest when `None`) off `redo` when `from_redo`, else off `undo`.
    pub(crate) fn take(&mut self, serial: Option<u64>, from_redo: bool) -> Option<UndoEntry> {
        let list = if from_redo { &mut self.redo } else { &mut self.undo };
        let at = match serial {
            Some(serial) => list.iter().position(|e| e.serial == serial)?,
            None => list.len().checked_sub(1)?,
        };
        Some(list.remove(at))
    }

    /// Puts an entry `take` gave from `undo` onto `redo` (`to_redo`), or one from `redo` back onto `undo`.
    pub(crate) fn put(&mut self, entry: UndoEntry, to_redo: bool) {
        let list = if to_redo { &mut self.redo } else { &mut self.undo };
        list.push(entry);
        if list.len() > UNDO_DEPTH {
            list.remove(0);
        }
    }
}

impl App {
    /// A new step: remembers it and offers its Undo in a toast.
    pub(crate) fn record_undo(&mut self, message: String, kind: UndoKind) {
        let serial = self.undo.record(message.clone(), kind);
        self.toasts.undo = Some((message.clone(), serial, false));
        self.queue_toast(message);
    }

    /// Undoes (or, with `redo`, redoes) entry `serial`, the newest when `None`, and offers the way back in a toast.
    /// An entry that restored nothing is dropped: its cards have all moved on.
    pub(crate) fn step_history(&mut self, serial: Option<u64>, redo: bool) {
        let Some(mut entry) = self.undo.take(serial, redo) else { return };
        let (done, total) = self.replay(&mut entry.kind, redo);
        let verb = if redo { "Redone" } else { "Undone" };
        let message = match (done, total) {
            (0, _) => format!("Nothing to {}: it has changed since", if redo { "redo" } else { "undo" }),
            (d, t) if d == t => format!("{verb}: {}", entry.message),
            (d, t) => format!("{verb} {d} of {t}: {}", entry.message),
        };
        if done > 0 {
            let serial = entry.serial;
            self.undo.put(entry, !redo);
            self.toasts.undo = Some((message.clone(), serial, !redo));
        }
        self.queue_toast(message);
        self.save_session();
    }

    /// Applies `kind` one way; returns how many of its parts took effect and how many there were.
    fn replay(&mut self, kind: &mut UndoKind, redo: bool) -> (usize, usize) {
        match kind {
            UndoKind::Fold(groups) => self.replay_fold(groups, redo),
            UndoKind::Revert { moved, created } => self.replay_revert(moved, *created, redo),
            UndoKind::Props(edits) => self.replay_props(edits, redo),
            UndoKind::LabelDelete { label, name, color, carriers } => {
                self.replay_label_delete(label, name, *color, carriers, redo)
            }
            UndoKind::Detach { node, stored, shown, url, body } => {
                (usize::from(self.replay_detach(*node, stored, shown, url, body, redo)), 1)
            }
        }
    }

    /// A label delete undone (a new label with the old name, colour and carriers; a name another label has since
    /// taken folds into that label) or redone (the label undo made is deleted). Parts: the label, then each carrier.
    fn replay_label_delete(
        &mut self,
        label: &mut Option<NodeId>,
        name: &str,
        color: Option<egui::Color32>,
        carriers: &[(NodeId, i64)],
        redo: bool,
    ) -> (usize, usize) {
        let total = 1 + carriers.len();
        if redo {
            let Some(id) = label.filter(|&id| self.doc.mirror.is_stored(id)) else { return (0, total) };
            self.delete_label_group(id);
            return (total, total);
        }
        let Some(id) = self.create_label(name, color) else { return (0, total) };
        *label = Some(id);
        let mut done = 1;
        for &(card, at) in carriers {
            if self.nodes.contains_key(&card) {
                self.edit_prop(card, field::PROPS, crate::labels::labels_path(id), Some(Leaf::Int(at)));
                done += 1;
            }
        }
        (done, total)
    }

    /// A detach undone (the card is inline and still holds `body`: the BODY tree is deleted, the path restored and
    /// the file read again) or redone (the card is that file's again, and detaches once more). False when the card
    /// has moved on.
    fn replay_detach(
        &mut self,
        node: NodeId,
        stored: &str,
        shown: &str,
        url: &str,
        body: &mut tree::Tree,
        redo: bool,
    ) -> bool {
        let Some(json) = self.nodes.get(&node).and_then(|n| n.as_json()) else { return false };
        if redo {
            if json.is_inline() || json.path() != shown {
                return false;
            }
            // The file may have changed since: the entry now holds what this detach wrote, for the next undo.
            return self.detach_json_untracked(node).map(|written| *body = written).is_ok();
        }
        if !json.is_inline() || self.doc.mirror.tree(node, field::BODY) != Some(&*body) {
            return false;
        }
        // One tombstone at the root, so a leaf another replica wrote meanwhile is hidden too.
        self.edit_prop(node, field::BODY, crate::store::body::root(), None);
        self.edit_field(node, field::PATH, FieldValue::Str(stored.to_owned()));
        self.edit_field(node, field::URL, FieldValue::Str(url.to_owned()));
        self.rebuild_file_json(node, shown.to_owned());
        true
    }

    /// Puts each edit's leaves back (undo) or forward (redo), but only where the mirror still shows exactly what is
    /// being left (the rule of `replay_revert`): a register edited again here or by another replica is skipped, as
    /// is a card closed since. The restore is a tombstone at `path` and a write per arriving leaf, ancestors first,
    /// which commits as the same registers the edit had.
    fn replay_props(&mut self, edits: &[PropEdit], redo: bool) -> (usize, usize) {
        let mut done = 0;
        let ordered: Vec<&PropEdit> = if redo { edits.iter().collect() } else { edits.iter().rev().collect() };
        for e in ordered {
            let after: Vec<(TreePath, Leaf)> = e.after.iter().map(|l| (e.path.clone(), l.clone())).collect();
            let (leaving, arriving) = if redo { (&e.before, &after) } else { (&after, &e.before) };
            let now = self.doc.mirror.tree(e.node, e.field).map(|t| tree::subtree(t, &e.path)).unwrap_or_default();
            if now != *leaving || !self.nodes.contains_key(&e.node) {
                continue;
            }
            self.edit_prop(e.node, e.field, e.path.clone(), None);
            for (path, leaf) in arriving {
                self.edit_prop(e.node, e.field, path.clone(), Some(leaf.clone()));
            }
            done += 1;
        }
        let mut bodies: Vec<NodeId> = edits.iter().filter(|e| e.field == field::BODY).map(|e| e.node).collect();
        bodies.dedup();
        for node in bodies {
            self.refresh_inline(node);
        }
        (done, edits.len())
    }

    /// Undo puts each card back to its before values, redo to its after values, but only a card that still holds
    /// the values being left (the rule of `regroup_held`): a card moved again, closed or changed remotely is skipped.
    fn replay_revert(&mut self, moved: &[Moved], created: Option<NodeId>, redo: bool) -> (usize, usize) {
        // A redo of a GroupWith brings the made group back first.
        if let (true, Some(group)) = (redo, created) {
            if !self.nodes.contains_key(&group) {
                self.reopen_node(group);
            }
        }
        // An undo leaves the drop's after values and arrives at its before values; a redo the reverse.
        let (leaving_after, arriving_after) = (!redo, redo);
        let side = |pair: &(Option<NodeId>, Option<NodeId>), after: bool| if after { pair.1 } else { pair.0 };
        let mut done = 0;
        for m in moved {
            let holds = self.card_in_group(m.id, side(&m.group, leaving_after))
                && self.doc.mirror.parent(m.id) == side(&m.parent, leaving_after)
                && self.doc.mirror.pos(m.id) == if leaving_after { m.pos.1 } else { m.pos.0 };
            if !holds {
                continue;
            }
            let (group, parent) = (side(&m.group, arriving_after), side(&m.parent, arriving_after));
            // A destination closed or deleted since would hide the card with it.
            let gone = |id: &Option<NodeId>| id.as_ref().is_some_and(|id| !self.nodes.contains_key(id));
            if gone(&group) || gone(&parent) || parent.is_some_and(|p| self.would_create_cycle(m.id, p)) {
                continue;
            }
            if m.group.0 != m.group.1 {
                self.edit_group(m.id, group);
                self.recheck_edge_scope(m.id);
            }
            if m.parent.0 != m.parent.1 {
                self.edit_parent(m.id, parent);
            }
            if m.pos.0 != m.pos.1 {
                let pos = if arriving_after { m.pos.1 } else { m.pos.0 };
                self.edit_pos(m.id, pos);
                // The card is drawn from the layout, which only a remote pos or a load moves otherwise.
                if let Some(pos) = pos {
                    self.snap_layout(m.id, pos);
                }
            }
            done += 1;
        }
        // An undone GroupWith closes the group it made once nothing is left in it.
        if let (false, Some(group)) = (redo, created) {
            if self.nodes.contains_key(&group) && self.members_of(group).next().is_none() {
                self.close_node(group);
            }
        }
        (done, moved.len())
    }

    /// Undo reopens each group (`reopen_node`, which puts back the members still where the fold sent them); redo
    /// folds each again as the group is now, if it and its destination are shown.
    fn replay_fold(&mut self, groups: &[(NodeId, Option<NodeId>)], redo: bool) -> (usize, usize) {
        let mut done = 0;
        for &(group, dest) in groups {
            if redo {
                if self.nodes.contains_key(&group) && dest.as_ref().is_none_or(|d| self.nodes.contains_key(d)) {
                    self.fold_group(group, dest);
                    done += 1;
                }
            } else if !self.nodes.contains_key(&group) {
                // A group shown again already (reopened from Recently Closed) has nothing to undo.
                self.reopen_node(group);
                done += 1;
            }
        }
        (done, groups.len())
    }
}

#[cfg(test)]
mod tests;
