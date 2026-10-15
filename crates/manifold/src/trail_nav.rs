//! Back and Forward for a tab (spec docs/superpowers/specs/2026-10-14-trail-design.md, decisions 6 and 7): the tab's
//! own trail (`nav_history`) first. At the root of the trail, Back goes to the card the tab was opened from
//! (reopening it if closed), and that card, when a tab, records `left_to` so its Forward returns.

use std::time::Instant;

use super::*;
use crate::nav_history::{Dir, NavAction};
use crate::store::model::{Kind, NodeId};

/// `id`'s raw `PARENT` in the document.
fn doc_parent(doc: &crate::store::model::Doc, id: &str) -> Option<String> {
    let node = doc.nodes.get(&id.parse::<NodeId>().ok()?)?;
    if let FieldValue::Id(Some(p)) = node.get(field::PARENT) {
        Some(p.to_string())
    } else {
        None
    }
}

impl App {
    /// A Back or Forward press on tab `id` (the toolbar chips, `GOBACK`/`GOFORWARD`). Ignored while a move is pending.
    pub(crate) fn navigate_history(&mut self, id: &str, dir: Dir) {
        let now = Instant::now();
        let history = self.nav_history.get(id);
        if history.is_some_and(|h| h.move_pending(now)) {
            return;
        }
        let own = history.is_some_and(|h| match dir {
            Dir::Back => h.can_go_back(),
            Dir::Forward => h.can_go_forward(),
        });
        if own {
            let Some(node) = self.nodes.get(id).and_then(|n| n.as_tab()) else { return };
            let (back, forward) = (node.can_go_back, node.can_go_forward);
            let action = self.nav_history.get_mut(id).map_or(NavAction::None, |h| h.go(dir, back, forward, now));
            self.send_nav_action(id, action);
            return;
        }
        match dir {
            Dir::Back => self.trail_back_to_parent(id),
            Dir::Forward => self.trail_forward_to_child(id),
        }
    }

