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

    /// The pane: the count for several cards, the card's name for one. Tasks 3 and 4 add the sections.
    pub(super) fn card_pane_contents(&mut self, ui: &mut egui::Ui, _actions: &mut Vec<GraphAction>) {
        match self.pane_card() {
            Some(id) => {
                ui.strong(self.accessible_name(id).unwrap_or_default());
            }
            None => {
                ui.label(format!("{} selected", self.selected_stored_count()));
            }
        }
    }
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
}
