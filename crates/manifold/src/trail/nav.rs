//! Back and Forward for a tab (spec docs/superpowers/specs/2026-10-14-trail-design.md, decisions 6 and 7): the tab's
//! own trail (`nav_history`) first. At the root of the trail, Back goes to the card the tab was opened from
//! (reopening it if closed), and that card, when a tab, records `left_to` so its Forward returns.

use std::time::Instant;

use crate::store::model::{Kind, NodeId};
use crate::trail::history::{Dir, NavAction};
use crate::*;

/// `id`'s raw `PARENT` in the document.
fn doc_parent(doc: &crate::store::model::Doc, id: NodeId) -> Option<NodeId> {
    let node = doc.nodes.get(&id)?;
    node.get(field::PARENT).as_id()
}

impl App {
    /// The mouse's Back and Forward buttons (4 and 5) step the focused tab like Alt+Left and Alt+Right.
    pub(crate) fn side_button_nav(&mut self, button: WinitMouseButton) {
        let dir = match button {
            WinitMouseButton::Back => Dir::Back,
            WinitMouseButton::Forward => Dir::Forward,
            _ => return,
        };
        if let Some(id) = self.focused {
            self.navigate_history(id, dir);
        }
    }

    /// A Back or Forward press on tab `id` (the toolbar chips, `GOBACK`/`GOFORWARD`). Ignored while a move is pending.
    pub(crate) fn navigate_history(&mut self, id: NodeId, dir: Dir) {
        let now = Instant::now();
        let history = self.scoped.nav_history.get(&id);
        if history.is_some_and(|h| h.move_pending(now)) {
            return;
        }
        let own = history.is_some_and(|h| match dir {
            Dir::Back => h.can_go_back(),
            Dir::Forward => h.can_go_forward(),
        });
        if own {
            let Some(node) = self.nodes.get(&id).and_then(|n| n.as_tab()) else { return };
            let (back, forward) = (node.can_go_back, node.can_go_forward);
            let action =
                self.scoped.nav_history.get_mut(&id).map_or(NavAction::None, |h| h.go(dir, back, forward, now));
            self.send_nav_action(id, action);
            return;
        }
        match dir {
            Dir::Back => self.trail_back_to_parent(id),
            Dir::Forward => self.trail_forward_to_child(id),
        }
    }

