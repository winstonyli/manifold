//! Links: stored `Kind::Link` nodes joining two cards by `from` and `to`. Like a jar, a link is mirror and store
//! state, never a graph card; it goes with either end (`queue_delete`, `remove_remotely_deleted`).

use std::collections::HashSet;

use crate::caps::{Storage, LINK, OPENED_FROM};
use crate::link_gestures::LinkRetarget;
use crate::session_sync::link_id;
use crate::store::model::{field, FieldValue, NodeId};
use crate::App;

/// Why `App::connect` refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectError {
    /// No such user-drawable edge, or its type table forbids these two node types.
    NotAllowed,
    /// An end is not a live card, or not stored yet (a running download).
    NotLive,
    /// The ends sit in different groups.
    DifferentLevel,
    /// An acyclic edge would close a loop.
    Cycle,
    /// The ends are already joined by this edge.
    Exists,
}

/// One edge at a node, as `App::edges_of` lists it.
pub(crate) struct EdgeRef {
    pub edge: &'static str,
    /// The node at the other end.
    pub other: String,
    /// The Link node, for an edge stored as one.
    pub link: Option<String>,
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
        from: &str,
        to: &str,
        linked: Option<&HashSet<NodeId>>,
    ) -> Result<(), ConnectError> {
        let row = crate::caps::edges().row(edge).filter(|r| r.verb().is_some()).ok_or(ConnectError::NotAllowed)?;
        let stored_card = |id: &str| self.nodes.contains_key(id) && !self.is_running_download(id);
        if !stored_card(from) || !stored_card(to) {
            return Err(ConnectError::NotLive);
        }
        if from == to || !crate::caps::edges().can_connect(edge, self.mirror.ty(from), self.mirror.ty(to)) {
            return Err(ConnectError::NotAllowed);
        }
        if self.mirror.group(from) != self.mirror.group(to) {
            return Err(ConnectError::DifferentLevel);
        }
        if row.is_acyclic() && self.would_create_cycle(from, to) {
            return Err(ConnectError::Cycle);
        }
        let exists = match row.storage() {
            Storage::Field(field::PARENT) => self.mirror.parent(from) == Some(to),
            Storage::Node(_) => match linked {
                Some(linked) => to.parse::<NodeId>().is_ok_and(|to| linked.contains(&to)),
                None => self.link_between(from, to).is_some(),
            },
            _ => return Err(ConnectError::NotAllowed),
        };
        if exists {
            return Err(ConnectError::Exists);
        }
        Ok(())
    }

    /// Draws an `edge` edge from `from` to `to`: a parent link, or a Link node, as the edge's table row stores it.
    pub(crate) fn connect(&mut self, edge: &str, from: &str, to: &str) -> Result<(), ConnectError> {
        self.check_connect(edge, from, to, None)?;
        match crate::caps::edges().row(edge).map(|r| r.storage()) {
            Some(Storage::Field(field::PARENT)) => self.edit_parent(from, Some(to.to_string())),
            Some(Storage::Node(_)) => {
                self.create_link(from, to);
            }
            _ => return Err(ConnectError::NotAllowed),
        }
        Ok(())
    }

    /// Removes the `edge` edge joining `from` and `to`, if there is one.
    pub(crate) fn disconnect(&mut self, edge: &str, from: &str, to: &str) {
        match crate::caps::edges().row(edge).map(|r| r.storage()) {
            Some(Storage::Field(field::PARENT)) => {
                if self.mirror.parent(from) == Some(to) {
                    self.edit_parent(from, None);
                }
            }
            Some(Storage::Node(_)) => {
                if let Some(link) = self.link_between(from, to) {
                    self.delete_link(&link);
                }
            }
            _ => {}
        }
    }

    /// The Link node joining `a` and `b`, in either order.
    fn link_between(&self, a: &str, b: &str) -> Option<String> {
        let (a, b) = (a.parse::<NodeId>().ok()?, b.parse::<NodeId>().ok()?);
        self.links_naming(&a.to_string()).into_iter().find(|link| {
            let (from, to) = self.mirror.link_ends(link);
            (from, to) == (Some(a), Some(b)) || (from, to) == (Some(b), Some(a))
        })
    }

    /// The cards a link joins to `from`, either way: `check_connect`'s "already linked", built once per query
    /// instead of once per candidate. A half-made link (an end `None`) joins nothing, as in `link_between`.
    fn linked_to(&self, from: &str) -> HashSet<NodeId> {
        let Ok(me) = from.parse::<NodeId>() else { return HashSet::new() };
        self.links_naming(from)
            .iter()
            .filter_map(|link| match self.mirror.link_ends(link) {
                (Some(a), Some(b)) if a == me => Some(b),
                (Some(a), Some(b)) if b == me => Some(a),
                _ => None,
            })
            .collect()
    }

    /// `linked_to(from)` for an edge stored as a Link node; `None` for another edge, which never reads it.
    fn linked_for(&self, edge: &str, from: &str) -> Option<HashSet<NodeId>> {
        let node_stored = crate::caps::edges().row(edge).is_some_and(|r| matches!(r.storage(), Storage::Node(_)));
        node_stored.then(|| self.linked_to(from))
    }

    /// Whether `other` is a live card shown at the same level as `node`: the far end of an edge worth listing.
    fn shown_with(&self, node: &str, other: &str) -> bool {
        other != node && self.nodes.contains_key(other) && self.mirror.group(other) == self.mirror.group(node)
    }

    /// The live edges at `node`: the card it was opened from (outgoing), the cards opened from it, and its links.
    /// An edge whose far end is missing, not live, or at another level is left out, as the read rule hides it.
    pub(crate) fn edges_of(&self, node: &str) -> Vec<EdgeRef> {
        let mut edges = Vec::new();
        if let Some(opener) = self.mirror.parent(node).filter(|o| self.shown_with(node, o)) {
            edges.push(EdgeRef { edge: OPENED_FROM, other: opener.to_string(), link: None, outgoing: true });
        }
        for child in self.children_of(node).filter(|c| self.shown_with(node, c)) {
            edges.push(EdgeRef { edge: OPENED_FROM, other: child.to_string(), link: None, outgoing: false });
        }
        let me = node.parse::<NodeId>().ok();
        for link in self.links_naming(node) {
            let (from, to) = self.mirror.link_ends(&link);
            let outgoing = from == me;
            let Some(other) = (if outgoing { to } else { from }) else { continue };
            if self.shown_with(node, &other.to_string()) {
                edges.push(EdgeRef { edge: LINK, other: other.to_string(), link: Some(link), outgoing });
            }
        }
        edges
    }

    /// Every live card `connect(edge, from, _)` would accept, in id order.
    pub(crate) fn valid_targets(&self, edge: &str, from: &str) -> Vec<String> {
        let linked = self.linked_for(edge, from);
        let mut targets: Vec<String> = self
            .nodes
            .keys()
            .filter(|to| self.check_connect(edge, from, to, linked.as_ref()).is_ok())
            .cloned()
            .collect();
        targets.sort();
        targets
    }

    /// Whether `valid_targets(edge, from)` is non-empty, stopping at the first (the node menu's edge rows).
    pub(crate) fn has_valid_target(&self, edge: &str, from: &str) -> bool {
        let linked = self.linked_for(edge, from);
        self.nodes.keys().any(|to| self.check_connect(edge, from, to, linked.as_ref()).is_ok())
    }

    /// Pick-target mode's end on a target: draws the chosen edge from the mode's source to `target`, ends the mode and
    /// saves. A refused edge (the target stopped being valid since it was drawn highlighted) says so in a toast.
    /// A drag that began on a link's curve (`retarget`) moves that link's free end to `target` instead, in place.
    pub(crate) fn finish_connect(&mut self, target: &str) {
        self.connect_drag = None;
        let retarget = self.retarget.take();
        let Some((from, edge)) = self.connecting.take() else { return };
        let done = match retarget {
            Some(retarget) => self.retarget_link(&retarget, &from, target),
            None => self.connect(edge, &from, target),
        };
        if done.is_err() {
            self.queue_toast("Can't connect these cards".to_string());
        }
        self.save_session();
    }

    /// Ends pick-target mode, and a connect-drag running as it, without drawing anything.
    pub(crate) fn cancel_connect(&mut self) {
        self.connecting = None;
        self.connect_drag = None;
        self.retarget = None;
    }

    /// Moves the free end of link `retarget.link` to `target` by rewriting that one field, so the Link node keeps its
    /// id and its other end; `fixed` is the card at the end that stays. `target` being the end it already has is no
    /// change. Refused as `connect` would refuse the new pair (a duplicate link, a card not live or on another
    /// level), and when the link is gone or no longer has `fixed` at that end; the link is then untouched.
    fn retarget_link(&mut self, retarget: &LinkRetarget, fixed: &str, target: &str) -> Result<(), ConnectError> {
        let link_is_live = retarget.link.parse::<NodeId>().is_ok_and(|id| self.mirror.is_link(id));
        let (from, to) = self.mirror.link_ends(&retarget.link);
        let (fixed_end, moving_end, free_field) =
            if retarget.fixed_is_from { (from, to, field::TO) } else { (to, from, field::FROM) };
        let fixed_still_there = fixed_end.is_some_and(|end| end.to_string() == fixed);
        let (Some(moving_end), true, true) = (moving_end, link_is_live, fixed_still_there) else {
            return Err(ConnectError::NotLive);
        };
        if moving_end.to_string() == target {
            return Ok(());
        }
        let (new_from, new_to) = if retarget.fixed_is_from { (fixed, target) } else { (target, fixed) };
        self.check_connect(LINK, new_from, new_to, None)?;
        self.edit_field(&retarget.link, free_field, FieldValue::Id(link_id(Some(target))));
        Ok(())
    }

    /// Ends pick-target mode, and a connect-drag, when `id`, a card being removed, is its source.
    pub(crate) fn forget_connect_source(&mut self, id: &str) {
        if self.connecting.as_ref().is_some_and(|(from, _)| from == id) || self.connect_drag.as_deref() == Some(id) {
            self.cancel_connect();
        }
    }

    /// Ends pick-target mode once its targets are no longer drawn: the Graph view not `graph_shown`, or the source
    /// gone or moved off the level shown (another replica's edit). A connect-drag left without its mode (a path that
    /// cleared `connecting` alone) ends too.
    pub(crate) fn end_stale_connect(&mut self, graph_shown: bool) {
        let stale = |(from, _): &(String, &str)| !graph_shown || !self.nodes.contains_key(from) || !self.in_level(from);
        if self.connecting.as_ref().is_some_and(stale) || (self.connecting.is_none() && self.connect_drag.is_some()) {
            self.cancel_connect();
        }
    }

    /// Makes a link from `from` to `to`, both card ids, and returns the new Link node's id. Queued for the next save.
    pub(crate) fn create_link(&mut self, from: &str, to: &str) -> String {
        let key = self.alloc_node_id();
        self.mirror.add_link(key.parse().expect("alloc_node_id returns a store id"));
        self.queue_create(&key);
        self.edit_field(&key, field::FROM, FieldValue::Id(link_id(Some(from))));
        self.edit_field(&key, field::TO, FieldValue::Id(link_id(Some(to))));
        key
    }

    /// Forgets link `id` and queues its delete.
    pub(crate) fn delete_link(&mut self, id: &str) {
        if let Ok(link) = id.parse() {
            self.mirror.forget_link(link);
        }
        self.queue_delete(id);
    }

    /// Remakes `old`'s links on `new`, the node replacing it, each keeping its direction: a tab turned into content
    /// (`redirect_tab_to_content`), a group merged into another (`merge_groups`). Call before `old` goes, whose delete
    /// takes its own links with it. Only a link to another live card carries over: not a dangling, half-made or self
    /// link, not one that would join `new` to itself, and not one to a card `new` was already linked to (that pair
    /// keeps its link).
    pub(crate) fn retarget_links(&mut self, old: &str, new: &str) {
        let (Ok(old_id), Ok(new_id)) = (old.parse::<NodeId>(), new.parse::<NodeId>()) else { return };
        let far_end =
            |(from, to): (Option<NodeId>, Option<NodeId>), me: NodeId| if from == Some(me) { to } else { from };
        let linked: Vec<NodeId> =
            self.links_naming(new).iter().filter_map(|link| far_end(self.mirror.link_ends(link), new_id)).collect();
        for link in self.links_naming(old) {
            let (Some(from), Some(to)) = self.mirror.link_ends(&link) else { continue };
            let other = match (from == old_id, to == old_id) {
                (true, false) => to,
                (false, true) => from,
                _ => continue,
            };
            let far = other.to_string();
            if other == new_id || linked.contains(&other) || !self.nodes.contains_key(&far) {
                continue;
            }
            if from == old_id {
                self.create_link(new, &far);
            } else {
                self.create_link(&far, new);
            }
        }
    }

    /// The Link nodes whose `from` or `to` is `node`.
    pub(crate) fn links_naming(&self, node: &str) -> Vec<String> {
        let Ok(node) = node.parse::<crate::store::model::NodeId>() else { return Vec::new() };
        self.mirror
            .link_ids()
            .filter(|link| {
                let (from, to) = self.mirror.link_ends(&link.to_string());
                from == Some(node) || to == Some(node)
            })
            .map(|link| link.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::caps::{LINK, OPENED_FROM};
    use crate::node::{DownloadPayload, NotePayload, Runtime};
    use crate::session_sync::tests::{add, sleeping_tab};
    use crate::store::model::{field, FieldValue, Kind, NodeId};
    use crate::store::testutil::XorShift;
    use crate::App;

    /// Wave 2 (H2): `valid_targets` holds exactly the cards `check_connect` accepts (`None`: the unhoisted scan),
    /// for every source and edge, over random cards (notes, sleeping tabs, completed and running downloads, two
    /// groups) in three levels, random connects, a half-made link and a dangling one; `has_valid_target` is its
    /// non-emptiness.
    #[test]
    fn valid_targets_match_check_connect() {
        let mut app = App::default();
        let groups: Vec<String> = (0..2)
            .map(|_| {
                let g = app.alloc_node_id();
                app.insert_group_at(g.clone(), None);
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
                app.edit_group(&id, Some(groups[i % 3 - 1].clone()));
            }
        }
        // Sorted: `HashMap` order is random, and the picks must repeat.
        let mut ids: Vec<String> = app.nodes.keys().cloned().collect();
        ids.sort();
        let mut rng = XorShift(0x5eed_1234);
        let mut links = 0;
        for _ in 0..300 {
            let (from, to) = (&ids[rng.below(ids.len() as u64) as usize], &ids[rng.below(ids.len() as u64) as usize]);
            let edge = if rng.below(2) == 0 { LINK } else { OPENED_FROM };
            if app.connect(edge, from, to).is_ok() && edge == LINK {
                links += 1;
            }
        }
        assert!(links >= 15, "{links} links made: raise the connect count");
        let (a, b, c) = (&ids[0], &ids[1], &ids[2]);
        let half = app.create_link(a, b);
        app.edit_field(&half, field::TO, FieldValue::Id(None));
        let ghost: NodeId = app.alloc_node_id().parse().unwrap(); // never inserted
        let dangling = app.create_link(a, c);
        app.edit_field(&dangling, field::TO, FieldValue::Id(Some(ghost)));
        for from in &ids {
            for edge in [LINK, OPENED_FROM] {
                let targets = app.valid_targets(edge, from);
                for to in &ids {
                    let accepted = app.check_connect(edge, from, to, None).is_ok();
                    assert_eq!(targets.binary_search(to).is_ok(), accepted, "{edge} {from} -> {to}");
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
        let g0 = f.groups[0].clone();
        let mut app = f.app;
        let at: Vec<String> = f.nodes.iter().filter(|id| app.mirror.group(id) == Some(g0.as_str())).cloned().collect();
        let n = at.len();
        let mut k = 0;
        for links in [20usize, 100, 300] {
            while app.mirror.link_ids().count() < links {
                let (a, b) = (&at[k % n], &at[(k * 7 + 1) % n]);
                if a != b {
                    app.create_link(a, b);
                }
                k += 1;
            }
            let from = &at[0];
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
