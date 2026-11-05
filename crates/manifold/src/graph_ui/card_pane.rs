//! The card pane (card-pane spec): what the selected cards are, in the right panel beside the canvas. It shows state
//! and edits it with the actions the menus send (`GraphAction`, one card's `NodeAction`s as `GraphAction::Node`);
//! commands stay in the right-click menu and the selection bar.

use super::*;
use crate::node_menu::{NodeAction, NodeMenuItem};

/// The Text section's height before it scrolls.
const TEXT_MAX_HEIGHT: f32 = 240.0;

impl App {
    /// The one stored card the pane describes: the stored selection when it is a single card still shown, on a
    /// level that is not a JSON level.
    pub(crate) fn pane_card(&self) -> Option<NodeId> {
        let mut stored = self.interaction.selected.iter().filter_map(|c| c.node());
        match (stored.next(), stored.next()) {
            (Some(id), None) if self.json_level.is_none() && self.nodes.contains_key(&id) => Some(id),
            _ => None,
        }
    }

    /// When the pane starts describing another card, the camera brings it into view (`camera.reveal`, eased by
    /// `pan_to` only when it is not fully visible; never during a drag, `apply_camera_requests`).
    pub(super) fn track_pane_card(&mut self) {
        let card = self.pane_card();
        if card == self.interaction.pane_card {
            return;
        }
        self.interaction.pane_card = card;
        if let Some(id) = card {
            self.camera.reveal = Some(id);
        }
    }

    /// The pane. One card: header, labels, place, links, text. Several: the count and the labels. Its rows step with
    /// Up/Down while one has focus, and never take focus themselves.
    pub(super) fn card_pane_contents(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.pane_card() {
            Some(id) => {
                self.pane_header(ui, actions, id);
                self.pane_labels(ui, actions);
                self.pane_place(ui, actions, id);
                self.pane_text_section(ui, id);
            }
            None => {
                ui.label(format!("{} selected", self.selected_stored_count()));
                self.pane_labels(ui, actions);
            }
        });
        let rows = crate::popups::take_list_rows(ui);
        crate::popups::step_focused_row(ui, &rows.rows, rows.partial, crate::popups::RowKeys::Arrows);
    }

    /// Whether the pane shows the rename field for `id`: the one draft (`renaming_group`) is this card's and its
    /// menu, which would show the same field, is not open on it.
    pub(crate) fn pane_renaming(&self, id: NodeId) -> bool {
        self.renaming_group.as_ref().is_some_and(|(of, _)| *of == id)
            && self.menus.node_menu.open.as_ref().map(|(of, _)| *of) != Some(id)
    }

    /// The card's name as a screen reader says it, a pencil that starts a rename (the menu's `Rename`), and its URL
    /// or path, selectable for copying.
    fn pane_header(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>, id: NodeId) {
        if self.pane_renaming(id) {
            let focus = crate::popups::take_focus(&mut self.pending_focus, crate::popups::FocusTarget::GroupRename);
            let Some((_, draft)) = self.renaming_group.as_mut() else { return };
            let field = crate::theme::toolbar_field(ui, |ui| {
                ui.add(crate::theme::field_edit(draft).desired_width(f32::INFINITY))
            });
            if focus {
                field.request_focus();
            }
            if field.lost_focus() {
                let action = if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    NodeAction::CancelRename
                } else {
                    NodeAction::CommitRename(draft.clone())
                };
                actions.push(GraphAction::Node(id, action));
            }
        } else {
            ui.horizontal(|ui| {
                let name = egui::RichText::new(self.accessible_name(id).unwrap_or_default()).strong();
                ui.add(egui::Label::new(name).wrap());
                if ui.small_button(egui_phosphor::light::PENCIL_SIMPLE).on_hover_text("Rename").clicked() {
                    actions.push(GraphAction::Node(id, NodeAction::Rename));
                }
            });
        }
        if let Some(url) = self.copyable_url(id) {
            ui.add(egui::Label::new(egui::RichText::new(url).small().weak()).selectable(true).wrap());
        }
    }

    /// Where the card sits: its group (with Remove from Group), a pinned mark, its jar rows, its links rows. The jar
    /// and links rows are the menu's own items (`draw_menu_item`), so the two never list them differently.
    fn pane_place(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>, id: NodeId) {
        let mut clicked = None;
        if let Some(group) = self.doc.mirror.group(id) {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let name = self.accessible_name(group).unwrap_or_default();
                ui.label(egui::RichText::new(format!("{} {name}", egui_phosphor::light::FOLDER)).weak());
                if ui.small_button(egui_phosphor::light::X).on_hover_text("Remove from Group").clicked() {
                    clicked = Some(NodeAction::RemoveFromGroup);
                }
            });
        }
        if self.doc.mirror.pinned(id) {
            ui.label(egui::RichText::new(format!("{} Pinned", egui_phosphor::light::PUSH_PIN)).weak());
        }
        if let Some(jars) = self.jar_menu(id) {
            crate::node_menu::draw_menu_item(ui, &NodeMenuItem::Jars { heading: "Jar", rows: jars.rows }, &mut clicked);
        }
        let edges = self.edge_menu(id);
        if !edges.is_empty() {
            crate::node_menu::draw_menu_item(ui, &NodeMenuItem::Edges(edges), &mut clicked);
        }
        if let Some(action) = clicked {
            actions.push(GraphAction::Node(id, action));
        }
    }

    /// The text the pane shows for `id`: a note's text (`note_text`), else the page text last stored for it
    /// (`search::text_store`); none when both are empty.
    pub(crate) fn pane_text(&self, id: NodeId) -> Option<&str> {
        let stored = || self.search.texts.get(id).map_or("", |r| r.text.as_str());
        Some(self.note_text(id)).filter(|t| !t.trim().is_empty()).or(Some(stored())).filter(|t| !t.trim().is_empty())
    }

    /// The Text section: read-only, capped in height, scrolled.
    fn pane_text_section(&self, ui: &mut egui::Ui, id: NodeId) {
        let Some(text) = self.pane_text(id) else { return };
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Text").small().weak());
        egui::ScrollArea::vertical().id_salt("card_pane_text").max_height(TEXT_MAX_HEIGHT).show(ui, |ui| {
            ui.add(egui::Label::new(text).selectable(true).wrap());
        });
    }

    /// The Labels section's rows: every live label, its stored name and colour, and how much of the selection carries
    /// it.
    pub(crate) fn pane_label_rows(&self) -> Vec<(NodeId, String, Option<egui::Color32>, crate::labels::LabelState)> {
        // name rule: the rows draw the name through the display rule and edit the stored one
        self.label_defs().into_iter().map(|(id, def)| (id, def.name, def.color, self.label_state(id))).collect()
    }

    /// The Labels section: a row per label (tick, rename and recolour, delete on the second press), then the field
    /// that makes a new label and puts it on the selection.
    fn pane_labels(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Labels").small().weak());
        let rows = self.pane_label_rows();
        let section = &mut self.interaction.label_section;
        for (id, name, color, state) in rows {
            label_row(ui, actions, section, id, &name, color, state);
        }
        let field = ui.add(egui::TextEdit::singleline(&mut section.draft).hint_text("New label").desired_width(160.0));
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !section.draft.trim().is_empty() {
            actions.push(GraphAction::NewLabel(std::mem::take(&mut section.draft)));
        }
    }
}

