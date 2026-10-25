//! Links: stored `Kind::Link` nodes joining two cards by `from` and `to`. A link is mirror and store state, never a
//! graph card; it goes with either end (`queue_delete`, `remove_remotely_deleted`).

use std::collections::HashSet;

use crate::caps::{Scope, Storage, LINK, OPENED_FROM};
use crate::link_gestures::{DragEnd, LinkRetarget};
use crate::store::model::{field, FieldValue, Kind, NodeId};
use crate::App;

/// The link gestures' state (`App::link`): pick-target mode and the connect-drag and retarget that run as it.
#[derive(Default)]
pub(crate) struct LinkGesture {
    /// Pick-target mode (links spec "Menus and pick-target mode"): the card whose menu chose a verb ("Link to…",
    /// "Opened from…") and the edge it draws. The next click on a valid target finishes it (`finish_connect`); Esc, a
    /// click elsewhere, any other action, leaving the level or the Graph view, or the source going, ends it.
    pub(crate) connecting: Option<(NodeId, &'static str)>,
    /// A connect-drag (link-gestures spec A): the card whose edge handle the primary button went down on. While it
    /// runs, `connecting` is `(source, LINK)`, so pick-target mode's rings show the targets. The release ends it on
    /// the card under the pointer (`end_connect_drag`). It ends with pick-target mode (`cancel_connect`,
    /// `finish_connect`), so every path that ends that mode ends this too.
    pub(crate) connect_drag: Option<NodeId>,
    /// Set with `connect_drag` when the drag began on an existing link's curve (`start_line_retarget`): the link whose
    /// free end follows the pointer, and which end stays. `connect_drag` is then the card at the fixed end. Cleared
    /// wherever `connect_drag` ends (`cancel_connect`, `finish_connect`, a session replace).
    pub(crate) retarget: Option<LinkRetarget>,
    /// While `retarget` runs: what a release at the pointer would do now (`App::retarget_preview`), set each frame by
    /// the graph pass for the band, the ring and EXPORTSTATE.
    pub(crate) retarget_preview: Option<RetargetOutcome>,
    /// The link line under the pointer when the primary press began with the camera still, and the camera rect then
    /// (`line_click`). Only that line, with the camera unmoved, can be taken by the click that ends the press.
    pub(crate) line_press: Option<(NodeId, egui::Rect)>,
}

impl LinkGesture {
    /// Ends pick-target mode and the connect-drag and retarget running as it, returning the mode's source and edge.
    pub(crate) fn end(&mut self) -> Option<(NodeId, &'static str)> {
        self.connect_drag = None;
        self.retarget = None;
        self.connecting.take()
    }
}

/// Why `App::connect` refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectError {
    /// No such user-drawable edge, or its type table forbids these two node types.
    NotAllowed,
    /// An end is not a live card, or not stored yet (a running download).
    NotLive,
    /// The ends sit in different groups.
    DifferentLevel,
    /// The target is outside the edge's reach (`Scope`): for `jar`, not at the source's level or above it.
    OutOfScope,
    /// An acyclic edge would close a loop.
    Cycle,
    /// The ends are already joined by this edge.
    Exists,
}

/// What releasing a dragged link end does (`App::retarget_outcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetargetOutcome {
    /// The end moves to the card it was released on.
    Retargeted,
    /// The link is deleted.
    Deleted,
    /// Nothing happens, silently.
    Cancelled,
    /// Nothing changes and a toast says so.
    Refused,
}

impl RetargetOutcome {
    /// The outcome's name in EXPORTSTATE's `retarget_preview`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Retargeted => "retarget",
            Self::Deleted => "delete",
            Self::Refused => "refused",
            Self::Cancelled => "cancel",
        }
    }
}

