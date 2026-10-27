//! The Graph view's one right panel (card-pane spec decision 1): a JSON level's outline, or the card pane while stored
//! cards are selected. One predicate (`App::right_panel`) decides it for the panel and for the scene area.

use super::*;

/// The right panel's narrowest and widest (before the half-window cap).
const RIGHT_PANEL_MIN: f32 = 160.0;
const RIGHT_PANEL_MAX: f32 = 640.0;

/// What the right panel holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RightPanel {
    /// A JSON level's outline, while it is not hidden.
    Outline,
    /// The card pane, while stored cards are selected (`card_pane`).
    Cards,
}

impl App {
    /// What the right panel shows now, if anything: a JSON level shows its outline (or nothing, hidden), any other
    /// level the card pane while stored cards are selected.
    pub(crate) fn right_panel(&self) -> Option<RightPanel> {
        if self.json_level.is_some() {
            return self.json_ui.json_outline_visible.then_some(RightPanel::Outline);
        }
        (self.selected_stored_count() > 0).then_some(RightPanel::Cards)
    }

    /// The widest the panel may be: `RIGHT_PANEL_MAX`, or half the window when that is less, never under the
    /// minimum.
    fn right_panel_max(&self) -> f32 {
        (self.screen_size_points().x / 2.0).clamp(RIGHT_PANEL_MIN, RIGHT_PANEL_MAX)
    }

    /// The width the scene area gives up to the panel: none while it is hidden.
    pub(crate) fn right_panel_width_shown(&self) -> f32 {
        match self.right_panel() {
            Some(_) => self.right_panel_width.clamp(RIGHT_PANEL_MIN, self.right_panel_max()),
            None => 0.0,
        }
    }

    /// The right panel, before the canvas takes the remaining space. Also keeps the pane's state with the selection:
    /// a newly described card is revealed.
    pub(super) fn build_right_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        let shown = self.right_panel();
        if shown != Some(RightPanel::Cards) {
            self.interaction.label_section = crate::labels::LabelSection::default();
        }
        self.track_pane_card();
        let Some(shown) = shown else { return };
        if shown == RightPanel::Outline {
            self.poll_json_search();
        }
        let panel = egui::Panel::right("right_panel")
            .resizable(true)
            .default_size(self.right_panel_width)
            .size_range(RIGHT_PANEL_MIN..=self.right_panel_max())
            .show(ui, |ui| match shown {
                RightPanel::Outline => self.json_outline_contents(ui),
                RightPanel::Cards => self.card_pane_contents(ui, actions),
            });
        self.right_panel_width = panel.response.rect.width();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::CardId;

    #[test]
    fn the_panel_shows_cards_while_stored_ones_are_selected() {
        let mut app = App::default();
        assert_eq!(app.right_panel(), None);
        let note = crate::session_sync::tests::add_note(&mut app, "n");
        app.interaction.selected.insert(CardId::from(note));
        assert_eq!(app.right_panel(), Some(RightPanel::Cards));
        assert_eq!(app.right_panel_width_shown(), 260.0);
        app.interaction.selected.insert(crate::card_id::testutil::card(999));
        assert_eq!(app.right_panel(), Some(RightPanel::Cards), "a virtual card beside it changes nothing");
        app.interaction.selected.clear();
        assert_eq!((app.right_panel(), app.right_panel_width_shown()), (None, 0.0));
    }

    /// The panel never takes more than half the window (1024 points wide with no window).
    #[test]
    fn the_panel_takes_at_most_half_the_window() {
        let mut app = App::default();
        let note = crate::session_sync::tests::add_note(&mut app, "n");
        app.interaction.selected.insert(CardId::from(note));
        app.right_panel_width = 900.0;
        assert_eq!(app.right_panel_width_shown(), 512.0);
        assert_eq!(app.graph_scene_outer_rect().width(), 512.0);
    }
}