#[allow(clippy::too_many_arguments, reason = "one row of the section, its parts already split out of the label")]
fn label_row(
    ui: &mut egui::Ui,
    actions: &mut Vec<GraphAction>,
    section: &mut crate::labels::LabelSection,
    id: NodeId,
    name: &str,
    color: Option<egui::Color32>,
    state: crate::labels::LabelState,
) {
    use crate::labels::LabelState;
    let shown = crate::names::to_plain(&crate::names::display_name(name, "?"));
    if let Some((_, draft)) = section.editing.as_mut().filter(|(edited, _)| *edited == id) {
        let edit = ui.vertical(|ui| {
            let field = ui.add(egui::TextEdit::singleline(draft).desired_width(160.0));
            if std::mem::take(&mut section.focus_pending) {
                field.request_focus();
            }
            if let Some(picked) = crate::node_menu::color_swatch_row(ui, color) {
                actions.push(GraphAction::RecolorLabel { label: id, color: picked });
            }
            field
        });
        let field = edit.inner;
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter {
            actions.push(GraphAction::RenameLabel { label: id, name: std::mem::take(draft) });
        }
        // Escape, Tab or a click outside the field and its swatches cancels: the pane stays open, so a rename that
        // outlived its focus would keep the keys from the graph. A click inside (a swatch) hands the keys back. egui
        // surrenders focus on the click's release, so test the pointer's position, not the press origin (gone by then).
        let click_inside = ui.input(|i| {
            (i.pointer.any_pressed() || i.pointer.any_released())
                && i.pointer.interact_pos().is_some_and(|p| edit.response.rect.contains(p))
        });
        if enter {
            section.editing = None;
        } else if field.lost_focus() {
            if click_inside {
                section.focus_pending = true;
            } else {
                section.editing = None;
            }
        }
        return;
    }
    ui.horizontal(|ui| {
        let mark = match state {
            LabelState::All => egui_phosphor::light::CHECK_SQUARE,
            LabelState::Some => egui_phosphor::light::MINUS_SQUARE,
            LabelState::None => egui_phosphor::light::SQUARE,
        };
        let dot = color.map_or(ui.visuals().weak_text_color(), |c| crate::theme::tag_shown(c, ui.visuals().dark_mode));
        let row = ui.button(format!("{mark} {shown}"));
        crate::popups::nav_row(ui, &row);
        ui.painter().circle_filled(row.rect.right_center() - egui::vec2(6.0, 0.0), 4.0, dot);
        if row.clicked() {
            section.confirm_delete = None;
            actions.push(GraphAction::ToggleLabel { label: id, on: state != LabelState::All });
        }
        if ui.small_button(egui_phosphor::light::PENCIL_SIMPLE).on_hover_text("Rename or recolour").clicked() {
            section.start_editing(id, name); // name rule: the draft starts as the stored name
        }
        let armed = section.confirm_delete == Some(id);
        let trash = if armed {
            egui::Button::new(egui::RichText::new("Delete?").color(ui.visuals().error_fg_color)).small()
        } else {
            egui::Button::new(egui_phosphor::light::TRASH).small()
        };
        if ui.add(trash).on_hover_text("Delete this label (click twice)").clicked() && section.press_trash(id) {
            actions.push(GraphAction::DeleteLabel(id));
        }
    });
}

#[cfg(test)]
mod tests;
