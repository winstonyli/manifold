//! The write queue (`PendingWrites`: what local edits owe the store), `build_ops` that turns it into ops, and `save_session`
//! and its aftermath, which commit it.

use super::*;

/// What the next save does to a node's existence (`NodeWrites::life`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Life {
    /// A node placed here that the next save creates (2b-5 Ruling 3): every way a node enters the live graph
    /// (`place_node`, `spawn_tab_async`, a jar's `add_jar`) queues it; the save creates it if the document lacks it,
    /// from the mirror or the spawn's snapshot (`snapshot`). One the document already holds (a restore, another
    /// replica's node) is dropped. Cleared by a successful commit once created.
    Create,
    /// A node removed here that the next save deletes (Ruling 4: `queue_delete`). Only a local removal queues: a
    /// node another replica deleted is already gone from the document, and a respawn, swap or reload only takes
    /// the node out of `nodes` for a while. Cleared by a successful commit.
    Delete,
}

/// Everything one node has waiting for the next save. A node the save creates or deletes needs none of the edits
/// (the save drops them: a `Create` carries the values, a `Delete` needs none).
#[derive(Default)]
pub(crate) struct NodeWrites {
    life: Option<Life>,
    /// A queued group `Move`: the target group, `None` for the top level.
    group: Option<Option<NodeId>>,
    /// Queued `Set`s of mirrored fields (and links), by field number, already encoded.
    fields: BTreeMap<u32, Vec<u8>>,
    /// A queued `closed` write from Close or Reopen (spec decisions 1 and 7): the order it was made in and the
    /// replica id, 0 to reopen. A reopen's re-stamps of nested roots (each keeps its closer) must commit in their old
    /// relative order, which the node order of the map would lose.
    closed: Option<(u64, u64)>,
    /// Queued path-keyed `Set`s of tree fields (`store::tree`), by `(field, path)`, already encoded; an empty
    /// encoding is a tombstone. No entry lies under another's path: the newer, shallower edit hides it.
    trees: BTreeMap<(u32, TreePath), Vec<u8>>,
}

impl NodeWrites {
    fn is_empty(&self) -> bool {
        self.life.is_none()
            && self.group.is_none()
            && self.fields.is_empty()
            && self.closed.is_none()
            && self.trees.is_empty()
    }
}

/// The session's write-back bookkeeping (`App::writes`): what the next save commits, and when saves happened or are
/// owed. A save commits in the order Creates (groups first), Moves, Sets, closed flags, Deletes (contract
/// "Transactions").
#[derive(Default)]
pub(crate) struct PendingWrites {
    /// The queued writes, one record per node, so the rules between them (a create withdraws a delete, a created or
    /// deleted node drops its edits, "is this edit queued?") live in one place.
    pub(super) nodes: BTreeMap<NodeId, NodeWrites>,
    /// Counts `queue_closed` calls, to commit the flags in the order they were made.
    closed_seq: u64,
    /// Another replica's text for a note being typed in, held back from its mirror and box until the blur
    /// (`end_note_edit`, 2b-4 Ruling 7), by node. Dropped when this replica commits its own text after it.
    pub(crate) held_text: HashMap<NodeId, String>,
    /// Store errors and repair notices toasted since the last commit
    /// (`store_error`, `take_store_recovery`), each shown once: a read-only
    /// file with a damaged tail reports both at every reload.
    pub(crate) store_notices: BTreeSet<String>,
    /// When a save that didn't commit (another instance kept writing, or
    /// the write failed) is tried again by `poll_store`; `None` when
    /// nothing is owed. Without it an edit could sit unsaved until the next
    /// local action or remote write.
    pub(crate) save_retry_at: Option<std::time::Instant>,
    /// When `poll_store` last looked at the store file's length and modified time (`STORE_STAT_GAP`).
    pub(crate) store_statted_at: Option<std::time::Instant>,
    /// When `save_session` last committed to the store -- shown in the
    /// settings popup's Sessions section next to a "Save Now" button, so a
    /// save that's actually continuous (every mutation calls
    /// `save_session`, not a timer) has some visible confirmation instead
    /// of being purely invisible. `None` until the first save this run.
    pub(crate) last_session_save: Option<std::time::Instant>,
}

impl PendingWrites {
    fn record(&mut self, node: NodeId) -> &mut NodeWrites {
        self.nodes.entry(node).or_default()
    }

