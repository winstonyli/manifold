//! The selection bar: Pin, Group and Close for the selected cards, floating above their bounding box.

use super::{App, GraphAction};
use crate::card_id::CardId;
use crate::store::model::NodeId;
use crate::theme::{accent, bar_frame, chip_icon_button, NamedTip};

/// The bar's estimated height, to decide whether it fits above the selection before it is laid out.
const BAR_HEIGHT: f32 = 40.0;
/// Space between the bar and the selection's box.
const BAR_GAP: f32 = 8.0;
/// Screen points between the selected cards and the box drawn around two or more of them.
const BOX_PAD: f32 = 10.0;

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
        // One selected card already has its own ring; two or more get a box around the lot, which the bar sits on.
        let boxed = self.selected_stored_count() > 1;
        let (min, max) =
            if boxed { (min - egui::Vec2::splat(BOX_PAD), max + egui::Vec2::splat(BOX_PAD)) } else { (min, max) };
        if boxed {
            let accent = crate::theme::accent(ui.visuals());
            let painter =
                ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("selection_box")));
            let rect = egui::Rect::from_min_max(min, max);
            // Outline only: a fill over the cards would dim them.
            painter.with_clip_rect(panel).rect_stroke(
                rect,
                crate::theme::CARD_RADIUS + BOX_PAD,
                egui::Stroke::new(1.5, accent),
                egui::StrokeKind::Outside,
            );
        }
        let above = min.y - BAR_GAP - BAR_HEIGHT >= panel.top();
        let (pivot, y) = if above {
            (egui::Align2::CENTER_BOTTOM, min.y - BAR_GAP)
        } else {
            (egui::Align2::CENTER_TOP, (max.y + BAR_GAP).min(panel.bottom() - BAR_HEIGHT))
        };
        let x = ((min.x + max.x) / 2.0).clamp(panel.left() + 90.0, panel.right() - 90.0);
        let stored = self.selected_stored_count();
        let inside = self.unselected_members_of_selection();
        egui::Area::new(egui::Id::new("selection_bar"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(egui::pos2(x, y))
            .show(ui.ctx(), |ui| {
                bar_frame(ui).inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Joined like the toolbar's chips: the buttons touch, and the bar's own padding is the only gap.
                        ui.spacing_mut().item_spacing.x = 0.0;
                        // The toolbar's button size: the app-wide padding makes a button 32 px tall, so the 26 px
                        // count slot sat 3 px above the buttons' centre line.
                        ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                        ui.spacing_mut().interact_size.y = 26.0;
                        // How many are selected: a round chip, its number centred in it by hand (a framed label sat
                        // low against the buttons).
                        let (rect, chip) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 11.0, ui.visuals().widgets.inactive.weak_bg_fill);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            stored.to_string(),
                            egui::TextStyle::Button.resolve(ui.style()),
                            ui.visuals().weak_text_color(),
                        );
                        chip.on_hover_text(format!("{stored} selected"));
                        // The glyph is accented once every selected card is pinned (a click then unpins).
                        let pinned = !self.selection_would_pin();
                        let pin = ui
                            .scope(|ui| {
                                if pinned {
                                    let accent = accent(ui.visuals());
                                    ui.visuals_mut().widgets.inactive.fg_stroke.color = accent;
                                    ui.visuals_mut().widgets.hovered.fg_stroke.color = accent;
                                }
                                chip_icon_button(ui, egui::Id::new("selection_pin"), egui_phosphor::light::PUSH_PIN)
                            })
                            .inner;
                        let pin_tip = if pinned { "Unpin" } else { "Pin: keep these cards where they are" };
                        if pin.named_tip(pin_tip).clicked() {
                            actions.push(GraphAction::PinSelected);
                        }
                        // Groups the selection into a new group at the level being viewed.
                        let group =
                            chip_icon_button(ui, egui::Id::new("selection_group"), egui_phosphor::light::FOLDER_PLUS);
                        if group.named_tip("Group").clicked() {
                            actions.push(GraphAction::GroupSelected);
                        }
                        let close = chip_icon_button(ui, egui::Id::new("selection_close"), egui_phosphor::light::X);
                        let close_rect = close.rect;
                        let tip = match inside {
                            0 => "Close".to_owned(),
                            n => format!("Close (a group closes with its {n} more inside)"),
                        };
                        if close.named_tip(tip).clicked() {
                            actions.push(GraphAction::CloseSelected);
                        }
                        // What a selected group takes with it, as a badge on the X.
                        if inside > 0 {
                            let center = close_rect.right_top() + egui::vec2(-4.0, 4.0);
                            let painter = ui.painter();
                            painter.circle_filled(center, 7.0, accent(ui.visuals()));
                            painter.text(
                                center,
                                egui::Align2::CENTER_CENTER,
                                format!("+{}", inside.min(9)),
                                egui::FontId::proportional(8.0),
                                egui::Color32::WHITE,
                            );
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
