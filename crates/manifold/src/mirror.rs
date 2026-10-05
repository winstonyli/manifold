//! The app's decoded copy of the document's fields that have moved off `GraphNode` and its payloads (node model
//! spec, 2b "Read model"). Every read goes through the accessors here, so making `Doc` the app's own state later
//! changes only where they read.
//!
//! The mirror equals the document except for fields with an edit still waiting for a save (`App::pending`);
//! `App::edit_field` is the one writer, and `App::mirror_remote` applies another replica's change.

use std::collections::HashMap;

use crate::session_sync::{color_of, Fields};
use crate::store::model::{field, field_def, FieldValue};

/// The fields the mirror holds. `diff` leaves them to `App::pending` on existing nodes.
#[cfg_attr(not(test), allow(dead_code))] // wired in by `App::edit_field` (plan 2b-1, Task 2); remove then
pub(crate) const MIRRORED: &[u32] = &[field::COLOR];

#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))] // wired in by `App::edit_field` (plan 2b-1, Task 2); remove then
pub(crate) struct Mirror {
    nodes: HashMap<String, Fields>,
}

#[cfg_attr(not(test), allow(dead_code))] // wired in by `App::edit_field` (plan 2b-1, Task 2); remove then
impl Mirror {
    /// `id`'s value of field `no`; the field's absent value when the mirror has none.
    pub(crate) fn get(&self, id: &str, no: u32) -> FieldValue {
        match self.nodes.get(id).and_then(|fields| fields.get(&no)) {
            Some(value) => value.clone(),
            None => FieldValue::absent(field_def(no).expect("a known field").ty),
        }
    }

    pub(crate) fn set(&mut self, id: &str, no: u32, value: FieldValue) {
        debug_assert!(MIRRORED.contains(&no), "field {no} is not mirrored");
        self.nodes.entry(id.to_owned()).or_default().insert(no, value);
    }

    /// Gives `id` these values only if it has no entry yet (a spawn landing: a new tab takes what it was started
    /// with, an existing node keeps what the mirror already holds).
    pub(crate) fn init(&mut self, id: &str, fields: impl IntoIterator<Item = (u32, FieldValue)>) {
        if !self.nodes.contains_key(id) {
            for (no, value) in fields {
                self.set(id, no, value);
            }
        }
    }

    /// Drops the entries `keep` rejects.
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.nodes.retain(|id, _| keep(id));
    }

    pub(crate) fn color(&self, id: &str) -> Option<egui::Color32> {
        color_of(&self.get(id, field::COLOR))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_sync::color_value;

    #[test]
    fn an_unknown_node_reads_as_absent() {
        let mirror = Mirror::default();
        assert_eq!(mirror.get("7", field::COLOR), FieldValue::Color(None));
        assert_eq!(mirror.color("7"), None);
    }

    #[test]
    fn a_set_value_reads_back() {
        let mut mirror = Mirror::default();
        mirror.set("7", field::COLOR, color_value(Some(egui::Color32::RED)));
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
        mirror.set("7", field::COLOR, color_value(None));
        assert_eq!(mirror.color("7"), None);
    }

    #[test]
    fn init_never_overwrites_an_entry() {
        let mut mirror = Mirror::default();
        mirror.init("7", [(field::COLOR, color_value(Some(egui::Color32::RED)))]);
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
        mirror.init("7", [(field::COLOR, color_value(Some(egui::Color32::BLUE)))]);
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
    }

    #[test]
    fn retain_drops_rejected_entries() {
        let mut mirror = Mirror::default();
        mirror.set("a", field::COLOR, color_value(Some(egui::Color32::RED)));
        mirror.set("b", field::COLOR, color_value(Some(egui::Color32::RED)));
        mirror.retain(|id| id == "a");
        assert_eq!(mirror.color("a"), Some(egui::Color32::RED));
        assert_eq!(mirror.color("b"), None);
    }
}
