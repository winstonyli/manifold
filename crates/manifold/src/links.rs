//! Links: stored `Kind::Link` nodes joining two cards by `from` and `to`. Like a jar, a link is mirror and store
//! state, never a graph card; it goes with either end (`queue_delete`, `remove_remotely_deleted`).

use crate::caps::{Storage, LINK, OPENED_FROM};
use crate::session_sync::link_id;
use crate::store::model::{field, FieldValue, NodeId};
use crate::App;

/// Why `App::connect` refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectError {
    /// No such user-drawable edge, or its type table forbids these two node types.
    NotAllowed,
    /// An end is not a live card.
    NotLive,
    /// The ends sit in different groups.
    DifferentLevel,
    /// An acyclic edge would close a loop.
    Cycle,
    /// The ends are already joined by this edge.
    Exists,
}

/// One edge at a node, as `App::edges_of` lists it.
#[allow(dead_code)] // read by tests until the edge menu (a later links task)
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
    /// type table so a missing card reads as `NotLive`, not as an unknown type.
    fn check_connect(&self, edge: &str, from: &str, to: &str) -> Result<(), ConnectError> {
        let row = crate::caps::edges().row(edge).filter(|r| r.verb().is_some()).ok_or(ConnectError::NotAllowed)?;
        if !self.nodes.contains_key(from) || !self.nodes.contains_key(to) {
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
            Storage::Node(_) => self.link_between(from, to).is_some(),
            _ => return Err(ConnectError::NotAllowed),
        };
        if exists {
            return Err(ConnectError::Exists);
        }
        Ok(())
    }

    /// Draws an `edge` edge from `from` to `to`: a parent link, or a Link node, as the edge's table row stores it.
    pub(crate) fn connect(&mut self, edge: &str, from: &str, to: &str) -> Result<(), ConnectError> {
        self.check_connect(edge, from, to)?;
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
    #[allow(dead_code)] // only tests call it until the edge menu (a later links task)
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

    /// Whether `other` is a live card shown at the same level as `node`: the far end of an edge worth listing.
    fn shown_with(&self, node: &str, other: &str) -> bool {
        other != node && self.nodes.contains_key(other) && self.mirror.group(other) == self.mirror.group(node)
    }

    /// The live edges at `node`: the card it was opened from (outgoing), the cards opened from it, and its links.
    /// An edge whose far end is missing, not live, or at another level is left out, as the read rule hides it.
    #[allow(dead_code)] // only tests call it until the edge menu (a later links task)
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
    #[allow(dead_code)] // only tests call it until the connect gesture (a later links task)
    pub(crate) fn valid_targets(&self, edge: &str, from: &str) -> Vec<String> {
        let mut targets: Vec<String> =
            self.nodes.keys().filter(|to| self.check_connect(edge, from, to).is_ok()).cloned().collect();
        targets.sort();
        targets
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
