//! Suggested groups on the canvas: a coloured ring round each member and, above the lot, a chip with the label and
//! Accept and Dismiss (`App::suggest_groups`).

use super::App;
use crate::theme::{bar_frame, chip_icon_button, NamedTip};

/// Space between a chip and the members' box.
const CHIP_GAP: f32 = 8.0;
/// Screen points between a member card and its ring.
const RING_PAD: f32 = 3.0;

/// Suggestion `i`'s colour: hues spread by the golden ratio so neighbours differ.
fn suggestion_color(i: usize) -> egui::Color32 {
    egui::ecolor::Hsva::new((i as f32 * 0.618_034).fract(), 0.65, 0.9, 1.0).into()
}

impl App {
    /// Draws the standing suggestions in screen space through `project`, kept inside `panel`; a click on a chip's
    /// button accepts or dismisses that suggestion.
    pub(super) fn draw_suggestions(
        &mut self,
        ui: &mut egui::Ui,
        panel: egui::Rect,
        project: &impl Fn(egui::Pos2) -> egui::Pos2,
    ) {
        if self.json_level.is_some() {
            return;
        }
        let shown = self.suggestions().to_vec();
        if shown.is_empty() {
            return;
        }
        let painter = ui
            .ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("suggestion_rings")))
            .with_clip_rect(panel);
        let mut accept = None;
        let mut dismiss = None;
        for (i, suggestion) in shown.iter().enumerate() {
            let color = suggestion_color(i);
            let mut bounds: Option<egui::Rect> = None;
            for &card in &suggestion.cards {
                let Some(world) = self.level_card_rect(crate::card_id::CardId::from(card)) else { continue };
                let rect = egui::Rect::from_min_max(project(world.min), project(world.max));
                painter.rect_stroke(
                    rect.expand(RING_PAD),
                    crate::theme::CARD_RADIUS + RING_PAD,
                    egui::Stroke::new(2.5, color),
                    egui::StrokeKind::Outside,
                );
                bounds = Some(bounds.map_or(rect, |b| b.union(rect)));
            }
            let Some(bounds) = bounds else { continue };
            let x = bounds.center().x.clamp(panel.left() + 90.0, panel.right() - 90.0);
            let y = (bounds.top() - CHIP_GAP).max(panel.top() + 40.0);
            egui::Area::new(egui::Id::new(("suggestion_chip", i)))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::CENTER_BOTTOM)
                .fixed_pos(egui::pos2(x, y))
                .show(ui.ctx(), |ui| {
                    bar_frame(ui).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                            ui.spacing_mut().interact_size.y = 26.0;
                            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 26.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 5.0, color);
                            ui.label(format!("{} ({})", suggestion.label, suggestion.cards.len()));
                            let ok = chip_icon_button(
                                ui,
                                egui::Id::new(("suggestion_accept", i)),
                                egui_phosphor::light::CHECK,
                            );
                            if ok.named_tip("Group these cards").clicked() {
                                accept = Some(i);
                            }
                            let no =
                                chip_icon_button(ui, egui::Id::new(("suggestion_dismiss", i)), egui_phosphor::light::X);
                            if no.named_tip("Dismiss").clicked() {
                                dismiss = Some(i);
                            }
                        });
                    });
                });
        }
        if let Some(i) = accept {
            self.accept_suggestion(i);
        } else if let Some(i) = dismiss {
            self.dismiss_suggestion(i);
        }
    }
}
