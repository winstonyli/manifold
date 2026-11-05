//! Recently Closed: close, reopen and eviction over the document's `closed` field.

use super::*;
use crate::store::model::{FieldValue, Kind, NodeId, Placement, Stamp};

/// How many roots the history keeps before `evict_closed` deletes the oldest, like a browser's own bounded
/// "Recently closed" list.
pub(crate) const MAX_CLOSED: usize = 20;

/// Whether a node of stored type `ty` (`Mirror::ty`) with `fields` (its snapshot's) is worth keeping when it is
/// closed. `false` for a completed download (it has no closed state -- starting it over isn't "reopening"), a
/// blank note (nothing to restore; the `Blank` tag) and no type at all; everything else -- tabs, groups, images,
/// PDFs, JSON files, jars and a type this build doesn't know (a shell, spec decision 4) -- is. The capability rows
/// in `caps/rows.rs` decide, by the row the type reads as (`card_kind::canonical_type`).
pub(crate) fn worth_reopening(ty: &str, fields: &session_sync::Fields) -> bool {
    use crate::caps::{Tag, TagSet};
    let ty = crate::card_kind::canonical_type(ty);
    let text = fields.get(&field::TEXT);
    let blank = !text.and_then(store::model::FieldValue::as_str).is_some_and(|text| !text.trim().is_empty());
    let tags = if ty == store::model::TYPE_NOTE && blank { TagSet::of(&[Tag::Blank]) } else { TagSet::EMPTY };
    crate::caps::registry().caps(ty, tags).restorable
}

/// Every node under `root` at any depth (`order` is the membership's source), each group visited once: a group
/// cycle (never built, but the walk must end) can't hang it.
pub(crate) fn members_under(
    root: store::model::NodeId,
    order: &[store::model::NodeId],
    group_of: impl Fn(&store::model::NodeId) -> Option<store::model::NodeId>,
    is_group: impl Fn(&store::model::NodeId) -> bool,
) -> Vec<store::model::NodeId> {
    let mut members: HashMap<store::model::NodeId, Vec<store::model::NodeId>> = HashMap::new();
    for id in order {
        if let Some(group) = group_of(id) {
            members.entry(group).or_default().push(*id);
        }
    }
    crate::walk::descendants(root, |g| members.get(g).cloned().unwrap_or_default(), is_group)
}

impl App {
    /// Whether closing `id` keeps it (spec decision 4): a node the document can hold whose kind and value are worth
    /// reopening (`worth_reopening`). Without a journal nothing is kept: the close is a plain destroy.
    fn restorable(&self, id: NodeId) -> bool {
        self.doc.journal.is_some() && self.snapshot(id).is_some_and(|s| worth_reopening(&s.ty, &s.fields))
    }

    /// Closes one node (spec decision 4), the single user-facing close: a card's hover ✕, its context-menu
    /// Delete/Close/"Remove from List", the Downloads popup's Remove, Ctrl+W, Close Selected and the rest. Typed text
    /// and the drawn position are written first; a node not worth reopening is destroyed as before; otherwise the
    /// members not worth reopening go (a blank note, a download), `closed` is queued on the root, and the subtree is
    /// hidden in place with its engines unloaded. Saves, then evicts past `MAX_CLOSED`.
    pub(crate) fn close_node(&mut self, id: NodeId) {
        if self.is_default_jar(id) {
            self.queue_toast("The Default jar can't be closed".to_string());
            return;
        }
        self.commit_note_buffers();
        // A node created this tick has no snapshot yet and its `closed` flag would be dropped as not editable (the
        // hide below would then lose it): flush the pending creates first. A save that fails leaves them pending
        // (checked below, before anything is hidden).
        if self.doc.writes.has_creates() {
            self.save_session();
        }
        if !self.restorable(id) {
            self.destroy_node(id);
            // A closed jar's tabs now resolve to the Default jar.
            self.sweep_jar_switches();
            self.save_session();
            return;
        }
        // The subtree as this replica holds it: the mirror's membership over `node_order`, so a tab mid-respawn and a
        // member not saved yet are found too.
        let members = members_under(id, &self.node_order, |m| self.doc.mirror.group(*m), |m| self.is_container(*m));
        let subtree: Vec<NodeId> = std::iter::once(id).chain(members).collect();
        let (kept, gone): (Vec<NodeId>, Vec<NodeId>) =
            subtree.into_iter().partition(|&key| key == id || self.restorable(key));
        // A kept node the flush could not create has no document entry: its `closed` write would be dropped as not
        // editable and the hide would lose the node for good. Leave everything shown instead.
        let uncreated = |key: &NodeId| self.doc.writes.life(*key) == Some(crate::session_sync::Life::Create);
        if kept.iter().any(uncreated) {
            self.queue_toast("Couldn't save the session, so nothing was closed".to_string());
            return;
        }
        // Where each card was drawn becomes its document pos (a reopen lands where it was shown).
        for &key in &kept {
            let carries_pos = Kind::from_type(self.doc.mirror.ty(key)).fields().contains(&field::POS);
            if carries_pos && self.layout.contains_key(&key) {
                self.edit_pos(key, Some(self.layout_pos(key)));
            }
        }
        for &key in &gone {
            self.remove_node(key);
        }
        self.doc.writes.queue_closed(id, self.doc.replica);
        let tabs: Vec<NodeId> = kept
            .iter()
            .copied()
            .filter(|k| self.nodes.get(k).is_some_and(|n| n.runtime.saved_tab().is_some()))
            .collect();
        self.begin_tabs_shutdown(&tabs);
        self.leave_hidden(id);
        for &key in &kept {
            self.hide_node(key);
        }
        self.sweep_jar_switches();
        self.save_session();
        self.evict_closed();
    }

