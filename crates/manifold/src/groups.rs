//! Group methods of `App`: naming, insertion, dissolving and destroying.

use super::*;

impl App {
    /// A fresh group's default display name: "Group N" for the smallest N
    /// no group is named yet. Not the group count plus one, which repeats a
    /// name after a dissolve (three groups "Group 1/2/3", dissolve "Group
    /// 2", and the next new one is "Group 3" again), and not the id, which
    /// is a long store id. Manually *renaming* two groups to the identical
    /// string is left alone -- that's a deliberate user choice.
    pub(crate) fn default_group_name(&self) -> String {
        let taken: HashSet<&str> = self
            .nodes
            .iter()
            .filter(|(id, _)| self.is_container(id))
            .map(|(id, _)| self.mirror.name(id)) // name rule: compares stored names
            .collect();
        (1..).map(|n| format!("Group {n}")).find(|name| !taken.contains(name.as_str())).expect("a free number")
    }

    /// How many groups `id` is nested in, by the mirror's `group` chain; capped at the chain's length so a cycle
    /// (two replicas' moves raced) ends.
    pub(crate) fn group_depth(&self, id: &str) -> usize {
        let (mut depth, mut cur) = (0, id);
        while let Some(group) = self.mirror.group(cur) {
            depth += 1;
            if depth > self.nodes.len() {
                break;
            }
            cur = group;
        }
        depth
    }

    /// Dissolves a group: members rejoin *its own parent's* level again
    /// (never closed — grouping is purely a display grouping, not an
    /// ownership one), and the group card itself is removed. A top-level
    /// group's parent is `None`, so this still means "become top-level"
    /// exactly as before nesting existed -- reads the group's own parent
    /// before removing it, rather than assuming `None`. `current_group`
    /// moves to that same parent if it happened to be the group just
    /// dissolved. Deletes the group: the user-facing Dissolve Group and Ungroup All close it instead (`ungroup`), so
    /// they can be undone. Not the card's close: that is `close_node`.
    ///
    /// A local dissolve: a member whose `jar` edge now reaches nothing it did before loses it (`recheck_edge_scope`).
    pub(crate) fn dissolve_group(&mut self, id: &str) {
        for member in self.dissolve_members(id, false) {
            self.recheck_edge_scope(&member);
        }
    }

    /// The menu's "Dissolve Group" and the toolbar's Ungroup All: each group's members move up to its own level and
    /// the empty group is closed (a Recently Closed entry) holding their ids (`fold_group`), so reopening it, from
    /// that list or the toast's Undo (`step_history`), puts them back (`regroup_held`).
    pub(crate) fn ungroup(&mut self, ids: &[String]) {
        let message = match ids {
            [id] => format!("Dissolved {}", self.drop_target_name(id)),
            _ => format!("Ungrouped {} groups", ids.len()),
        };
        let mut folded = Vec::new();
        for id in ids {
            let parent = self.mirror.group(id).map(str::to_owned);
            self.fold_group(id, parent.clone());
            folded.push((id.clone(), parent));
        }
        self.record_undo(message, UndoKind::Fold(folded));
    }

    /// Moves group `id`'s members into `dest` (`None`: the top level) and closes the emptied group, recording where
    /// they went and who they were in `field::HELD` ("<dest> <member>...", `-` for the top level) so `regroup_held`
    /// can put them back. Shared by `ungroup` (dest: the group's own level) and `merge_groups` (dest: the target).
    pub(crate) fn fold_group(&mut self, id: &str, dest: Option<String>) {
        let members: Vec<String> = self.members_of(id).map(str::to_owned).collect();
        for member in &members {
            self.edit_group(member, dest.clone());
        }
        let held = std::iter::once(dest.as_deref().unwrap_or("-")).chain(members.iter().map(String::as_str));
        self.edit_field(id, field::HELD, FieldValue::Str(held.collect::<Vec<_>>().join(" ")));
        self.close_node(id);
        for member in &members {
            self.recheck_edge_scope(member);
        }
    }

