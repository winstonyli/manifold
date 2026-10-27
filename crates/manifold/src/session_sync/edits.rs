//! The `edit_*` writers: a UI change becomes a mirror write plus a queued field, move or group change.

use super::*;

impl App {
    /// The one writer of a node's tree fields: the leaf at `path` becomes `leaf`, or, for `None`, `path` and
    /// everything under it is deleted (one tombstone hides the subtree, so a subtree delete is the same call). The
    /// mirror shows it at once; the next save commits it as a `Set` at `path`. Returns what an undo needs, or `None`
    /// when the mirror already showed exactly this. The caller records the step (`record_undo(.., UndoKind::Props)`).
    pub(crate) fn edit_prop(
        &mut self,
        id: NodeId,
        field: u32,
        path: TreePath,
        leaf: Option<Leaf>,
    ) -> Option<crate::undo::PropEdit> {
        debug_assert!(TREE_FIELDS.iter().any(|&(no, _)| no == field), "a tree field");
        debug_assert!(!path.is_root(), "a leaf has a path");
        debug_assert!(self.doc.mirror.has(id), "a node the mirror knows");
        let before = self.doc.mirror.edit_tree(id, field, &path, leaf.clone())?;
        let enc = leaf.as_ref().map_or_else(Vec::new, Leaf::encode);
        self.doc.writes.queue_tree(id, field, path.clone(), enc);
        Some(crate::undo::PropEdit { node: id, field, path, before, after: leaf })
    }

    /// Replaces `id`'s BODY tree with `tree`: the mirror shows it at once and the next save commits the difference
    /// (a `Set` per changed leaf, a tombstone per vanished one). The bulk twin of `edit_prop`, which clones the whole
    /// tree per leaf. Returns the new `Mirror::tree_version`.
    pub(crate) fn set_body(&mut self, id: NodeId, tree: crate::store::tree::Tree) -> u64 {
        debug_assert!(self.doc.mirror.has(id), "a node the mirror knows");
        let old = self.doc.mirror.tree(id, field::BODY).cloned().unwrap_or_default();
        for path in old.keys().filter(|p| !tree.contains_key(*p)) {
            self.doc.writes.queue_tree(id, field::BODY, path.clone(), Vec::new());
        }
        for (path, leaf) in tree.iter().filter(|(p, l)| old.get(*p) != Some(*l)) {
            self.doc.writes.queue_tree(id, field::BODY, path.clone(), leaf.encode());
        }
        self.doc.mirror.set_tree(id, field::BODY, tree);
        self.doc.mirror.tree_version(id, field::BODY)
    }

    /// The one writer of mirrored fields (node model spec, 2b "Read model"): the mirror shows `value` at once, and
    /// the next save commits it as a `Set`. A value the mirror already shows queues nothing.
    pub(crate) fn edit_field(&mut self, id: NodeId, no: u32, value: FieldValue) {
        debug_assert!(!matches!(no, field::GROUP | field::PARENT), "a link: `edit_group` / `edit_parent`");
        if self.doc.mirror.get(id, no) == value {
            return;
        }
        self.doc.writes.queue_field(id, no, encode_field(no, &value));
        self.doc.mirror.set(id, no, value);
    }

    /// The one writer of a tab's shared url: called with the url at the moment it is shared (a typed
    /// navigation, or a report `shares_url_change` accepts). A move that is not shared (a report labelled not
    /// user-started, or a redirect during a load the host started, `note_host_load`) changes only the live url; an
    /// unlabelled redirect report after a link click is shared.
    pub(crate) fn edit_url(&mut self, id: NodeId, url: &str) {
        self.edit_field(id, field::URL, FieldValue::Str(url.to_owned()));
    }

    /// The one writer of a note's text: what its box holds, committed at the blur (`end_note_edit`)
    /// and by every save (`commit_note_buffers`), never per keystroke, since a note can reach 1 MB. Text the mirror
    /// already shows queues nothing and copies nothing.
    pub(crate) fn edit_text(&mut self, id: NodeId, text: &str) {
        if self.doc.mirror.text(id) != text {
            self.edit_field(id, field::TEXT, FieldValue::Str(text.to_owned()));
        }
    }

    /// The one writer of a Json node's view: `view` is the stored text (`JsonView::to_field`). The
    /// mirror's decoded cache (`Mirror::view`) follows at once. Written by a level edit (`edit_json_view`) and by a
    /// reload that prunes overrides its new value can't resolve (`reload_fetched_node`).
    pub(crate) fn edit_view(&mut self, id: NodeId, view: &str) {
        self.edit_field(id, field::VIEW, FieldValue::Str(view.to_owned()));
    }