    /// Reopen (spec decision 7): clears `closed` on `id` and re-stamps each closed root directly under it, oldest
    /// first, so they enter the history newest in their old order. The save's visibility pass shows what is no
    /// longer hidden (`apply_visibility`), and `apply_remote` sweeps the jar switches. A node that stays hidden
    /// (its group is closed) or lands at another level says so.
    pub(crate) fn reopen_node(&mut self, id: NodeId) {
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
        // A closed node `d` under `id` with no other closed node between them.
        let directly_under = |d: store::model::NodeId| {
            let mut cur = doc.nodes.get(&d).and_then(|n| n.group);
            for _ in 0..=doc.nodes.len() {
                let Some(c) = cur else { return false };
                if c == id {
                    return true;
                }
                let Some(n) = doc.nodes.get(&c) else { return false };
                if !n.deleted && n.closed() {
                    return false;
                }
                cur = n.group;
            }
            false
        };
        // Each keeps the replica that closed it: a re-stamp moves it in the history, not to the reopener.
        let mut nested: Vec<(Stamp, store::model::NodeId, u64)> = doc
            .with_descendants(std::collections::BTreeSet::from([id]))
            .into_iter()
            .filter(|&d| d != id && directly_under(d))
            .filter_map(|d| {
                let n = doc.nodes.get(&d).filter(|n| n.is_live() && n.closed())?;
                Some((n.reg(field::CLOSED)?.stamp, d, n.closer()))
            })
            .collect();
        nested.sort();
        self.doc.writes.queue_closed(id, 0);
        for (_, d, closer) in nested {
            self.doc.writes.queue_closed(d, closer);
        }
        self.save_session();
        self.evict_closed();
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
        if !doc.nodes.get(&id).is_some_and(|n| n.is_live() && !n.closed()) {
            return;
        }
        // Where the document shows it, not where the graph does: a tab still spawning or waking has no `nodes` entry.
        let placement = doc.placement(id);
        if matches!(placement, Placement::Shown(_)) {
            self.regroup_held(id);
        }
        match placement {
            Placement::Hidden => {
                self.queue_toast("Reopened inside a closed group: it shows when that group is reopened".to_string());
            }
            Placement::Shown(group) if group != self.current_group => {
                let level = match group {
                    None => "the top level".to_string(),
                    Some(g) => {
                        let shown = crate::names::display_name(self.doc.mirror.name(g), crate::names::UNTITLED);
                        let shown = crate::names::to_plain(&shown);
                        format!("“{shown}”")
                    }
                };
                self.queue_toast(format!("Reopened in {level}"));
            }
            Placement::Shown(_) => self.camera.reveal = Some(id),
        }
    }

    /// Eviction (spec decision 6): every root past `MAX_CLOSED` has its whole subtree Deleted, nested closed nodes
    /// included, so nothing under it resurfaces. Idempotent across replicas; the only place `closed` becomes
    /// `deleted`. After a local close, after remote ops (`apply_remote`) and at load.
    pub(crate) fn evict_closed(&mut self) {
        // The document lags this replica's own queued close and reopen writes (`PendingWrites::closed`): counting it now could
        // evict the very node being reopened. The save that commits them evicts afterwards (`apply_visibility`'s
        // `apply_remote`, or the caller's own call).
        if self.doc.writes.has_closed() {
            return;
        }
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
        let roots = doc.closed_roots();
        if roots.len() <= MAX_CLOSED {
            return;
        }
        // Past the cap the oldest go, but an emptied group (`App::ungroup`) before any real close: one Ungroup All of
        // many groups must not push out the tabs a user closed.
        let emptied = |id: &NodeId| {
            doc.nodes
                .get(id)
                .is_some_and(|n| matches!(n.get(field::HELD), FieldValue::Held { members, .. } if !members.is_empty()))
        };
        let excess = roots.len() - MAX_CLOSED;
        let oldest_first = || roots.iter().rev().map(|&(id, _)| id);
        let mut victims: Vec<NodeId> = oldest_first().filter(emptied).take(excess).collect();
        victims.extend(oldest_first().filter(|id| !emptied(id)).take(excess - victims.len()));
        let evicted: Vec<NodeId> = doc
            .with_descendants(victims.into_iter().collect())
            .into_iter()
            .filter(|id| doc.nodes.get(id).is_some_and(|n| !n.deleted))
            .collect();
        for &id in &evicted {
            self.queue_delete(id);
        }
        // Files are released only after the Delete committed (decision 11: `release_backing` reads `backing_keep`).
        if self.save_session() {
            for &id in &evicted {
                self.forget_deleted(id);
            }
        }
    }
}

#[cfg(test)]
mod tests;
