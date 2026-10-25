//! Labels: stored `Kind::Label` nodes a card can carry by name and colour (`PROPS/labels`). A label is mirror and
//! store state, never a graph card, like a link (`links.rs`).

use std::collections::BTreeMap;

use crate::names::{clean_name, name_key};
use crate::session_sync::color_value;
use crate::store::model::{field, FieldValue, Kind, NodeId};
use crate::App;

/// A label's synced definition, decoded from its mirrored fields (`Mirror::label_def`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LabelDef {
    pub(crate) name: String,
    pub(crate) color: Option<egui::Color32>,
}

#[allow(dead_code, reason = "used by the membership API and the selection bar menu (Tasks 4 and 8)")]
impl App {
    /// The live labels, by id.
    pub(crate) fn label_defs(&self) -> BTreeMap<NodeId, LabelDef> {
        self.doc.mirror.stored_ids(Kind::Label).map(|id| (id, self.doc.mirror.label_def(id))).collect()
    }

    /// The live label whose name has the same key as `name` (`names::name_key`), other than `except`.
    fn label_named(&self, name: &str, except: Option<NodeId>) -> Option<NodeId> {
        let key = name_key(name);
        self.label_defs().into_iter().find(|(id, d)| Some(*id) != except && name_key(&d.name) == key).map(|(id, _)| id)
    }

    /// Makes a label named `name` (cleaned) and returns its id, or the live label that already has that name; `None`
    /// for a name that is empty once cleaned. With no `color`, it takes a palette colour no label has. Queued for the
    /// next save.
    pub(crate) fn create_label(&mut self, name: &str, color: Option<egui::Color32>) -> Option<NodeId> {
        let name = clean_name(name).into_owned();
        if name.is_empty() {
            return None;
        }
        if let Some(existing) = self.label_named(&name, None) {
            return Some(existing);
        }
        let color = color.unwrap_or_else(|| {
            crate::theme::free_tag_color(&self.label_defs().values().map(|d| d.color).collect::<Vec<_>>())
        });
        let id = self.alloc_node_id();
        self.doc.mirror.add_stored(id, Kind::Label);
        self.queue_create(id);
        self.edit_field(id, field::NAME, FieldValue::Str(name));
        self.edit_field(id, field::COLOR, color_value(Some(color)));
        Some(id)
    }

    /// Renames label `id`; false for a name that is empty once cleaned or that another label has.
    pub(crate) fn rename_label(&mut self, id: NodeId, name: &str) -> bool {
        let name = clean_name(name).into_owned();
        if name.is_empty() || self.label_named(&name, Some(id)).is_some() {
            return false;
        }
        self.edit_field(id, field::NAME, FieldValue::Str(name));
        true
    }

    pub(crate) fn recolor_label(&mut self, id: NodeId, color: Option<egui::Color32>) {
        self.edit_field(id, field::COLOR, color_value(color));
    }

    /// Forgets label `id` and queues its delete. Cards keep their membership entries, which show nothing once the
    /// label is gone (`Mirror::card_labels`).
    pub(crate) fn delete_label(&mut self, id: NodeId) {
        self.doc.mirror.forget_stored(id);
        self.queue_delete(id);
    }
}
