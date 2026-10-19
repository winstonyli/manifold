//! Group methods of `App`: naming, insertion, dissolving and destroying.

use super::*;

/// What `App::undo_dissolve` needs to bring a dissolved group back.
pub(crate) struct Dissolved {
    name: String,
    color: FieldValue,
    jar: FieldValue,
    pos: egui::Vec2,
    group: Option<String>,
    members: Vec<String>,
}

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
    /// dissolved (should only actually matter defensively — the "Dissolve
    /// Group" row and Ungroup All only reach groups one level up from the
    /// level currently being viewed). Not the card's close: that is
    /// `close_node`.
    ///
    /// A local dissolve: a member whose `jar` edge now reaches nothing it did before loses it (`recheck_edge_scope`).
    pub(crate) fn dissolve_group(&mut self, id: &str) {
        for member in self.dissolve_members(id, false) {
            self.recheck_edge_scope(&member);
        }
    }

    /// The menu's "Dissolve Group": `dissolve_group`, then a toast with an Undo button (`undo_dissolve`) while it shows.
    pub(crate) fn dissolve_group_undoable(&mut self, id: &str) {
        let message = format!("Dissolved {}", self.drop_target_name(id));
        let undo = Dissolved {
            name: self.mirror.name(id).to_owned(), // name rule: copied back verbatim to the restored group
            color: self.mirror.get(id, field::COLOR),
            jar: self.mirror.get(id, field::JAR),
            pos: self.layout_pos(id),
            group: self.mirror.group(id).map(str::to_owned),
            members: self.members_of(id).map(str::to_owned).collect(),
        };
        self.dissolve_group(id);
        self.toasts.undo = Some((message.clone(), undo));
        self.queue_toast(message);
    }

    /// Brings a dissolved group back as a new group with its name, color, jar and place, around the members that are
    /// still where the dissolve left them. Edges the dissolve cleared as out of reach (`recheck_edge_scope`) stay
    /// cleared.
    pub(crate) fn undo_dissolve(&mut self, undo: Dissolved) {
        let id = self.alloc_node_id();
        let at = self.placement(&id, Some(undo.pos));
        let at = Placement { group: undo.group.clone(), ..at };
        // name rule: the restored group seeds with the stored name it had, drawn later by display_name
        self.place_group(Some(id.clone()), &undo.name, at);
        self.edit_field(&id, field::COLOR, undo.color);
        self.edit_field(&id, field::JAR, undo.jar);
        for member in undo.members {
            if self.nodes.contains_key(&member) && self.mirror.group(&member) == undo.group.as_deref() {
                self.edit_group(&member, Some(id.clone()));
            }
        }
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
