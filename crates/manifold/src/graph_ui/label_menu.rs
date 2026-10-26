//! The selection bar's Label menu: every live label with a check for how much of the selection carries it, a name
//! field that makes a new one, and per-row rename, recolour and delete. It only emits `GraphAction`s.

use super::{App, GraphAction};
use crate::labels::LabelState;
use crate::node_menu::color_swatch_row;
use crate::popups::{clicked_outside_popup, menu_popup};
use crate::store::model::NodeId;

/// Where the bar drew its Label button this frame, which the menu hangs from.
pub(super) fn anchor_id() -> egui::Id {
    egui::Id::new("selection_label_anchor")
}

impl App {
    pub(super) fn draw_label_menu(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        if self.selected_stored_count() == 0 {
            self.interaction.label_menu = None;
        }
        let Some(mut menu) = self.interaction.label_menu.take() else { return };
        let Some(anchor) = ui.ctx().data(|d| d.get_temp::<egui::Rect>(anchor_id())) else { return };
        let rows: Vec<_> = self
            .label_defs()
            .into_iter()
            // name rule: the rows draw the name through the display rule and edit the stored one
            .map(|(id, def)| (id, def.name.clone(), def.color, self.label_state(id)))
            .collect();
        let area = menu_popup(
            ui.ctx(),
            egui::Area::new(egui::Id::new("label_menu")).fixed_pos(anchor.left_bottom() + egui::vec2(0.0, 4.0)),
            1.0,
            |ui| {
                ui.set_min_width(180.0);
                for (id, name, color, state) in rows {
                    label_row(ui, actions, &mut menu, id, &name, color, state);
                }
                let field =
                    ui.add(egui::TextEdit::singleline(&mut menu.draft).hint_text("New label").desired_width(160.0));
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !menu.draft.trim().is_empty()
                {
                    actions.push(GraphAction::NewLabel(std::mem::take(&mut menu.draft)));
                }
            },
        );
        let close =
            ui.input(|i| i.key_pressed(egui::Key::Escape)) || clicked_outside_popup(ui, area.rect, Some(anchor));
        self.interaction.label_menu = (!close).then_some(menu);
    }
}

#[allow(clippy::too_many_arguments, reason = "one row of the menu, its parts already split out of the label")]
fn label_row(
    ui: &mut egui::Ui,
    actions: &mut Vec<GraphAction>,
    menu: &mut crate::labels::LabelMenu,
    id: NodeId,
    name: &str,
    color: Option<egui::Color32>,
    state: LabelState,
) {
    let shown = crate::names::to_plain(&crate::names::display_name(name, "?"));
    if let Some((_, draft)) = menu.editing.as_mut().filter(|(edited, _)| *edited == id) {
        let field = ui.add(egui::TextEdit::singleline(draft).desired_width(160.0));
        field.request_focus();
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some(picked) = color_swatch_row(ui, color) {
            actions.push(GraphAction::RecolorLabel { label: id, color: picked });
        }
        if enter {
            actions.push(GraphAction::RenameLabel { label: id, name: std::mem::take(draft) });
        }
        if enter || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            menu.editing = None;
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
        ui.painter().circle_filled(row.rect.right_center() - egui::vec2(6.0, 0.0), 4.0, dot);
        if row.clicked() {
            menu.confirm_delete = None;
            actions.push(GraphAction::ToggleLabel { label: id, on: state != LabelState::All });
        }
        if ui.small_button(egui_phosphor::light::PENCIL_SIMPLE).on_hover_text("Rename or recolour").clicked() {
            menu.confirm_delete = None;
            menu.editing = Some((id, name.to_owned())); // name rule: the draft starts as the stored name
        }
        let armed = menu.confirm_delete == Some(id);
        let trash = if armed {
            egui::Button::new(egui::RichText::new("Delete?").color(ui.visuals().error_fg_color)).small()
        } else {
            egui::Button::new(egui_phosphor::light::TRASH).small()
        };
        if ui.add(trash).on_hover_text("Delete this label (click twice)").clicked() && menu.press_trash(id) {
            actions.push(GraphAction::DeleteLabel(id));
        }
    });
}