    /// The one writer of a group's name: the rename commit (`NodeAction::CommitRename`), with the
    /// name already cleaned. A new group's is seeded at placement instead (`place_group`). A jar's is `commit_rename`'s.
    pub(crate) fn edit_name(&mut self, id: NodeId, name: &str) {
        // name rule: compares stored names
        if self.doc.mirror.name(id) != name {
            self.edit_field(id, field::NAME, FieldValue::Str(name.to_owned()));
        }
    }

    /// What a note's card shows, borrowed: the box's edit buffer while it is typed in, the mirror's text
    /// otherwise. "" for a node that isn't a note.
    pub(crate) fn note_text(&self, id: NodeId) -> &str {
        match self.nodes.get(&id).and_then(|n| n.as_note()) {
            Some(note) if note.editing => &note.text,
            _ => self.doc.mirror.text(id),
        }
    }

    /// Whether node `id` is known by its stored name alone (a group, a jar): no derived label to fall back on, so
    /// the name is its own text.
    pub(crate) fn named_by_name(&self, id: NodeId) -> bool {
        matches!(self.nodes.get(&id).map(|n| &n.runtime), Some(Runtime::None | Runtime::Jar))
    }

    /// The stored text a card is known by, borrowed: a note's (`note_text`), a group's or a jar's raw name
    /// (`Mirror::name`), which the caller draws through a name rule or matches as stored. "" for other kinds.
    pub(crate) fn own_text(&self, id: NodeId) -> &str {
        if self.named_by_name(id) {
            // name rule: raw, for `accessible_name` (display_name) and the filter (matches the stored text)
            return self.doc.mirror.name(id);
        }
        self.note_text(id)
    }

    /// Whether `id` is a note being typed in: its box holds the text, and another replica's is held.
    pub(super) fn is_typed_in(&self, id: NodeId) -> bool {
        self.nodes.get(&id).and_then(|n| n.as_note()).is_some_and(|n| n.editing)
    }

    /// Commits every note's box that differs from the mirror (`edit_text`), so a save commits what is being typed.
    /// A note nobody is typing in has its box equal to the mirror, so it compares and copies nothing.
    pub(crate) fn commit_note_buffers(&mut self) {
        let typed: Vec<(NodeId, String)> = self
            .nodes
            .iter()
            .filter_map(|(id, n)| Some((id, n.as_note()?)))
            .filter(|(&id, note)| note.text != self.doc.mirror.text(id))
            .map(|(&id, note)| (id, note.text.clone()))
            .collect();
        for (id, text) in typed {
            self.edit_text(id, &text);
        }
    }

    /// Ends the edit of note `id` (its box lost focus, or stopped being laid out: `end_note_edits`). What the user
    /// typed since the last commit wins, as a pending local edit does, and another replica's text held meanwhile
    /// (`held_text`) is dropped; with nothing typed, the held text applies to the mirror and the box. The
    /// caller saves.
    pub(crate) fn end_note_edit(&mut self, id: NodeId) {
        let held = self.doc.writes.held_text.remove(&id);
        let Some(note) = self.nodes.get_mut(&id).and_then(|n| n.as_note_mut()) else { return };
        note.editing = false;
        let buffer = std::mem::take(&mut note.text);
        let pending = self.doc.writes.field_queued(id, field::TEXT);
        let text = match held {
            Some(theirs) if !pending && buffer == self.doc.mirror.text(id) => {
                self.doc.mirror.set(id, field::TEXT, FieldValue::Str(theirs.clone()));
                theirs
            }
            _ => {
                self.edit_text(id, &buffer);
                buffer
            }
        };
        if let Some(note) = self.nodes.get_mut(&id).and_then(|n| n.as_note_mut()) {
            note.text = text;
        }
    }

    /// A note's box lost focus: its edit ends (`end_note_edit`) and is saved.
    pub(crate) fn blur_note(&mut self, id: NodeId) {
        self.end_note_edit(id);
        self.save_session();
    }

    /// The one writer of a node's document pos: a drag release, with where the card was dropped
    /// (`release_drag`). A new node's is seeded at placement instead (`place_node`, `seed_landing`). Physics never
    /// calls it: a settle moves only `App::layout`. `None` (and a non-finite pos) writes the absent value, Pos (0,0).
    pub(crate) fn edit_pos(&mut self, id: NodeId, pos: Option<egui::Vec2>) {
        self.edit_field(id, field::POS, pos.map_or(FieldValue::Pos(0.0, 0.0), pos_value));
    }

