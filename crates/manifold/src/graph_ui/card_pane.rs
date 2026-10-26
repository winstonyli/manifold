//! The card pane (card-pane spec): what the selected cards are, in the right panel beside the canvas. It shows state
//! and edits it with the actions the menus send (`GraphAction`, one card's `NodeAction`s as `GraphAction::Node`);
//! commands stay in the right-click menu and the selection bar.

use super::*;

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

    /// R2: when the pane starts describing another card, the camera brings it into view (`camera.reveal`, eased by
    /// `pan_to` only when it is not fully visible), unless a drag holds it: easing would move the world under the
    /// pointer.
    pub(super) fn track_pane_card(&mut self) {
        let card = self.pane_card();
        if card == self.interaction.pane_card {
            return;
        }
        self.interaction.pane_card = card;
        if let (Some(id), None) = (card, self.interaction.dragging) {
            self.camera.reveal = Some(id);
        }
    }

    /// The pane: the count for several cards, the card's name for one, then the Labels section. Task 4 adds the rest.
    pub(super) fn card_pane_contents(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        match self.pane_card() {
            Some(id) => {
                ui.strong(self.accessible_name(id).unwrap_or_default());
                self.pane_labels(ui, actions);
            }
            None => {
                ui.label(format!("{} selected", self.selected_stored_count()));
                self.pane_labels(ui, actions);
            }
        }
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
        let field = ui.add(egui::TextEdit::singleline(draft).desired_width(160.0));
        field.request_focus();
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some(picked) = crate::node_menu::color_swatch_row(ui, color) {
            actions.push(GraphAction::RecolorLabel { label: id, color: picked });
        }
        if enter {
            actions.push(GraphAction::RenameLabel { label: id, name: std::mem::take(draft) });
        }
        if enter || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            section.editing = None;
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
            section.confirm_delete = None;
            section.editing = Some((id, name.to_owned())); // name rule: the draft starts as the stored name
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
mod tests {
    use super::*;
    use crate::card_id::CardId;

    /// R2: the card the pane starts describing is revealed once; a second card, a drag or a second selected card is
    /// not.
    #[test]
    fn the_pane_reveals_the_card_it_starts_describing() {
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "a");
        let b = crate::session_sync::tests::add_note(&mut app, "b");
        app.interaction.selected.insert(CardId::from(a));
        app.track_pane_card();
        assert_eq!(app.camera.reveal, Some(a));
        app.camera.reveal = None;
        app.track_pane_card();
        assert_eq!(app.camera.reveal, None, "only when it changes");
        app.interaction.selected.insert(CardId::from(b));
        app.track_pane_card();
        assert_eq!((app.pane_card(), app.camera.reveal), (None, None), "two cards: nothing to reveal");
        app.interaction.selected.clear();
        app.track_pane_card();
        app.interaction.dragging = Some(CardId::from(b));
        app.interaction.selected.insert(CardId::from(b));
        app.track_pane_card();
        assert_eq!((app.pane_card(), app.camera.reveal), (Some(b), None), "a drag holds it: no ease");
    }

    /// The Labels section lists every label with how much of the selection carries it.
    #[test]
    fn the_label_rows_show_how_much_of_the_selection_carries_each() {
        use crate::labels::LabelState;
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "a");
        let b = crate::session_sync::tests::add_note(&mut app, "b");
        let urgent = app.create_label("urgent", None).unwrap();
        let later = app.create_label("later", None).unwrap();
        app.interaction.selected.extend([CardId::from(a), CardId::from(b)]);
        app.label_selection(urgent, true);
        app.interaction.selected.remove(&CardId::from(b));
        let states = |app: &App| -> Vec<(NodeId, LabelState)> {
            app.pane_label_rows().into_iter().map(|(id, _, _, state)| (id, state)).collect()
        };
        assert!(states(&app).contains(&(urgent, LabelState::All)));
        app.interaction.selected.insert(CardId::from(b));
        app.set_label(&[b], urgent, false);
        assert!(states(&app).contains(&(urgent, LabelState::Some)));
        assert!(states(&app).contains(&(later, LabelState::None)));
    }

    /// A label rename in progress is dropped with the pane, not carried to the next selection.
    #[test]
    fn the_label_section_resets_when_the_pane_closes() {
        let mut app = App::default();
        let a = crate::session_sync::tests::add_note(&mut app, "a");
        let urgent = app.create_label("urgent", None).unwrap();
        app.interaction.selected.insert(CardId::from(a));
        app.interaction.label_section.editing = Some((urgent, "half-typed".into()));
        app.interaction.label_section.draft = "new".into();
        let ctx = crate::theme::testutil::ctx(true);
        let frame = |app: &mut App| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.build_right_panel(ui, &mut actions));
            output.textures_delta.clear(); // nothing uploads the font atlas here
        };
        frame(&mut app);
        assert!(app.interaction.label_section.editing.is_some(), "kept while the pane shows");
        app.interaction.selected.clear();
        frame(&mut app);
        assert!(app.interaction.label_section.editing.is_none() && app.interaction.label_section.draft.is_empty());
    }
}
