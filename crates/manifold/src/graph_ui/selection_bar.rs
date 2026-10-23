//! The selection bar: Pin, Group and Close for the selected cards, floating above their bounding box.

use super::{App, GraphAction};
use crate::card_id::CardId;
use crate::store::model::NodeId;
use crate::theme::bar_frame;

/// The bar's estimated height, to decide whether it fits above the selection before it is laid out.
const BAR_HEIGHT: f32 = 40.0;
/// Space between the bar and the selection.
const BAR_GAP: f32 = 8.0;

impl App {
    /// The selected stored cards' bounding box in world space, or `None` with nothing (stored) selected. A virtual
    /// card has no node to pin, group or close, so it does not count.
    fn selection_bounds(&self) -> Option<egui::Rect> {
        self.interaction
            .selected
            .iter()
            .filter(|id| id.node().is_some())
            .filter_map(|&id| self.level_card_rect(id))
            .reduce(|a, b| a.union(b))
    }

    /// How many cards inside the selected groups (at any depth) are not themselves selected: they close with the
    /// group (`GraphAction::CloseSelected`).
    pub(super) fn unselected_members_of_selection(&self) -> usize {
        let mut inside = std::collections::HashSet::new();
        let mut frontier: Vec<NodeId> = self.interaction.selected.iter().filter_map(|c| c.node()).collect();
        while let Some(group) = frontier.pop() {
            for member in self.members_of(group) {
                if inside.insert(member) {
                    frontier.push(member);
                }
            }
        }
        inside.iter().filter(|id| !self.interaction.selected.contains(&CardId::from(**id))).count()
    }

    /// Whether the Pin button would pin: any selected card is unpinned (the rule of `App::pin_shortcut`).
    fn selection_would_pin(&self) -> bool {
        self.interaction.selected.iter().filter_map(|c| c.node()).any(|id| !self.doc.mirror.pinned(id))
    }

    /// The bar above the selection (below it when there is no room above), in screen space through `project`, kept
    /// inside `panel`. Hidden on a JSON level, where nothing is selectable, and with nothing selected.
    pub(super) fn draw_selection_bar(
        &self,
        ui: &mut egui::Ui,
        actions: &mut Vec<GraphAction>,
        panel: egui::Rect,
        project: &impl Fn(egui::Pos2) -> egui::Pos2,
    ) {
        if self.json_level.is_some() {
            return;
        }
        let Some(bounds) = self.selection_bounds() else { return };
        let (min, max) = (project(bounds.min), project(bounds.max));
        let above = min.y - BAR_GAP - BAR_HEIGHT >= panel.top();
        let (pivot, y) = if above {
            (egui::Align2::CENTER_BOTTOM, min.y - BAR_GAP)
        } else {
            (egui::Align2::CENTER_TOP, (max.y + BAR_GAP).min(panel.bottom() - BAR_HEIGHT))
        };
        let x = ((min.x + max.x) / 2.0).clamp(panel.left() + 120.0, panel.right() - 120.0);
        let stored = self.selected_stored_count();
        let inside = self.unselected_members_of_selection();
        let pin_label = if self.selection_would_pin() { "Pin" } else { "Unpin" };
        egui::Area::new(egui::Id::new("selection_bar"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(egui::pos2(x, y))
            .show(ui.ctx(), |ui| {
                bar_frame(ui).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        crate::theme::menu_item_style(ui);
                        ui.label(egui::RichText::new(stored.to_string()).weak());
                        if ui
                            .button(format!("{} {pin_label}", egui_phosphor::light::PUSH_PIN))
                            .on_hover_text("Pinned cards stay where they are when the layout moves")
                            .clicked()
                        {
                            actions.push(GraphAction::PinSelected);
                        }
                        // Groups the selection into a new group at the level being viewed.
                        if ui.button(format!("{} Group", egui_phosphor::light::FOLDER_PLUS)).clicked() {
                            actions.push(GraphAction::GroupSelected);
                        }
                        let close = match inside {
                            0 => format!("{} Close", egui_phosphor::light::X),
                            n => format!("{} Close (+ {n} inside)", egui_phosphor::light::X),
                        };
                        if ui.button(close).on_hover_text("A group closes with everything in it").clicked() {
                            actions.push(GraphAction::CloseSelected);
                        }
                    });
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NotePayload, Runtime};

    /// The bar's Close names the cards that close with a selected group and are not selected themselves, at any depth.
    #[test]
    fn close_selected_counts_what_a_selected_group_takes_with_it() {
        use crate::fetched::Placement;
        let mut app = App::default();
        let top = Placement { group: None, pos: egui::Vec2::ZERO };
        use crate::card_id::testutil::node;
        let [outer, inner, a, b, c] = [1, 2, 3, 4, 5].map(node);
        app.place_group(Some(outer), "Outer", top);
        app.place_group(Some(inner), "Inner", Placement { group: Some(outer), pos: egui::Vec2::ZERO });
        for (id, group) in [(a, outer), (b, inner), (c, inner)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id, egui::Vec2::ZERO, crate::store::model::Kind::Note, note);
            app.edit_group(id, Some(group));
        }
        app.interaction.selected.insert(CardId::from(a));
        assert_eq!(app.unselected_members_of_selection(), 0, "a note holds nothing");
        app.interaction.selected.insert(CardId::from(outer));
        assert_eq!(app.unselected_members_of_selection(), 3, "inner, b and c; a is selected itself");
        app.interaction.selected.insert(CardId::from(c));
        assert_eq!(app.unselected_members_of_selection(), 2, "c is selected too");
    }
}