    /// A reopened group puts back the members `fold_group` took out of it that are still where it left them (one
    /// moved elsewhere meanwhile stays), then forgets them. Nothing if the group is not shown (its own group is
    /// closed).
    pub(crate) fn regroup_held(&mut self, id: &str) {
        let FieldValue::Str(held) = self.mirror.get(id, field::HELD) else { return };
        let mut words = held.split(' ');
        let Some(dest) = words.next() else { return };
        let dest = (dest != "-").then_some(dest);
        for member in words {
            if self.nodes.contains_key(member) && self.mirror.group(member) == dest {
                self.edit_group(member, Some(id.to_owned()));
            }
        }
        self.edit_field(id, field::HELD, FieldValue::Str(String::new()));
        self.save_session();
    }

    /// `dissolve_group` without the edge recheck, for a remote delete: the remote device's own recheck arrives as
    /// ordinary tab edits (spec "Inheritance"). The members move up in the mirror only (`follow_remote_group`).
    pub(crate) fn dissolve_group_quietly(&mut self, id: &str) {
        self.dissolve_members(id, true);
    }

    /// Moves group `id`'s members to its own group and removes it; `remote` for another replica's delete, which
    /// queues no delete of its own (the document already lacks the group). Returns the members moved.
    fn dissolve_members(&mut self, id: &str, remote: bool) -> Vec<String> {
        let parent = self.mirror.group(id).filter(|_| self.nodes.contains_key(id)).map(str::to_owned);
        let members: Vec<String> = self.members_of(id).map(str::to_owned).collect();
        for member in &members {
            if remote {
                self.follow_remote_group(member, parent.clone());
            } else {
                self.edit_group(member, parent.clone());
            }
        }
        self.nodes.remove(id);
        if !remote {
            self.queue_delete(id);
        }
        self.node_order.retain(|g| g != id);
        if self.current_group.as_deref() == Some(id) {
            self.switch_group_level(parent);
        }
        self.forget_id(id);
        members
    }

    /// Shared by `new_group_shortcut` (`pos` `None`: a jittered spawn) and
    /// the empty-canvas "New Tab/Note/Group/File" card (`pending_new_node`),
    /// which passes the exact position that was right-clicked instead.
    /// Doesn't save; the caller does.
    pub(crate) fn insert_group_at(&mut self, id: String, pos: Option<egui::Vec2>) {
        let name = self.default_group_name();
        let at = self.placement(&id, pos);
        self.place_group(Some(id), &name, at);
    }

    /// Destroys a group and everything in it, at any depth, then the group itself, and saves once. Records nothing.
    pub(crate) fn destroy_group(&mut self, id: &str) {
        self.begin_tabs_shutdown(&self.tabs_under(id));
        self.destroy_group_members(id, &mut HashSet::new());
        // The one save for the whole subtree: no member's removal saves (`destroy_group_members`), so the group and
        // everything in it go in one commit, and another replica never reads it half deleted.
        self.save_session();
    }

    /// Every tab member of group `id` and of the groups inside it, each group once (as `destroy_group_members` walks).
    pub(crate) fn tabs_under(&self, id: &str) -> Vec<String> {
        let mut seen = HashSet::from([id.to_owned()]);
        let mut groups = vec![id.to_owned()];
        let mut tabs = Vec::new();
        while let Some(group) = groups.pop() {
            for member in self.members_of(&group) {
                if !self.is_container(member) {
                    tabs.push(member.to_owned());
                } else if seen.insert(member.to_owned()) {
                    groups.push(member.to_owned());
                }
            }
        }
        tabs
    }

    /// `destroy_group`'s walk, with no save: a tab member is `remove_tab`d (not `destroy_tab`, which saves), every
    /// other member `destroy_node`d, each group once, so a group cycle in the mirror (two replicas' moves raced; the
    /// next save shows the document's groups) can't recurse forever. A group already being destroyed further up is
    /// left to it.
    fn destroy_group_members(&mut self, id: &str, visited: &mut HashSet<String>) {
        visited.insert(id.to_owned());
        let member_ids: Vec<String> = self.members_of(id).map(str::to_owned).collect();
        for tid in member_ids {
            if self.is_container(&tid) {
                if !visited.contains(&tid) {
                    self.destroy_group_members(&tid, visited);
                }
            } else if self.card_kind(&tid).is_some_and(|k| k.ty == store::model::TYPE_TAB) {
                // No save per tab: `destroy_group` commits the whole subtree once.
                self.remove_tab(&tid);
            } else {
                self.destroy_node(&tid);
            }
        }
        self.dissolve_group(id);
    }
}