    /// Drops `node`'s record once nothing is queued in it.
    fn prune(&mut self, node: NodeId) {
        if self.nodes.get(&node).is_some_and(NodeWrites::is_empty) {
            self.nodes.remove(&node);
        }
    }

    pub(crate) fn life(&self, node: NodeId) -> Option<Life> {
        self.nodes.get(&node).and_then(|w| w.life)
    }

    /// Queues `node` for creation (`in_doc`: the document already has an entry for it, so none is needed) and
    /// withdraws a pending delete of it: it is shown again.
    pub(crate) fn queue_create(&mut self, node: NodeId, in_doc: bool) {
        self.record(node).life = (!in_doc).then_some(Life::Create);
        self.prune(node);
    }

    /// Queues `node` for deletion, withdrawing a pending create of it.
    pub(crate) fn queue_delete(&mut self, node: NodeId) {
        self.record(node).life = Some(Life::Delete);
    }

    pub(crate) fn field_queued(&self, node: NodeId, no: u32) -> bool {
        self.nodes.get(&node).is_some_and(|w| w.fields.contains_key(&no))
    }

    pub(crate) fn queue_field(&mut self, node: NodeId, no: u32, enc: Vec<u8>) {
        self.record(node).fields.insert(no, enc);
    }

    /// Queues a write of `enc` (empty: a tombstone) at `path` in `node`'s tree field `field`, dropping the queued
    /// writes under `path`: it is newer and hides them.
    pub(crate) fn queue_tree(&mut self, node: NodeId, field: u32, path: TreePath, enc: Vec<u8>) {
        let trees = &mut self.record(node).trees;
        let hidden: Vec<_> =
            trees.range((field, path.clone())..(field, path.subtree_end())).map(|(key, _)| key.clone()).collect();
        for key in hidden {
            trees.remove(&key);
        }
        trees.insert((field, path), enc);
    }

    /// `node`'s queued writes to tree field `field`, in path order (an ancestor before what lies under it).
    pub(crate) fn tree_edits(&self, node: NodeId, field: u32) -> impl Iterator<Item = (&TreePath, &Vec<u8>)> + '_ {
        self.nodes.get(&node).into_iter().flat_map(move |w| {
            w.trees
                .range((field, TreePath::default())..)
                .take_while(move |((f, _), _)| *f == field)
                .map(|((_, path), enc)| (path, enc))
        })
    }

    pub(crate) fn tree_queued(&self, node: NodeId, field: u32) -> bool {
        self.tree_edits(node, field).next().is_some()
    }

    pub(crate) fn tree_sets(&self) -> impl Iterator<Item = (NodeId, u32, &TreePath, &Vec<u8>)> + '_ {
        self.nodes.iter().flat_map(|(&n, w)| w.trees.iter().map(move |((f, path), enc)| (n, *f, path, enc)))
    }

    pub(crate) fn move_queued(&self, node: NodeId) -> bool {
        self.nodes.get(&node).is_some_and(|w| w.group.is_some())
    }

    /// Queues a `Move` of `node` to `target`, `None` for the top level.
    pub(crate) fn queue_move(&mut self, node: NodeId, target: Option<NodeId>) {
        self.record(node).group = Some(target);
    }

    /// Queues a `closed` write for `node`: `closer` is the replica that closed it, 0 to reopen.
    pub(crate) fn queue_closed(&mut self, node: NodeId, closer: u64) {
        self.closed_seq += 1;
        let seq = self.closed_seq;
        self.record(node).closed = Some((seq, closer));
    }

    pub(crate) fn has_closed(&self) -> bool {
        self.nodes.values().any(|w| w.closed.is_some())
    }

    #[cfg(test)]
    pub(crate) fn has_deletes(&self) -> bool {
        self.deletes().next().is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_fields(&self) -> bool {
        self.fields().next().is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_moves(&self) -> bool {
        self.moves().next().is_some()
    }

    pub(crate) fn has_creates(&self) -> bool {
        self.creates().next().is_some()
    }

    pub(crate) fn creates(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.iter().filter(|(_, w)| w.life == Some(Life::Create)).map(|(&n, _)| n)
    }

    pub(crate) fn deletes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.iter().filter(|(_, w)| w.life == Some(Life::Delete)).map(|(&n, _)| n)
    }

    pub(crate) fn moves(&self) -> impl Iterator<Item = (NodeId, Option<NodeId>)> + '_ {
        self.nodes.iter().filter_map(|(&n, w)| Some((n, w.group?)))
    }

    pub(crate) fn fields(&self) -> impl Iterator<Item = (NodeId, u32, &Vec<u8>)> + '_ {
        self.nodes.iter().flat_map(|(&n, w)| w.fields.iter().map(move |(&no, enc)| (n, no, enc)))
    }

    /// The queued `closed` writes as `(node, replica)`, in the order they were made.
    pub(crate) fn closed(&self) -> Vec<(NodeId, u64)> {
        let mut flags: Vec<(u64, NodeId, u64)> =
            self.nodes.iter().filter_map(|(&n, w)| w.closed.map(|(seq, closer)| (seq, n, closer))).collect();
        flags.sort();
        flags.into_iter().map(|(_, n, closer)| (n, closer)).collect()
    }

    /// Forgets the creates `keep` rejects.
    pub(crate) fn retain_creates(&mut self, keep: impl Fn(NodeId) -> bool) {
        for (&node, w) in &mut self.nodes {
            if w.life == Some(Life::Create) && !keep(node) {
                w.life = None;
            }
        }
        self.nodes.retain(|_, w| !w.is_empty());
    }

    /// After a commit: the deletes are done.
    pub(crate) fn clear_deletes(&mut self) {
        for w in self.nodes.values_mut().filter(|w| w.life == Some(Life::Delete)) {
            w.life = None;
        }
        self.nodes.retain(|_, w| !w.is_empty());
    }

    /// After a commit: the field sets and moves are done.
    pub(crate) fn clear_edits(&mut self) {
        for w in self.nodes.values_mut() {
            w.fields.clear();
            w.trees.clear();
            w.group = None;
        }
        self.nodes.retain(|_, w| !w.is_empty());
    }

    /// After a commit: takes the `closed` writes, returning their nodes.
    pub(crate) fn take_closed(&mut self) -> BTreeSet<NodeId> {
        let mut taken = BTreeSet::new();
        for (&node, w) in &mut self.nodes {
            if w.closed.take().is_some() {
                taken.insert(node);
            }
        }
        self.nodes.retain(|_, w| !w.is_empty());
        taken
    }
}

