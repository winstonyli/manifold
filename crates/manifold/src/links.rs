//! Links: stored `Kind::Link` nodes joining two cards by `from` and `to`. Like a jar, a link is mirror and store
//! state, never a graph card; it goes with either end (`queue_delete`, `remove_remotely_deleted`).

use crate::session_sync::link_id;
use crate::store::model::{field, FieldValue};
use crate::App;

impl App {
    /// Makes a link from `from` to `to`, both card ids, and returns the new Link node's id. Queued for the next save.
    #[allow(dead_code)] // only tests call it until the connect gesture (a later links task)
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