    fn send_nav_action(&mut self, id: &str, action: NavAction) {
        let message = match action {
            NavAction::GoBack => HostToView::GoBack,
            NavAction::GoForward => HostToView::GoForward,
            // A plain move is a navigation like a typed one: `navigate_tab` notes it and shares the url.
            NavAction::Navigate(url) => return self.navigate_tab(id, url),
            NavAction::None => return,
        };
        if let Some(node) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) {
            node.tab.send(&message);
        }
    }

    /// The chips' enablement for tab `id`: the trail's own Back, else an opened-from parent; the trail's own Forward,
    /// else a valid `left_to`.
    pub(crate) fn trail_enabled(&self, id: &str) -> (bool, bool) {
        let history = self.nav_history.get(id);
        let back = history.is_some_and(|h| h.can_go_back()) || self.trail_parent(id).is_some();
        let forward = history.is_some_and(|h| h.can_go_forward()) || self.left_to_child(id).is_some();
        (back, forward)
    }

    /// The card `id` was opened from (decision 6): its `PARENT` from the mirror (which sees an edit or a Create not yet
    /// in the document), else from the document (the mirror has no links for a hidden node); while that one is not
    /// live, its own `PARENT` from the document. `None` when no live ancestor is left (plan D16 for a remote delete).
    pub(crate) fn trail_parent(&self, id: &str) -> Option<String> {
        let doc = self.journal.as_ref().map(|j| j.doc());
        let mut p = match self.mirror.parent(id) {
            Some(p) => p.to_owned(),
            None => doc_parent(doc?, id)?,
        };
        for _ in 0..=doc.map_or(0, |d| d.nodes.len()) {
            if self.is_live_id(&p) {
                return Some(p);
            }
            p = doc_parent(doc?, &p)?;
        }
        None
    }

    /// A row of the branch list: a plain move to `entry` of tab `tab`'s trail.
    pub(crate) fn trail_pick(&mut self, tab: &str, entry: usize) {
        let Some(h) = self.nav_history.get_mut(tab) else { return };
        let action = h.go_to(entry, Instant::now());
        self.send_nav_action(tab, action);
    }

    /// "Open as card" on a branch row (decision 9): a new tab opened from `tab` at that entry's URL, with opened-from
    /// and the group set as for any opened tab. The trail is unchanged.
    pub(crate) fn open_branch_as_card(&mut self, tab: &str, entry: usize) {
        let Some(url) = self.nav_history.get(tab).and_then(|h| h.entry_url(entry)).map(str::to_owned) else { return };
        self.spawn_tab_from(tab, &url);
    }

    /// `id` is shown (`nodes`), or live in the document (hidden by a close).
    fn is_live_id(&self, id: &str) -> bool {
        self.nodes.contains_key(id) || self.doc_node(id).is_some_and(|n| n.is_live())
    }

    fn doc_node(&self, id: &str) -> Option<&crate::store::model::Node> {
        let id: NodeId = id.parse().ok()?;
        self.journal.as_ref()?.doc().nodes.get(&id)
    }

    fn trail_is_tab(&self, id: &str) -> bool {
        self.mirror.ty(id) == crate::store::model::TYPE_TAB || self.doc_node(id).is_some_and(|n| n.kind() == Kind::Tab)
    }

    /// Back at the root of `id`'s trail: focus its parent, which, when a tab, remembers `id` for its Forward.
    fn trail_back_to_parent(&mut self, id: &str) {
        let Some(parent) = self.trail_parent(id) else { return };
        if self.trail_is_tab(&parent) {
            self.nav_history.entry(parent.clone()).or_default().set_left_to(Some(id.to_owned()));
        }
        self.trail_focus(&parent);
    }

    /// `parent`'s `left_to` while it is valid (decision 7): the child is live and its opened-from walk ends at
    /// `parent` (a grandparent Back resolved to after a delete keeps it).
    fn left_to_child(&self, parent: &str) -> Option<String> {
        let child = self.nav_history.get(parent)?.left_to()?;
        (self.is_live_id(child) && self.trail_parent(child).as_deref() == Some(parent)).then(|| child.to_owned())
    }

    /// Forward with no Forward in the trail: back to the child Back left, or, if that is stale, forget it.
    fn trail_forward_to_child(&mut self, id: &str) {
        match self.left_to_child(id) {
            Some(child) => self.trail_focus(&child),
            None => {
                if let Some(h) = self.nav_history.get_mut(id) {
                    h.set_left_to(None);
                }
            }
        }
    }

    /// Shows `id` for the opened-from step: reopens the closed nodes that hide it, outermost first (a nested closed
    /// node stays closed when its group reopens, so this repeats), then focuses it. A reopen that has not put `id`
    /// in `nodes` yet (a tab still spawning) leaves it to `poll_trail_focus`.
    fn trail_focus(&mut self, id: &str) {
        let mut last = None;
        for _ in 0..=self.journal.as_ref().map_or(0, |j| j.doc().nodes.len()) {
            let Some(root) = self.outermost_closed(id) else { break };
            if last == Some(root) {
                // The reopen did not take (a failed or read-only save leaves the flag).
                self.queue_toast("Couldn't reopen that card".to_string());
                return;
            }
            last = Some(root);
            self.reopen_node(root);
        }
        if self.nodes.contains_key(id) {
            self.finish_trail_focus(id);
        } else {
            self.spawns.pending_trail_focus = Some(id.to_owned());
        }
    }

    /// The outermost closed node on `id`'s `group` chain, `id` included, from the document.
    fn outermost_closed(&self, id: &str) -> Option<NodeId> {
        let doc = self.journal.as_ref()?.doc();
        let mut cur = Some(id.parse::<NodeId>().ok()?);
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
    /// level in the Graph view (plan D14).
    fn finish_trail_focus(&mut self, id: &str) {
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
        self.interaction.selected.clear();
        self.interaction.selected.insert(id.to_owned());
    }

    /// Finishes a `trail_focus` once its node is in `nodes`; drops one whose node is gone. Called each tick after the
    /// store is polled.
    pub(crate) fn poll_trail_focus(&mut self) {
        let Some(id) = self.spawns.pending_trail_focus.clone() else { return };
        if self.nodes.contains_key(&id) {
            self.spawns.pending_trail_focus = None;
            self.finish_trail_focus(&id);
        } else if !self.is_live_id(&id) {
            self.spawns.pending_trail_focus = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::nav_history::NavHistory;
    use crate::session_sync::tests::{add, add_group, app_on, live_tab, sleeping_tab, Peer};
    use crate::store::journal::Journal;
    use crate::store::model::ActorId;
    use crate::store::ops::OpBody;
    use crate::store::testutil::temp_dir;

    /// Tab `child` (live, focused, a one-page trail) opened from tab `parent` (asleep), both at the top level.
    fn opened_from(path: &std::path::Path) -> (App, String, String, Peer) {
        let mut app = app_on(path, 1);
        let parent = add(&mut app, Kind::Tab, sleeping_tab("https://p/", "P"));
        let (child, peer) = live_tab(&mut app, ViewKind::Webview2, "https://c/");
        app.edit_parent(&child, Some(parent.clone()));
        assert!(app.save_session());
        app.nav_history.insert(child.clone(), NavHistory::visited(&["https://c/"]));
        app.focus_tab(&child);
        (app, parent, child, peer)
    }

    fn session(name: &str) -> std::path::PathBuf {
        temp_dir(name).join("session.mnf")
    }

    #[test]
    fn back_at_the_root_focuses_the_parent_and_forward_returns() {
        let (mut app, parent, child, _peer) = opened_from(&session("trail-back-forward"));
        assert_eq!(app.trail_enabled(&child), (true, false));
        app.navigate_history(&child, Dir::Back);
        assert_eq!(app.focused.as_deref(), Some(parent.as_str()));
        assert_eq!(app.nav_history.get(&parent).and_then(|h| h.left_to()), Some(child.as_str()));
        assert_eq!(app.trail_enabled(&parent), (false, true));
        app.navigate_history(&parent, Dir::Forward);
        assert_eq!(app.focused.as_deref(), Some(child.as_str()));
    }

    #[test]
    fn back_goes_to_a_parent_on_another_level() {
        let (mut app, parent, child, _peer) = opened_from(&session("trail-back-level"));
        let g = add_group(&mut app, "G");
        app.edit_group(&parent, Some(g.clone()));
        assert!(app.save_session());
        app.navigate_history(&child, Dir::Back);
        assert_eq!((app.focused.as_deref(), app.current_group.as_deref()), (Some(parent.as_str()), Some(g.as_str())));
    }

    #[test]
    fn back_to_a_note_leaves_the_item_view_and_selects_it() {
        let mut app = app_on(&session("trail-back-note"), 1);
        let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
        let (child, _peer) = live_tab(&mut app, ViewKind::Webview2, "https://c/");
        app.edit_parent(&child, Some(note.clone()));
        assert!(app.save_session());
        app.focus_tab(&child);
        app.navigate_history(&child, Dir::Back);
        assert!(app.transition.is_some(), "on its way to the Graph view");
        assert!(app.interaction.selected.len() == 1 && app.interaction.selected.contains(&note));
        assert!(!app.nav_history.contains_key(&note), "a non-tab parent gets no trail");
    }

    #[test]
    fn back_reopens_a_parent_hidden_in_nested_closed_groups() {
        let (mut app, parent, child, _peer) = opened_from(&session("trail-back-closed"));
        let g = add_group(&mut app, "G");
        app.edit_group(&parent, Some(g.clone()));
        assert!(app.save_session());
        app.close_node(&parent);
        app.close_node(&g);
        assert!(!app.nodes.contains_key(&parent) && !app.nodes.contains_key(&g));
        assert!(app.trail_enabled(&child).0, "a hidden parent is still live");
        app.navigate_history(&child, Dir::Back);
        assert!(app.nodes.contains_key(&g) && app.nodes.contains_key(&parent), "both reopened, outermost first");
        assert_eq!((app.focused.as_deref(), app.current_group.as_deref()), (Some(parent.as_str()), Some(g.as_str())));
    }

    #[test]
    fn back_skips_a_deleted_parent_and_is_disabled_when_no_ancestor_is_left() {
        let (mut app, parent, child, _peer) = opened_from(&session("trail-back-deleted"));
        let grand = add(&mut app, Kind::Tab, sleeping_tab("https://g/", "G"));
        app.edit_parent(&parent, Some(grand.clone()));
        assert!(app.save_session());
        app.destroy_node(&parent);
        assert!(app.save_session());
        assert_eq!(app.trail_parent(&child).as_deref(), Some(grand.as_str()), "a delete hands the child up");
        app.navigate_history(&child, Dir::Back);
        assert_eq!(app.focused.as_deref(), Some(grand.as_str()));
        app.focus_tab(&child);
        app.destroy_node(&grand);
        assert!(app.save_session());
        assert!(!app.trail_enabled(&child).0);
    }

    /// Plan D16: a remote delete leaves the child's `PARENT` naming the deleted node, whose fields the delete cleared
    /// (`store/ops.rs`, `OpBody::Delete`), so the walk ends there.
    #[test]
    fn a_remote_delete_of_the_parent_disables_back() {
        let path = session("trail-remote-delete");
        let (mut app, parent, child, _peer) = opened_from(&path);
        let grand = add(&mut app, Kind::Tab, sleeping_tab("https://g/", "G"));
        app.edit_parent(&parent, Some(grand.clone()));
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        app.close_node(&parent);
        assert!(app.save_session());
        other.begin().unwrap().commit(vec![OpBody::Delete { node: parent.parse().unwrap() }]).unwrap();
        assert!(app.save_session());
        assert!(!app.nodes.contains_key(&parent));
        assert!(!app.trail_enabled(&child).0);
    }

    #[test]
    fn forward_on_the_parent_reopens_a_closed_child() {
        let mut app = app_on(&session("trail-forward-closed"), 1);
        let parent = add(&mut app, Kind::Tab, sleeping_tab("https://p/", "P"));
        let child = add(&mut app, Kind::Tab, sleeping_tab("https://c/", "C"));
        app.edit_parent(&child, Some(parent.clone()));
        assert!(app.save_session());
        app.nav_history.entry(parent.clone()).or_default().set_left_to(Some(child.clone()));
        app.close_node(&child);
        assert!(app.trail_enabled(&parent).1, "a closed child is still live");
        app.navigate_history(&parent, Dir::Forward);
        assert!(app.nodes.contains_key(&child));
        assert_eq!(app.focused.as_deref(), Some(child.as_str()));
    }

    #[test]
    fn the_parents_own_forward_wins_over_left_to() {
        let mut app = app_on(&session("trail-own-forward"), 1);
        let (parent, mut peer) = live_tab(&mut app, ViewKind::Webview2, "https://p1/");
        let child = add(&mut app, Kind::Tab, sleeping_tab("https://c/", "C"));
        app.edit_parent(&child, Some(parent.clone()));
        assert!(app.save_session());
        let now = Instant::now();
        let mut h = NavHistory::visited(&["https://p1/", "https://p2/"]);
        assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("https://p1/".into()));
        h.observe("https://p1/", "", false, false, now);
        h.set_left_to(Some(child.clone()));
        app.nav_history.insert(parent.clone(), h);
        app.focus_tab(&parent);
        app.navigate_history(&parent, Dir::Forward);
        assert_eq!(app.focused.as_deref(), Some(parent.as_str()), "stays on the parent");
        assert!(peer.sent().iter().any(|m| matches!(m, HostToView::Navigate { url, .. } if url == "https://p2/")));
    }

    #[test]
    fn a_stale_left_to_is_cleared() {
        let (mut app, parent, child, _peer) = opened_from(&session("trail-stale-left-to"));
        app.navigate_history(&child, Dir::Back);
        app.edit_parent(&child, None);
        assert!(app.save_session());
        assert!(!app.trail_enabled(&parent).1, "the child no longer names this parent");
        app.navigate_history(&parent, Dir::Forward);
        assert_eq!(app.nav_history.get(&parent).and_then(|h| h.left_to()), None);
        assert_eq!(app.focused.as_deref(), Some(parent.as_str()));
    }

    #[test]
    fn a_focus_waits_for_its_node_to_appear() {
        let mut app = app_on(&session("trail-pending-focus"), 1);
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", "T"));
        assert!(app.save_session());
        app.close_node(&tab);
        app.spawns.pending_trail_focus = Some(tab.clone());
        app.poll_trail_focus();
        assert_eq!(app.spawns.pending_trail_focus.as_deref(), Some(tab.as_str()), "hidden but live: keep waiting");
        app.reopen_node(tab.parse().unwrap());
        app.poll_trail_focus();
        assert_eq!((app.spawns.pending_trail_focus.as_deref(), app.focused.as_deref()), (None, Some(tab.as_str())));
        app.spawns.pending_trail_focus = Some("no-such-node".to_string());
        app.poll_trail_focus();
        assert_eq!(app.spawns.pending_trail_focus, None, "a node that is gone is not waited for");
    }

    /// A live tab whose trail is a -> {b, c}, at c: Back's branch list holds b (entry 1).
    fn tab_with_a_sibling(name: &str) -> (App, String, Peer) {
        let mut app = app_on(&session(name), 1);
        let (tab, peer) = live_tab(&mut app, ViewKind::Webview2, "https://c/");
        assert!(app.save_session());
        let now = Instant::now();
        let mut h = NavHistory::visited(&["https://a/", "https://b/"]);
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
        h.observe("https://a/", "", false, true, now);
        h.observe("https://c/", "", false, false, now);
        app.nav_history.insert(tab.clone(), h);
        (app, tab, peer)
    }

    #[test]
    fn a_branch_pick_navigates_the_tab() {
        let (mut app, tab, mut peer) = tab_with_a_sibling("trail-pick");
        app.trail_pick(&tab, 1);
        assert!(peer.sent().iter().any(|m| matches!(m, HostToView::Navigate { url, .. } if url == "https://b/")));
    }

    #[test]
    fn a_plain_move_records_what_navigate_tab_records() {
        let (mut app, tab, _peer) = tab_with_a_sibling("trail-pick-books");
        app.trail_pick(&tab, 1);
        assert_eq!(app.nodes.get(&tab).and_then(|n| n.as_tab()).map(|n| n.url.as_str()), Some("https://b/"));
        assert!(app.spawns.navigated_from.contains_key(&tab), "the origin is noted");
        assert_eq!(app.mirror.url(&tab), "https://b/", "the url is edited, so it is shared");
    }

    #[test]
    fn open_as_card_opens_a_child_tab_and_leaves_the_trail() {
        let (mut app, tab, _peer) = tab_with_a_sibling("trail-promote");
        let before = app.nav_history[&tab].branches(Dir::Back);
        app.open_branch_as_card(&tab, 1);
        let spawn = &app.test_spawns.last().unwrap().0;
        assert_eq!((spawn.url.as_str(), spawn.parent.as_deref()), ("https://b/", Some(tab.as_str())));
        assert_eq!(app.nav_history[&tab].branches(Dir::Back), before);
    }

    #[test]
    fn the_branch_list_draws_a_foreground_popup() {
        let (mut app, tab, _peer) = tab_with_a_sibling("trail-popup");
        app.menus.open_popup = Some(AnchoredPopup::BackBranches);
        let ctx = app.egui_ctx.clone();
        let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(20.0, 20.0));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.build_trail_branches_popup(ui, Dir::Back, anchor, &tab, 1.0);
        });
        output.textures_delta.clear();
        let id = egui::Id::new(("trail_branches_popup", AnchoredPopup::BackBranches));
        let rect = ctx.memory(|m| m.area_rect(id)).expect("the popup's area");
        assert!(rect.width() >= 200.0, "{rect:?}");
        assert_eq!(app.menus.open_popup, Some(AnchoredPopup::BackBranches), "no click: it stays open");
    }
}