/// Whether tab `id`'s trail is kept at a save (trail spec decision 10): shown (`nodes`), or live in the document (a
/// closed tab, or one hidden in a closed group, which a reopen brings back). With no document (no session yet, a
/// store error) everything is kept.
pub(crate) fn keep_trail(
    nodes: &crate::node::NodeMap,
    journal: Option<&crate::store::journal::Journal>,
    id: NodeId,
) -> bool {
    nodes.contains_key(&id)
        || journal.is_none_or(|j| j.doc().nodes.get(&id).is_some_and(crate::store::model::Node::is_live))
}

/// What `save_session` builds from the queued writes against the document, before the commit.
pub(super) struct Built {
    ops: Vec<OpBody>,
    /// Nodes this commit creates.
    created: BTreeSet<NodeId>,
    /// Nodes this commit moves.
    moved: BTreeSet<NodeId>,
    /// Each pending link's target as this commit leaves it (see `build_ops`).
    links: BTreeMap<(NodeId, u32), Option<NodeId>>,
}

/// The ops that commit `writes` on `doc` (pure: no `App` state), in the order Creates, Moves, Sets, closed flags,
/// Deletes. `candidates` are the queued creates' snapshots.
pub(super) fn build_ops(doc: &Doc, writes: &PendingWrites, candidates: BTreeMap<NodeId, Snapshot>) -> Built {
    // A queued node is created only if the document has no entry for it at all: a tombstone is never
    // recreated (ids are never reused), and one another replica created meanwhile stands.
    let new: BTreeMap<NodeId, Snapshot> =
        candidates.into_iter().filter(|(id, _)| !doc.nodes.contains_key(id)).collect();
    // A queued delete applies to a node the document still shows live (Ruling 4); one it never had, or
    // another replica deleted meanwhile, is just forgotten. Nothing queued is shown here: `materialize`
    // drops a remote edit to it, and showing it again locally withdraws the delete (`queue_create`).
    let deleted: BTreeSet<NodeId> =
        writes.deletes().filter(|id| doc.nodes.get(id).is_some_and(Node::is_live)).collect();
    let mut ops = create_ops(doc, &new, &deleted);
    let created: BTreeSet<NodeId> = new.keys().copied().collect();
    // A created node's tree rides with its `Create`: the snapshot holds the mirror's tree as of this save, edits
    // included, so the queued edits of a created node are dropped below like its scalar ones.
    for (id, snap) in new.iter().filter(|(id, _)| !deleted.contains(id)) {
        for (&field, tree) in &snap.trees {
            for (path, leaf) in tree {
                ops.push(OpBody::Set { node: *id, field, path: path.clone(), enc: leaf.encode() });
            }
        }
    }
    let resolve = |target: Option<NodeId>| resolve_id(doc, &created, &deleted, target);
    // Edits to mirrored fields and moves. A node this commit creates carries its values in the `Create`,
    // and one it deletes needs none; one the document doesn't show live would get a placeholder from a
    // `Set`. Those edits are dropped.
    let editable = |node: &NodeId| {
        !created.contains(node) && !deleted.contains(node) && doc.nodes.get(node).is_some_and(Node::is_live)
    };
    // Each pending link's target as this commit leaves it: what the mirror shows once it lands. A moved
    // node's group is read back from the document after the commit instead: the store may refuse the move.
    let mut moved: BTreeSet<NodeId> = BTreeSet::new();
    let mut links: BTreeMap<(NodeId, u32), Option<NodeId>> = BTreeMap::new();
    // Moves before sets (Ruling 4). A move to where the document already shows the node is left out.
    for (node, group) in writes.moves() {
        if !editable(&node) {
            continue;
        }
        let group = resolve(group);
        moved.insert(node);
        if group != doc.effective_group(node) {
            ops.push(OpBody::Move { node, group });
        }
    }
    for (node, no, enc) in writes.fields() {
        if !editable(&node) {
            continue;
        }
        // Every link (an `Id` field: `parent`, a tab's `jar`) is resolved alike (2b-3 Ruling 8).
        let def = field_def(no);
        let enc = if def.is_some_and(|def| def.ty == FieldType::Id) {
            let target = match def.and_then(|def| decode_field(def, enc).ok()) {
                Some(FieldValue::Id(target)) => resolve(target),
                _ => None,
            };
            links.insert((node, no), target);
            encode_field(no, &FieldValue::Id(target))
        } else {
            enc.clone()
        };
        ops.push(OpBody::set(node, no, enc));
    }
    for (node, field, path, enc) in writes.tree_sets() {
        if editable(&node) {
            ops.push(OpBody::Set { node, field, path: path.clone(), enc: enc.clone() });
        }
    }
    for (node, closer) in writes.closed() {
        if editable(&node) {
            let enc = encode_field(field::CLOSED, &FieldValue::Replica(closer));
            ops.push(OpBody::set(node, field::CLOSED, enc));
        }
    }
    ops.extend(deleted.iter().map(|&node| OpBody::Delete { node }));
    Built { ops, created, moved, links }
}

