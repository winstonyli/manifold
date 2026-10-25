//! Labels: stored `Kind::Label` nodes a card can carry by name and colour (`PROPS/labels`). A label is mirror and
//! store state, never a graph card, like a link (`links.rs`).

use std::collections::BTreeMap;

use crate::names::{clean_name, name_key};
use crate::session_sync::color_value;
use crate::store::model::{field, FieldValue, Kind, NodeId};
use crate::store::tree::{Leaf, Seg, TreePath};
use crate::undo::{PropEdit, UndoKind};
use crate::App;

/// A label's synced definition, decoded from its mirrored fields (`Mirror::label_def`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LabelDef {
    pub(crate) name: String,
    pub(crate) color: Option<egui::Color32>,
}

/// The selection bar's Label menu while it is open: the name being typed for a new label, and the label whose row is
/// being edited (its draft name).
#[derive(Debug, Default)]
pub(crate) struct LabelMenu {
    pub(crate) draft: String,
    pub(crate) editing: Option<(NodeId, String)>,
}

/// How much of the selection carries a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelState {
    All,
    Some,
    None,
}

/// The key under `PROPS` that holds a card's label memberships: `PROPS/labels/@<label id>` is `Int(added_at_ms)`.
pub(crate) const LABELS_KEY: &str = "labels";

/// Where `card` records that it carries `label`.
pub(crate) fn labels_path(label: NodeId) -> TreePath {
    TreePath::default().child(&Seg::Key(LABELS_KEY.to_owned())).child(&Seg::Id(label))
}

impl App {
    /// The live labels, by id.
    pub(crate) fn label_defs(&self) -> BTreeMap<NodeId, LabelDef> {
        self.doc.mirror.stored_ids(Kind::Label).map(|id| (id, self.doc.mirror.label_def(id))).collect()
    }

    /// The names of `card`'s live labels, oldest first.
    pub(crate) fn label_names(&self, card: NodeId) -> Vec<String> {
        // name rule: the stored names, for matching (the filter and the index hold stored text)
        self.doc.mirror.card_labels(card).into_iter().map(|l| self.doc.mirror.label_def(l).name).collect()
    }

    /// The selected stored cards, in id order.
    fn selected_cards(&self) -> Vec<NodeId> {
        let mut cards: Vec<NodeId> = self.interaction.selected.iter().filter_map(|c| c.node()).collect();
        cards.sort_unstable();
        cards
    }

    /// Whether all, some or none of the selected cards carry `label`; none when nothing is selected.
    pub(crate) fn label_state(&self, label: NodeId) -> LabelState {
        let cards = self.selected_cards();
        let with = cards.iter().filter(|&&c| self.doc.mirror.card_labels(c).contains(&label)).count();
        match (with, cards.len()) {
            (0, _) => LabelState::None,
            (w, n) if w == n => LabelState::All,
            _ => LabelState::Some,
        }
    }

    /// Puts `label` on the selected cards (`on`) or takes it off, as one undo step; nothing when none would change.
    pub(crate) fn label_selection(&mut self, label: NodeId, on: bool) {
        let edits = self.set_label(&self.selected_cards(), label, on);
        let (verb, n) = (if on { "Label" } else { "Unlabel" }, edits.len());
        self.record_label_undo(&format!("{verb} {n} card{}", if n == 1 { "" } else { "s" }), edits);
    }

    /// The Label menu's name field: makes (or finds) the label and puts it on the selection.
    pub(crate) fn new_label_on_selection(&mut self, name: &str) {
        if let Some(label) = self.create_label(name, None) {
            self.label_selection(label, true);
        }
    }

    /// The live label whose name has the same key as `name` (`names::name_key`), other than `except`.
    pub(crate) fn label_named(&self, name: &str, except: Option<NodeId>) -> Option<NodeId> {
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

    /// The Label menu's open state, toggled by the bar's button.
    pub(crate) fn toggle_label_menu(&mut self) {
        self.interaction.label_menu =
            if self.interaction.label_menu.is_some() { None } else { Some(LabelMenu::default()) };
    }

    pub(crate) fn recolor_label(&mut self, id: NodeId, color: Option<egui::Color32>) {
        self.edit_field(id, field::COLOR, color_value(color));
    }

    /// A millisecond stamp for a new membership: the wall clock, but always past the last one handed out, so two adds
    /// in one millisecond still order (`Mirror::card_labels`).
    fn next_label_ms(&mut self) -> u64 {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.doc.label_ms = now.max(self.doc.label_ms + 1);
        self.doc.label_ms
    }

    /// Puts `label` on each of `cards` (`on`) or takes it off. A card already in that state writes nothing. Returns
    /// the edits made; the caller records the undo step (`record_label_undo`).
    pub(crate) fn set_label(&mut self, cards: &[NodeId], label: NodeId, on: bool) -> Vec<PropEdit> {
        if !self.doc.mirror.is_stored(label) {
            return Vec::new();
        }
        let path = labels_path(label);
        let mut edits = Vec::new();
        for &card in cards {
            let has = self.doc.mirror.tree(card, field::PROPS).is_some_and(|t| t.contains_key(&path));
            if has == on || !self.doc.mirror.has(card) {
                continue;
            }
            let leaf = on.then(|| Leaf::Int(self.next_label_ms() as i64));
            edits.extend(self.edit_prop(card, field::PROPS, path.clone(), leaf));
        }
        edits
    }

    /// One undo step for `edits` from `set_label`; nothing for none.
    pub(crate) fn record_label_undo(&mut self, message: &str, edits: Vec<PropEdit>) {
        if !edits.is_empty() {
            self.record_undo(message.to_owned(), UndoKind::Props(edits));
        }
    }

    /// Forgets label `id` and queues its delete. Cards keep their membership entries, which show nothing once the
    /// label is gone (`Mirror::card_labels`).
    pub(crate) fn delete_label(&mut self, id: NodeId) {
        self.doc.mirror.forget_stored(id);
        self.queue_delete(id);
    }
}