    /// The one writer of a tab's engine: called when a switch starts (`start_swap_to`, a wake into
    /// another engine), and with the old engine when a switch fails and falls back. Compared as decoded, so asking for
    /// the engine the mirror already shows queues nothing and keeps a code this build doesn't know.
    pub(crate) fn edit_view_kind(&mut self, id: NodeId, kind: ViewKind) {
        if self.doc.mirror.view_kind(id) != kind {
            self.edit_field(id, field::VIEW_KIND, FieldValue::ViewCode(view_code(kind)));
        }
    }

    /// The one writer of a node's parent link: the mirror shows it at once and the next save commits a `Set PARENT`.
    pub(crate) fn edit_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        if self.doc.mirror.parent(id) == parent {
            return;
        }
        self.doc.writes.queue_field(id, field::PARENT, encode_field(field::PARENT, &FieldValue::Id(parent)));
        self.doc.mirror.set_parent(id, parent);
    }

    /// The one writer of a node's group: shown at once, committed as a `Move` by the next save.
    pub(crate) fn edit_group(&mut self, id: NodeId, group: Option<NodeId>) {
        if self.doc.mirror.group(id) == group {
            return;
        }
        // The Default stays at the top level (spec "The Default jar").
        if group.is_some() && self.is_default_jar(id) {
            return;
        }
        self.doc.writes.queue_move(id, group);
        self.doc.mirror.set_group(id, group);
    }

    /// Whether node `id` is shown at the level being viewed.
    pub(crate) fn in_level(&self, id: NodeId) -> bool {
        self.doc.mirror.group(id) == self.current_group
    }

    /// Every node whose mirror group is `group` (`linked_ids`).
    pub(crate) fn members_of(&self, group: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.linked_ids().filter(move |&id| self.doc.mirror.group(id) == Some(group))
    }

    /// The ids a cascade (a dissolve, merge or removal; a jar re-snapshot, default or delete) must reach: every live
    /// node, and every tab mid-respawn. A respawn takes the tab out of `nodes` until it lands but keeps its mirror
    /// entry, which the landing keeps (`Mirror::seed`), so a cascade that skipped it would leave it naming a removed
    /// node or an old jar. A new spawn has no mirror entry yet and is left out: it lands from its `TabSpawn`
    /// (`seed_landing`), and a cascade writing to it would make a partial entry that stops that seed.
    pub(crate) fn linked_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        let respawning =
            self.spawns.spawning.keys().filter(|id| !self.nodes.contains_key(*id) && self.doc.mirror.has(**id));
        self.nodes.keys().chain(respawning).copied()
    }

    /// Whether `id` is one of `linked_ids`.
    pub(crate) fn is_linked(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id) || (self.spawns.spawning.contains_key(&id) && self.doc.mirror.has(id))
    }

    /// "Remove from Group" (the tab card's eject button, dragging a card out through the way back, and the tab and
    /// item menus): moves `id` up one level, into the group of the group it's in, since groups nest. `None` (the
    /// top level) when that group isn't itself in one.
    pub(crate) fn remove_from_group(&mut self, id: NodeId) {
        if !self.nodes.contains_key(&id) {
            return;
        }
        let up = self.doc.mirror.group(id).and_then(|g| self.doc.mirror.group(g));
        self.edit_group(id, up);
    }

    /// A remote removal's cascade: the document already shows `id`'s new parent (a node only this replica has
    /// carries it in its own `Create`), so only the mirror changes; queuing it would revert a concurrent remote
    /// re-parent. A local edit still pending for the link is rewritten instead, so it keeps winning.
    pub(crate) fn follow_remote_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        if self.doc.writes.field_queued(id, field::PARENT) {
            self.edit_parent(id, parent);
        } else {
            self.doc.mirror.set_parent(id, parent);
        }
    }

    /// `follow_remote_parent` for the group: a remote group delete's members move up in the mirror only, so a
    /// concurrent remote merge or move of them stands.
    pub(crate) fn follow_remote_group(&mut self, id: NodeId, group: Option<NodeId>) {
        if self.doc.writes.move_queued(id) {
            self.edit_group(id, group);
        } else {
            self.doc.mirror.set_group(id, group);
        }
    }

    /// Every node whose mirror parent is `id` (`linked_ids`).
    pub(crate) fn children_of(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.linked_ids().filter(move |&child| self.opened_from(child) == Some(id))
    }
}