/// One edge at a node, as `App::edges_of` lists it.
pub(crate) struct EdgeRef {
    pub edge: &'static str,
    /// The node at the other end.
    pub other: NodeId,
    /// The Link node, for an edge stored as one.
    pub link: Option<NodeId>,
    /// Whether the edge leaves this node (`from` is this node).
    pub outgoing: bool,
}

impl App {
    /// Whether an `edge` edge from `from` to `to` may be drawn now, and if not, why not. Checks liveness before the
    /// type table so a missing card reads as `NotLive`, not as an unknown type. A running download is not live
    /// here: it is not stored until it completes, so the save would write it as none (a half-made link). With
    /// `linked` (`linked_for`), whether a link already joins them is a set lookup; with `None`, a scan of the links
    /// (`link_between`).
    fn check_connect(
        &self,
        edge: &str,
        from: NodeId,
        to: NodeId,
        linked: Option<&HashSet<NodeId>>,
    ) -> Result<(), ConnectError> {
        let row = crate::caps::edges().row(edge).filter(|r| r.verb().is_some()).ok_or(ConnectError::NotAllowed)?;
        let stored_card = |id: NodeId| self.nodes.contains_key(&id) && !self.is_running_download(id);
        if !stored_card(from) || !stored_card(to) {
            return Err(ConnectError::NotLive);
        }
        let ty = |id: NodeId| crate::card_kind::canonical_type(self.doc.mirror.ty(id));
        if from == to || !crate::caps::edges().can_connect(edge, ty(from), ty(to)) {
            return Err(ConnectError::NotAllowed);
        }
        if !self.in_scope(row.scope(), from, to) {
            return Err(match row.scope() {
                Scope::SameLevel => ConnectError::DifferentLevel,
                Scope::SameLevelOrAncestor => ConnectError::OutOfScope,
            });
        }
        if row.is_acyclic() && self.would_create_cycle(from, to) {
            return Err(ConnectError::Cycle);
        }
        let exists = match row.storage() {
            // `parent` lives in the mirror's links, not its field map.
            Storage::Field(field::PARENT) => self.doc.mirror.parent(from) == Some(to),
            Storage::Field(no) => self.doc.mirror.get(from, no) == FieldValue::Id(Some(to)),
            Storage::Node(_) => match linked {
                Some(linked) => linked.contains(&to),
                None => self.link_between(from, to).is_some(),
            },
            Storage::Tree => return Err(ConnectError::NotAllowed),
        };
        if exists {
            return Err(ConnectError::Exists);
        }
        Ok(())
    }

    /// Draws an `edge` edge from `from` to `to`: a parent link, a field, or a Link node, as the edge's table row
    /// stores it.
    pub(crate) fn connect(&mut self, edge: &str, from: NodeId, to: NodeId) -> Result<(), ConnectError> {
        self.check_connect(edge, from, to, None)?;
        match crate::caps::edges().row(edge).map(|r| r.storage()) {
            Some(Storage::Field(field::PARENT)) => self.edit_parent(from, Some(to)),
            Some(Storage::Field(no)) => self.edit_field(from, no, FieldValue::Id(Some(to))),
            Some(Storage::Node(_)) => {
                self.create_link(from, to);
            }
            _ => return Err(ConnectError::NotAllowed),
        }
        Ok(())
    }

    /// Removes the `edge` edge joining `from` and `to`, if there is one.
    pub(crate) fn disconnect(&mut self, edge: &str, from: NodeId, to: NodeId) {
        match crate::caps::edges().row(edge).map(|r| r.storage()) {
            Some(Storage::Field(field::PARENT)) => {
                if self.doc.mirror.parent(from) == Some(to) {
                    self.edit_parent(from, None);
                }
            }
            Some(Storage::Field(no)) => {
                if self.doc.mirror.get(from, no) == FieldValue::Id(Some(to)) {
                    self.edit_field(from, no, FieldValue::Id(None));
                }
            }
            Some(Storage::Node(_)) => {
                if let Some(link) = self.link_between(from, to) {
                    self.delete_link(link);
                }
            }
            _ => {}
        }
    }

