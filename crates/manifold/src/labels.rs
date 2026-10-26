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

/// The card pane's Labels section: the name being typed for a new label, and the label whose row is being edited (its
/// draft name).
#[derive(Debug, Default)]
pub(crate) struct LabelSection {
    pub(crate) draft: String,
    pub(crate) editing: Option<(NodeId, String)>,
    /// The label whose trash button was pressed once: the next press on it deletes.
    pub(crate) confirm_delete: Option<NodeId>,
    /// The rename field takes the keys on its first frame only, so a click away can take them back.
    pub(crate) focus_pending: bool,
}

impl LabelSection {
    /// Starts renaming label `id`, its draft the stored `name`.
    pub(crate) fn start_editing(&mut self, id: NodeId, name: &str) {
        self.confirm_delete = None;
        self.editing = Some((id, name.to_owned()));
        self.focus_pending = true;
    }

    /// The trash button of label `id` was pressed: true when that confirms a delete, else it arms the button.
    pub(crate) fn press_trash(&mut self, id: NodeId) -> bool {
        if self.confirm_delete == Some(id) {
            self.confirm_delete = None;
            return true;
        }
        self.confirm_delete = Some(id);
        false
    }
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
    /// The live labels, one per name (the lowest id of those sharing it, `Mirror::canonical_label`), by id.
    pub(crate) fn label_defs(&self) -> BTreeMap<NodeId, LabelDef> {
        self.doc
            .mirror
            .stored_ids(Kind::Label)
            .filter(|&id| self.doc.mirror.canonical_label(id) == id)
            .map(|id| (id, self.doc.mirror.label_def(id)))
            .collect()
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

    /// The Labels section's name field: makes (or finds) the label and puts it on the selection.
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

    /// Every live label's id by name key (`names::name_key`), skipping labels whose key is empty. An owned map, so a
    /// caller can match names while it also borrows the app mutably.
    pub(crate) fn label_keys(&self) -> std::collections::HashMap<String, NodeId> {
        self.label_defs().into_iter().map(|(id, d)| (name_key(&d.name), id)).filter(|(k, _)| !k.is_empty()).collect()
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

    /// `rename_label` from the menu: a refusal says why in a toast.
    pub(crate) fn rename_label_or_toast(&mut self, id: NodeId, name: &str) {
        if self.rename_label(id, name) {
            return;
        }
        let cleaned = clean_name(name);
        self.queue_toast(if cleaned.is_empty() {
            "A label needs a name".to_string()
        } else {
            format!("A label named \"{cleaned}\" already exists")
        });
    }

    /// The labels that read as `id`: it and every label with the same name key (`Mirror::label_members`).
    fn label_group_of(&self, id: NodeId) -> Vec<NodeId> {
        self.doc.mirror.label_members(self.doc.mirror.canonical_label(id))
    }

    /// Renames label `id` and every label that reads as it; false for a name that is empty once cleaned or that
    /// another label has.
    pub(crate) fn rename_label(&mut self, id: NodeId, name: &str) -> bool {
        let name = clean_name(name).into_owned();
        let canonical = self.doc.mirror.canonical_label(id);
        if name.is_empty() || self.label_named(&name, Some(canonical)).is_some() {
            return false;
        }
        for member in self.label_group_of(id) {
            self.edit_field(member, field::NAME, FieldValue::Str(name.clone()));
        }
        true
    }

    /// Recolours label `id` and every label that reads as it.
    pub(crate) fn recolor_label(&mut self, id: NodeId, color: Option<egui::Color32>) {
        for member in self.label_group_of(id) {
            self.edit_field(member, field::COLOR, color_value(color));
        }
    }

    /// A millisecond stamp for a new membership: the wall clock, but always past the last one handed out, so two adds
    /// in one millisecond still order (`Mirror::card_labels`).
    fn next_label_ms(&mut self) -> u64 {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.doc.label_ms = now.max(self.doc.label_ms + 1);
        self.doc.label_ms
    }

    /// Puts `label` on each of `cards` (`on`) or takes it off. A card already in that state writes nothing; a card
    /// carrying any label that reads as `label` has it. Off removes every such membership. Returns the edits made; the
    /// caller records the undo step (`record_label_undo`).
    pub(crate) fn set_label(&mut self, cards: &[NodeId], label: NodeId, on: bool) -> Vec<PropEdit> {
        if !self.doc.mirror.is_stored(label) {
            return Vec::new();
        }
        let members = self.label_group_of(label);
        let canonical = self.doc.mirror.canonical_label(label);
        let mut edits = Vec::new();
        for &card in cards {
            let held: Vec<TreePath> = {
                let tree = self.doc.mirror.tree(card, field::PROPS);
                let paths = members.iter().map(|&m| labels_path(m));
                paths.filter(|p| tree.is_some_and(|t| t.contains_key(p))).collect()
            };
            if held.is_empty() != on || !self.doc.mirror.has(card) {
                continue;
            }
            if on {
                let leaf = Some(Leaf::Int(self.next_label_ms() as i64));
                edits.extend(self.edit_prop(card, field::PROPS, labels_path(canonical), leaf));
            } else {
                for path in held {
                    edits.extend(self.edit_prop(card, field::PROPS, path, None));
                }
            }
        }
        edits
    }

    /// One undo step for `edits` from `set_label`; nothing for none.
    pub(crate) fn record_label_undo(&mut self, message: &str, edits: Vec<PropEdit>) {
        if !edits.is_empty() {
            self.record_undo(message.to_owned(), UndoKind::Props(edits));
        }
    }

    /// Deletes label `id` and every label that reads as it, as one undo step (`UndoKind::LabelDelete`).
    pub(crate) fn delete_label(&mut self, id: NodeId) {
        let def = self.doc.mirror.label_def(self.doc.mirror.canonical_label(id));
        let carriers = self.label_carriers(id);
        self.delete_label_group(id);
        let shown = crate::names::to_plain(&crate::names::display_name(&def.name, "label"));
        let message = format!("Deleted label \"{shown}\"");
        // name rule: the stored name, kept to make the label again
        let name = def.name;
        self.record_undo(message, UndoKind::LabelDelete { label: None, name, color: def.color, carriers });
    }

    /// Forgets label `id` and every label that reads as it, and queues their deletes. Cards keep their membership
    /// entries, which show nothing once the labels are gone (`Mirror::card_labels`).
    pub(crate) fn delete_label_group(&mut self, id: NodeId) {
        for member in self.label_group_of(id) {
            self.doc.mirror.forget_stored(member);
            self.queue_delete(member);
        }
    }

    /// The cards carrying label `id` (or one that reads as it), each with its earliest membership stamp, by card id.
    fn label_carriers(&self, id: NodeId) -> Vec<(NodeId, i64)> {
        let members = self.label_group_of(id);
        let mut out: Vec<(NodeId, i64)> = self
            .nodes
            .keys()
            .filter_map(|&card| {
                let tree = self.doc.mirror.tree(card, field::PROPS)?;
                let at = tree
                    .iter()
                    .filter_map(|(path, leaf)| match (path.segments().as_slice(), leaf) {
                        ([Seg::Key(k), Seg::Id(l)], Leaf::Int(at)) if k == LABELS_KEY && members.contains(l) => {
                            Some(*at)
                        }
                        _ => None,
                    })
                    .min()?;
                Some((card, at))
            })
            .collect();
        out.sort_unstable();
        out
    }
}