impl App {
    /// What the store holds for `id`, or is about to, as a `Snapshot`: a graph node's fields of its kind as the mirror
    /// holds them (2b-5: every stored field of a graph node is mirror state), or the snapshot a
    /// tab's spawn started with (`spawning`, for one not shown yet). A tab's url is the shared one, not the live one
    /// its payload holds (2b-3 Ruling 1); its engine is the one a swap in progress already shows as its target
    /// (`start_swap_to`); a file card's path is the stored one, not the one its payload resolved here (2b-5 Ruling
    /// 5). Its type is the mirror's string (`Mirror::ty`, the one source of a node's type), kept as written. `None` for an in-progress download
    /// (it can't resume, so it isn't created until it completes), for a node with no type yet, and for an id that is
    /// none of these. A type this build doesn't know is snapshotted with its string and the generic fields
    /// (`Kind::fields`).
    pub(crate) fn snapshot(&self, id: NodeId) -> Option<Snapshot> {
        if self.nodes.contains_key(&id) {
            if self.is_running_download(id) {
                return None;
            }
            let ty = self.doc.mirror.ty(id);
            if ty.is_empty() {
                return None;
            }
            let kind = Kind::from_type(ty);
            let fields = kind
                .fields()
                .iter()
                .map(|&no| match no {
                    field::PARENT => (no, FieldValue::Id(self.doc.mirror.parent(id))),
                    _ => (no, self.doc.mirror.get(id, no)),
                })
                .collect();
            return Some(Snapshot {
                ty: ty.to_owned(),
                group: self.doc.mirror.group(id),
                fields,
                trees: self.doc.mirror.trees_of(id),
            });
        }
        if self.doc.mirror.is_link(id) {
            let fields = Kind::Link.fields().iter().map(|&no| (no, self.doc.mirror.get(id, no))).collect();
            return Some(Snapshot { ty: TYPE_LINK.to_owned(), group: None, fields, trees: Trees::new() });
        }
        self.spawns.spawning.get(&id).cloned()
    }

