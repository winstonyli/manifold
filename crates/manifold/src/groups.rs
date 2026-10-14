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

    /// Dissolves a group: members rejoin *its own parent's* level again
    /// (never closed — grouping is purely a display grouping, not an
    /// ownership one), and the group card itself is removed. A top-level
    /// group's parent is `None`, so this still means "become top-level"
    /// exactly as before nesting existed -- reads the group's own parent
    /// before removing it, rather than assuming `None`. `current_group`
    /// moves to that same parent if it happened to be the group just
    /// dissolved (should only actually matter defensively — the close
    /// button that calls this only ever renders one level up from the
    /// group being closed, never on the level currently being viewed).
    ///
    /// A local dissolve: unset tabs among the members re-snapshot their jar from the new level (deviation 12).
    pub(crate) fn dissolve_group(&mut self, id: &str) {
        for member in self.dissolve_members(id, false) {
            self.refollow(&member);
        }
    }

    /// `dissolve_group` without the jar re-snapshot, for a remote delete: the remote device's own refollow arrives as
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

    /// Destroys a group and everything in it, at any depth, then the group itself, and saves once. Records nothing:
    /// `close_node` has already recorded the whole subtree as one entry.
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
            } else if self.is_default_jar(&tid) {
                // The Default is never closed: it moves to the top level (a plain dissolve could refuse, as moves
                // into a group are refused for it, `edit_group`).
                self.edit_group(&tid, None);
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
