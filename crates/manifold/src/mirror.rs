//! The app's decoded copy of the document's fields that have moved off `GraphNode` and its payloads (node model
//! spec, 2b "Read model"). Every read goes through the accessors here, so making `Doc` the app's own state later
//! changes only where they read.
//!
//! The mirror equals the document except for fields and links with an edit still waiting for a save
//! (`App::pending`, `App::pending_moves`); `App::edit_field`, `App::edit_parent` and `App::edit_group` are the
//! writers, and `App::mirror_remote` applies another replica's change.

use std::collections::HashMap;

use crate::session_sync::{color_of, color_value, Fields};
use crate::store::model::{field, field_def, FieldValue};

/// The fields the mirror holds. `diff` leaves them to `App::pending` on existing nodes. `URL` is mirrored for every
/// kind that has it, but only tabs seed and read it; content kinds read their payload's until 2b-5 (2b-3 Ruling 2).
pub(crate) const MIRRORED: &[u32] = &[field::COLOR, field::PINNED, field::URL];

#[derive(Default)]
pub(crate) struct Mirror {
    nodes: HashMap<String, Fields>,
    links: HashMap<String, Links>,
}

/// A node's links to other nodes, as ids: `Option<String>` so hand-built test ids work and per-frame filters
/// compare without allocating.
#[derive(Default)]
struct Links {
    parent: Option<String>,
    group: Option<String>,
}

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

    /// The node this one was opened from (`None` for a root).
    pub(crate) fn parent(&self, id: &str) -> Option<&str> {
        self.links.get(id)?.parent.as_deref()
    }

    pub(crate) fn set_parent(&mut self, id: &str, parent: Option<String>) {
        self.links.entry(id.to_owned()).or_default().parent = parent;
    }

    /// The group this node is shown in (`None` at the top level). Groups nest, so a group has one too.
    pub(crate) fn group(&self, id: &str) -> Option<&str> {
        self.links.get(id)?.group.as_deref()
    }

    pub(crate) fn set_group(&mut self, id: &str, group: Option<String>) {
        self.links.entry(id.to_owned()).or_default().group = group;
    }

    /// Drops the entries `keep` rejects.
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.nodes.retain(|id, _| keep(id));
        self.links.retain(|id, _| keep(id));
    }

    pub(crate) fn color(&self, id: &str) -> Option<egui::Color32> {
        color_of(&self.get(id, field::COLOR))
    }

    /// Graph view's pin toggle (tabs only): the force-directed layout never moves a pinned tab, as it never moves
    /// a dragged node (`graph_layout::step`); it still pushes its neighbours. Only tabs carry the field.
    pub(crate) fn pinned(&self, id: &str) -> bool {
        matches!(self.get(id, field::PINNED), FieldValue::Bool(true))
    }

    /// A tab's shared url: the document's, or the one a local edit is about to write (Ruling 1). Its live url, which
    /// every engine report moves, is `TabPayload::url` / `UnloadedTabPayload::url`. "" when the mirror has none.
    pub(crate) fn url(&self, id: &str) -> String {
        match self.get(id, field::URL) {
            FieldValue::Str(url) => url,
            _ => String::new(),
        }
    }

    pub(crate) fn carried(&self, id: &str) -> Carried {
        Carried {
            color: self.color(id),
            pinned: self.pinned(id),
            parent: self.parent(id).map(str::to_owned),
            group: self.group(id).map(str::to_owned),
            url: self.url(id),
        }
    }

    /// `init` for everything a tab carries. An empty `url` (any node but a tab) is the field's absent value, so it
    /// is left out.
    pub(crate) fn seed(&mut self, id: &str, carried: &Carried) {
        let url = (!carried.url.is_empty()).then(|| (field::URL, FieldValue::Str(carried.url.clone())));
        let fields = [(field::COLOR, color_value(carried.color)), (field::PINNED, FieldValue::Bool(carried.pinned))];
        self.init(id, fields.into_iter().chain(url));
        if !self.links.contains_key(id) {
            let links = Links { parent: carried.parent.clone(), group: carried.group.clone() };
            self.links.insert(id.to_owned(), links);
        }
    }
}

/// What a tab carries across a respawn or lands with: the mirror's values for it. `place_node` seeds a new node's
/// links with it too.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Carried {
    pub color: Option<egui::Color32>,
    pub pinned: bool,
    pub parent: Option<String>,
    pub group: Option<String>,
    /// The shared url (`Mirror::url`); "" for a node that isn't a tab.
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_then_carried_round_trips_and_never_overwrites() {
        let mut mirror = Mirror::default();
        let first = Carried {
            color: Some(egui::Color32::RED),
            pinned: true,
            parent: Some("p".into()),
            group: Some("g".into()),
            url: "https://a/".into(),
        };
        mirror.seed("7", &first);
        assert_eq!(mirror.carried("7"), first);
        mirror.seed("7", &Carried { color: None, pinned: false, parent: None, group: None, url: "https://b/".into() });
        assert_eq!(mirror.carried("7"), first);
    }

    #[test]
    fn a_url_reads_back_and_a_node_without_one_has_none() {
        let mut mirror = Mirror::default();
        assert_eq!(mirror.url("7"), "");
        mirror.seed("8", &Carried::default());
        assert!(!mirror.nodes["8"].contains_key(&field::URL), "an empty url seeds nothing");
        mirror.set("7", field::URL, FieldValue::Str("https://a/".into()));
        assert_eq!(mirror.url("7"), "https://a/");
    }

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

    #[test]
    fn pinned_reads_a_bool_and_defaults_to_false() {
        let mut mirror = Mirror::default();
        assert!(!mirror.pinned("7"));
        mirror.set("7", field::PINNED, FieldValue::Bool(true));
        assert!(mirror.pinned("7"));
    }
}