    /// Whether card `key` is a download still in progress (or failed): placed, but not stored until it completes
    /// (`snapshot`), so nothing stored may name it yet (`check_connect`).
    pub(crate) fn is_running_download(&self, key: NodeId) -> bool {
        self.nodes.get(&key).and_then(|n| n.as_download()).is_some_and(|dl| !dl.completed)
    }

    /// Queues `id`, a node that just entered the live graph (`place_node`, `spawn_tab_async`, a jar's `add_jar`),
    /// for the next save to create (Ruling 3), and withdraws a pending delete of it: it is shown again. One the
    /// document already has an entry for (shown again from a restore or another replica's op, or a tombstone) needs
    /// no create.
    pub(crate) fn queue_create(&mut self, id: NodeId) {
        let in_doc = self.doc.journal.as_ref().is_some_and(|j| j.doc().nodes.contains_key(&id));
        self.doc.writes.queue_create(id, in_doc);
    }

    /// Queues `id`, a node removed here, for the next save to delete (Ruling 4). The save deletes it if the document
    /// still shows it live, so a node never created is just forgotten. Until then another replica's edit to it is
    /// dropped rather than showing it again (`materialize`). Only a local removal queues (a remote delete queues
    /// only the deletes of the links naming the node, `remove_remotely_deleted`): a node another replica
    /// deleted is already gone from the document, and a crash respawn, an engine swap
    /// and a content reload take a node out of `nodes` only for a while.
    pub(crate) fn queue_delete(&mut self, id: NodeId) {
        self.doc.writes.queue_delete(id);
        // A link goes with either end. `delete_link` forgets the link before it queues the delete, so a link naming
        // a link (remote data can) is found once at most: the recursion ends.
        for link in self.links_naming(id) {
            self.delete_link(link);
        }
    }

    /// Saves browsing history to disk if `history_dirty` is set, then clears
    /// the flag. Called from `save_session` and `close_store` on every return
    /// path. History doesn't depend on the store, so it's saved regardless of
    /// store success or failure.
    pub(super) fn flush_history(&mut self) {
        if self.history_ui.history_dirty {
            crate::history::save(&self.history);
            self.history_ui.history_dirty = false;
        }
        let (nodes, journal) = (&self.nodes, self.doc.journal.as_ref());
        crate::nav_history::save(&mut self.nav_history, |id| keep_trail(nodes, journal, id));
    }

