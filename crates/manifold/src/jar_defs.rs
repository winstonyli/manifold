//! Named cookie jars as synced definitions (`App::jar_defs`, spec "Data model"). A jar is a `Kind::Jar` store node
//! but never a graph node: `session_sync` bridges these to the document in `shared_nodes`, `materialize` and
//! `remove_remotely_deleted`.

use crate::session_sync::{color_of, Fields, Shared};
use crate::store::model::{field, FieldValue, NodeId};
use crate::theme::TAG_COLOR_PALETTE;
use crate::{App, JarDef};

impl App {
    /// A new named jar with the next palette colour. Saved by the caller's `save_session`.
    #[allow(dead_code)] // The jars page (a later task) calls it.
    pub(crate) fn create_jar(&mut self, name: &str) -> NodeId {
        let id: NodeId = self.alloc_node_id().parse().expect("alloc_node_id returns a store id");
        let color = Some(TAG_COLOR_PALETTE[self.jar_defs.len() % TAG_COLOR_PALETTE.len()]);
        self.jar_defs.insert(id, JarDef { name: name.trim().to_string(), color, ephemeral: false });
        id
    }

    pub(crate) fn jar_named(&self, name: &str) -> Option<NodeId> {
        self.jar_defs.iter().find(|(_, d)| d.name == name).map(|(id, _)| *id)
    }

    #[allow(dead_code)] // The jars page (a later task) calls it.
    pub(crate) fn rename_jar(&mut self, id: NodeId, name: &str) {
        let name = name.trim();
        if let (false, Some(def)) = (name.is_empty(), self.jar_defs.get_mut(&id)) {
            def.name = name.to_string();
        }
    }

    pub(crate) fn set_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        if let Some(def) = self.jar_defs.get_mut(&id) {
            def.ephemeral = on;
        }
    }

    /// The user deleted jar `id`: tabs and groups that named it fall back to the default jar (deviation 9), then it is
    /// forgotten. Its document node is deleted by the next save (`diff`: live no longer has it).
    #[allow(dead_code)] // The jars page (a later task) calls it.
    pub(crate) fn delete_jar(&mut self, id: NodeId) {
        for node in self.nodes.values_mut() {
            match &mut node.payload {
                crate::node::NodePayload::Tab(t) if t.jar.jar == Some(id) => t.jar = Default::default(),
                crate::node::NodePayload::Unloaded(t) if t.jar.jar == Some(id) => t.jar = Default::default(),
                crate::node::NodePayload::Group(g) if g.default_jar == Some(id) => g.default_jar = None,
                _ => {}
            }
        }
        self.forget_jar(id);
    }

    /// Drops jar `id`'s definition (a local or remote delete). Task 4 also drops its cookies and file.
    pub(crate) fn forget_jar(&mut self, id: NodeId) {
        self.jar_defs.remove(&id);
    }

    /// A `Kind::Jar` change from the document: applies the fields that differ from what this replica last saw.
    pub(crate) fn materialize_jar(&mut self, id: NodeId, fields: &Fields, seen: &Option<Shared>, now: &Shared) {
        let changed = |no: u32| seen.as_ref().is_none_or(|s| s.fields.get(&no) != now.fields.get(&no));
        let def =
            self.jar_defs.entry(id).or_insert_with(|| JarDef { name: String::new(), color: None, ephemeral: false });
        if changed(field::NAME) {
            def.name = match fields.get(&field::NAME) {
                Some(FieldValue::Str(s)) => s.clone(),
                _ => String::new(),
            };
        }
        if changed(field::COLOR) {
            def.color = fields.get(&field::COLOR).and_then(color_of);
        }
        if changed(field::EPHEMERAL) {
            let on = matches!(fields.get(&field::EPHEMERAL), Some(FieldValue::Bool(true)));
            self.set_jar_ephemeral(id, on);
        }
    }
}