    fn send_nav_action(&mut self, id: NodeId, action: NavAction) {
        let message = match action {
            NavAction::GoBack => HostToView::GoBack,
            NavAction::GoForward => HostToView::GoForward,
            // A plain move loads like a typed url and is shared, under the trail's own move.
            NavAction::Navigate(url) => return self.load_url(id, url),
            NavAction::None => return,
        };
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.tab.send(&message);
        }
    }

    /// The chips' enablement for tab `id`: the trail's own Back, else an opened-from parent; the trail's own Forward,
    /// else a valid `left_to`.
    pub(crate) fn trail_enabled(&self, id: NodeId) -> (bool, bool) {
        let history = self.scoped.nav_history.get(&id);
        let back = history.is_some_and(|h| h.can_go_back()) || self.trail_parent(id).is_some();
        let forward = history.is_some_and(|h| h.can_go_forward()) || self.left_to_child(id).is_some();
        (back, forward)
    }

    /// The card `id` was opened from (decision 6): its `PARENT` from the mirror (which sees an edit or a Create not yet
    /// in the document), else from the document (the mirror has no links for a hidden node); while that one is not
    /// live, its own `PARENT` from the document. `None` when no live ancestor is left (a
    /// remote delete leaves a child's `PARENT` naming the deleted node).
    pub(crate) fn trail_parent(&self, id: NodeId) -> Option<NodeId> {
        let doc = self.doc.journal.as_ref().map(|j| j.doc());
        let mut p = match self.doc.mirror.parent(id) {
            Some(p) => p,
            None => doc_parent(doc?, id)?,
        };
        for _ in 0..=doc.map_or(0, |d| d.nodes.len()) {
            if self.is_live_id(p) {
                return Some(p);
            }
            p = doc_parent(doc?, p)?;
        }
        None
    }

    /// A row of the branch list: a plain move to `entry` of tab `tab`'s trail.
    pub(crate) fn trail_pick(&mut self, tab: NodeId, entry: usize) {
        let Some(h) = self.scoped.nav_history.get_mut(&tab) else { return };
        let action = h.go_to(entry, Instant::now());
        self.send_nav_action(tab, action);
    }

    /// "Open as card" on a branch row (decision 9): a new tab opened from `tab` at that entry's URL, with opened-from
    /// and the group set as for any opened tab, at `pos` (world) when given. The trail is unchanged.
    pub(crate) fn open_branch_as_card(&mut self, tab: NodeId, entry: usize, pos: Option<egui::Vec2>) {
        let Some(url) = self.scoped.nav_history.get(&tab).and_then(|h| h.entry_url(entry)).map(str::to_owned) else {
            return;
        };
        self.spawn_tab_from(tab, &url, pos);
    }

    /// `id` is shown (`nodes`), or live in the document (hidden by a close).
    fn is_live_id(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id) || self.doc_node(id).is_some_and(|n| n.is_live())
    }

    fn doc_node(&self, id: NodeId) -> Option<&crate::store::model::Node> {
        self.doc.journal.as_ref()?.doc().nodes.get(&id)
    }

    fn trail_is_tab(&self, id: NodeId) -> bool {
        self.doc.mirror.ty(id) == crate::store::model::TYPE_TAB
            || self.doc_node(id).is_some_and(|n| n.kind() == Kind::Tab)
    }

    /// Back at the root of `id`'s trail: focus its parent, which, when a tab, remembers `id` for its Forward.
    fn trail_back_to_parent(&mut self, id: NodeId) {
        let Some(parent) = self.trail_parent(id) else { return };
        if self.trail_is_tab(parent) {
            self.scoped.nav_history.entry(parent).or_default().set_left_to(Some(id));
        }
        self.trail_focus(parent);
    }

    /// `parent`'s `left_to` while it is valid (decision 7): the child is live and its opened-from walk ends at
    /// `parent` (a grandparent Back resolved to after a delete keeps it).
    fn left_to_child(&self, parent: NodeId) -> Option<NodeId> {
        let child = self.scoped.nav_history.get(&parent)?.left_to()?;
        (self.is_live_id(child) && self.trail_parent(child) == Some(parent)).then_some(child)
    }

    /// Forward with no Forward in the trail: back to the child Back left, or, if that is stale, forget it.
    fn trail_forward_to_child(&mut self, id: NodeId) {
        match self.left_to_child(id) {
            Some(child) => self.trail_focus(child),
            None => {
                if let Some(h) = self.scoped.nav_history.get_mut(&id) {
                    h.set_left_to(None);
                }
            }
        }
    }

    /// Shows `id` for the opened-from step: reopens the closed nodes that hide it, outermost first (a nested closed
    /// node stays closed when its group reopens, so this repeats), then focuses it. A reopen that has not put `id`
    /// in `nodes` yet (a tab still spawning) leaves it to `poll_trail_focus`.
    fn trail_focus(&mut self, id: NodeId) {
        let mut last = None;
        for _ in 0..=self.doc.journal.as_ref().map_or(0, |j| j.doc().nodes.len()) {
            let Some(root) = self.outermost_closed(id) else { break };
            if last == Some(root) {
                // The reopen did not take (a failed or read-only save leaves the flag).
                self.queue_toast("Couldn't reopen that card".to_string());
                return;
            }
            last = Some(root);
            self.reopen_node(root);
        }
        if self.nodes.contains_key(&id) {
            self.finish_trail_focus(id);
        } else {
            self.spawns.pending_trail_focus = Some(id);
        }
    }

    /// The outermost closed node on `id`'s `group` chain, `id` included, from the document.
    fn outermost_closed(&self, id: NodeId) -> Option<NodeId> {
        let doc = self.doc.journal.as_ref()?.doc();
        let mut cur = Some(id);
        let mut outer = None;
        for _ in 0..=doc.nodes.len() {
            let Some(c) = cur else { break };
            let Some(node) = doc.nodes.get(&c) else { break };
            if node.is_live() && node.closed() {
                outer = Some(c);
            }
            cur = node.group;
        }
        outer
    }

    /// `id` is in `nodes`: a tab or image opens in the Item view on its level; any other card is selected on its
    /// level in the Graph view.
    fn finish_trail_focus(&mut self, id: NodeId) {
        let ty = self.card_kind(id).map(|k| k.ty);
        if matches!(ty, Some(crate::store::model::TYPE_TAB | crate::store::model::TYPE_IMAGE)) {
            self.show_level_of(id);
            self.focus_item(id);
            return;
        }
        if self.view_mode == ViewMode::Item {
            self.leave_to_graph();
        }
        self.show_level_of(id);
        self.camera.reveal = Some(id);
        self.interaction.selected.clear();
        self.interaction.selected.insert(CardId::from(id));
    }

    /// Finishes a `trail_focus` once its node is in `nodes`; drops one whose node is gone. Called each tick after the
    /// store is polled.
    pub(crate) fn poll_trail_focus(&mut self) {
        let Some(id) = self.spawns.pending_trail_focus else { return };
        if self.nodes.contains_key(&id) {
            self.spawns.pending_trail_focus = None;
            self.finish_trail_focus(id);
        } else if !self.is_live_id(id) {
            self.spawns.pending_trail_focus = None;
        }
    }
}

#[cfg(test)]
mod tests;