    /// What a successful `save_session` commit changes beyond the store: the mirror takes what the commit resolved,
    /// the queues clear, and visibility follows the flags and moves.
    pub(super) fn after_commit(&mut self, built: Built) {
        let Built { created, moved, links, .. } = built;
        // The mirror already shows what this commit wrote (it is what the ops were built from); only
        // the links the commit resolved can differ from it.
        if let Some(journal) = &self.doc.journal {
            let doc = journal.doc();
            // A node this commit created shows the links its `Create` resolved (`resolve_id`: a
            // tab's jar, a group's default jar, any Id field), as an edited one does. Only one
            // with an entry: a new spawn's lands from its `TabSpawn` (`linked_ids`). `parent` lives
            // in the mirror's links and `group` is the moves' (below).
            for id in &created {
                let Some(node) = doc.nodes.get(id) else { continue };
                if !self.doc.mirror.has(*id) {
                    continue;
                }
                for &no in node.kind().fields() {
                    let is_id = field_def(no).is_some_and(|def| def.ty == FieldType::Id);
                    if !is_id || no == field::PARENT {
                        continue;
                    }
                    let stored = node.get(no);
                    if self.doc.mirror.get(*id, no) != stored {
                        self.doc.mirror.set(*id, no, stored);
                    }
                }
            }
            // A moved node shows the group the document holds: the target, none for one this commit
            // deleted (Ruling 5), or where it was when the store refused the move as a cycle (a
            // concurrent move raced it), which the mirror would otherwise keep showing.
            for &node in &moved {
                let group = doc.effective_group(node);
                if self.doc.mirror.group(node) != group {
                    self.doc.mirror.set_group(node, group);
                }
            }
            // Created ids are done; one still queued shows nothing yet (an in-progress download) and
            // waits for the save after it does.
            self.doc.writes.retain_creates(|id| !doc.nodes.contains_key(&id));
        }
        self.doc.writes.clear_deletes();
        // A link target this commit wrote as none (Ruling 5: deleted here) is none in the mirror too.
        for (&(node, no), &target) in &links {
            if no == field::PARENT {
                if self.doc.mirror.parent(node) != target {
                    self.doc.mirror.set_parent(node, target);
                }
            } else {
                self.doc.mirror.set(node, no, FieldValue::Id(target));
            }
        }
        // A remote value parked for a tab still spawning (`remote_in_flight`) predates what this
        // commit wrote; its landing must not replay it over the local edit.
        // One pass over the parked snapshots: each part this commit wrote is re-read from the mirror.
        let (fields, moved): (Vec<(NodeId, u32)>, Vec<NodeId>) = (
            self.doc.writes.fields().map(|(n, no, _)| (n, no)).collect(),
            self.doc.writes.moves().map(|(n, _)| n).collect(),
        );
        let treed: Vec<NodeId> = self.doc.writes.tree_sets().map(|(n, ..)| n).collect();
        for (&node, remote) in &mut self.spawns.remote_in_flight {
            let Remote::Shown(snap) = remote else { continue };
            for &(_, no) in fields.iter().filter(|(n, _)| *n == node) {
                snap.fields.insert(
                    no,
                    match no {
                        field::PARENT => FieldValue::Id(self.doc.mirror.parent(node)),
                        _ => self.doc.mirror.get(node, no),
                    },
                );
            }
            if moved.contains(&node) {
                snap.group = self.doc.mirror.group(node);
            }
            if treed.contains(&node) {
                snap.trees = self.doc.mirror.trees_of(node);
            }
        }
        // Text committed here is newer than another replica's held before it: the held text is dropped.
        let texts: Vec<NodeId> =
            self.doc.writes.fields().filter(|&(_, no, _)| no == field::TEXT).map(|(n, ..)| n).collect();
        for node in texts {
            self.doc.writes.held_text.remove(&node);
        }
        self.doc.writes.clear_edits();
        let flagged = self.doc.writes.take_closed();
        // A tab still spawning keeps its entry, and its layout: a respawn has removed it from `nodes`
        // until it lands, and `Mirror::init` then leaves an existing entry alone.
        let (nodes, spawning) = (&self.nodes, &self.spawns.spawning);
        self.doc.mirror.retain(|id| nodes.contains_key(&id) || spawning.contains_key(&id));
        self.layout.retain(|id, _| nodes.contains_key(id) || spawning.contains_key(id));
        self.doc.writes.held_text.retain(|id, _| nodes.get(id).and_then(|n| n.as_note()).is_some_and(|n| n.editing));
        self.doc.writes.store_notices.clear();
        self.doc.writes.last_session_save = Some(std::time::Instant::now());
        self.doc.writes.save_retry_at = None;
        self.flush_history();
        // What this commit's flags and moves changed about visibility (a reopen, a nested root
        // re-stamped, a hidden Default moved out) is shown or hidden the way a remote change is.
        self.apply_visibility(flagged.into_iter().chain(moved).collect());
        #[cfg(debug_assertions)]
        self.check_graph_matches_document();
    }

