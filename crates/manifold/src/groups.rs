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
            .map(|(id, _)| self.doc.mirror.name(id)) // name rule: compares stored names
            .collect();
        (1..).map(|n| format!("Group {n}")).find(|name| !taken.contains(name.as_str())).expect("a free number")
    }

    /// How many groups `id` is nested in, by the mirror's `group` chain; capped at the chain's length so a cycle
    /// (two replicas' moves raced) ends.
    pub(crate) fn group_depth(&self, id: &str) -> usize {
        crate::walk::ancestors(id.to_owned(), |k| self.doc.mirror.group(k).map(str::to_owned)).count()
    }

    /// The menu's "Dissolve Group" and the toolbar's Ungroup All: each group's members move up to its own level and
    /// the empty group is closed (a Recently Closed entry) holding their ids (`fold_group`), so reopening it, from
    /// that list or the toast's Undo (`step_history`), puts them back (`regroup_held`).
    pub(crate) fn ungroup(&mut self, ids: &[String]) {
        let message = match ids {
            [id] => format!("Dissolved {}", self.drop_target_name(id)),
            _ => format!("Ungrouped {} groups", ids.len()),
        };
        let folds = ids.iter().map(|id| (id.clone(), self.doc.mirror.group(id).map(str::to_owned))).collect();
        self.fold_groups(folds, message);
    }

    /// Folds each (group, destination) with `fold_group` and records them as one undo step. The one way a user-facing
    /// fold happens: `ungroup` and `merge_groups` both come here.
    pub(crate) fn fold_groups(&mut self, folds: Vec<(String, Option<String>)>, message: String) {
        for (id, dest) in &folds {
            self.fold_group(id, dest.clone());
        }
        self.record_undo(message, UndoKind::Fold(folds));
    }

    /// Moves group `id`'s members into `dest` (`None`: the top level) and closes the emptied group, recording where
    /// they went and who they were in `field::HELD` ("<dest> <member>...", `-` for the top level) so `regroup_held`
    /// can put them back. Records no undo step itself (redo of a fold calls it too): go through `fold_groups`.
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
        let FieldValue::Str(held) = self.doc.mirror.get(id, field::HELD) else { return };
        let mut words = held.split(' ');
        let Some(dest) = words.next() else { return };
        let dest = (dest != "-").then_some(dest);
        for member in words {
            if self.card_in_group(member, dest) {
                self.edit_group(member, Some(id.to_owned()));
            }
        }
        self.edit_field(id, field::HELD, FieldValue::Str(String::new()));
        self.save_session();
    }

    /// Whether card `id` is shown and in group `group` (`None`: the top level): the check before putting it back
    /// anywhere (`regroup_held`, `replay_revert`), so a card moved on or closed meanwhile stays.
    pub(crate) fn card_in_group(&self, id: &str, group: Option<&str>) -> bool {
        self.nodes.contains_key(id) && self.doc.mirror.group(id) == group
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
        let below = self.members_under(id);
        let tabs = below.iter().filter(|m| !self.is_container(m)).cloned().collect::<Vec<_>>();
        self.begin_tabs_shutdown(&tabs);
        self.destroy_group_members(id, below);
        // The one save for the whole subtree: no member's removal saves (`destroy_group_members`), so the group and
        // everything in it go in one commit, and another replica never reads it half deleted.
        self.save_session();
    }

    /// Every member of group `id` and of the groups inside it, groups included, each once.
    pub(crate) fn members_under(&self, id: &str) -> Vec<String> {
        crate::walk::descendants(
            id.to_owned(),
            |g| self.members_of(g).map(str::to_owned).collect(),
            |m| self.is_container(m),
        )
    }

    /// `destroy_group`'s removal, with no save: `below` is everything under group `id` (`members_under`, each once, so
    /// a group cycle in the mirror can't loop it). A tab is `remove_tab`d (not `destroy_tab`, which saves), every
    /// other member `destroy_node`d, then the groups removed, the innermost first (a group's members move up, `take_out`).
    fn destroy_group_members(&mut self, id: &str, below: Vec<String>) {
        let (groups, members): (Vec<String>, Vec<String>) = below.into_iter().partition(|m| self.is_container(m));
        for tid in members {
            if self.card_kind(&tid).is_some_and(|k| k.ty == store::model::TYPE_TAB) {
                // No save per tab: `destroy_group` commits the whole subtree once.
                self.remove_tab(&tid);
            } else {
                self.destroy_node(&tid);
            }
        }
        for group in groups.iter().rev() {
            self.remove_node(group);
        }
        self.remove_node(id);
    }
}