    /// `level` and every group enclosing it, out to the top level (`None`), nearest first. Borrowed and
    /// allocation-free: `effective_jar` runs it per card per frame. Bounded so a cycle in bad data ends.
    pub(crate) fn levels_from(&self, level: Option<NodeId>) -> impl Iterator<Item = Option<NodeId>> + '_ {
        std::iter::successors(Some(level), move |at| at.map(|g| self.doc.mirror.group(g))).take(self.nodes.len() + 2)
    }

    /// The level an edge from `id` reaches out from (spec "Scope", decision 2026-10-14): a group's own contents,
    /// so its edge may name a card inside it; any other card's group.
    pub(crate) fn reach_origin(&self, id: NodeId) -> Option<NodeId> {
        if self.is_container(id) {
            Some(id)
        } else {
            self.doc.mirror.group(id)
        }
    }

    /// Whether `to` is in reach of `from` under `scope`. `SameLevel`: both in one group. `SameLevelOrAncestor`:
    /// `to` sits at `from`'s `reach_origin` or in a group enclosing it.
    pub(crate) fn in_scope(&self, scope: Scope, from: NodeId, to: NodeId) -> bool {
        let at = self.doc.mirror.group(to);
        match scope {
            Scope::SameLevel => self.doc.mirror.group(from) == at,
            Scope::SameLevelOrAncestor => self.levels_from(self.reach_origin(from)).any(|level| level == at),
        }
    }

    /// Whether `node` is `root` or sits anywhere inside it.
    pub(crate) fn within(&self, node: NodeId, root: NodeId) -> bool {
        node == root || self.levels_from(self.doc.mirror.group(node)).any(|level| level == Some(root))
    }

    /// The Link node joining `a` and `b`, in either order.
    fn link_between(&self, a: NodeId, b: NodeId) -> Option<NodeId> {
        self.links_naming(a).into_iter().find(|&link| {
            let (from, to) = self.doc.mirror.link_ends(link);
            (from, to) == (Some(a), Some(b)) || (from, to) == (Some(b), Some(a))
        })
    }

    /// The cards a link joins to `from`, either way: `check_connect`'s "already linked", built once per query
    /// instead of once per candidate. A half-made link (an end `None`) joins nothing, as in `link_between`.
    fn linked_to(&self, from: NodeId) -> HashSet<NodeId> {
        let me = from;
        self.links_naming(from)
            .into_iter()
            .filter_map(|link| match self.doc.mirror.link_ends(link) {
                (Some(a), Some(b)) if a == me => Some(b),
                (Some(a), Some(b)) if b == me => Some(a),
                _ => None,
            })
            .collect()
    }

    /// `linked_to(from)` for an edge stored as a Link node; `None` for another edge, which never reads it.
    fn linked_for(&self, edge: &str, from: NodeId) -> Option<HashSet<NodeId>> {
        let node_stored = crate::caps::edges().row(edge).is_some_and(|r| matches!(r.storage(), Storage::Node(_)));
        node_stored.then(|| self.linked_to(from))
    }

    /// Whether `other` is a live card shown at the same level as `node`: the far end of an edge worth listing.
    fn shown_with(&self, node: NodeId, other: NodeId) -> bool {
        other != node && self.nodes.contains_key(&other) && self.doc.mirror.group(other) == self.doc.mirror.group(node)
    }

    /// The card `id` was opened from, when that card is live here (spec decision 9). The stored edge survives a
    /// close of its parent and reads as none until the parent is reopened; a deleted parent reads as none forever.
    /// Every reader of the `opened-from` edge goes through this; the writers (`edit_parent`, a spawn's carried
    /// `parent`, the save) read `Mirror::parent` directly.
    pub(crate) fn opened_from(&self, id: NodeId) -> Option<NodeId> {
        self.doc.mirror.parent(id).filter(|p| self.nodes.contains_key(p))
    }

    /// The live edges at `node`: the card it was opened from (outgoing), the cards opened from it, and its links.
    /// An edge whose far end is missing, not live, or at another level is left out, as the read rule hides it.
    pub(crate) fn edges_of(&self, node: NodeId) -> Vec<EdgeRef> {
        let mut edges = Vec::new();
        if let Some(opener) = self.opened_from(node).filter(|&o| self.shown_with(node, o)) {
            edges.push(EdgeRef { edge: OPENED_FROM, other: opener, link: None, outgoing: true });
        }
        for child in self.children_of(node).filter(|&c| self.shown_with(node, c)) {
            edges.push(EdgeRef { edge: OPENED_FROM, other: child, link: None, outgoing: false });
        }
        let me = Some(node);
        // Field edges other than `parent` (the `jar` edge): the one a source names, and the sources naming this node.
        let field_rows: Vec<(&'static str, u32)> = crate::caps::edges()
            .user_creatable()
            .filter_map(|r| match r.storage() {
                Storage::Field(no) if no != field::PARENT => Some((r.id(), no)),
                _ => None,
            })
            .collect();
        for (edge, no) in field_rows {
            if let FieldValue::Id(Some(to)) = self.doc.mirror.get(node, no) {
                if self.shown_with(node, to) {
                    edges.push(EdgeRef { edge, other: to, link: None, outgoing: true });
                }
            }
            for &other in self.nodes.keys().filter(|&&o| self.shown_with(node, o)) {
                if self.doc.mirror.get(other, no) == FieldValue::Id(me) {
                    edges.push(EdgeRef { edge, other, link: None, outgoing: false });
                }
            }
        }
        for link in self.links_naming(node) {
            let (from, to) = self.doc.mirror.link_ends(link);
            let outgoing = from == me;
            let Some(other) = (if outgoing { to } else { from }) else { continue };
            if self.shown_with(node, other) {
                edges.push(EdgeRef { edge: LINK, other, link: Some(link), outgoing });
            }
        }
        edges
    }

    /// Every live card `connect(edge, from, _)` would accept, in id order.
    pub(crate) fn valid_targets(&self, edge: &str, from: NodeId) -> Vec<NodeId> {
        let linked = self.linked_for(edge, from);
        let mut targets: Vec<NodeId> = self
            .nodes
            .keys()
            .filter(|&&to| self.check_connect(edge, from, to, linked.as_ref()).is_ok())
            .copied()
            .collect();
        targets.sort();
        targets
    }

    /// Whether `valid_targets(edge, from)` is non-empty, stopping at the first (the node menu's edge rows).
    pub(crate) fn has_valid_target(&self, edge: &str, from: NodeId) -> bool {
        let linked = self.linked_for(edge, from);
        self.nodes.keys().any(|&to| self.check_connect(edge, from, to, linked.as_ref()).is_ok())
    }

    /// Pick-target mode's end on a target: draws the chosen edge from the mode's source to `target`, ends the mode and
    /// saves. A refused edge (the target stopped being valid since it was drawn highlighted) says so in a toast.
    pub(crate) fn finish_connect(&mut self, target: NodeId) {
        let Some((from, edge)) = self.link.end() else { return };
        if self.connect(edge, from, target).is_err() {
            self.queue_toast("Can't connect these cards".to_string());
        }
        self.save_session();
    }

    /// What a release at `end` would do to the link being dragged, if one is: `retarget_outcome`, the very function
    /// `finish_retarget` acts on, so the preview and the release cannot differ.
    pub(crate) fn retarget_preview(&self, end: &DragEnd) -> Option<RetargetOutcome> {
        Some(self.retarget_outcome(self.link.retarget.as_ref()?, self.link.connect_drag?, end))
    }

    /// Ends the connect-drag of a link's curve (`retarget`, its fixed end on `fixed`) at `end`: acts on
    /// `retarget_outcome` and saves. The one place a retarget release is decided.
    pub(crate) fn finish_retarget(&mut self, retarget: &LinkRetarget, fixed: NodeId, end: &DragEnd) {
        match self.retarget_outcome(retarget, fixed, end) {
            RetargetOutcome::Retargeted => {
                if let DragEnd::Card(target) = end {
                    let free_field = if retarget.fixed_is_from { field::TO } else { field::FROM };
                    self.edit_field(retarget.link, free_field, FieldValue::Id(Some(*target)));
                }
            }
            RetargetOutcome::Deleted => self.delete_link(retarget.link),
            RetargetOutcome::Cancelled => return,
            RetargetOutcome::Refused => self.queue_toast("Can't connect these cards".to_string()),
        }
        self.save_session();
    }

    /// Ends pick-target mode, and a connect-drag running as it, without drawing anything.
    pub(crate) fn cancel_connect(&mut self) {
        self.link.end();
    }

    /// What releasing the free end of link `retarget.link` at `end` does, its other end staying on `fixed`:
    /// - the link gone or no longer holding `fixed` at that end, a release not over the canvas
    ///   (`DragEnd::Abandoned`), or one on the end's own card: `Cancelled`, the link untouched, silently;
    /// - empty canvas, or a card `check_connect` says is already joined to `fixed` (`ConnectError::Exists`):
    ///   `Deleted`, the link goes as a click on it would;
    /// - the fixed end's own card, or a card `check_connect` refuses for any other reason: `Refused`, link kept;
    /// - otherwise `Retargeted`: the free end moves there by rewriting that one field, so the Link node keeps its
    ///   id, other end and direction.
    fn retarget_outcome(&self, retarget: &LinkRetarget, fixed: NodeId, end: &DragEnd) -> RetargetOutcome {
        let link_is_live = self.doc.mirror.is_link(retarget.link);
        let (from, to) = self.doc.mirror.link_ends(retarget.link);
        let (fixed_end, moving_end) = if retarget.fixed_is_from { (from, to) } else { (to, from) };
        let fixed_still_there = fixed_end == Some(fixed);
        let (Some(moving_end), true, true) = (moving_end, link_is_live, fixed_still_there) else {
            return RetargetOutcome::Cancelled;
        };
        match end {
            DragEnd::Abandoned => RetargetOutcome::Cancelled,
            DragEnd::Canvas => RetargetOutcome::Deleted,
            DragEnd::Source => RetargetOutcome::Refused,
            DragEnd::Card(target) if moving_end == *target => RetargetOutcome::Cancelled,
            DragEnd::Card(target) => {
                let (new_from, new_to) = if retarget.fixed_is_from { (fixed, *target) } else { (*target, fixed) };
                match self.check_connect(LINK, new_from, new_to, None) {
                    Ok(()) => RetargetOutcome::Retargeted,
                    Err(ConnectError::Exists) => RetargetOutcome::Deleted,
                    Err(_) => RetargetOutcome::Refused,
                }
            }
        }
    }

    /// Ends pick-target mode, and a connect-drag, when `id`, a card being removed, is its source.
    pub(crate) fn forget_connect_source(&mut self, id: NodeId) {
        if self.link.connecting.as_ref().is_some_and(|&(from, _)| from == id) || self.link.connect_drag == Some(id) {
            self.cancel_connect();
        }
    }

    /// Ends pick-target mode once its targets are no longer drawn: the Graph view not `graph_shown`, or the source
    /// gone or moved off the level shown (another replica's edit). A connect-drag left without its mode (a path that
    /// cleared `connecting` alone) ends too.
    pub(crate) fn end_stale_connect(&mut self, graph_shown: bool) {
        let stale =
            |&(from, _): &(NodeId, &str)| !graph_shown || !self.nodes.contains_key(&from) || !self.in_level(from);
        if self.link.connecting.as_ref().is_some_and(stale)
            || (self.link.connecting.is_none() && self.link.connect_drag.is_some())
        {
            self.cancel_connect();
        }
    }

    /// Makes a link from `from` to `to`, both card ids, and returns the new Link node's id. Queued for the next save.
    pub(crate) fn create_link(&mut self, from: NodeId, to: NodeId) -> NodeId {
        let key = self.alloc_node_id();
        self.doc.mirror.add_stored(key, Kind::Link);
        self.queue_create(key);
        self.edit_field(key, field::FROM, FieldValue::Id(Some(from)));
        self.edit_field(key, field::TO, FieldValue::Id(Some(to)));
        key
    }

    /// Forgets link `id` and queues its delete.
    pub(crate) fn delete_link(&mut self, id: NodeId) {
        self.doc.mirror.forget_stored(id);
        self.queue_delete(id);
    }

    /// Remakes `old`'s links on `new`, the node replacing it, each keeping its direction: a tab turned into content
    /// (`redirect_tab_to_content`), a group merged into another (`merge_groups`). Call before `old` goes, whose delete
    /// takes its own links with it. Only a link to another live card carries over: not a dangling, half-made or self
    /// link, not one that would join `new` to itself, and not one to a card `new` was already linked to (that pair
    /// keeps its link).
    pub(crate) fn retarget_links(&mut self, old: NodeId, new: NodeId) {
        let (old_id, new_id) = (old, new);
        let far_end =
            |(from, to): (Option<NodeId>, Option<NodeId>), me: NodeId| if from == Some(me) { to } else { from };
        let linked: Vec<NodeId> = self
            .links_naming(new)
            .into_iter()
            .filter_map(|link| far_end(self.doc.mirror.link_ends(link), new_id))
            .collect();
        for link in self.links_naming(old) {
            let (Some(from), Some(to)) = self.doc.mirror.link_ends(link) else { continue };
            let other = match (from == old_id, to == old_id) {
                (true, false) => to,
                (false, true) => from,
                _ => continue,
            };
            if other == new_id || linked.contains(&other) || !self.nodes.contains_key(&other) {
                continue;
            }
            if from == old_id {
                self.create_link(new, other);
            } else {
                self.create_link(other, new);
            }
        }
    }

    /// The Link nodes whose `from` or `to` is `node`.
    pub(crate) fn links_naming(&self, node: NodeId) -> Vec<NodeId> {
        self.doc
            .mirror
            .link_ids()
            .filter(|&link| {
                let (from, to) = self.doc.mirror.link_ends(link);
                from == Some(node) || to == Some(node)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::caps::{LINK, OPENED_FROM};
    use crate::card_id::testutil::node;
    use crate::node::{DownloadPayload, NotePayload, Runtime};
    use crate::session_sync::tests::{add, sleeping_tab};
    use crate::store::model::{field, FieldValue, Kind, NodeId};
    use crate::store::testutil::XorShift;
    use crate::App;

    /// `LinkGesture::end` clears pick-target mode, the connect-drag and the retarget (what `cancel_connect` and
    /// `finish_connect` cleared by hand) and returns the mode's source; `line_press` and the preview are not its
    /// business.
    #[test]
    fn end_clears_the_mode_the_drag_and_the_retarget_only() {
        let rect = egui::Rect::NOTHING;
        let mut gesture = super::LinkGesture {
            connecting: Some((node(1), LINK)),
            connect_drag: Some(node(1)),
            retarget: Some(super::LinkRetarget { link: node(2), fixed_is_from: true }),
            retarget_preview: Some(super::RetargetOutcome::Deleted),
            line_press: Some((node(2), rect)),
        };
        assert_eq!(gesture.end(), Some((node(1), LINK)));
        assert!(gesture.connecting.is_none() && gesture.connect_drag.is_none() && gesture.retarget.is_none());
        assert!(gesture.retarget_preview.is_some() && gesture.line_press.is_some());
        assert_eq!(gesture.end(), None);
    }

    /// Wave 2 (H2): `valid_targets` holds exactly the cards `check_connect` accepts (`None`: the unhoisted scan),
    /// for every source and edge, over random cards (notes, sleeping tabs, completed and running downloads, two
    /// groups) in three levels, random connects, a half-made link and a dangling one; `has_valid_target` is its
    /// non-emptiness.
    #[test]
    fn valid_targets_match_check_connect() {
        let mut app = App::default();
        let groups: Vec<NodeId> = (0..2)
            .map(|_| {
                let g = app.alloc_node_id();
                app.insert_group_at(g, None);
                g
            })
            .collect();
        let download = |completed| {
            Runtime::Download(DownloadPayload {
                path: "f.bin".into(),
                bytes_received: 1,
                total_bytes: Some(2),
                completed,
                failed: false,
            })
        };
        for i in 0..60usize {
            let id = match i % 4 {
                0 => add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false })),
                1 => add(&mut app, Kind::Tab, sleeping_tab("https://a.test/", "a")),
                2 => add(&mut app, Kind::Download, download(true)),
                _ => add(&mut app, Kind::Download, download(false)),
            };
            if !i.is_multiple_of(3) {
                app.edit_group(id, Some(groups[i % 3 - 1]));
            }
        }
        // Sorted: `HashMap` order is random, and the picks must repeat.
        let mut ids: Vec<NodeId> = app.nodes.keys().copied().collect();
        ids.sort();
        let mut rng = XorShift(0x5eed_1234);
        let mut links = 0;
        for _ in 0..300 {
            let (from, to) = (ids[rng.below(ids.len() as u64) as usize], ids[rng.below(ids.len() as u64) as usize]);
            let edge = if rng.below(2) == 0 { LINK } else { OPENED_FROM };
            if app.connect(edge, from, to).is_ok() && edge == LINK {
                links += 1;
            }
        }
        assert!(links >= 15, "{links} links made: raise the connect count");
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        let half = app.create_link(a, b);
        app.edit_field(half, field::TO, FieldValue::Id(None));
        let ghost = app.alloc_node_id(); // never inserted
        let dangling = app.create_link(a, c);
        app.edit_field(dangling, field::TO, FieldValue::Id(Some(ghost)));
        for &from in &ids {
            for edge in [LINK, OPENED_FROM] {
                let targets = app.valid_targets(edge, from);
                for &to in &ids {
                    let accepted = app.check_connect(edge, from, to, None).is_ok();
                    assert_eq!(targets.binary_search(&to).is_ok(), accepted, "{edge} {from} -> {to}");
                }
                assert_eq!(app.has_valid_target(edge, from), !targets.is_empty(), "{edge} {from}");
            }
        }
    }

    /// Wave 2 (H2): pick-target mode's per-frame queries at the first group of the 2000-node fixture, with 20, 100
    /// and 300 links among its cards. Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins link_query_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn link_query_cost() {
        use crate::bench::{level_fixture, report, timed};
        let f = level_fixture(false);
        let g0 = f.groups[0];
        let mut app = f.app;
        let at: Vec<NodeId> = f.nodes.iter().filter(|&&id| app.doc.mirror.group(id) == Some(g0)).copied().collect();
        let n = at.len();
        let mut k = 0;
        for links in [20usize, 100, 300] {
            while app.doc.mirror.link_ids().count() < links {
                let (a, b) = (at[k % n], at[(k * 7 + 1) % n]);
                if a != b {
                    app.create_link(a, b);
                }
                k += 1;
            }
            let from = at[0];
            report(&format!("valid_targets(LINK), L={links}"), timed(50, || drop(app.valid_targets(LINK, from))));
            report(
                &format!("has_valid_target(LINK), L={links}"),
                timed(50, || {
                    std::hint::black_box(app.has_valid_target(LINK, from));
                }),
            );
        }
    }
}