    /// Saves local edits (spec "Local edits become ops"): take the lock,
    /// show what other replicas wrote, and commit the queued edits as one
    /// transaction, in the order Creates (groups first), Moves, Sets, the closed flags, then Deletes (`writes`).
    /// On error the queues stay for the next save (contract "Transactions").
    /// `true` when the store now holds every local edit: `false` read-only,
    /// on error, and when another instance kept writing. The last two are
    /// tried again by `poll_store` after `SAVE_RETRY`.
    pub(crate) fn save_session(&mut self) -> bool {
        // Each round applies what others wrote since the last; four rounds
        // of fresh remote frames means another instance is busy: try again
        // later (`save_retry_at`) rather than spin.
        // What is being typed is committed too (2b-4 Ruling 6).
        self.commit_note_buffers();
        for _ in 0..4 {
            // Projected before `begin`, since the `Txn` borrows the journal: what each queued node shows (one that
            // shows nothing yet, an in-progress download, waits).
            let candidates: BTreeMap<NodeId, Snapshot> =
                self.doc.writes.creates().filter_map(|id| Some((id, self.snapshot(id)?))).collect();
            let Some(journal) = self.doc.journal.as_mut() else {
                self.flush_history();
                return false;
            };
            let txn = match journal.begin() {
                Ok(txn) => txn,
                Err(err) => {
                    self.store_error(err);
                    self.doc.writes.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
                    self.flush_history();
                    return false;
                }
            };
            if !txn.remote.is_empty() {
                let changes = Self::remote_changes(txn.doc(), &txn.remote);
                drop(txn);
                self.take_store_recovery();
                self.apply_remote(changes);
                continue;
            }
            if txn.read_only() {
                drop(txn);
                self.notice("This session was written by a newer manifold; changes aren't saved".to_string());
                self.take_store_recovery();
                self.flush_history();
                return false;
            }
            let mut built = build_ops(txn.doc(), &self.doc.writes, candidates);
            let result = if built.ops.is_empty() {
                drop(txn);
                Ok(())
            } else {
                txn.commit(std::mem::take(&mut built.ops))
            };
            self.take_store_recovery();
            match result {
                Ok(()) => {
                    self.after_commit(built);
                    return true;
                }
                Err(err) => {
                    self.store_error(err);
                    self.doc.writes.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
                    self.flush_history();
                    return false;
                }
            }
        }
        self.doc.writes.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY);
        self.flush_history();
        false
    }

    /// Ruling 14, test and debug builds: after a successful save the live graph and the document agree. Every store
    /// id in `nodes` (an in-progress download aside) and every jar is live in the document; a tab whose spawn is
    /// in flight has an entry (live, or a tombstone another replica left while it was in flight, which the landing
    /// applies); and every live document node is one of them. A failure names the funnel that was missed: a node
    /// that entered the graph without `place_node`, `spawn_tab_async` or `add_jar` (a missed create), or left it
    /// without `queue_delete` (a missed delete). Each node's stored type is one its runtime runs, so a create that
    /// passed a mismatched kind (an image type on a pdf runtime) is caught before a painter dispatches on it.
    #[cfg(debug_assertions)]
    pub(super) fn check_graph_matches_document(&self) {
        let Some(journal) = &self.doc.journal else { return };
        let doc = journal.doc();
        let live = |id: &NodeId| doc.nodes.get(id).is_some_and(Node::is_live);
        for (&id, node) in &self.nodes {
            if node.as_download().is_some_and(|dl| !dl.completed) || self.spawns.spawning.contains_key(&id) {
                continue;
            }
            assert!(live(&id), "node {id} is shown but not live in the document: a create funnel was missed");
            let ty = self.card_kind(id).map(|kind| kind.ty);
            assert!(
                ty.is_some_and(|ty| runtime_has_type(&node.runtime, ty)),
                "node {id} has type {ty:?} that its runtime does not run: a create funnel passed a mismatched kind"
            );
            for &(no, name) in &TREE_FIELDS {
                assert!(
                    self.doc.writes.tree_queued(id, no)
                        || self.doc.mirror.tree(id, no).cloned().unwrap_or_default()
                            == doc.nodes.get(&id).map(Node::trees).and_then(|mut t| t.remove(&no)).unwrap_or_default(),
                    "node {id}: the mirror's {name} tree differs from the document's: a tree write bypassed the mirror"
                );
            }
        }
        for id in self.doc.mirror.link_ids() {
            assert!(live(&id), "link {id} is shown but not live in the document: a create funnel was missed");
        }
        for id in self.spawns.spawning.keys() {
            assert!(
                doc.nodes.contains_key(id),
                "tab {id} is spawning but the document has no entry: a create funnel was missed"
            );
        }
        for (id, _) in doc.shown() {
            let shown =
                self.nodes.contains_key(&id) || self.spawns.spawning.contains_key(&id) || self.doc.mirror.is_link(id);
            assert!(shown, "node {id} is live in the document but not shown: a delete funnel was missed");
        }
    }
}
